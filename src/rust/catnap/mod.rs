// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

mod coroutines;
mod queue;
mod runtime;
mod socket;

//==============================================================================
// Exports
//==============================================================================

pub use self::{
    queue::CatnapQueue,
    runtime::PosixRuntime,
};

//==============================================================================
// Imports
//==============================================================================

use self::{
    coroutines::{
        accept::accept_coroutine,
        close::close_coroutine,
        connect::connect_coroutine,
        pop::pop_coroutine,
        push::push_coroutine,
    },
    socket::Socket,
};
use crate::{
    demikernel::config::Config,
    pal::linux,
    runtime::{
        fail::Fail,
        limits,
        memory::{
            DemiBuffer,
            MemoryRuntime,
        },
        queue::{
            IoQueueTable,
            Operation,
            OperationResult,
            OperationTask,
        },
        types::{
            demi_accept_result_t,
            demi_opcode_t,
            demi_qr_value_t,
            demi_qresult_t,
            demi_sgarray_t,
        },
        QDesc,
        QToken,
        QType,
    },
    scheduler::{
        TaskHandle,
        Yielder,
        YielderHandle,
    },
};
use ::std::{
    cell::{
        RefCell,
        RefMut,
    },
    mem,
    net::{
        Ipv4Addr,
        SocketAddrV4,
    },
    os::unix::prelude::RawFd,
    pin::Pin,
    rc::Rc,
};

//======================================================================================================================
// Types
//======================================================================================================================

//======================================================================================================================
// Structures
//======================================================================================================================

/// Catnap LibOS
pub struct CatnapLibOS {
    /// Table of queue descriptors.
    qtable: Rc<RefCell<IoQueueTable<CatnapQueue>>>,
    /// Underlying runtime.
    runtime: PosixRuntime,
}

//======================================================================================================================
// Associate Functions
//======================================================================================================================

/// Associate Functions for Catnap LibOS
impl CatnapLibOS {
    /// Instantiates a Catnap LibOS.
    pub fn new(_config: &Config) -> Self {
        let qtable: Rc<RefCell<IoQueueTable<CatnapQueue>>> = Rc::new(RefCell::new(IoQueueTable::<CatnapQueue>::new()));
        let runtime: PosixRuntime = PosixRuntime::new();
        Self { qtable, runtime }
    }

    /// Creates a socket.
    pub fn socket(&mut self, domain: libc::c_int, typ: libc::c_int, _protocol: libc::c_int) -> Result<QDesc, Fail> {
        trace!("socket() domain={:?}, type={:?}, protocol={:?}", domain, typ, _protocol);

        // Parse communication domain.
        if domain != libc::AF_INET {
            return Err(Fail::new(libc::ENOTSUP, "communication domain not supported"));
        }

        // Parse socket type and protocol.
        if (typ != libc::SOCK_STREAM) && (typ != libc::SOCK_DGRAM) {
            return Err(Fail::new(libc::ENOTSUP, "socket type not supported"));
        }

        // Create socket.
        match unsafe { libc::socket(domain, typ, 0) } {
            fd if fd >= 0 => {
                let qtype: QType = match typ {
                    libc::SOCK_STREAM => QType::TcpSocket,
                    libc::SOCK_DGRAM => QType::UdpSocket,
                    _ => return Err(Fail::new(libc::ENOTSUP, "socket type not supported")),
                };

                // Set socket options.
                unsafe {
                    if typ == libc::SOCK_STREAM {
                        if linux::set_tcp_nodelay(fd) != 0 {
                            let errno: libc::c_int = *libc::__errno_location();
                            warn!("cannot set TCP_NONDELAY option (errno={:?})", errno);
                        }
                    }
                    if linux::set_nonblock(fd) != 0 {
                        let errno: libc::c_int = *libc::__errno_location();
                        warn!("cannot set O_NONBLOCK option (errno={:?})", errno);
                    }
                    if linux::set_so_reuseport(fd) != 0 {
                        let errno: libc::c_int = *libc::__errno_location();
                        warn!("cannot set SO_REUSEPORT option (errno={:?})", errno);
                    }
                }

                trace!("socket: {:?}, domain: {:?}, typ: {:?}", fd, domain, typ);
                let qd: QDesc = self.qtable.borrow_mut().alloc(CatnapQueue::new(qtype, Some(fd)));
                Ok(qd)
            },
            _ => {
                let errno: libc::c_int = unsafe { *libc::__errno_location() };
                Err(Fail::new(errno, "failed to create socket"))
            },
        }
    }

    /// Binds a socket to a local endpoint.
    pub fn bind(&mut self, qd: QDesc, local: SocketAddrV4) -> Result<(), Fail> {
        trace!("bind() qd={:?}, local={:?}", qd, local);
        let mut qtable: RefMut<IoQueueTable<CatnapQueue>> = self.qtable.borrow_mut();

        // Check if we are binding to the wildcard address.
        // FIXME: https://github.com/demikernel/demikernel/issues/189
        if local.ip() == &Ipv4Addr::UNSPECIFIED {
            let cause: String = format!("cannot bind to wildcard address (qd={:?})", qd);
            error!("bind(): {}", cause);
            return Err(Fail::new(libc::ENOTSUP, &cause));
        }

        // Check if we are binding to the wildcard port.
        // FIXME: https://github.com/demikernel/demikernel/issues/582
        if local.port() == 0 {
            let cause: String = format!("cannot bind to port 0 (qd={:?})", qd);
            error!("bind(): {}", cause);
            return Err(Fail::new(libc::ENOTSUP, &cause));
        }

        // Check if queue descriptor is valid.
        if qtable.get(&qd).is_none() {
            let cause: String = format!("invalid queue descriptor {:?}", qd);
            error!("bind(): {}", &cause);
            return Err(Fail::new(libc::EBADF, &cause));
        }

        // Check wether the address is in use.
        for (_, queue) in qtable.get_values() {
            if let Some(addr) = queue.get_socket().local() {
                if addr == local {
                    let cause: String = format!("address is already bound to a socket (qd={:?}", qd);
                    error!("bind(): {}", &cause);
                    return Err(Fail::new(libc::EADDRINUSE, &cause));
                }
            }
        }

        // Get a mutable reference to the queue table.
        // The following call to expect() is safe because we checked before that the queue descriptor is valid.
        let queue: &mut CatnapQueue = qtable
            .get_mut(&qd)
            .expect("queue descriptor should be in the queue table");

        // Get reference to the underlying file descriptor.
        // The following call to expect() is safe because when creating a queue we assigned it a valid file descritor.
        let fd: RawFd = queue.get_fd().expect("queue should have a file descriptor");

        // Create a socket that is bound to the target local address.
        let bound_socket: Socket = {
            let unbound_socket: &Socket = queue.get_socket();
            unbound_socket.bind(local)?
        };

        // Bind underlying socket.
        let saddr: libc::sockaddr = linux::socketaddrv4_to_sockaddr(&local);
        match unsafe {
            libc::bind(
                fd,
                &saddr as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        } {
            stats if stats == 0 => {
                // Update socket.
                queue.set_socket(&bound_socket);
                Ok(())
            },
            _ => {
                let errno: libc::c_int = unsafe { *libc::__errno_location() };
                error!("failed to bind socket (errno={:?})", errno);
                Err(Fail::new(errno, "operation failed"))
            },
        }
    }

    /// Sets a socket as a passive one.
    pub fn listen(&mut self, qd: QDesc, backlog: usize) -> Result<(), Fail> {
        trace!("listen() qd={:?}, backlog={:?}", qd, backlog);

        // We just assert backlog here, because it was previously checked at PDPIX layer.
        debug_assert!((backlog > 0) && (backlog <= libc::SOMAXCONN as usize));

        // Issue listen operation.
        match self.qtable.borrow_mut().get_mut(&qd) {
            Some(queue) => match queue.get_fd() {
                Some(fd) => {
                    // Create a listening socket.
                    let listening_socket: Socket = {
                        let bound_socket: &Socket = queue.get_socket();
                        bound_socket.listen()?
                    };

                    if unsafe { libc::listen(fd, backlog as i32) } != 0 {
                        let errno: libc::c_int = unsafe { *libc::__errno_location() };
                        error!("failed to listen ({:?})", errno);
                        return Err(Fail::new(errno, "operation failed"));
                    }

                    // Update socket.
                    queue.set_socket(&listening_socket);
                    Ok(())
                },
                None => unreachable!("CatnapQueue has invalid underlying file descriptor"),
            },
            None => Err(Fail::new(libc::EBADF, "invalid queue descriptor")),
        }
    }

    /// Accepts connections on a socket.
    pub fn accept(&mut self, qd: QDesc) -> Result<QToken, Fail> {
        trace!("accept(): qd={:?}", qd);
        let mut qtable: RefMut<IoQueueTable<CatnapQueue>> = self.qtable.borrow_mut();
        let fd: RawFd = match qtable.get_mut(&qd) {
            Some(queue) => match queue.get_fd() {
                Some(fd) => {
                    // Create an accepting socket.
                    {
                        let listening_socket: &Socket = queue.get_socket();
                        let accepting_socket: Socket = listening_socket.accept()?;
                        queue.set_socket(&accepting_socket);
                    };
                    fd
                },
                None => unreachable!("CatnapQueue has invalid underlying file descriptor"),
            },
            None => return Err(Fail::new(libc::EBADF, "invalid queue descriptor")),
        };

        let new_qd: QDesc = qtable.alloc(CatnapQueue::new(QType::TcpSocket, None));
        let qtable_ptr: Rc<RefCell<IoQueueTable<CatnapQueue>>> = self.qtable.clone();
        let yielder: Yielder = Yielder::new();
        let yielder_handle: YielderHandle = yielder.get_handle();
        let coroutine: Pin<Box<Operation>> = Box::pin(async move {
            // Wait for the accept operation to complete.
            let result: Result<(RawFd, SocketAddrV4), Fail> = accept_coroutine(fd, yielder).await;
            // Handle result: Borrow the queue table to either set the socket fd and addr or free the queue
            // metadata on error.
            match result {
                Ok((new_fd, addr)) => {
                    let mut qtable_: RefMut<IoQueueTable<CatnapQueue>> = qtable_ptr.borrow_mut();

                    // Update new (connected) socket.
                    // Note that we do it first, because it is unlikely that the new socket has been closed.
                    {
                        let queue: &mut CatnapQueue = match qtable_.get_mut(&new_qd) {
                            Some(queue) => queue,
                            None => {
                                let cause: String = format!("invalid queue descriptor {:?}", new_qd);
                                error!("accept(): {}", &cause);
                                return (qd, OperationResult::Failed(Fail::new(libc::EBADF, &cause)));
                            },
                        };

                        let connected_socket: Socket = {
                            let unbound_socket: &Socket = queue.get_socket();
                            match unbound_socket.connected(addr) {
                                Ok(socket) => socket,
                                Err(e) => return (qd, OperationResult::Failed(e)),
                            }
                        };
                        queue.set_socket(&connected_socket);
                        queue.set_fd(new_fd);
                    }

                    // Update listening socket.
                    {
                        let queue: &mut CatnapQueue = match qtable_.get_mut(&qd) {
                            Some(queue) => queue,
                            None => {
                                let cause: String = format!("invalid queue descriptor {:?}", qd);
                                error!("accept(): {}", &cause);
                                return (qd, OperationResult::Failed(Fail::new(libc::EBADF, &cause)));
                            },
                        };

                        let listening_socket: Socket = {
                            let accepting_socket: &Socket = queue.get_socket();
                            match accepting_socket.accepted() {
                                Ok(socket) => socket,
                                Err(e) => return (qd, OperationResult::Failed(e)),
                            }
                        };
                        queue.set_socket(&listening_socket);
                    }
                    (qd, OperationResult::Accept((new_qd, addr)))
                },
                Err(e) => {
                    warn!("accept() listening_qd={:?} new_qd={:?}: {:?}", qd, new_qd, &e);
                    // assert definitely no pending ops on new_qd
                    qtable_ptr.borrow_mut().free(&new_qd);
                    (qd, OperationResult::Failed(e))
                },
            }
        });
        let task_id: String = format!("Catnap::pop for qd={:?}", qd);
        let task: OperationTask = OperationTask::new(task_id, coroutine);
        match self.runtime.scheduler.insert(task) {
            Some(handle) => {
                // Borrow the scheduler handle and yielder handle to register a way to wake the coroutine.
                // Safe to unwrap here because we have a linear flow from the last time that we looked up the queue.
                qtable.get_mut(&qd).unwrap().add_pending_op(&handle, &yielder_handle);
                Ok(handle.get_task_id().into())
            },
            None => {
                qtable.free(&new_qd);
                Err(Fail::new(libc::EAGAIN, "cannot schedule co-routine"))
            },
        }
    }

    /// Establishes a connection to a remote endpoint.
    pub fn connect(&mut self, qd: QDesc, remote: SocketAddrV4) -> Result<QToken, Fail> {
        trace!("connect() qd={:?}, remote={:?}", qd, remote);
        // Issue connect operation.
        match self.qtable.borrow_mut().get_mut(&qd) {
            Some(queue) => match queue.get_fd() {
                Some(fd) => {
                    // Create a connecting socket.
                    let active_socket: &Socket = queue.get_socket();
                    let previous_socket_state: Socket = active_socket.clone();
                    let connecting_socket: Socket = active_socket.connect(remote)?;

                    // Spawn connect co-rountine.
                    let qtable_ptr: Rc<RefCell<IoQueueTable<CatnapQueue>>> = self.qtable.clone();
                    let yielder: Yielder = Yielder::new();
                    let yielder_handle: YielderHandle = yielder.get_handle();
                    let coroutine: Pin<Box<Operation>> = Box::pin(async move {
                        let result: Result<(), Fail> = connect_coroutine(fd, remote, yielder).await;

                        // Parse result.
                        match result {
                            Ok(()) => {
                                // Succeeded to connect, thus set socket as "connected".
                                let mut qtable_: RefMut<IoQueueTable<CatnapQueue>> = qtable_ptr.borrow_mut();
                                let queue: &mut CatnapQueue = qtable_.get_mut(&qd).expect("queue cannot be None");
                                let connecting_socket: &Socket = queue.get_socket();
                                let connected_socket: Socket = match connecting_socket.connected(remote) {
                                    Ok(socket) => socket,
                                    Err(e) => {
                                        warn!(
                                            "connect() failed to set socket as connected (qd={:?}, error={:?})",
                                            qd, &e
                                        );
                                        return (qd, OperationResult::Failed(e));
                                    },
                                };
                                queue.set_socket(&connected_socket);
                                (qd, OperationResult::Connect)
                            },
                            Err(e) => {
                                // Failed to connect, check if that happened due to other reason than cancellation.
                                if e.errno != libc::ECANCELED {
                                    let mut qtable_: RefMut<IoQueueTable<CatnapQueue>> = qtable_ptr.borrow_mut();
                                    let queue: &mut CatnapQueue = qtable_.get_mut(&qd).expect("queue cannot be None");
                                    let socket: &Socket = queue.get_socket();

                                    // Failed to connect, rollback only if the operation was not cancelled.
                                    if socket.is_connecting() {
                                        debug!(
                                            "connect() rolling back socket state (qd={:?}, state={:?})",
                                            qd, previous_socket_state
                                        );
                                        queue.set_socket(&previous_socket_state);
                                    }
                                }

                                warn!("connect() failed (qd={:?}, error={:?})", qd, e.cause);
                                (qd, OperationResult::Failed(e))
                            },
                        }
                    });
                    let task_id: String = format!("Catnap::connect for qd={:?}", qd);
                    let task: OperationTask = OperationTask::new(task_id, coroutine);
                    let handle: TaskHandle = match self.runtime.scheduler.insert(task) {
                        Some(handle) => handle,
                        None => return Err(Fail::new(libc::EAGAIN, "cannot schedule co-routine")),
                    };

                    // Borrow the scheduler handle and yielder handle to register a way to wake the coroutine.
                    queue.add_pending_op(&handle, &yielder_handle);
                    // Update socket.
                    queue.set_socket(&connecting_socket);
                    Ok(handle.get_task_id().into())
                },
                None => unreachable!("CatnapQueue has invalid underlying file descriptor"),
            },
            None => Err(Fail::new(libc::EBADF, "invalid queue descriptor")),
        }
    }

    /// Closes a socket.
    pub fn close(&mut self, qd: QDesc) -> Result<(), Fail> {
        trace!("close() qd={:?}", qd);
        let mut qtable: RefMut<IoQueueTable<CatnapQueue>> = self.qtable.borrow_mut();
        match qtable.get_mut(&qd) {
            Some(queue) => match queue.get_fd() {
                Some(fd) => {
                    // Set socket as closing.
                    let socket: &Socket = queue.get_socket();
                    let closing_socket: Socket = socket.close()?;
                    queue.set_socket(&closing_socket);

                    // Close underlying socket.
                    if unsafe { libc::close(fd) } != 0 {
                        let errno: libc::c_int = unsafe { *libc::__errno_location() };
                        let cause: String = format!("failed to close underlying socket (errno={:?})", errno);
                        error!("close(): {:?}", &cause);
                        return Err(Fail::new(errno, &cause));
                    }

                    // Cancel all pending operations.
                    queue.cancel_pending_ops(Fail::new(libc::ECANCELED, "This queue was closed"));

                    // Update socket state.
                    let socket: &Socket = queue.get_socket();
                    let closed_socket: Socket = socket.closed()?;
                    queue.set_socket(&closed_socket);

                    qtable.free(&qd);
                    Ok(())
                },
                None => unreachable!("CatnapQueue has invalid underlying file descriptor"),
            },
            None => Err(Fail::new(libc::EBADF, "invalid queue descriptor")),
        }
    }

    /// Asynchronous close
    pub fn async_close(&mut self, qd: QDesc) -> Result<QToken, Fail> {
        trace!("async_close() qd={:?}", qd);
        let mut qtable: RefMut<IoQueueTable<CatnapQueue>> = self.qtable.borrow_mut();

        match qtable.get_mut(&qd) {
            Some(queue) => match queue.get_fd() {
                Some(fd) => {
                    // Set socket as closing.
                    let socket: &Socket = queue.get_socket();
                    let closing_socket: Socket = socket.close()?;
                    queue.set_socket(&closing_socket);

                    let qtable_ptr: Rc<RefCell<IoQueueTable<CatnapQueue>>> = self.qtable.clone();
                    // Don't register this Yielder because we shouldn't have to cancel the close operation.
                    let yielder: Yielder = Yielder::new();
                    let coroutine: Pin<Box<Operation>> = Box::pin(async move {
                        // Wait for close operation to complete.
                        let result: Result<(), Fail> = close_coroutine(fd, yielder).await;
                        // Handle result: if successful, borrow the queue table to free the qd and metadata.
                        match result {
                            Ok(()) => {
                                // Update socket.
                                let mut qtable_: RefMut<IoQueueTable<CatnapQueue>> = qtable_ptr.borrow_mut();
                                match qtable_.get_mut(&qd) {
                                    Some(queue) => {
                                        // Cancel all pending operations.
                                        queue.cancel_pending_ops(Fail::new(libc::ECANCELED, "This queue was closed"));
                                        // Update socket state.
                                        let socket: &Socket = queue.get_socket();
                                        let closed_socket: Socket = match socket.closed() {
                                            Ok(socket) => socket,
                                            Err(e) => {
                                                // This is unlikely to happen. If it does, it means that we have
                                                // some uncovered race condition in the close co-routine.
                                                warn!("close(): failed to set socket state ({:?})", e);
                                                return (qd, OperationResult::Failed(e));
                                            },
                                        };
                                        queue.set_socket(&closed_socket);
                                    },
                                    None => {
                                        let cause: &String = &format!("invalid queue descriptor: {:?}", qd);
                                        error!("{}", &cause);
                                        return (qd, OperationResult::Failed(Fail::new(libc::EBADF, cause)));
                                    },
                                }
                                qtable_.free(&qd);
                                (qd, OperationResult::Close)
                            },
                            Err(e) => {
                                warn!("async_close() qd={:?}: {:?}", qd, &e);
                                (qd, OperationResult::Failed(e))
                            },
                        }
                    });
                    let task_id: String = format!("Catnap::close for qd={:?}", qd);
                    let task: OperationTask = OperationTask::new(task_id, coroutine);
                    let handle: TaskHandle = match self.runtime.scheduler.insert(task) {
                        Some(handle) => handle,
                        None => return Err(Fail::new(libc::EAGAIN, "cannot schedule co-routine")),
                    };
                    Ok(handle.get_task_id().into())
                },
                None => unreachable!("CatnapQueue has invalid underlying file descriptor"),
            },
            None => return Err(Fail::new(libc::EBADF, "invalid queue descriptor")),
        }
    }

    /// Pushes a scatter-gather array to a socket.
    pub fn push(&mut self, qd: QDesc, sga: &demi_sgarray_t) -> Result<QToken, Fail> {
        trace!("push() qd={:?}", qd);
        match self.runtime.clone_sgarray(sga) {
            Ok(buf) => {
                if buf.len() == 0 {
                    return Err(Fail::new(libc::EINVAL, "zero-length buffer"));
                }

                // Issue push operation.
                match self.qtable.borrow_mut().get_mut(&qd) {
                    Some(queue) => match queue.get_fd() {
                        Some(fd) => {
                            let yielder: Yielder = Yielder::new();
                            let yielder_handle: YielderHandle = yielder.get_handle();
                            let coroutine: Pin<Box<Operation>> = Box::pin(async move {
                                // Wait for push to complete.
                                let result: Result<(), Fail> = push_coroutine(fd, buf, None, yielder).await;
                                // Handle result.
                                match result {
                                    Ok(()) => (qd, OperationResult::Push),
                                    Err(e) => {
                                        warn!("push() qd={:?}: {:?}", qd, &e);
                                        (qd, OperationResult::Failed(e))
                                    },
                                }
                            });
                            let task_id: String = format!("Catnap::push for qd={:?}", qd);
                            let task: OperationTask = OperationTask::new(task_id, coroutine);
                            let handle: TaskHandle = match self.runtime.scheduler.insert(task) {
                                Some(handle) => handle,
                                None => return Err(Fail::new(libc::EAGAIN, "cannot schedule co-routine")),
                            };
                            // Borrow the scheduler handle and yielder handle to register a way to wake the coroutine.
                            queue.add_pending_op(&handle, &yielder_handle);

                            Ok(handle.get_task_id().into())
                        },
                        None => unreachable!("CatnapQueue has invalid underlying file descriptor"),
                    },
                    None => Err(Fail::new(libc::EBADF, "invalid queue descriptor")),
                }
            },
            Err(e) => Err(e),
        }
    }

    /// Pushes a scatter-gather array to a socket.
    pub fn pushto(&mut self, qd: QDesc, sga: &demi_sgarray_t, remote: SocketAddrV4) -> Result<QToken, Fail> {
        trace!("pushto() qd={:?}", qd);

        match self.runtime.clone_sgarray(sga) {
            Ok(buf) => {
                if buf.len() == 0 {
                    return Err(Fail::new(libc::EINVAL, "zero-length buffer"));
                }

                // Issue pushto operation.
                match self.qtable.borrow_mut().get_mut(&qd) {
                    Some(queue) => match queue.get_fd() {
                        Some(fd) => {
                            let yielder: Yielder = Yielder::new();
                            let yielder_handle: YielderHandle = yielder.get_handle();
                            let coroutine: Pin<Box<Operation>> = Box::pin(async move {
                                // Wait for pushto to complete.
                                let result: Result<(), Fail> = push_coroutine(fd, buf, Some(remote), yielder).await;
                                // Process result.
                                match result {
                                    Ok(()) => (qd, OperationResult::Push),
                                    Err(e) => {
                                        warn!("pushto() qd={:?}: {:?}", qd, &e);
                                        (qd, OperationResult::Failed(e))
                                    },
                                }
                            });
                            let task_id: String = format!("Catnap::pushto for qd={:?}", qd);
                            let task: OperationTask = OperationTask::new(task_id, Box::pin(coroutine));
                            let handle: TaskHandle = match self.runtime.scheduler.insert(task) {
                                Some(handle) => handle,
                                None => return Err(Fail::new(libc::EAGAIN, "cannot schedule co-routine")),
                            };
                            // Borrow the scheduler handle and yielder handle to register a way to wake the coroutine.
                            queue.add_pending_op(&handle, &yielder_handle);
                            Ok(handle.get_task_id().into())
                        },
                        None => unreachable!("CatnapQueue has invalid underlying file descriptor"),
                    },
                    None => Err(Fail::new(libc::EBADF, "invalid queue descriptor")),
                }
            },
            Err(e) => Err(e),
        }
    }

    /// Pops data from a socket.
    pub fn pop(&mut self, qd: QDesc, size: Option<usize>) -> Result<QToken, Fail> {
        trace!("pop() qd={:?}, size={:?}", qd, size);

        // We just assert 'size' here, because it was previously checked at PDPIX layer.
        debug_assert!(size.is_none() || ((size.unwrap() > 0) && (size.unwrap() <= limits::POP_SIZE_MAX)));

        // Issue pop operation.
        match self.qtable.borrow_mut().get_mut(&qd) {
            Some(queue) => match queue.get_fd() {
                Some(fd) => {
                    let yielder: Yielder = Yielder::new();
                    let yielder_handle: YielderHandle = yielder.get_handle();
                    let coroutine: Pin<Box<Operation>> = Box::pin(async move {
                        // Wait for pop to complete.
                        let result: Result<(Option<SocketAddrV4>, DemiBuffer), Fail> =
                            pop_coroutine(fd, size, yielder).await;
                        // Process result.
                        match result {
                            Ok((addr, buf)) => (qd, OperationResult::Pop(addr, buf)),
                            Err(e) => {
                                warn!("pop() qd={:?}: {:?}", qd, &e);
                                (qd, OperationResult::Failed(e))
                            },
                        }
                    });
                    let task_id: String = format!("Catnap::pop for qd={:?}", qd);
                    let task: OperationTask = OperationTask::new(task_id, Box::pin(coroutine));
                    let handle: TaskHandle = match self.runtime.scheduler.insert(task) {
                        Some(handle) => handle,
                        None => return Err(Fail::new(libc::EAGAIN, "cannot schedule co-routine")),
                    };
                    // Borrow the scheduler handle and yielder handle to register a way to wake the coroutine.
                    queue.add_pending_op(&handle, &yielder_handle);
                    let qt: QToken = handle.get_task_id().into();
                    Ok(qt)
                },
                None => unreachable!("CatnapQueue has invalid underlying file descriptor"),
            },
            None => Err(Fail::new(libc::EBADF, "invalid queue descriptor")),
        }
    }

    pub fn poll(&self) {
        self.runtime.scheduler.poll()
    }

    pub fn schedule(&mut self, qt: QToken) -> Result<TaskHandle, Fail> {
        match self.runtime.scheduler.from_task_id(qt.into()) {
            Some(handle) => Ok(handle),
            None => return Err(Fail::new(libc::EINVAL, "invalid queue token")),
        }
    }

    pub fn pack_result(&mut self, handle: TaskHandle, qt: QToken) -> Result<demi_qresult_t, Fail> {
        let (qd, r): (QDesc, OperationResult) = self.take_result(handle);
        Ok(pack_result(&self.runtime, r, qd, qt.into()))
    }

    /// Allocates a scatter-gather array.
    pub fn sgaalloc(&self, size: usize) -> Result<demi_sgarray_t, Fail> {
        trace!("sgalloc() size={:?}", size);
        self.runtime.alloc_sgarray(size)
    }

    /// Frees a scatter-gather array.
    pub fn sgafree(&self, sga: demi_sgarray_t) -> Result<(), Fail> {
        trace!("sgafree()");
        self.runtime.free_sgarray(sga)
    }

    /// Takes out the result from the [OperationTask] associated with the target [TaskHandle].
    fn take_result(&mut self, handle: TaskHandle) -> (QDesc, OperationResult) {
        let task: OperationTask = if let Some(task) = self.runtime.scheduler.remove(&handle) {
            OperationTask::from(task.as_any())
        } else {
            panic!("Removing task that does not exist (either was previously removed or never inserted)");
        };

        let (qd, result): (QDesc, OperationResult) = task.get_result().expect("The coroutine has not finished");
        match result {
            OperationResult::Close => {},
            _ => {
                match self.qtable.borrow_mut().get_mut(&qd) {
                    Some(queue) => queue.remove_pending_op(&handle),
                    None => debug!("Catnap::take_result() qd={:?}, This queue was closed", qd),
                };
            },
        }

        (qd, result)
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl Drop for CatnapLibOS {
    // Releases all sockets allocated by Catnap.
    fn drop(&mut self) {
        for (_, queue) in self.qtable.borrow().get_values() {
            if let Some(fd) = queue.get_fd() {
                if unsafe { libc::close(fd) } != 0 {
                    let errno: libc::c_int = unsafe { *libc::__errno_location() };
                    error!("close() failed (error={:?}", errno);
                    warn!("leaking fd={:?}", fd);
                }
            }
        }
    }
}

//==============================================================================
// Standalone Functions
//==============================================================================

/// Packs a [OperationResult] into a [demi_qresult_t].
fn pack_result(rt: &PosixRuntime, result: OperationResult, qd: QDesc, qt: u64) -> demi_qresult_t {
    match result {
        OperationResult::Connect => demi_qresult_t {
            qr_opcode: demi_opcode_t::DEMI_OPC_CONNECT,
            qr_qd: qd.into(),
            qr_qt: qt,
            qr_ret: 0,
            qr_value: unsafe { mem::zeroed() },
        },
        OperationResult::Accept((new_qd, addr)) => {
            let saddr: libc::sockaddr = linux::socketaddrv4_to_sockaddr(&addr);
            let qr_value: demi_qr_value_t = demi_qr_value_t {
                ares: demi_accept_result_t {
                    qd: new_qd.into(),
                    addr: saddr,
                },
            };
            demi_qresult_t {
                qr_opcode: demi_opcode_t::DEMI_OPC_ACCEPT,
                qr_qd: qd.into(),
                qr_qt: qt,
                qr_ret: 0,
                qr_value,
            }
        },
        OperationResult::Push => demi_qresult_t {
            qr_opcode: demi_opcode_t::DEMI_OPC_PUSH,
            qr_qd: qd.into(),
            qr_qt: qt,
            qr_ret: 0,
            qr_value: unsafe { mem::zeroed() },
        },
        OperationResult::Pop(addr, bytes) => match rt.into_sgarray(bytes) {
            Ok(mut sga) => {
                if let Some(addr) = addr {
                    sga.sga_addr = linux::socketaddrv4_to_sockaddr(&addr);
                }
                let qr_value: demi_qr_value_t = demi_qr_value_t { sga };
                demi_qresult_t {
                    qr_opcode: demi_opcode_t::DEMI_OPC_POP,
                    qr_qd: qd.into(),
                    qr_qt: qt,
                    qr_ret: 0,
                    qr_value,
                }
            },
            Err(e) => {
                warn!("Operation Failed: {:?}", e);
                demi_qresult_t {
                    qr_opcode: demi_opcode_t::DEMI_OPC_FAILED,
                    qr_qd: qd.into(),
                    qr_qt: qt,
                    qr_ret: e.errno as i64,
                    qr_value: unsafe { mem::zeroed() },
                }
            },
        },
        OperationResult::Close => demi_qresult_t {
            qr_opcode: demi_opcode_t::DEMI_OPC_CLOSE,
            qr_qd: qd.into(),
            qr_qt: qt,
            qr_ret: 0,
            qr_value: unsafe { mem::zeroed() },
        },
        OperationResult::Failed(e) => {
            warn!("Operation Failed: {:?}", e);
            demi_qresult_t {
                qr_opcode: demi_opcode_t::DEMI_OPC_FAILED,
                qr_qd: qd.into(),
                qr_qt: qt,
                qr_ret: e.errno as i64,
                qr_value: unsafe { mem::zeroed() },
            }
        },
    }
}
