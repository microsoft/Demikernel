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
    time::{Duration, SystemTime},
};
use std::{collections::HashMap, thread};

//======================================================================================================================
// Structures
//======================================================================================================================

#[cfg(feature = "auto-calibrate")]
const SAMPLE_SIZE: usize = 16641;

thread_local!(
    /// Global thread-local instance of the profiler.
    pub static PROFILER: RefCell<Profiler> = RefCell::new(Profiler::new())
);

/// A `Profiler` stores the scope tree and keeps track of the currently active
/// scope.
///
/// Note that there is a global thread-local instance of `Profiler` in
/// [`PROFILER`](constant.PROFILER.html), so it is not possible to manually
/// create an instance of `Profiler`.
pub struct Profiler {
    all_scopes: Vec<Scope>,
    scope_lookup: HashMap<&'static str, usize>,
    current_scope_index: Option<usize>,
    root_scopes: Vec<usize>,
    // roots: Vec<Rc<RefCell<Scope>>>,
    // current: Option<Rc<RefCell<Scope>>>,
    perf_callback: Option<demi_callback_t>,
    #[cfg(feature = "auto-calibrate")]
    clock_drift: u64,
}

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
    PROFILER.with(|p| p.borrow().write(out, max_depth))
}

/// Reset profiling information.
pub fn reset() {
    PROFILER.with(|p| p.borrow_mut().reset());
}

pub fn set_callback(perf_callback: demi_callback_t) {
    PROFILER.with(|p| p.borrow_mut().set_callback(perf_callback));
}

impl Profiler {
    fn new() -> Profiler {
        Profiler {
            all_scopes: Vec::new(),
            scope_lookup: HashMap::new(),
            root_scopes: Vec::new(),
            current_scope_index: None,
            // roots: Vec::new(),
            // current: None,
            perf_callback: None,
            #[cfg(feature = "auto-calibrate")]
            clock_drift: Self::clock_drift(SAMPLE_SIZE),
        }
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
        let scope_index = self.get_or_create_scope_index(name);
        self.enter_scope(scope_index)
    }

    pub fn get_or_create_scope_index(&mut self, name: &'static str) -> usize {
        match self.scope_lookup.contains_key(name) {
            true => self.scope_lookup[name],
            false => {
                let scope = Scope::new(name, self.current_scope_index, self.perf_callback);
                self.all_scopes.push(scope);
                let new_scope_index = self.all_scopes.len() - 1;
                self.scope_lookup.insert(name, new_scope_index);
                if self.current_scope_index.is_none() {
                    self.root_scopes.push(new_scope_index);
                } else {
                    self.all_scopes[self.current_scope_index.unwrap()].add_child_scope_index(new_scope_index);
                }
                new_scope_index
            },
        }
    }

    pub fn get_or_create_root_scope(&mut self, name: &'static str) -> usize {
        let scope_index: usize = self.get_or_create_scope_index(name);
        if !self.root_scopes.contains(&scope_index) {
            self.root_scopes.push(scope_index);
        }
        return scope_index;
    }

    /// Create and enter a coroutine scope. These are special async scopes that are always rooted because they do not
    /// run under other scopes.
    #[inline]
    pub async fn coroutine_scope<F: FusedFuture>(name: &'static str, mut coroutine: Pin<Box<F>>) -> F::Output {
        AsyncScope::new(
            PROFILER.with(|p| p.borrow_mut().get_or_create_scope_index(name)),
            coroutine.as_mut(),
        )
        .await
    }

    // /// Looks up the scope at the root level using the name, creating a new one if not found.
    // fn get_root_scope(&mut self, name: &'static str) -> Rc<RefCell<Scope>> {
    //     //Check if `name` already is a root.
    //     let existing_root = self.roots.iter().find(|root| root.borrow().get_name() == name).cloned();

    //     existing_root.unwrap_or_else(|| {
    //         // Add a new root node.
    //         let new_scope: Scope = Scope::new(name, None, self.perf_callback);
    //         let succ = Rc::new(RefCell::new(new_scope));

    //         self.roots.push(succ.clone());

    //         succ
    //     })
    // }

    // /// Look up the scope using the name.
    // pub fn get_scope(&mut self, name: &'static str) -> Rc<RefCell<Scope>> {
    //     // Check if we have already registered `name` at the current point in
    //     // the tree.
    //     if let Some(current) = self.current.as_ref() {
    //         // We are currently in some scope.
    //         let existing_succ = current
    //             .borrow()
    //             .get_succs()
    //             .iter()
    //             .find(|succ| succ.borrow().get_name() == name)
    //             .cloned();

    //         existing_succ.unwrap_or_else(|| {
    //             // Add new successor node to the current node.
    //             let new_scope: Scope = Scope::new(name, Some(current.clone()), self.perf_callback);
    //             let succ = Rc::new(RefCell::new(new_scope));

    //             current.borrow_mut().add_succ(succ.clone());

    //             succ
    //         })
    //     } else {
    //         // We are currently not within any scope.
    //         self.get_root_scope(name)
    //     }
    // }

    /// Actually enter a scope.
    fn enter_scope(&mut self, scope_index: usize) -> Guard {
        let guard = self.all_scopes[scope_index].enter();
        self.current_scope_index = Some(scope_index);

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
        self.current_scope_index = if let Some(current_scope_index) = self.current_scope_index {
            cfg_if::cfg_if! {
                if #[cfg(feature = "auto-calibrate")] {
                    let d = duration.checked_sub(self.clock_drift);
                    self.all_scopes[current_scope_index].leave(d.unwrap_or(duration));
                } else {
                    self.all_scopes[current_scope_index].leave(duration);
                }
            }
            self.all_scopes[current_scope_index].get_parent_scope_index()
        } else {
            log::error!("Called perftools::profiler::leave() while not in any scope");
            None
        };

        // self.current = if let Some(current) = self.current.as_ref() {
        //     cfg_if::cfg_if! {
        //         if #[cfg(feature = "auto-calibrate")] {
        //             let d = duration.checked_sub(self.clock_drift);
        //             current.borrow_mut().leave(d.unwrap_or(duration));
        //         } else {
        //             current.borrow_mut().leave(duration);
        //         }
        //     }

        //     // Set current scope back to the parent node (if any).
        //     current.borrow().get_pred().as_ref().cloned()
        // } else {
        //     // This should not happen with proper usage.
        //     log::error!("Called perftools::profiler::leave() while not in any scope");

        //     None
        // };
    }

    fn write<W: io::Write>(&self, out: &mut W, max_depth: Option<usize>) -> io::Result<()> {
        // let total_duration = self.roots.iter().map(|root| root.borrow().get_duration_sum()).sum();
        let total_duration = self
            .root_scopes
            .iter()
            .map(|root_index| self.all_scopes[*root_index].get_duration_sum())
            .sum();
        let thread_id: thread::ThreadId = thread::current().id();
        let ns_per_cycle: f64 = Self::measure_ns_per_cycle();

        writeln!(
            out,
            "call_depth,thread_id,function_name,num_calls,percent_time,cycles_per_call,nanoseconds_per_call"
        )?;

        for root_index in self.root_scopes.iter() {
            let root = &self.all_scopes[*root_index];
            // get vec of children for root
            let children_indices: &Vec<usize> = root.get_children();
            let mut children_scopes: Vec<&Scope> = Vec::new();
            for child_index in children_indices.iter() {
                children_scopes.push(&self.all_scopes[*child_index]);
            }
            root.write_recursive(
                out,
                thread_id,
                total_duration,
                0,
                max_depth,
                ns_per_cycle,
                children_scopes,
            )?;
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

impl Drop for Profiler {
    fn drop(&mut self) {
        self.write(&mut std::io::stdout(), None)
            .expect("failed to write to stdout");
    }
}
