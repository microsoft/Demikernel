// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//================&======================================================================================================
// Exports
//======================================================================================================================

mod duplex_pipe;
mod queue;
mod runtime;
mod socket;
//======================================================================================================================
// Imports
//======================================================================================================================

use self::{
    duplex_pipe::DuplexPipe,
    queue::CatloopQueue,
    runtime::CatloopRuntime,
};
use crate::{
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
    net::SocketAddrV4,
    pin::Pin,
    rc::Rc,
};

//======================================================================================================================
// Structures
//======================================================================================================================

/// [CatloopLibOS] represents a multi-queue Catloop library operating system that provides the Demikernel network API
/// on top of shared memory queues provided by Catmem. [CatloopLibOS] is stateless and purely contains multi-queue
/// functionality necessary to run the Catloop libOS. All state is kept in the [state], while [runtime] holds the
/// coroutine scheduler and [catmem] holds a reference to the underlying Catmem libOS instatnce.
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
    /// Shift value that is applied to all queue tokens that are managed by the Catmem LibOS.
    /// This is required to avoid collisions between queue tokens that are managed by Catmem LibOS and Catloop LibOS.
    const QTOKEN_SHIFT: u64 = 65536;

    /// Instantiates a new LibOS.
    pub fn new(runtime: DemiRuntime) -> Self {
        Self {
            state: Rc::new(RefCell::<CatloopRuntime>::new(CatloopRuntime::new())),
            catmem: Rc::new(RefCell::<CatmemLibOS>::new(CatmemLibOS::new())),
            runtime,
        }
    }

    /// Creates a socket. This function contains the libOS-level functionality needed to create a CatloopQueue that
    /// wraps the underlying Catmem queue.
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
        let qd: QDesc = self
            .state
            .borrow_mut()
            .alloc_queue(CatloopQueue::new(qtype, self.catmem.clone())?);
        Ok(qd)
    }

    /// Binds a socket to a local endpoint. This function contains the libOS-level functionality needed to bind a
    /// CatloopQueue to a local address.
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
        let queue: CatloopQueue = state.get_queue(qd)?;

        // Check that the socket associated with the queue is not listening.
        queue.bind(local)
    }

    /// Sets a CatloopQueue and as a passive one. This function contains the libOS-level
    /// functionality to move the CatloopQueue into a listening state.
    // FIXME: https://github.com/demikernel/demikernel/issues/697
    pub fn listen(&mut self, qd: QDesc, backlog: usize) -> Result<(), Fail> {
        trace!("listen() qd={:?}, backlog={:?}", qd, backlog);

        // We just assert backlog here, because it was previously checked at PDPIX layer.
        debug_assert!((backlog > 0) && (backlog <= SOMAXCONN as usize));

        // Check if the queue descriptor is registered in the sockets table.
        let state: Ref<CatloopRuntime> = self.state.borrow();
        let queue: CatloopQueue = state.get_queue(qd)?;
        queue.listen()
    }

    /// Synchronous cross-queue code to start accepting a connection. This function schedules the asynchronous
    /// coroutine and performs any necessary synchronous, multi-queue operations at the libOS-level before beginning
    /// the accept.
    pub fn accept(&mut self, qd: QDesc) -> Result<QToken, Fail> {
        trace!("accept() qd={:?}", qd);

        let queue: CatloopQueue = self.state.borrow().get_queue(qd)?;
        let state_ptr: Rc<RefCell<CatloopRuntime>> = self.state.clone();
        let local: SocketAddrV4 = queue.local().expect("socket should be bound to local address");
        let runtime: DemiRuntime = self.runtime.clone();
        let new_port: u16 = match self.state.borrow_mut().alloc_ephemeral_port(None) {
            Ok(new_port) => new_port.unwrap(),
            Err(e) => return Err(e),
        };
        let coroutine = move |yielder: Yielder| -> Result<TaskHandle, Fail> {
            // Asynchronous accept code.
            let coroutine: Pin<Box<Operation>> =
                self.accept_coroutine(qd, SocketAddrV4::new(*local.ip(), new_port), yielder)?;
            // Insert async coroutine into the scheduler.
            let task_name: String = format!("Catloop::accept for qd={:?}", qd);
            runtime.insert_coroutine(&task_name, coroutine)
        };
        let qt: QToken = queue.accept(coroutine)?;
        state_ptr
            .borrow_mut()
            .insert_catloop_qt(qt, demi_opcode_t::DEMI_OPC_ACCEPT, qd);

        // Check if the returned queue token falls in the space of queue tokens of the Catmem LibOS.
        if Into::<u64>::into(qt) >= Self::QTOKEN_SHIFT {
            // This queue token may colide with a queue token in the Catmem LibOS. Warn and keep going.
            let message: String = format!("too many pending operations in Catloop");
            warn!("accept(): {}", &message);
        }

        Ok(qt)
    }

    /// Asynchronous cross-queue code for accepting a connection. This function returns a coroutine that runs
    /// asynchronously to accept a connection and performs any necessary multi-queue operations at the libOS-level after
    /// the accept succeeds or fails.
    fn accept_coroutine(
        &self,
        qd: QDesc,
        new_addr: SocketAddrV4,
        yielder: Yielder,
    ) -> Result<Pin<Box<Operation>>, Fail> {
        let state_ptr: Rc<RefCell<CatloopRuntime>> = self.state.clone();
        let queue: CatloopQueue = self.state.borrow().get_queue(qd)?;

        Ok(Box::pin(async move {
            // Wait for the accept to complete.
            let result: Result<CatloopQueue, Fail> = queue.do_accept(new_addr.ip(), new_addr.port(), &yielder).await;
            // Handle result: if successful, borrow the state to update state.
            let mut state_ = state_ptr.borrow_mut();
            match result {
                Ok(new_queue) => {
                    let new_qd: QDesc = state_.alloc_queue(new_queue);
                    (qd, OperationResult::Accept((new_qd, new_addr)))
                },
                Err(e) => {
                    // Rollback the port allocation.
                    if state_.free_ephemeral_port(new_addr.port()).is_err() {
                        warn!("accept(): leaking ephemeral port (port={})", new_addr.port());
                    }
                    (qd, OperationResult::Failed(e))
                },
            }
        }))
    }

    /// Synchronous code to establish a connection to a remote endpoint. This function schedules the asynchronous
    /// coroutine and performs any necessary synchronous, multi-queue operations at the libOS-level before beginning
    /// the connect.
    pub fn connect(&mut self, qd: QDesc, remote: SocketAddrV4) -> Result<QToken, Fail> {
        trace!("connect() qd={:?}, remote={:?}", qd, remote);
        let state_ptr: Rc<RefCell<CatloopRuntime>> = self.state.clone();
        let queue: CatloopQueue = self.state.borrow().get_queue(qd)?;
        let coroutine = move |yielder: Yielder| -> Result<TaskHandle, Fail> {
            let coroutine: Pin<Box<Operation>> = self.connect_coroutine(qd, remote, yielder)?;
            let task_name: String = format!("Catloop::connect for qd={:?}", qd);
            self.runtime.insert_coroutine(&task_name, coroutine)
        };
        let qt: QToken = queue.connect(coroutine)?;
        state_ptr
            .borrow_mut()
            .insert_catloop_qt(qt, demi_opcode_t::DEMI_OPC_CONNECT, qd);

        // Check if the returned queue token falls in the space of queue tokens of the Catmem LibOS.
        if Into::<u64>::into(qt) >= Self::QTOKEN_SHIFT {
            // This queue token may colide with a queue token in the Catmem LibOS. Warn and keep going.
            let message: String = format!("too many pending operations in Catloop");
            warn!("connect(): {}", &message);
        }

        Ok(qt)
    }

    /// Asynchronous code to establish a connection to a remote endpoint. This function returns a coroutine that runs
    /// asynchronously to connect a queue and performs any necessary multi-queue operations at the libOS-level after
    /// the connect succeeds or fails.
    fn connect_coroutine(
        &self,
        qd: QDesc,
        remote: SocketAddrV4,
        yielder: Yielder,
    ) -> Result<Pin<Box<Operation>>, Fail> {
        let queue: CatloopQueue = self.state.borrow().get_queue(qd)?;
        Ok(Box::pin(async move {
            // Wait for connect operation to complete.
            match queue.do_connect(remote, &yielder).await {
                Ok(()) => (qd, OperationResult::Connect),
                Err(e) => {
                    warn!("connect() failed (qd={:?}, error={:?})", qd, e.cause);
                    (qd, OperationResult::Failed(e))
                },
            }
        }))
    }

    /// Synchronously closes a CatloopQueue and its underlying Catmem queues.
    pub fn close(&self, qd: QDesc) -> Result<(), Fail> {
        trace!("close() qd={:?}", qd);
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();
        let queue: CatloopQueue = state.get_queue(qd)?;
        queue.close()?;
        if let Some(addr) = queue.local() {
            if EphemeralPorts::is_private(addr.port()) {
                if state.free_ephemeral_port(addr.port()).is_err() {
                    // We fail if and only if we attempted to free a port that was not allocated.
                    // This is unexpected, but if it happens, issue a warning and keep going,
                    // otherwise we would leave the queue in a dangling state.
                    warn!("close(): leaking ephemeral port (port={})", addr.port());
                }
            }
        }
        state.free_queue(qd);
        Ok(())
    }

    /// Synchronous code to asynchronously close a queue. This function schedules the coroutine that asynchronously
    /// runs the close and any synchronous multi-queue functionality before the close begins.
    pub fn async_close(&self, qd: QDesc) -> Result<QToken, Fail> {
        let state_ptr: Rc<RefCell<CatloopRuntime>> = self.state.clone();
        let queue: CatloopQueue = self.state.borrow().get_queue(qd)?;
        // Note that this coroutine is only inserted if we do not allocate a Catmem coroutine.
        let coroutine = move |_: Yielder| -> Result<TaskHandle, Fail> {
            let coroutine: Pin<Box<Operation>> = self.async_close_coroutine(qd)?;
            let task_name: String = format!("Catloop::close for qd={:?}", qd);
            self.runtime.insert_coroutine(&task_name, coroutine)
        };

        let (qt, inserted_catloop): (QToken, bool) = queue.async_close(coroutine)?;
        if inserted_catloop {
            state_ptr
                .borrow_mut()
                .insert_catloop_qt(qt, demi_opcode_t::DEMI_OPC_CONNECT, qd);

            // Check if the returned queue token falls in the space of queue tokens of the Catmem LibOS.
            if Into::<u64>::into(qt) >= Self::QTOKEN_SHIFT {
                // This queue token may colide with a queue token in the Catmem LibOS. Warn and keep going.
                let message: String = format!("too many pending operations in Catloop");
                warn!("async_close(): {}", &message);
            }
            Ok(qt)
        } else {
            state_ptr
                .borrow_mut()
                .insert_catmem_qt(qt, demi_opcode_t::DEMI_OPC_CLOSE, qd);
            Ok(Self::shift_qtoken(qt))
        }
    }

    /// Asynchronous code to close a queue. This function returns a coroutine that runs asynchronously to close a queue
    /// and the underlying Catmem queue and performs any necessary multi-queue operations at the libOS-level after
    /// the close succeeds or fails.
    fn async_close_coroutine(&self, qd: QDesc) -> Result<Pin<Box<Operation>>, Fail> {
        let state_ptr: Rc<RefCell<CatloopRuntime>> = self.state.clone();
        Ok(Box::pin(async move {
            let mut state: RefMut<CatloopRuntime> = state_ptr.borrow_mut();
            let queue: CatloopQueue = state.get_queue(qd).expect("queue should exist");
            match queue.do_async_close() {
                Ok(()) => {
                    if let Some(addr) = queue.local() {
                        if EphemeralPorts::is_private(addr.port()) {
                            if state.free_ephemeral_port(addr.port()).is_err() {
                                // We fail if and only if we attempted to free a port that was not allocated.
                                // This is unexpected, but if it happens, issue a warning and keep going,
                                // otherwise we would leave the queue in a dangling state.
                                warn!("close(): leaking ephemeral port (port={})", addr.port());
                            }
                        }
                    }

                    state.free_queue(qd);
                    (qd, OperationResult::Close)
                },
                Err(e) => (qd, OperationResult::Failed(e)),
            }
        }))
    }

    /// Pushes a scatter-gather array to a Catmem queue. Returns the (shifted) qtoken for the Catmem coroutine that
    /// runs this push operation.
    pub fn push(&self, qd: QDesc, sga: &demi_sgarray_t) -> Result<QToken, Fail> {
        trace!("push() qd={:?}", qd);
        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();
        let queue: CatloopQueue = state.get_queue(qd)?;
        let qt: QToken = queue.push(sga)?;
        state.insert_catmem_qt(qt, demi_opcode_t::DEMI_OPC_PUSH, qd);

        Ok(Self::shift_qtoken(qt))
    }

    /// Pops data from an underlying Catmem queue. Returns the (shifted) qtoken for the Catmem coroutine that runs this
    /// pop operations.
    pub fn pop(&mut self, qd: QDesc, size: Option<usize>) -> Result<QToken, Fail> {
        trace!("pop() qd={:?}, size={:?}", qd, size);

        // We just assert 'size' here, because it was previously checked at PDPIX layer.
        debug_assert!(size.is_none() || ((size.unwrap() > 0) && (size.unwrap() <= limits::POP_SIZE_MAX)));

        let mut state: RefMut<CatloopRuntime> = self.state.borrow_mut();
        let queue: CatloopQueue = state.get_queue(qd)?;
        let qt: QToken = queue.pop(size)?;
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
        let state: Ref<CatloopRuntime> = self.state.borrow();

        let task: OperationTask = self.runtime.remove_coroutine(&handle);
        let (qd, result): (QDesc, OperationResult) = task.get_result().expect("The coroutine has not finished");

        match state.get_queue(qd) {
            Ok(queue) => queue.remove_pending_op(&handle),
            Err(_) => debug!("take_result(): this queue was closed (qd={:?})", qd),
        };

        (qd, result)
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
