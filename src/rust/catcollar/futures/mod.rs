// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//==============================================================================
// Exports
//==============================================================================

pub mod accept;
pub mod close;
pub mod connect;
pub mod pop;
pub mod push;
pub mod pushto;

//==============================================================================
// Imports
//==============================================================================

use self::{
    accept::AcceptFuture,
    close::CloseFuture,
    connect::ConnectFuture,
    pop::PopFuture,
    push::PushFuture,
    pushto::PushtoFuture,
};
use crate::{
    runtime::{
        OperationResult,
        QDesc,
    },
    scheduler::{
        FutureResult,
        SchedulerFuture,
    },
};
use ::std::{
    any::Any,
    future::Future,
    os::unix::prelude::RawFd,
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

//==============================================================================
// Structures
//==============================================================================

/// Operations Descriptor
pub enum Operation {
    /// Accept operation.
    Accept(FutureResult<AcceptFuture>),
    /// Connection operation
    Connect(FutureResult<ConnectFuture>),
    /// Push operation
    Push(FutureResult<PushFuture>),
    /// Pushto operation.
    Pushto(FutureResult<PushtoFuture>),
    /// Pop operation.
    Pop(FutureResult<PopFuture>),
    /// Close operation.
    Close(FutureResult<CloseFuture>),
}

//==============================================================================
// Associate Functions
//==============================================================================

/// Associate Functions for Operation Descriptor
impl Operation {
    /// Gets the [OperationResult] output by the target [Operation].
    pub fn get_result(self) -> (QDesc, Option<QDesc>, Option<RawFd>, OperationResult) {
        match self {
            // Accept operation.
            Operation::Accept(FutureResult {
                future,
                done: Some(Ok((new_fd, addr))),
            }) => (
                future.get_qd(),
                Some(future.get_new_qd()),
                Some(new_fd),
                OperationResult::Accept((future.get_new_qd(), addr)),
            ),
            Operation::Accept(FutureResult {
                future,
                done: Some(Err(e)),
            }) => (
                future.get_qd(),
                Some(future.get_new_qd()),
                None,
                OperationResult::Failed(e),
            ),

            // Connect operation.
            Operation::Connect(FutureResult {
                future,
                done: Some(Ok(())),
            }) => (future.get_qd(), None, None, OperationResult::Connect),
            Operation::Connect(FutureResult {
                future,
                done: Some(Err(e)),
            }) => (future.get_qd(), None, None, OperationResult::Failed(e)),

            // Push operation.
            Operation::Push(FutureResult {
                future,
                done: Some(Ok(())),
            }) => (future.get_qd(), None, None, OperationResult::Push),
            Operation::Push(FutureResult {
                future,
                done: Some(Err(e)),
            }) => (future.get_qd(), None, None, OperationResult::Failed(e)),

            // Pushto operation.
            Operation::Pushto(FutureResult {
                future,
                done: Some(Ok(())),
            }) => (future.get_qd(), None, None, OperationResult::Push),
            Operation::Pushto(FutureResult {
                future,
                done: Some(Err(e)),
            }) => (future.get_qd(), None, None, OperationResult::Failed(e)),

            // Pop operation.
            Operation::Pop(FutureResult {
                future,
                done: Some(Ok((addr, buf))),
            }) => (future.get_qd(), None, None, OperationResult::Pop(addr, buf)),
            Operation::Pop(FutureResult {
                future,
                done: Some(Err(e)),
            }) => (future.get_qd(), None, None, OperationResult::Failed(e)),

            // Close operation.
            Operation::Close(FutureResult {
                future,
                done: Some(Ok(())),
            }) => (future.get_qd(), None, None, OperationResult::Close),
            Operation::Close(FutureResult {
                future,
                done: Some(Err(e)),
            }) => (future.get_qd(), None, None, OperationResult::Failed(e)),

            _ => panic!("future not ready"),
        }
    }
}

//==============================================================================
// Trait Implementations
//==============================================================================

/// Scheduler Future Trait Implementation for Operation Descriptors
impl SchedulerFuture for Operation {
    fn as_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    fn get_future(&self) -> &dyn Future<Output = ()> {
        todo!()
    }
}

/// Future Trait Implementation for Operation Descriptors
impl Future for Operation {
    type Output = ();

    /// Polls the target [FutureOperation].
    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            Operation::Accept(ref mut f) => Future::poll(Pin::new(f), ctx),
            Operation::Connect(ref mut f) => Future::poll(Pin::new(f), ctx),
            Operation::Push(ref mut f) => Future::poll(Pin::new(f), ctx),
            Operation::Pushto(ref mut f) => Future::poll(Pin::new(f), ctx),
            Operation::Pop(ref mut f) => Future::poll(Pin::new(f), ctx),
            Operation::Close(ref mut f) => Future::poll(Pin::new(f), ctx),
        }
    }
}

/// From Trait Implementation for Operation Descriptors
impl From<AcceptFuture> for Operation {
    fn from(f: AcceptFuture) -> Self {
        Operation::Accept(FutureResult::new(f, None))
    }
}

/// From Trait Implementation for Operation Descriptors
impl From<ConnectFuture> for Operation {
    fn from(f: ConnectFuture) -> Self {
        Operation::Connect(FutureResult::new(f, None))
    }
}

/// From Trait Implementation for Operation Descriptors
impl From<PushFuture> for Operation {
    fn from(f: PushFuture) -> Self {
        Operation::Push(FutureResult::new(f, None))
    }
}

/// From Trait Implementation for Operation Descriptors
impl From<PushtoFuture> for Operation {
    fn from(f: PushtoFuture) -> Self {
        Operation::Pushto(FutureResult::new(f, None))
    }
}

/// From Trait Implementation for Operation Descriptors
impl From<PopFuture> for Operation {
    fn from(f: PopFuture) -> Self {
        Operation::Pop(FutureResult::new(f, None))
    }
}

/// From Trait Implementation for Operation Descriptors
impl From<CloseFuture> for Operation {
    fn from(f: CloseFuture) -> Self {
        Operation::Close(FutureResult::new(f, None))
    }
}
