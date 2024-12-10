// Copyright(c) Microsoft Corporation.
// Licensed under the MIT license.

use crate::{async_timer, perftools::profiler, timer};
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

        crate::ensure_eq!(p.root_scopes.len(), 2);

        for root in p.root_scopes.iter() {
            crate::ensure_eq!(root.borrow().parent_scope.is_none(), true);
            crate::ensure_eq!(root.borrow().children_scopes.is_empty(), true);
        }

        crate::ensure_eq!(p.root_scopes[0].borrow().name, "b");
        crate::ensure_eq!(p.root_scopes[1].borrow().name, "a");

        crate::ensure_eq!(p.root_scopes[0].borrow().num_calls, 6);
        crate::ensure_eq!(p.root_scopes[1].borrow().num_calls, 1);

        Ok(())
    })
}

#[test]
fn test_succ_reuse() -> Result<()> {
    use std::ptr;

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

        let root = p.root_scopes[0].borrow();
        crate::ensure_eq!(root.name, "a");
        crate::ensure_eq!(root.parent_scope.is_none(), true);
        crate::ensure_eq!(root.children_scopes.len(), 1);
        crate::ensure_eq!(root.num_calls, 6);

        let succ = root.children_scopes[0].borrow();
        crate::ensure_eq!(succ.name, "b");
        crate::ensure_eq!(
            ptr::eq(succ.parent_scope.as_ref().unwrap().as_ref(), p.root_scopes[0].as_ref()),
            true
        );
        crate::ensure_eq!(succ.children_scopes.is_empty(), true);
        crate::ensure_eq!(succ.num_calls, 3);

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

            crate::ensure_eq!(profiler::PROFILER.with(|p| p.borrow().current.is_some()), true);

            timer!("d");
        }
    }

    profiler::PROFILER.with(|p| -> Result<()> {
        let p = p.borrow();

        crate::ensure_eq!(p.root_scopes.is_empty(), true);
        crate::ensure_eq!(p.current.is_none(), true);

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

            let root = p.root_scopes[0].borrow();
            crate::ensure_eq!(root.name, "dummy");
            crate::ensure_eq!(root.num_calls, self.as_ref().iterations);
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
