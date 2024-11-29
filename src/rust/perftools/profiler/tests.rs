// Copyright(c) Microsoft Corporation.
// Licensed under the MIT license.

use crate::{
    async_timer,
    perftools::profiler::{self, scope::Scope},
    timer,
};
use ::anyhow::Result;
use ::std::{
    future::Future,
    task::{Context, Poll, Waker},
};
use std::pin::{pin, Pin};

#[test]
fn test_multiple_roots() -> Result<()> {
    profiler::reset();

    for i in 0..=5 {
        if i == 5 {
            timer!("a");
        }
        {
            timer!("b");
        }
    }

    profiler::PROFILER.with(|p| -> Result<()> {
        let p = p.borrow();

        // crate::ensure_eq!(p.roots.len(), 2);

        // for root in p.roots.iter() {
        //     crate::ensure_eq!(root.borrow().get_pred().is_none(), true);
        //     crate::ensure_eq!(root.borrow().get_succs().is_empty(), true);
        // }

        // crate::ensure_eq!(p.roots[0].borrow().get_name(), "b");
        // crate::ensure_eq!(p.roots[1].borrow().get_name(), "a");

        // crate::ensure_eq!(p.roots[0].borrow().get_num_calls(), 6);
        // crate::ensure_eq!(p.roots[1].borrow().get_num_calls(), 1);

        crate::ensure_eq!(p.root_scopes.len(), 2);

        for root_scope_index in p.root_scopes.iter() {
            let root_scope: &Scope = &p.all_scopes[*root_scope_index];
            crate::ensure_eq!(root_scope.get_parent_scope_index().is_none(), true);
            crate::ensure_eq!(root_scope.get_children().is_empty(), true);
        }

        crate::ensure_eq!(p.all_scopes[p.root_scopes[0]].get_name(), "b");
        crate::ensure_eq!(p.all_scopes[p.root_scopes[1]].get_name(), "a");

        crate::ensure_eq!(p.all_scopes[p.root_scopes[0]].get_num_calls(), 6);
        crate::ensure_eq!(p.all_scopes[p.root_scopes[1]].get_num_calls(), 1);

        Ok(())
    })
}

#[test]
fn test_child_reuse() -> Result<()> {
    profiler::reset();

    for i in 0..=5 {
        timer!("a");
        if i > 2 {
            timer!("b");
        }
    }

    crate::ensure_eq!(profiler::PROFILER.with(|p| p.borrow().root_scopes.len()), 1);

    profiler::PROFILER.with(|p| -> Result<()> {
        let p = p.borrow();

        crate::ensure_eq!(p.root_scopes.len(), 1);

        let root_scope: &Scope = &p.all_scopes[p.root_scopes[0]];
        crate::ensure_eq!(root_scope.get_name(), "a");
        crate::ensure_eq!(root_scope.get_parent_scope_index().is_none(), true);
        crate::ensure_eq!(root_scope.get_children().len(), 1);
        crate::ensure_eq!(root_scope.get_num_calls(), 6);

        let child: &Scope = &p.all_scopes[root_scope.get_children()[0]];
        crate::ensure_eq!(child.get_name(), "b");
        crate::ensure_eq!(
            p.all_scopes[child.get_parent_scope_index().unwrap()].get_name(),
            p.all_scopes[p.root_scopes[0]].get_name()
        );
        crate::ensure_eq!(child.get_children().is_empty(), true);
        crate::ensure_eq!(child.get_num_calls(), 3);

        Ok(())
    })
}

#[test]
fn test_reset_during_frame() -> Result<()> {
    profiler::reset();

    for i in 0..=5 {
        timer!("a");
        timer!("b");
        {
            timer!("c");
            if i == 5 {
                profiler::reset();
            }

            crate::ensure_eq!(
                profiler::PROFILER.with(|p| p.borrow().current_scope_index.is_some()),
                true
            );

            timer!("d");
        }
    }

    profiler::PROFILER.with(|p| -> Result<()> {
        let p = p.borrow();

        crate::ensure_eq!(p.root_scopes.is_empty(), true);
        crate::ensure_eq!(p.current_scope_index.is_none(), true);

        Ok(())
    })
}

struct DummyCoroutine {
    iterations: usize,
}

impl Future for DummyCoroutine {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, _ctx: &mut Context) -> Poll<Self::Output> {
        match profiler::PROFILER.with(|p| -> Result<()> {
            let p = p.borrow();
            crate::ensure_eq!(p.root_scopes.len(), 1);

            let root_scope: &Scope = &p.all_scopes[p.root_scopes[0]];
            crate::ensure_eq!(root_scope.get_name(), "dummy");
            crate::ensure_eq!(root_scope.get_num_calls(), self.as_ref().iterations);
            Ok(())
        }) {
            Ok(()) => {
                self.get_mut().iterations += 1;
                Poll::Pending
            },
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

#[test]
fn test_async() -> Result<()> {
    let mut coroutine = DummyCoroutine { iterations: 0 };
    let mut task = pin!(async_timer!("dummy", pin!(coroutine)));
    let waker = Waker::noop();
    let mut context = Context::from_waker(&waker);

    for _ in 0..10 {
        match Future::poll(task.as_mut(), &mut context) {
            Poll::Pending => (),
            Poll::Ready(r) => return r,
        }
    }

    Ok(())
}
