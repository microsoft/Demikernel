// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//================&======================================================================================================
// Exports
//======================================================================================================================

mod duplex_pipe;
mod futures;
mod queue;
mod runtime;
//======================================================================================================================
// Imports
//======================================================================================================================

use self::{
    duplex_pipe::DuplexPipe,
    queue::CatloopQueue,
    runtime::CatloopRuntime,
};
use crate::{
    catloop::futures::{
        accept::accept_coroutine,
        connect::connect_coroutine,
    },
    catmem::CatmemLibOS,
    demi_sgarray_t,
    inetstack::protocols::ip::EphemeralPorts,
    pal::{
        constants::SOMAXCONN,
        data_structures::SockAddr,
        linux,
    },
    runtime::{
        fail::Fail,
        limits,
        types::{
            demi_accept_result_t,
            demi_opcode_t,
            demi_qr_value_t,
            demi_qresult_t,
        },
        DemiRuntime,
        Operation,
        OperationResult,
        OperationTask,
        QDesc,
        QToken,
    },
    scheduler::{
        TaskHandle,
        Yielder,
        YielderHandle,
    },
    QType,
};
use ::std::{
    cell::{
        Ref,
        RefCell,
        RefMut,
    },
    mem,
    net::{
        Ipv4Addr,
        SocketAddrV4,
    },
    pin::Pin,
    rc::Rc,
    slice,
};

//======================================================================================================================
// Structures
//======================================================================================================================

#[derive(Copy, Clone)]
pub enum Socket {
    Active(Option<SocketAddrV4>),
    Passive(SocketAddrV4),
}

/// A LibOS that exposes exposes sockets semantics on a memory queue.
pub struct CatloopLibOS {
    /// Catloop state.
    state: Rc<RefCell<CatloopRuntime>>,
    /// Underlying catmem transport.
    catmem: Rc<RefCell<CatmemLibOS>>,
    /// Underlying coroutine runtime.
    runtime: DemiRuntime,
}

//======================================================================================================================
// Associated Functions
//======================================================================================================================

impl CatloopLibOS {
    /// Magic payload used to identify connect requests.  It must be a single
    /// byte to ensure atomicity while keeping the connection establishment
    /// protocol. The rationale for this lies on the fact that a pipe in Catmem
    /// LibOS operates atomically on bytes. If we used a longer byte sequence,
    /// we would need to introduce additional logic to make sure that
    /// concurrent processes would not be enabled to establish a connection, if
    /// they sent connection bytes in an interleaved, but legit order.
    const MAGIC_CONNECT: u8 = 0x1b;
    /// Shift value that is applied to all queue tokens that are managed by the Catmem LibOS.
    /// This is required to avoid collisions between queue tokens that are managed by Catmem LibOS and Catloop LibOS.
    const QTOKEN_SHIFT: u64 = 65536;

    /// Instantiates a new LibOS.
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::<CatloopRuntime>::new(CatloopRuntime::new())),
            catmem: Rc::new(RefCell::<CatmemLibOS>::new(CatmemLibOS::new())),
            runtime: DemiRuntime::new(),
        }
    }

    /// Creates a socket.
    pub fn socket(&mut self, domain: libc::c_int, typ: libc::c_int, _protocol: libc::c_int) -> Result<QDesc, Fail> {
        trace!("socket() domain={:?}, type={:?}, protocol={:?}", domain, typ, _protocol);

        // Parse communication domain.
        if domain != libc::AF_INET {
            let cause: String = format!("communication domain not supported (domain={:?})", domain);
            error!("socket(): {}", cause);
            return Err(Fail::new(libc::ENOTSUP, &cause));
        }

        // Parse socket type and protocol.
        let qtype: QType = match typ {
            libc::SOCK_STREAM => QType::TcpSocket,
            libc::SOCK_DGRAM => QType::UdpSocket,
            _ => {
                let cause: String = format!("socket type not supported (typ={:?})", typ);
                error!("socket(): {}", cause);
                return Err(Fail::new(libc::ENOTSUP, &cause));
            },
        };

        // Create fake socket.
        let qd: QDesc = self.state.borrow_mut().alloc_queue(qtype);
        Ok(qd)
    }

    /// Binds a socket to a local endpoint.
    pub fn bind(&mut self, qd: QDesc, local: SocketAddrV4) -> Result<(), Fail> {
        trace!("bind() qd={:?}, local={:?}", qd, local);

        // Check if we are binding to the wildcard port.
        if local.port() == 0 {
            let cause: String = format!("cannot bind to port 0 (qd={:?})", qd);
            error!("bind(): {}", cause);
            return Err(Fail::new(libc::ENOTSUP, &cause));
        }

        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();

        // Check whether the address is in use.
        if !state.check_bind_addr(local) {
            let cause: String = format!("address is already bound to a socket (qd={:?}", qd);
            error!("bind(): {}", cause);
            return Err(Fail::new(libc::EADDRINUSE, &cause));
        }
        // Check if this is an ephemeral port.
        if EphemeralPorts::is_private(local.port()) {
            // Allocate ephemeral port from the pool, to leave ephemeral port allocator in a consistent state.
            state.alloc_ephemeral_port(Some(local.port()))?;
        }

        // Check if queue descriptor is valid and get a mutable reference.
        let queue: &mut CatloopQueue = state.get_queue(qd)?;

        // Check that the socket associated with the queue is not listening.
        if let Socket::Passive(_) = queue.get_socket() {
            let cause: String = format!("Cannot bind a listening queue (qd={:?})", qd);
            error!("bind(): {}", &cause);
            return Err(Fail::new(libc::EBADF, &cause));
        }

        // Make sure the queue is not already bound to a pipe.
        if queue.get_pipe().is_some() {
            let cause: String = format!("socket is already bound to an address (qd={:?})", qd);
            error!("bind(): {}", &cause);
            return Err(Fail::new(libc::EINVAL, &cause));
        }

        // Create underlying memory channels.
        let ipv4: &Ipv4Addr = local.ip();
        let port: u16 = local.port();
        let duplex_pipe: Rc<DuplexPipe> = Rc::new(DuplexPipe::create_duplex_pipe(self.catmem.clone(), &ipv4, port)?);
        queue.set_pipe(duplex_pipe);
        queue.set_socket(Socket::Active(Some(local)));
        Ok(())
    }

    /// Sets a socket as a passive one.
    // FIXME: https://github.com/demikernel/demikernel/issues/697
    pub fn listen(&mut self, qd: QDesc, backlog: usize) -> Result<(), Fail> {
        trace!("listen() qd={:?}, backlog={:?}", qd, backlog);

        // We just assert backlog here, because it was previously checked at PDPIX layer.
        debug_assert!((backlog > 0) && (backlog <= SOMAXCONN as usize));

        // Check if the queue descriptor is registered in the sockets table.
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();
        let queue: &mut CatloopQueue = state.get_queue(qd)?;
        match queue.get_socket() {
            Socket::Active(Some(local)) => {
                queue.set_socket(Socket::Passive(local));
                Ok(())
            },
            Socket::Active(None) => {
                let cause: String = format!("Cannot call listen on an unbound socket (qd={:?})", qd);
                error!("listen(): {}", &cause);
                Err(Fail::new(libc::EOPNOTSUPP, &cause))
            },
            Socket::Passive(_) => {
                let cause: String = format!("cannot call listen on an already listening socket (qd={:?})", qd);
                error!("listen(): {}", &cause);
                Err(Fail::new(libc::EBADF, &cause))
            },
        }
    }

    /// Accepts connections on a socket.
    pub fn accept(&mut self, qd: QDesc) -> Result<QToken, Fail> {
        trace!("accept() qd={:?}", qd);

        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();
        let (control_duplex_pipe, local): (Rc<DuplexPipe>, SocketAddrV4) = {
            // Check if queue descriptor is valid and get a mutable reference.
            let queue: &mut CatloopQueue = state.get_queue(qd)?;

            // Issue accept operation.
            match queue.get_socket() {
                Socket::Passive(local) => match queue.get_pipe() {
                    Some(pipe) => (pipe.clone(), local),
                    None => {
                        let cause: String = format!("invalid queue descriptor (qd={:?})", qd);
                        error!("accept(): {}", cause);
                        return Err(Fail::new(libc::EINVAL, &cause));
                    },
                },
                Socket::Active(_) => {
                    let cause: String = format!("cannot call accept on an active socket (qd={:?})", qd);
                    error!("accept(): {}", &cause);
                    return Err(Fail::new(libc::EBADF, &cause));
                },
            }
        };

        let new_qd: QDesc = state.alloc_queue(QType::TcpSocket);
        let new_port: u16 = match state.alloc_ephemeral_port(None) {
            Ok(new_port) => new_port.unwrap(),
            Err(e) => return Err(e),
        };

        let yielder: Yielder = Yielder::new();
        // Will use this later on close.
        let yielder_handle: YielderHandle = yielder.get_handle();
        let coroutine: Pin<Box<Operation>> = self.accept_async(
            qd,
            new_qd,
            SocketAddrV4::new(local.ip().clone(), new_port),
            control_duplex_pipe,
            yielder,
        )?;
        let task_id: String = format!("Catloop::accept for qd={:?}", qd);
        let handle: TaskHandle = match self.runtime.insert_coroutine(&task_id, coroutine) {
            Ok(handle) => handle,
            Err(e) => {
                // Rollback the port allocation
                if state.free_ephemeral_port(new_port).is_err() {
                    warn!("accept(): leaking ephemeral port (port={})", new_port);
                }
                state.free_queue(new_qd);
                return Err(e);
            },
        };
        state.get_queue(qd)?.add_pending_op(&handle, &yielder_handle);
        let qt: QToken = handle.get_task_id().into();
        state.insert_catloop_qt(qt, demi_opcode_t::DEMI_OPC_ACCEPT, qd);

        // Check if the returned queue token falls in the space of queue tokens of the Catmem LibOS.
        if Into::<u64>::into(qt) >= Self::QTOKEN_SHIFT {
            // This queue token may colide with a queue token in the Catmem LibOS. Warn and keep going.
            let message: String = format!("too many pending operations in Catloop");
            warn!("accept(): {}", &message);
        }

        Ok(qt)
    }

    /// This internal function is designed to help scope the arguments that we pass to the async coroutine more
    /// explicitly.
    fn accept_async(
        &self,
        qd: QDesc,
        new_qd: QDesc,
        new_addr: SocketAddrV4,
        control_duplex_pipe: Rc<DuplexPipe>,
        yielder: Yielder,
    ) -> Result<Pin<Box<Operation>>, Fail> {
        let state_ptr: Rc<RefCell<CatloopRuntime>> = self.state.clone();
        let catmem_ptr: Rc<RefCell<CatmemLibOS>> = self.catmem.clone();

        Ok(Box::pin(async move {
            // Wait for the accept to complete.
            let result: Result<(SocketAddrV4, Rc<DuplexPipe>), Fail> =
                accept_coroutine(new_addr.ip(), catmem_ptr, control_duplex_pipe, new_addr.port(), yielder).await;
            // Handle result: if successful, borrow the state to update state.
            let mut state_ = state_ptr.borrow_mut();
            match result {
                Ok((remote, duplex_pipe)) => {
                    let queue: &mut CatloopQueue = state_
                        .get_queue(new_qd)
                        .expect("New qd should have been already allocated");
                    queue.set_socket(Socket::Active(Some(remote)));
                    queue.set_pipe(duplex_pipe.clone());
                    (qd, OperationResult::Accept((new_qd, remote)))
                },
                Err(e) => {
                    // Rollback the port allocation.
                    if state_.free_ephemeral_port(new_addr.port()).is_err() {
                        warn!("accept(): leaking ephemeral port (port={})", new_addr.port());
                    }
                    state_.free_queue(new_qd);
                    (qd, OperationResult::Failed(e))
                },
            }
        }))
    }

    /// Establishes a connection to a remote endpoint.
    pub fn connect(&mut self, qd: QDesc, remote: SocketAddrV4) -> Result<QToken, Fail> {
        trace!("connect() qd={:?}, remote={:?}", qd, remote);
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();

        // Issue connect operation.
        match state.get_queue(qd)?.get_socket() {
            Socket::Active(_) => {
                let state_ptr: Rc<RefCell<CatloopRuntime>> = self.state.clone();
                let yielder: Yielder = Yielder::new();
                // Will use this later on close.
                let yielder_handle: YielderHandle = yielder.get_handle();
                let catmem_ptr: Rc<RefCell<CatmemLibOS>> = self.catmem.clone();
                let coroutine: Pin<Box<Operation>> = Box::pin(async move {
                    let result: Result<(SocketAddrV4, Rc<DuplexPipe>), Fail> =
                        connect_coroutine(catmem_ptr, remote, yielder).await;
                    match result {
                        Ok((remote, duplex_pipe)) => {
                            let mut state_: RefMut<CatloopRuntime> = state_ptr.borrow_mut();
                            let queue: &mut CatloopQueue =
                                state_.get_queue(qd).expect("New qd should have been already allocated");
                            // TODO: check whether we need to close the original control duplex pipe allocated on bind().
                            queue.set_socket(Socket::Active(Some(remote)));
                            queue.set_pipe(duplex_pipe.clone());
                            (qd, OperationResult::Connect)
                        },
                        Err(e) => (qd, OperationResult::Failed(e)),
                    }
                });
                let task_id: String = format!("Catloop::connect for qd={:?}", qd);
                let handle: TaskHandle = self.runtime.insert_coroutine(&task_id, coroutine)?;
                state
                    .get_queue(qd)
                    .expect("queue should have already been found")
                    .add_pending_op(&handle, &yielder_handle);
                let qt: QToken = handle.get_task_id().into();
                state.insert_catloop_qt(qt, demi_opcode_t::DEMI_OPC_CONNECT, qd);

                // Check if the returned queue token falls in the space of queue tokens of the Catmem LibOS.
                if Into::<u64>::into(qt) >= Self::QTOKEN_SHIFT {
                    // This queue token may colide with a queue token in the Catmem LibOS. Warn and keep going.
                    let message: String = format!("too many pending operations in Catloop");
                    warn!("connect(): {}", &message);
                }

                Ok(qt)
            },
            Socket::Passive(_) => {
                let cause: String = format!("cannot call connect on a listening socket (qd={:?})", qd);
                error!("connect(): {}", &cause);
                Err(Fail::new(libc::EOPNOTSUPP, &cause))
            },
        }
    }

    /// Closes a socket.
    pub fn close(&mut self, qd: QDesc) -> Result<(), Fail> {
        trace!("close() qd={:?}", qd);
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();

        // Socket is not bound to a duplex pipe.
        if let Some(duplex_pipe) = state.get_queue(qd)?.get_pipe() {
            duplex_pipe.close()?;
        }

        // Rollback the port allocation.
        if let Socket::Active(Some(addr)) | Socket::Passive(addr) = state
            .get_queue(qd)
            .expect("queue should have already been found")
            .get_socket()
        {
            if EphemeralPorts::is_private(addr.port()) {
                if state.free_ephemeral_port(addr.port()).is_err() {
                    // We fail if and only if we attempted to free a port that was not allocated.
                    // This is unexpected, but if it happens, issue a warning and keep going,
                    // otherwise we would leave the queue in a dangling state.
                    warn!("close(): leaking ephemeral port (port={})", addr.port());
                }
            }
        }
        state
            .get_queue(qd)
            .expect("queue should have already been found")
            .cancel_pending_ops(Fail::new(libc::ECANCELED, "this queue was closed"));
        state.free_queue(qd);
        Ok(())
    }

    /// Asynchronously closes a socket.
    pub fn async_close(&mut self, qd: QDesc) -> Result<QToken, Fail> {
        trace!("async_close() qd={:?}", qd);
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();
        // Remove socket from sockets table.
        if let Some(duplex_pipe) = state.get_queue(qd)?.get_pipe() {
            let qt: QToken = duplex_pipe.async_close()?;
            state.insert_catloop_qt(qt, demi_opcode_t::DEMI_OPC_CLOSE, qd);
            Ok(Self::shift_qtoken(qt))
        } else {
            let yielder: Yielder = Yielder::new();
            let yielder_handle: YielderHandle = yielder.get_handle();
            let state_ptr: Rc<RefCell<CatloopRuntime>> = self.state.clone();
            let coroutine: Pin<Box<Operation>> = Box::pin(async move {
                // If this was an async_close(), harvest resources.
                let mut state: RefMut<CatloopRuntime> = state_ptr.borrow_mut();
                // Rollback the port allocation.
                if let Socket::Active(Some(addr)) | Socket::Passive(addr) =
                    state.get_queue(qd).expect("queue should exist").get_socket()
                {
                    if EphemeralPorts::is_private(addr.port()) {
                        if state.free_ephemeral_port(addr.port()).is_err() {
                            // We fail if and only if we attempted to free a port that was not allocated.
                            // This is unexpected, but if it happens, issue a warning and keep going,
                            // otherwise we would leave the queue in a dangling state.
                            warn!(
                                "pack_result(): attempting to free an ephemeral port that was not allocated (port={})",
                                addr.port()
                            );
                            warn!("pack_result(): leaking ephemeral port (port={})", addr.port());
                        }
                    }
                }
                state
                    .get_queue(qd)
                    .expect("queue should have already been found")
                    .cancel_pending_ops(Fail::new(libc::ECANCELED, "this queue was closed"));
                state.free_queue(qd);

                (qd, OperationResult::Close)
            });
            let task_id: String = format!("Catloop::close for qd={:?}", qd);
            let handle: TaskHandle = self.runtime.insert_coroutine(&task_id, coroutine)?;
            state
                .get_queue(qd)
                .expect("queue should have already been found")
                .add_pending_op(&handle, &yielder_handle);
            let qt: QToken = handle.get_task_id().into();
            state.insert_catloop_qt(qt, demi_opcode_t::DEMI_OPC_CONNECT, qd);

            // Check if the returned queue token falls in the space of queue tokens of the Catmem LibOS.
            if Into::<u64>::into(qt) >= Self::QTOKEN_SHIFT {
                // This queue token may colide with a queue token in the Catmem LibOS. Warn and keep going.
                let message: String = format!("too many pending operations in Catloop");
                warn!("async_close(): {}", &message);
            }

            Ok(qt)
        }
    }

    /// Pushes a scatter-gather array to a socket.
    pub fn push(&mut self, qd: QDesc, sga: &demi_sgarray_t) -> Result<QToken, Fail> {
        trace!("push() qd={:?}", qd);
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();
        let queue: &mut CatloopQueue = state.get_queue(qd)?;

        let catmem_qd: QDesc = match queue.get_pipe() {
            Some(duplex_pipe) => duplex_pipe.tx(),
            None => unreachable!("push() an unconnected queue"),
        };

        let qt: QToken = self.catmem.borrow_mut().push(catmem_qd, sga)?;
        state.insert_catmem_qt(qt, demi_opcode_t::DEMI_OPC_PUSH, qd);

        Ok(Self::shift_qtoken(qt))
    }

    /// Pops data from a socket.
    pub fn pop(&mut self, qd: QDesc, size: Option<usize>) -> Result<QToken, Fail> {
        trace!("pop() qd={:?}, size={:?}", qd, size);

        // We just assert 'size' here, because it was previously checked at PDPIX layer.
        debug_assert!(size.is_none() || ((size.unwrap() > 0) && (size.unwrap() <= limits::POP_SIZE_MAX)));

        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();
        let queue: &mut CatloopQueue = state.get_queue(qd)?;
        let catmem_qd: QDesc = match queue.get_pipe() {
            Some(duplex_pipe) => duplex_pipe.rx(),
            None => unreachable!("pop() an unconnected queue"),
        };

        let qt: QToken = self.catmem.borrow_mut().pop(catmem_qd, size)?;
        state.insert_catmem_qt(qt, demi_opcode_t::DEMI_OPC_POP, qd);

        Ok(Self::shift_qtoken(qt))
    }

    /// Allocates a scatter-gather array.
    pub fn sgaalloc(&self, size: usize) -> Result<demi_sgarray_t, Fail> {
        self.catmem.borrow_mut().alloc_sgarray(size)
    }

    /// Releases a scatter-gather array.
    pub fn sgafree(&self, sga: demi_sgarray_t) -> Result<(), Fail> {
        self.catmem.borrow_mut().free_sgarray(sga)
    }

    /// Inserts a queue token into the scheduler.
    pub fn schedule(&mut self, qt: QToken) -> Result<TaskHandle, Fail> {
        let state: Ref<CatloopRuntime> = self.state.borrow();

        // Check if the queue token came from the Catloop LibOS.
        if let Some((ref opcode, _)) = state.get_catloop_qt(qt) {
            // Check if the queue token concerns an expected operation.
            if opcode != &demi_opcode_t::DEMI_OPC_ACCEPT && opcode != &demi_opcode_t::DEMI_OPC_CONNECT {
                let cause: String = format!("unexpected queue token (qt={:?})", qt);
                error!("schedule(): {:?}", &cause);
                return Err(Fail::new(libc::EINVAL, &cause));
            }

            return self.runtime.from_task_id(qt.into());
        }

        // The queue token is not registered in Catloop LibOS, thus un-shift it and try Catmem LibOs.
        let qt: QToken = Self::try_unshift_qtoken(qt);

        // Check if the queue token came from the Catmem LibOS.
        if let Some((ref opcode, _)) = state.get_catmem_qt(qt) {
            // Check if the queue token concerns an expected operation.
            if opcode != &demi_opcode_t::DEMI_OPC_PUSH && opcode != &demi_opcode_t::DEMI_OPC_POP {
                let cause: String = format!("unexpected queue token (qt={:?})", qt);
                error!("schedule(): {:?}", &cause);
                return Err(Fail::new(libc::EINVAL, &cause));
            }

            // The queue token came from the Catmem LibOS, thus forward operation.
            return self.catmem.borrow().from_task_id(qt);
        }

        // The queue token is not registered in Catloop LibOS nor Catmem LibOS.
        let cause: String = format!("unregistered queue token (qt={:?})", qt);
        error!("schedule(): {:?}", &cause);
        Err(Fail::new(libc::EINVAL, &cause))
    }

    /// Constructs an operation result from a scheduler handler and queue token pair.
    pub fn pack_result(&mut self, handle: TaskHandle, qt: QToken) -> Result<demi_qresult_t, Fail> {
        // Check if the queue token came from the Catloop LibOS.
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();

        if let Some((opcode, _)) = state.free_catloop_qt(qt) {
            // Check if the queue token concerns an expected operation.
            match opcode {
                demi_opcode_t::DEMI_OPC_ACCEPT | demi_opcode_t::DEMI_OPC_CONNECT | demi_opcode_t::DEMI_OPC_CLOSE => {},
                _ => {
                    let cause: String = format!("unexpected queue token (qt={:?})", qt);
                    error!("pack_result(): {:?}", &cause);
                    return Err(Fail::new(libc::EINVAL, &cause));
                },
            }
            drop(state);
            // Construct operation result.
            let (qd, r): (QDesc, OperationResult) = self.take_result(handle);
            let qr: demi_qresult_t = pack_result(r, qd, qt.into());

            return Ok(qr);
        }

        // This is not a queue token from the Catloop LibOS, un-shift it and try Catmem LibOs.
        let qt: QToken = Self::try_unshift_qtoken(qt);

        // Check if the queue token came from the Catmem LibOS.
        if let Some((opcode, catloop_qd)) = state.free_catmem_qt(qt) {
            // Check if the queue token concerns an expected operation.
            match opcode {
                demi_opcode_t::DEMI_OPC_PUSH | demi_opcode_t::DEMI_OPC_POP | demi_opcode_t::DEMI_OPC_CLOSE => {},
                _ => {
                    let cause: String = format!("unexpected queue token (qt={:?})", qt);
                    error!("pack_result(): {:?}", &cause);
                    return Err(Fail::new(libc::EINVAL, &cause));
                },
            }

            // The queue token came from the Catmem LibOS, thus forward operation.
            let mut qr: demi_qresult_t = self.catmem.borrow_mut().pack_result(handle, qt)?;

            // We temper queue descriptor and queue token that were stored in the queue result returned by Catmem LibOS,
            // because we only distribute to the application identifiers that are managed by Catloop LibLOS.
            qr.qr_qd = catloop_qd.to_owned().into();
            qr.qr_qt = Self::shift_qtoken(qt).into();

            return Ok(qr);
        }

        // The queue token is not registered in Catloop LibOS nor Catmem LibOS.
        let cause: String = format!("unregistered queue token (qt={:?})", qt);
        error!("pack_result(): {:?}", &cause);
        Err(Fail::new(libc::EINVAL, &cause))
    }

    /// Polls scheduling queues.
    pub fn poll(&self) {
        self.catmem.borrow().poll();
        self.runtime.poll()
    }

    /// Takes out the [OperationResult] associated with the target [TaskHandle].
    fn take_result(&mut self, handle: TaskHandle) -> (QDesc, OperationResult) {
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();

        let task: OperationTask = self.runtime.remove_coroutine(&handle);
        let (qd, result): (QDesc, OperationResult) = task.get_result().expect("The coroutine has not finished");

        match state.get_queue(qd) {
            Ok(queue) => queue.remove_pending_op(&handle),
            Err(_) => debug!("take_result(): this queue was closed (qd={:?})", qd),
        };

        (qd, result)
    }

    /// Cooks a magic connect message.
    pub fn cook_magic_connect(catmem: &Rc<RefCell<CatmemLibOS>>) -> Result<demi_sgarray_t, Fail> {
        let sga: demi_sgarray_t = catmem
            .borrow_mut()
            .alloc_sgarray(mem::size_of_val(&CatloopLibOS::MAGIC_CONNECT))?;

        let ptr: *mut u8 = sga.sga_segs[0].sgaseg_buf as *mut u8;
        unsafe {
            *ptr = CatloopLibOS::MAGIC_CONNECT;
        }

        Ok(sga)
    }

    /// Checks for a magic connect message.
    pub fn is_magic_connect(sga: &demi_sgarray_t) -> bool {
        let len: usize = sga.sga_segs[0].sgaseg_len as usize;
        if len == mem::size_of_val(&CatloopLibOS::MAGIC_CONNECT) {
            let ptr: *mut u8 = sga.sga_segs[0].sgaseg_buf as *mut u8;
            let slice: &mut [u8] = unsafe { slice::from_raw_parts_mut(ptr, len) };
            let bytes = CatloopLibOS::MAGIC_CONNECT.to_ne_bytes();
            if slice[..] == bytes[..] {
                return true;
            }
        }

        false
    }

    /// Shifts a queue token by a certain amount.
    fn shift_qtoken(qt: QToken) -> QToken {
        let mut qt: u64 = qt.into();
        qt += Self::QTOKEN_SHIFT;
        qt.into()
    }

    /// Un-shifts a queue token by a certain amount. This is the inverse of [shift_qtoken].
    fn try_unshift_qtoken(qt: QToken) -> QToken {
        let mut qt: u64 = qt.into();
        // Avoid underflow.
        if qt >= Self::QTOKEN_SHIFT {
            qt -= Self::QTOKEN_SHIFT;
        }
        qt.into()
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl Drop for CatloopLibOS {
    // Nothing to do here for a stateless object.
    fn drop(&mut self) {}
}

//======================================================================================================================
// Standalone Functions
//======================================================================================================================

/// Packs a [OperationResult] into a [demi_qresult_t].
fn pack_result(result: OperationResult, qd: QDesc, qt: u64) -> demi_qresult_t {
    match result {
        OperationResult::Connect => demi_qresult_t {
            qr_opcode: demi_opcode_t::DEMI_OPC_CONNECT,
            qr_qd: qd.into(),
            qr_qt: qt,
            qr_ret: 0,
            qr_value: unsafe { mem::zeroed() },
        },
        OperationResult::Accept((new_qd, addr)) => {
            let saddr: SockAddr = linux::socketaddrv4_to_sockaddr(&addr);
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
        OperationResult::Close => demi_qresult_t {
            qr_opcode: demi_opcode_t::DEMI_OPC_CLOSE,
            qr_qd: qd.into(),
            qr_qt: qt.into(),
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
        _ => panic!("Should be forwarded on to catmem"),
    }
}
