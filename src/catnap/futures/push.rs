// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//==============================================================================
// Imports
//==============================================================================

use ::nix::{
    errno::Errno,
    sys::socket,
};
use ::runtime::{
    fail::Fail,
    memory::Bytes,
    QDesc,
};
use ::std::{
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

/// Push Operation Descriptor
pub struct PushFuture {
    /// Associated queue descriptor.
    qd: QDesc,
    // Underlying file descriptor.
    fd: RawFd,
    /// Buffer to send.
    buf: Bytes,
}

//==============================================================================
// Associate Functions
//==============================================================================

/// Associate Functions for Push Operation Descriptors
impl PushFuture {
    /// Creates a descriptor for a push operation.
    pub fn new(qd: QDesc, fd: RawFd, buf: Bytes) -> Self {
        Self { qd, fd, buf }
    }

    /// Returns the queue descriptor associated to the target [PushFuture].
    pub fn get_qd(&self) -> QDesc {
        self.qd
    }
}

//==============================================================================
// Trait Implementations
//==============================================================================

/// Future Trait Implementation for Push Operation Descriptors
impl Future for PushFuture {
    type Output = Result<(), Fail>;

    /// Polls the target [PushFuture].
    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        let self_: &mut PushFuture = self.get_mut();
        match socket::send(self_.fd, &self_.buf[..], socket::MsgFlags::empty()) {
            // Some bytes were sent, operation completed.
            Ok(nbytes) if nbytes > 0 => {
                trace!("data pushed ({:?}/{:?} bytes)", nbytes, self_.buf.len());
                Poll::Ready(Ok(()))
            },
            // No bytes were sent, let the user know about this.
            Ok(nbytes) if nbytes == 0 => {
                warn!("no data pushed");
                Poll::Ready(Err(Fail::new(libc::EAGAIN, "no data pushed")))
            },
            // Operation in progress.
            Err(e) if e == Errno::EWOULDBLOCK || e == Errno::EAGAIN => {
                ctx.waker().wake_by_ref();
                Poll::Pending
            },
            // Error.
            Err(e) => {
                warn!("push failed ({:?})", e);
                Poll::Ready(Err(Fail::new(e as i32, "operation failed")))
            },
            // Should not happen.
            _ => panic!("unexpected error for send()"),
        }
    }
}
