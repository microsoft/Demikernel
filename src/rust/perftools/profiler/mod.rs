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
    perftools::profiler::scope::Guard,
    runtime::{types::demi_callback_t, SharedObject},
};
use ::futures::future::FusedFuture;
use ::std::{
    io,
    pin::Pin,
    time::{Duration, SystemTime},
};
use scope::SharedScope;
use std::{
    ops::{Deref, DerefMut},
    thread,
};

//======================================================================================================================
// Structures
//======================================================================================================================

#[cfg(feature = "auto-calibrate")]
const SAMPLE_SIZE: usize = 16641;

thread_local!(
    /// Global thread-local instance of the profiler.
    pub static THREAD_LOCAL_PROFILER: SharedProfiler = SharedProfiler::new();
);

/// A `Profiler` stores the scope tree and keeps track of the currently active
/// scope.
///
/// Note that there is a global thread-local instance of `Profiler` in
/// [`PROFILER`](constant.PROFILER.html), so it is not possible to manually
/// create an instance of `Profiler`.
pub struct Profiler {
    root_scopes: Vec<SharedScope>,
    current_scope: Option<SharedScope>,
    ns_per_cycle: f64,
    perf_callback: Option<demi_callback_t>,
    #[cfg(feature = "auto-calibrate")]
    clock_drift: u64,
}

#[derive(Clone)]
pub struct SharedProfiler(SharedObject<Profiler>);

//======================================================================================================================
// Associated Functions
//======================================================================================================================

/// Print profiling scope tree.
///
/// Percentages represent the amount of time taken relative to the parent node.
///
/// Frequencies are computed with respect to the total amount of time spent in
/// root nodes. Thus, if you have multiple root nodes and they do not cover
/// all code that runs in your program, the printed frequencies will be
/// overestimated.
pub fn write<W: io::Write>(out: &mut W, max_depth: Option<usize>) -> io::Result<()> {
    THREAD_LOCAL_PROFILER.with(|p| p.write(out, max_depth))
}

/// Reset profiling information.
pub fn reset() {
    THREAD_LOCAL_PROFILER.with(|p| p.clone().reset());
}

pub fn set_callback(perf_callback: demi_callback_t) {
    THREAD_LOCAL_PROFILER.with(|p| p.clone().set_callback(perf_callback));
}

impl SharedProfiler {
    pub fn new() -> Self {
        Self(SharedObject::new(Profiler {
            root_scopes: Vec::new(),
            current_scope: None,
            ns_per_cycle: Self::measure_ns_per_cycle(),
            perf_callback: None,
            #[cfg(feature = "auto-calibrate")]
            clock_drift: Self::clock_drift(SAMPLE_SIZE),
        }))
    }

    /// Set a callback and stop collecting statistics.
    pub fn set_callback(&mut self, perf_callback: demi_callback_t) {
        self.perf_callback = Some(perf_callback)
    }

    /// Create and enter a syncronous scope. Returns a [`Guard`](struct.Guard.html) that should be
    /// dropped upon leaving the scope.
    ///
    /// Usually, this method will be called by the
    /// [`profile`](macro.profile.html) macro, so it does not need to be used
    /// directly.
    #[inline]
    pub fn sync_scope(&mut self, name: &'static str) -> Guard {
        let scope = self.get_or_add_scope(name);
        self.enter_scope(scope)
    }

    /// Create and enter a coroutine scope. These are special async scopes that are always rooted because they do not
    /// run under other scopes.
    #[inline]
    pub async fn coroutine_scope<F: FusedFuture>(name: &'static str, mut coroutine: Pin<Box<F>>) -> F::Output {
        AsyncScope::new(
            THREAD_LOCAL_PROFILER.with(|p| p.clone().get_or_add_root_scope(name)),
            coroutine.as_mut(),
        )
        .await
    }

    fn get_or_add_root_scope(&mut self, name: &'static str) -> SharedScope {
        let existing_root = self.root_scopes.iter().find(|root| root.get_name() == name).cloned();

        existing_root.unwrap_or_else(|| {
            let new_scope: SharedScope = SharedScope::new(name, None, self.perf_callback);
            self.root_scopes.push(new_scope.clone());
            new_scope
        })
    }

    pub fn get_or_add_scope(&mut self, name: &'static str) -> SharedScope {
        match self.current_scope.as_ref() {
            Some(current_scope) => {
                let existing_scope = current_scope
                    .get_children()
                    .iter()
                    .find(|child| child.get_name() == name)
                    .cloned();

                existing_scope.unwrap_or_else(|| {
                    let new_scope: SharedScope =
                        SharedScope::new(name, Some(current_scope.clone()), self.perf_callback);
                    current_scope.clone().add_child(new_scope.clone());
                    new_scope
                })
            },
            None => self.get_or_add_root_scope(name),
        }
    }

    fn enter_scope(&mut self, mut scope: SharedScope) -> Guard {
        trace!("Entering scope: {}", scope.get_name());
        let guard = scope.enter();
        self.current_scope = Some(scope);

        guard
    }

    fn reset(&mut self) {
        self.root_scopes.clear();

        // Note that we could now still be anywhere in the previous profiling
        // tree, so we can not simply reset `self.current`. However, as the
        // frame comes to an end we will eventually leave a root node, at which
        // point `self.current` will be set to `None`.
    }

    /// Leave the current scope.
    #[inline]
    fn leave_scope(&mut self, duration: u64) {
        self.current_scope = if let Some(current) = self.current_scope.as_ref() {
            cfg_if::cfg_if! {
                if #[cfg(feature = "auto-calibrate")] {
                    let d = duration.checked_sub(self.clock_drift);
                    current.clone().leave(d.unwrap_or(duration));
                } else {
                    current.clone().leave(duration);
                }
            }

            // Set current scope back to the parent node (if any).
            current.get_parent().as_ref().cloned()
        } else {
            // This should not happen with proper usage.
            log::error!("Called perftools::profiler::leave() while not in any scope");

            None
        };
    }

    fn write<W: io::Write>(&self, out: &mut W, max_depth: Option<usize>) -> io::Result<()> {
        let total_duration = self.root_scopes.iter().map(|root| root.get_duration_sum()).sum();
        let thread_id: thread::ThreadId = thread::current().id();

        writeln!(
            out,
            "call_depth,thread_id,function_name,num_calls,percent_time,cycles_per_call,nanoseconds_per_call"
        )?;
        for root in self.root_scopes.iter() {
            root.write_recursive(out, thread_id, total_duration, 0, max_depth, self.ns_per_cycle)?;
        }

        out.flush()
    }

    fn measure_ns_per_cycle() -> f64 {
        let start: SystemTime = SystemTime::now();
        let (start_cycle, _): (u64, u32) = unsafe { x86::time::rdtscp() };

        test::black_box((0..10000).fold(0, |old, new| old ^ new)); // dummy calculations for measurement

        let (end_cycle, _): (u64, u32) = unsafe { x86::time::rdtscp() };
        let since_the_epoch: Duration = SystemTime::now().duration_since(start).expect("Time went backwards");
        let in_ns: u64 = since_the_epoch.as_secs() * 1_000_000_000 + since_the_epoch.subsec_nanos() as u64;

        in_ns as f64 / (end_cycle - start_cycle) as f64
    }

    #[cfg(feature = "auto-calibrate")]
    fn clock_drift(nsamples: usize) -> u64 {
        let mut total = 0;

        for _ in 0..nsamples {
            let now: u64 = x86::time::rdtscp();
            let duration: u64 = x86::time::rdtscp() - now;

            let d = total + duration;
        }

        total / (nsamples as u64)
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl Deref for SharedProfiler {
    type Target = Profiler;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl DerefMut for SharedProfiler {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}

impl Drop for SharedProfiler {
    fn drop(&mut self) {
        self.write(&mut std::io::stdout(), None)
            .expect("failed to write to stdout");
    }
}
