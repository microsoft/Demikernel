// Copyright(c) Microsoft Corporation.
// Licensed under the MIT license.

//! This module provides data structures related to scoping blocks of code for our profiler.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{perftools::profiler::PROFILER, runtime::types::demi_callback_t};
use ::std::{
    cell::RefCell,
    fmt::{self, Debug},
    future::Future,
    io,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
    thread,
};

//======================================================================================================================
// Structures
//======================================================================================================================

/// Internal representation of scopes as a tree. This tracks a single profiling block of code in relationship to other
/// profiled blocks.
pub struct Scope {
    pub name: &'static str,
    pub parent_scope: Option<Rc<RefCell<Scope>>>,
    pub children_scopes: Vec<Rc<RefCell<Scope>>>,
    /// Callback to report statistics. If this is set to None, we collect averages by default.
    pub perf_callback: Option<demi_callback_t>,
    pub num_calls: usize,
    /// In total, how much time has been spent in this scope?
    pub duration_sum: u64,
}

/// A guard that is created when entering a scope and dropped when leaving it.
pub struct Guard {
    enter_time: u64,
}

/// A scope over an async block that may yield and re-enter several times.
pub struct AsyncScope<'a, F: Future> {
    scope: Rc<RefCell<Scope>>,
    future: Pin<&'a mut F>,
}

//======================================================================================================================
// Associated Functions
//======================================================================================================================

impl Scope {
    pub fn new(
        name: &'static str,
        parent_scope: Option<Rc<RefCell<Scope>>>,
        perf_callback: Option<demi_callback_t>,
    ) -> Scope {
        Scope {
            name,
            parent_scope,
            children_scopes: Vec::new(),
            num_calls: 0,
            duration_sum: 0,
            perf_callback,
        }
    }

    pub fn add_child_scope(&mut self, child_scope: Rc<RefCell<Scope>>) {
        self.children_scopes.push(child_scope.clone())
    }

    #[cfg(test)]
    pub fn get_num_calls(&self) -> usize {
        self.num_calls
    }

    /// Enter this scope. Returns a `Guard` instance that should be dropped when leaving the scope.
    #[inline]
    pub fn enter(&self) -> Guard {
        Guard::enter()
    }

    /// Leave this scope. Called automatically by the `Guard` instance.
    #[inline]
    pub fn leave(&mut self, duration: u64) {
        if let Some(callback_fn) = self.perf_callback {
            callback_fn(self.name.as_ptr() as *const i8, self.name.len() as u32, duration);
        } else {
            self.num_calls += 1;
            // Even though this is extremely unlikely, let's not panic on overflow.
            self.duration_sum = self.duration_sum + duration;
        }
    }

    pub fn write_recursive<W: io::Write>(
        &self,
        out: &mut W,
        thread_id: thread::ThreadId,
        grand_total_duration: u64,
        depth: usize,
        ns_per_cycle: f64,
    ) -> io::Result<()> {
        let duration_sum: f64 = (self.duration_sum) as f64;
        // Use the grand total duration if this is a root scope.
        let parent_duration_sum: f64 = self
            .parent_scope
            .clone()
            .map_or((grand_total_duration) as f64, |parent_scope| {
                (parent_scope.borrow().duration_sum) as f64
            });
        let percent_time = duration_sum / parent_duration_sum * 100.0;

        // Write markers.
        let mut markers = String::from("+");
        for _ in 0..depth {
            markers.push('+');
        }
        writeln!(
            out,
            "{},{},{},{},{}",
            format!("{},{:?},{}", markers, thread_id, self.name),
            self.num_calls,
            percent_time,
            duration_sum / (self.num_calls as f64),
            duration_sum / (self.num_calls as f64) * ns_per_cycle,
        )?;

        // Write children
        for succ in &self.children_scopes {
            succ.borrow()
                .write_recursive(out, thread_id, grand_total_duration, depth + 1, ns_per_cycle)?;
        }

        Ok(())
    }
}

impl<'a, F: Future> AsyncScope<'a, F> {
    pub fn new(scope: Rc<RefCell<Scope>>, future: Pin<&'a mut F>) -> Self {
        Self { scope, future }
    }
}

impl Guard {
    #[inline]
    pub fn enter() -> Self {
        let now: u64 = unsafe { x86::time::rdtscp().0 };
        Self { enter_time: now }
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl<'a, F: Future> Future for AsyncScope<'a, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        let self_: &mut Self = self.get_mut();

        let _guard = PROFILER.with(|p| p.borrow_mut().enter_scope(self_.scope.clone()));
        Future::poll(self_.future.as_mut(), ctx)
    }
}

impl Drop for Guard {
    #[inline]
    fn drop(&mut self) {
        let now: u64 = unsafe { x86::time::rdtscp().0 };
        let duration: u64 = now - self.enter_time;

        PROFILER.with(|p| p.borrow_mut().leave_scope(duration));
    }
}
