// Copyright(c) Microsoft Corporation.
// Licensed under the MIT license.

//! This module provides data structures related to scoping blocks of code for our profiler.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{perftools::profiler::PROFILER, runtime::types::demi_callback_t};
use ::std::{
    fmt::{self, Debug},
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    thread,
};

//======================================================================================================================
// Structures
//======================================================================================================================

/// Internal representation of scopes as a tree. This tracks a single profiling block of code in relationship to other
/// profiled blocks.
pub struct Scope {
    /// Name of the scope.
    name: &'static str,
    parent_scope_index: Option<usize>,
    children_scope_indices: Vec<usize>,
    /// Callback to report statistics. If this is set to None, we collect averages by default.
    perf_callback: Option<demi_callback_t>,
    /// How often has this scope been visited?
    num_calls: usize,
    /// In total, how much time has been spent in this scope?
    duration_sum: u64,
}

/// A guard that is created when entering a scope and dropped when leaving it.
pub struct Guard {
    enter_time: u64,
}

/// A scope over an async block that may yield and re-enter several times.
pub struct AsyncScope<'a, F: Future> {
    scope_index: usize,
    future: Pin<&'a mut F>,
}

//======================================================================================================================
// Associated Functions
//======================================================================================================================

impl Scope {
    pub fn new(name: &'static str, parent_scope_index: Option<usize>, perf_callback: Option<demi_callback_t>) -> Scope {
        Scope {
            name,
            parent_scope_index,
            children_scope_indices: Vec::new(),
            num_calls: 0,
            duration_sum: 0,
            perf_callback,
        }
    }

    #[allow(dead_code)]
    pub fn get_name(&self) -> &'static str {
        self.name
    }

    pub fn get_parent_scope_index(&self) -> Option<usize> {
        self.parent_scope_index
    }

    pub fn get_children(&self) -> &Vec<usize> {
        &self.children_scope_indices
    }

    pub fn add_child_scope_index(&mut self, child_scope_index: usize) {
        self.children_scope_indices.push(child_scope_index)
    }

    #[cfg(test)]
    pub fn get_num_calls(&self) -> usize {
        self.num_calls
    }

    pub fn get_duration_sum(&self) -> u64 {
        self.duration_sum
    }

    /// Enter this scope. Returns a `Guard` instance that should be dropped
    /// when leaving the scope.
    #[inline]
    pub fn enter(&mut self) -> Guard {
        Guard::enter()
    }

    /// Leave this scope. Called automatically by the `Guard` instance.
    #[inline]
    pub fn leave(&mut self, duration: u64) {
        if let Some(callback) = self.perf_callback {
            callback(self.name.as_ptr() as *const i8, self.name.len() as u32, duration);
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
        total_duration: u64,
        depth: usize,
        max_depth: Option<usize>,
        ns_per_cycle: f64,
        children_scopes: Vec<&Scope>,
    ) -> io::Result<()> {
        if let Some(d) = max_depth {
            if depth > d {
                return Ok(());
            }
        }

        let total_duration_secs = (total_duration) as f64;
        let duration_sum_secs = (self.duration_sum) as f64;
        // ToDo: This is a bug. We should be using the sum of the predicted durations of the children.
        let pred_sum_secs = total_duration_secs;
        let percent_time = duration_sum_secs / pred_sum_secs * 100.0;

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
            duration_sum_secs / (self.num_calls as f64),
            duration_sum_secs / (self.num_calls as f64) * ns_per_cycle,
        )?;

        for child_scope in children_scopes {
            child_scope.write_recursive(
                out,
                thread_id,
                total_duration,
                depth + 1,
                max_depth,
                ns_per_cycle,
                Vec::new(),
            )?;
        }

        Ok(())
    }
}

impl<'a, F: Future> AsyncScope<'a, F> {
    pub fn new(scope_index: usize, future: Pin<&'a mut F>) -> Self {
        Self { scope_index, future }
    }
}

impl Guard {
    #[inline]
    pub fn enter() -> Self {
        let (now, _): (u64, u32) = unsafe { x86::time::rdtscp() };
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

        let _guard = PROFILER.with(|p| p.borrow_mut().enter_scope(self_.scope_index));
        Future::poll(self_.future.as_mut(), ctx)
    }
}

impl Drop for Guard {
    #[inline]
    fn drop(&mut self) {
        let (now, _): (u64, u32) = unsafe { x86::time::rdtscp() };
        let duration: u64 = now - self.enter_time;

        PROFILER.with(|p| p.borrow_mut().leave_scope(duration));
    }
}
