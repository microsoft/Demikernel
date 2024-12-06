// Copyright(c) Microsoft Corporation.
// Licensed under the MIT license.

//! This module provides a small performance profiler for the Demikernel libOSes.

//======================================================================================================================
// Exports
//======================================================================================================================

mod scope;
pub use crate::perftools::profiler::scope::AsyncScope;
#[cfg(test)]
mod tests;

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{
    perftools::profiler::scope::{Guard, Scope},
    runtime::types::demi_callback_t,
};
use ::futures::future::FusedFuture;
use ::std::{
    cell::RefCell,
    io,
    pin::Pin,
    rc::Rc,
    time::{Duration, SystemTime},
};
use std::thread;

//======================================================================================================================
// Structures
//======================================================================================================================

thread_local!(
    pub static PROFILER: RefCell<Profiler> = RefCell::new(Profiler::new())
);

/// A `Profiler` stores the scope tree and keeps track of the currently active scope. Note that there is a global
/// thread-local instance of `Profiler` in [`PROFILER`](constant.PROFILER.html), so it is not possible to manually
/// create an instance of `Profiler`.
pub struct Profiler {
    root_scopes: Vec<Rc<RefCell<Scope>>>,
    current: Option<Rc<RefCell<Scope>>>,
    perf_callback: Option<demi_callback_t>,
}

//======================================================================================================================
// Associated Functions
//======================================================================================================================

/// Print profiling scope tree. Percentages represent the amount of time taken relative to the parent node. Frequencies
/// are computed with respect to the total amount of time spent in root nodes. Thus, if you have multiple root nodes
/// and they do not cover all code that runs in your program, the printed frequencies will be overestimated.
pub fn write<W: io::Write>(out: &mut W) -> io::Result<()> {
    PROFILER.with(|p| p.borrow().write(out))
}

pub fn reset() {
    PROFILER.with(|p| p.borrow_mut().reset());
}

pub fn set_callback(perf_callback: demi_callback_t) {
    PROFILER.with(|p| p.borrow_mut().set_callback(perf_callback));
}

impl Profiler {
    fn new() -> Profiler {
        Profiler {
            root_scopes: Vec::new(),
            current: None,
            perf_callback: None,
        }
    }

    pub fn set_callback(&mut self, perf_callback: demi_callback_t) {
        self.perf_callback = Some(perf_callback)
    }

    /// Create and enter a syncronous scope. Returns a [`Guard`](struct.Guard.html) that should be dropped upon
    /// leaving the scope. Usually, this method will be called by the [`profile`](macro.profile.html) macro,
    /// so it does not need to be used directly.
    #[inline]
    pub fn sync_scope(&mut self, name: &'static str) -> Guard {
        let scope = self.get_or_create_scope(name);
        self.enter_scope(scope)
    }

    /// Create a special async scopes that is rooted because it does not run under other scopes.
    #[inline]
    pub async fn coroutine_scope<F: FusedFuture>(name: &'static str, mut coroutine: Pin<Box<F>>) -> F::Output {
        AsyncScope::new(
            PROFILER.with(|p| p.borrow_mut().get_or_create_root_scope(name)),
            coroutine.as_mut(),
        )
        .await
    }

    fn get_or_create_root_scope(&mut self, name: &'static str) -> Rc<RefCell<Scope>> {
        let existing_root_scope = self
            .root_scopes
            .iter()
            .find(|root_scope| root_scope.borrow().name == name)
            .cloned();

        existing_root_scope.unwrap_or_else(|| {
            let new_scope: Scope = Scope::new(name, None, self.perf_callback);
            let new_root_scope: Rc<RefCell<Scope>> = Rc::new(RefCell::new(new_scope));

            self.root_scopes.push(new_root_scope.clone());

            new_root_scope
        })
    }

    pub fn get_or_create_scope(&mut self, name: &'static str) -> Rc<RefCell<Scope>> {
        match self.current.as_ref() {
            Some(current_scope) => {
                let existing_scope = current_scope
                    .borrow()
                    .children_scopes
                    .iter()
                    .find(|child_scope| child_scope.borrow().name == name)
                    .cloned();

                existing_scope.unwrap_or_else(|| {
                    let new_scope: Scope = Scope::new(name, Some(current_scope.clone()), self.perf_callback);
                    let new_child_scope = Rc::new(RefCell::new(new_scope));

                    current_scope.borrow_mut().add_child_scope(new_child_scope.clone());

                    new_child_scope
                })
            },
            None => self.get_or_create_root_scope(name),
        }
    }

    fn enter_scope(&mut self, scope: Rc<RefCell<Scope>>) -> Guard {
        let guard = scope.borrow_mut().enter();
        self.current = Some(scope);

        guard
    }

    fn reset(&mut self) {
        self.root_scopes.clear();

        // Note that we could now still be anywhere in the previous profiling
        // tree, so we can not simply reset `self.current`. However, as the
        // frame comes to an end we will eventually leave a root node, at which
        // point `self.current` will be set to `None`.
    }

    #[inline]
    fn leave_scope(&mut self, duration: u64) {
        self.current = if let Some(current) = self.current.as_ref() {
            current.borrow_mut().leave(duration);
            current.borrow().parent_scope.as_ref().cloned()
        } else {
            // This should not happen with proper usage.
            log::error!("Called perftools::profiler::leave() while not in any scope");

            None
        };
    }

    fn write<W: io::Write>(&self, out: &mut W) -> io::Result<()> {
        let thread_id = thread::current().id();
        let grand_total_duration = {
            self.root_scopes
                .iter()
                .map(|root_scope| root_scope.borrow().duration_sum)
                .sum()
        };
        let ns_per_cycle = Self::measure_ns_per_cycle();

        // Header row
        writeln!(
            out,
            "call_depth,thread_id,function_name,num_calls,percent_time,cycles_per_call,nanoseconds_per_call"
        )?;

        self.write_root_scopes(out, thread_id, grand_total_duration, ns_per_cycle)?;
        out.flush()
    }

    fn write_root_scopes<W: io::Write>(
        &self,
        out: &mut W,
        thread_id: thread::ThreadId,
        grand_total_duration: u64,
        ns_per_cycle: f64,
    ) -> Result<(), io::Error> {
        for root_scope in self.root_scopes.iter() {
            root_scope
                .borrow()
                .write_recursive(out, thread_id, grand_total_duration, 0, ns_per_cycle)?;
        }
        Ok(())
    }

    fn measure_ns_per_cycle() -> f64 {
        let start: SystemTime = SystemTime::now();
        let start_cycle: u64 = unsafe { x86::time::rdtscp().0 };

        test::black_box((0..10000).fold(0, |old, new| old ^ new)); // dummy calculations for measurement

        let end_cycle: u64 = unsafe { x86::time::rdtscp().0 };
        let since_the_epoch: Duration = SystemTime::now().duration_since(start).expect("Time went backwards");
        let in_ns: u64 = since_the_epoch.as_secs() * 1_000_000_000 + since_the_epoch.subsec_nanos() as u64;

        in_ns as f64 / (end_cycle - start_cycle) as f64
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl Drop for Profiler {
    fn drop(&mut self) {
        self.write(&mut std::io::stdout()).expect("failed to write to stdout");
    }
}
