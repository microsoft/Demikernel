// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{
    catmem::SharedCatmemLibOS,
    pal,
    runtime::{
        fail::Fail,
        memory::{
            DemiBuffer,
            MemoryRuntime,
        },
        network::socket::{
            operation::SocketOp,
            state::SocketStateMachine,
        },
        queue::{
            QDesc,
            QToken,
        },
        scheduler::Yielder,
        types::{
            demi_opcode_t,
            demi_qresult_t,
            demi_sgarray_t,
        },
        OperationResult,
        SharedDemiRuntime,
    },
};
use ::rand::{
    rngs::SmallRng,
    RngCore,
    SeedableRng,
};
use ::socket2::Type;
use ::std::{
    collections::HashSet,
    mem,
    net::{
        Ipv4Addr,
        SocketAddrV4,
    },
    slice,
};

//======================================================================================================================
// Constants
//======================================================================================================================

/// Maximum number of connection attempts.
/// This was chosen arbitrarily.
const MAX_ACK_RECEIVED_ATTEMPTS: usize = 1024;
/// Seed number for generating request IDs.
#[cfg(debug_assertions)]
const REQUEST_ID_SEED: u64 = 95;

//======================================================================================================================
// Structures
//======================================================================================================================

/// A socket.
pub struct Socket {
    /// The state of the socket.
    state: SocketStateMachine,
    /// Underlying shared runtime.
    runtime: SharedDemiRuntime,
    /// Underlying Catmem LibOS.
    catmem: SharedCatmemLibOS,
    /// Underlying shared memory pipe.
    catmem_qd: Option<QDesc>,
    /// The local address to which the socket is bound.
    local: Option<SocketAddrV4>,
    /// The remote address to which the socket is connected.
    remote: Option<SocketAddrV4>,
    /// Maximum backlog length for passive sockets.
    backlog: usize,
    /// Pending connect requests for passive sockets.
    pending_request_ids: HashSet<RequestId>,
    /// Random number generator for request ids.
    rng: SmallRng,
}

/// Unique identifier for a request.
#[derive(Hash, Eq, PartialEq, Copy, Clone)]
pub struct RequestId(u64);

//======================================================================================================================
// Associated Functions
//======================================================================================================================

impl Socket {
    /// Creates a new socket that is not bound to an address.
    pub fn new(runtime: SharedDemiRuntime, catmem: SharedCatmemLibOS) -> Result<Self, Fail> {
        Ok(Self {
            state: SocketStateMachine::new_unbound(Type::STREAM),
            runtime,
            catmem,
            catmem_qd: None,
            local: None,
            remote: None,
            backlog: 1,
            pending_request_ids: HashSet::<RequestId>::new(),
            #[cfg(debug_assertions)]
            rng: SmallRng::seed_from_u64(REQUEST_ID_SEED),
            #[cfg(not(debug_assertions))]
            rng: SmallRng::from_entropy(),
        })
    }

    /// Allocates a new socket that is bound to [local].
    fn alloc(
        runtime: SharedDemiRuntime,
        catmem: SharedCatmemLibOS,
        catmem_qd: QDesc,
        local: Option<SocketAddrV4>,
        remote: Option<SocketAddrV4>,
    ) -> Self {
        Self {
            state: SocketStateMachine::new_established(),
            runtime,
            catmem,
            catmem_qd: Some(catmem_qd),
            local,
            remote,
            backlog: 1,
            pending_request_ids: HashSet::<RequestId>::new(),
            #[cfg(debug_assertions)]
            rng: SmallRng::seed_from_u64(REQUEST_ID_SEED),
            #[cfg(not(debug_assertions))]
            rng: SmallRng::from_entropy(),
        }
    }

    /// Binds the target socket to `local` address.
    /// TODO: Should probably move the create of the duplex pipe to listen.
    pub fn bind(&mut self, local: SocketAddrV4) -> Result<(), Fail> {
        self.state.prepare(SocketOp::Bind)?;
        // Create underlying memory channels.
        let ipv4: &Ipv4Addr = local.ip();
        let port: u16 = local.port();
        self.catmem_qd = match self.catmem.create_pipe(&format_pipe_str(ipv4, port)) {
            Ok(qd) => {
                self.state.commit();
                Some(qd)
            },
            Err(e) => {
                self.state.abort();
                return Err(e);
            },
        };
        self.local = Some(local);
        Ok(())
    }

    /// Enables this socket to accept incoming connections.
    pub fn listen(&mut self, backlog: usize) -> Result<(), Fail> {
        // We just assert backlog here, because it was previously checked at PDPIX layer.
        debug_assert!((backlog > 0) && (backlog <= pal::constants::SOMAXCONN as usize));

        self.state.prepare(SocketOp::Listen)?;
        self.backlog = backlog;
        self.state.commit();
        Ok(())
    }

    pub fn accept<F>(&mut self, coroutine_constructor: F) -> Result<QToken, Fail>
    where
        F: FnOnce() -> Result<QToken, Fail>,
    {
        self.state.may_accept()?;
        self.do_generic_sync_control_path_call(coroutine_constructor)
    }

    /// Attempts to accept a new connection on this socket. On success, returns a new Socket for the accepted connection.
    pub async fn do_accept(&mut self, ipv4: Ipv4Addr, new_port: u16, yielder: &Yielder) -> Result<Self, Fail> {
        loop {
            self.state.may_accept()?;

            // Check if backlog is full.
            if self.pending_request_ids.len() >= self.backlog {
                // It is, thus just log a debug message, pending requests will still be queued anyways.
                let cause: String = format!(
                    "backlog is full (backlog={:?}), pending={:?}",
                    self.backlog,
                    self.pending_request_ids.len()
                );
                debug!("do_accept(): {:?}", &cause);
            }

            // Grab next request from the control duplex pipe.
            let new_qd: QDesc = match self
                .pop_request_id(self.catmem_qd.expect("should be connected"), &yielder)
                .await
            {
                // Received a request id so create the new connection. This involves create the new duplex pipe
                // and sending the port number to the remote.
                Ok(request_id) => {
                    if self.pending_request_ids.contains(&request_id) {
                        debug!("do_accept(): duplicate request (request_id={:?})", request_id.0);
                        continue;
                    } else {
                        self.pending_request_ids.insert(request_id);
                        match self.create_pipe(&ipv4, new_port, &yielder).await {
                            Ok(new_qd) => new_qd,
                            Err(e) => {
                                return Err(e);
                            },
                        }
                    }
                },
                // Some error.
                Err(e) => {
                    return Err(e);
                },
            };

            let new_socket: Self = Self::alloc(
                self.runtime.clone(),
                self.catmem.clone(),
                new_qd,
                None,
                Some(SocketAddrV4::new(ipv4, new_port)),
            );
            let result: Result<RequestId, Fail> = self.pop_request_id(new_qd, &yielder).await;

            // Check that the remote has retrieved the port number and responded with a valid request id.
            match result {
                // Valid response. Connection successfully established, so return new port and pipe to application.
                Ok(request_id) => {
                    // If we've never seen this before, something has gone very wrong.
                    assert!(self.pending_request_ids.contains(&request_id));
                    self.pending_request_ids.remove(&request_id);
                    self.state.commit();
                    return Ok(new_socket);
                },
                // Some error.
                Err(e) => {
                    // Clean up newly allocated duplex pipe.
                    self.catmem.close(new_qd)?;
                    return Err(e);
                },
            };
        }
    }

    pub fn connect<F>(&mut self, coroutine_constructor: F) -> Result<QToken, Fail>
    where
        F: FnOnce() -> Result<QToken, Fail>,
    {
        self.state.prepare(SocketOp::Connect)?;
        self.do_generic_sync_control_path_call(coroutine_constructor)
    }

    /// Connects this socket to [remote].
    pub async fn do_connect(&mut self, remote: SocketAddrV4, yielder: &Yielder) -> Result<(), Fail> {
        let ipv4: &Ipv4Addr = remote.ip();
        let port: u16 = remote.port().into();
        let mut catmem: SharedCatmemLibOS = self.catmem.clone();
        let request_id: RequestId = RequestId(self.rng.next_u64());

        // Gets the port for the new connection from the server by sending a connection request repeatedly until a port
        // comes back.
        let result: Result<(QDesc, SocketAddrV4), Fail> = {
            let new_port: u16 = match self.get_port(ipv4, port, &request_id, yielder).await {
                Ok(new_port) => new_port,
                Err(e) => {
                    self.state.prepare(SocketOp::Closed)?;
                    self.state.commit();
                    return Err(e);
                },
            };

            // Open underlying pipes.
            let remote: SocketAddrV4 = SocketAddrV4::new(*ipv4, new_port);
            let new_qd: QDesc = match catmem.open_pipe(&format_pipe_str(ipv4, new_port)) {
                Ok(new_qd) => new_qd,
                Err(e) => {
                    self.state.prepare(SocketOp::Closed)?;
                    self.state.commit();
                    return Err(e);
                },
            };
            // Send an ack to the server over the new pipe.
            if let Err(e) = self.send_ack(new_qd, &request_id, &yielder).await {
                self.state.prepare(SocketOp::Closed)?;
                self.state.commit();
                return Err(e);
            }
            Ok((new_qd, remote))
        };

        match result {
            Ok((new_qd, remote)) => {
                self.state.prepare(SocketOp::Established)?;
                self.state.commit();
                self.catmem_qd = Some(new_qd);
                self.remote = Some(remote);
                Ok(())
            },
            Err(e) => {
                self.state.prepare(SocketOp::Closed)?;
                self.state.commit();
                Err(e)
            },
        }
    }

    /// Asynchronously closes this socket by allocating a coroutine.
    pub fn async_close<F>(&mut self, coroutine_constructor: F) -> Result<QToken, Fail>
    where
        F: FnOnce() -> Result<QToken, Fail>,
    {
        self.state.prepare(SocketOp::Close)?;
        self.do_generic_sync_control_path_call(coroutine_constructor)
    }

    /// Closes `socket`.
    pub async fn do_close(&mut self, yielder: Yielder) -> Result<(QDesc, OperationResult), Fail> {
        // TODO: Should we assert that we're still in the close state?
        let catmem_qd: Option<QDesc> = self.catmem_qd;
        if let Some(qd) = catmem_qd {
            match self.catmem.clone().close_coroutine(qd, yielder).await {
                (qd, OperationResult::Close) => {
                    self.state.prepare(SocketOp::Closed)?;
                    self.state.commit();
                    Ok((qd, OperationResult::Close))
                },
                (qd, OperationResult::Failed(e)) => {
                    // Where to revert to?
                    Ok((qd, OperationResult::Failed(e)))
                },
                _ => panic!("Should not return anything other than close or fail"),
            }
        } else {
            // We know that the queue descriptor will be replaced so it doesn't matter what we put here.
            Ok((QDesc::from(QDesc::MAX), OperationResult::Close))
        }
    }

    /// Schedule a coroutine to push to this queue. This function contains all of the single-queue,
    /// asynchronous code necessary to run push a buffer and any single-queue functionality after the push completes.
    pub fn push<F>(&self, coroutine_constructor: F) -> Result<QToken, Fail>
    where
        F: FnOnce() -> Result<QToken, Fail>,
    {
        self.state.may_push()?;
        coroutine_constructor()
    }

    /// Asynchronous code for pushing to the underlying Catmem transport.
    pub async fn do_push(&mut self, buf: DemiBuffer, yielder: Yielder) -> Result<(QDesc, OperationResult), Fail> {
        self.state.may_push()?;
        // It is safe to unwrap here, because we have just checked for the socket state
        // and by construction it should be connected. If not, the socket state machine
        // was not correctly driven.
        let qd: QDesc = self.catmem_qd.expect("socket should be connected");
        Ok(self.catmem.clone().push_coroutine(qd, buf, yielder).await)
    }

    /// Schedule a coroutine to pop from the underlying Catmem queue. This function contains all of the single-queue,
    /// asynchronous code necessary to run push a buffer and any single-queue functionality after the pop completes.
    pub fn pop<F>(&self, coroutine_constructor: F) -> Result<QToken, Fail>
    where
        F: FnOnce() -> Result<QToken, Fail>,
    {
        self.state.may_pop()?;
        coroutine_constructor()
    }

    /// Asynchronous code for popping from the underlying Catmem transport.
    pub async fn do_pop(&mut self, size: Option<usize>, yielder: Yielder) -> Result<(QDesc, OperationResult), Fail> {
        self.state.may_pop()?;
        // It is safe to unwrap here, because we have just checked for the socket state
        // and by construction it should be connected. If not, the socket state machine
        // was not correctly driven.
        let qd: QDesc = self.catmem_qd.expect("socket should be connected");
        match self.catmem.clone().pop_coroutine(qd, size, yielder).await {
            (qd, OperationResult::Pop(_, buf)) => Ok((qd, OperationResult::Pop(self.remote(), buf))),
            (qd, result) => Ok((qd, result)),
        }
    }

    /// Returns the `local` address to which [self] is bound.
    pub fn local(&self) -> Option<SocketAddrV4> {
        self.local
    }

    /// Returns the `remote` address tot which [self] is connected.
    pub fn remote(&self) -> Option<SocketAddrV4> {
        self.remote
    }

    /// Generic function for spawning a control-path coroutine on [self].
    fn do_generic_sync_control_path_call<F>(&mut self, coroutine_constructor: F) -> Result<QToken, Fail>
    where
        F: FnOnce() -> Result<QToken, Fail>,
    {
        // Spawn coroutine.
        match coroutine_constructor() {
            // We successfully spawned the coroutine.
            Ok(qt) => {
                // Commit the operation on the socket.
                self.state.commit();
                Ok(qt)
            },
            // We failed to spawn the coroutine.
            Err(e) => {
                // Abort the operation on the socket.
                self.state.abort();
                Err(e)
            },
        }
    }

    /// Gets the next connection request.
    async fn pop_request_id(&mut self, catmem_qd: QDesc, yielder: &Yielder) -> Result<RequestId, Fail> {
        // Issue pop. No need to bound the pop because we've quantized it already in the concurrent ring buffer.
        let qt: QToken = self.catmem.pop(catmem_qd, Some(mem::size_of::<RequestId>()))?;
        // Yield until pop completes.
        while !self.runtime.has_completed(qt)? {
            if let Err(e) = yielder.yield_once().await {
                return Err(e);
            }
        }
        // Re-acquire mutable reference.
        // Retrieve operation result and check if it is what we expect.
        let qr: demi_qresult_t = self.runtime.remove_coroutine_and_get_result(qt)?;
        match qr.qr_opcode {
            // We expect a successful completion for previous pop().
            demi_opcode_t::DEMI_OPC_POP => {},
            // We may get some error.
            demi_opcode_t::DEMI_OPC_FAILED => {
                let cause: String = format!(
                    "failed to establish connection (qd={:?}, qt={:?}, errno={:?})",
                    qr.qr_qd, qt, qr.qr_ret
                );
                error!("pop_request_id(): {:?}", &cause);
                return Err(Fail::new(qr.qr_ret as i32, &cause));
            },
            // We do not expect anything else.
            _ => {
                // The following statement is unreachable because we have issued a pop operation.
                // If we successfully complete a different operation, something really bad happen in the scheduler.
                unreachable!("unexpected operation on control duplex pipe")
            },
        }

        // Extract scatter-gather array from operation result.
        let sga: demi_sgarray_t = unsafe { qr.qr_value.sga };

        // Parse and check request.
        let result: Result<RequestId, Fail> = get_connect_id(&sga);
        self.runtime.sgafree(sga)?;
        result
    }

    // Sends the port number to the peer process.
    async fn create_pipe(&mut self, ipv4: &Ipv4Addr, port: u16, yielder: &Yielder) -> Result<QDesc, Fail> {
        let catmem_qd: QDesc = self.catmem_qd.expect("should be connected");
        // Create underlying pipes before sending the port number through the
        // control duplex pipe. This prevents us from running into a race
        // condition were the remote makes progress faster than us and attempts
        // to open the duplex pipe before it is created.
        let new_qd: QDesc = self.catmem.create_pipe(&format_pipe_str(ipv4, port))?;
        // Allocate a scatter-gather array and send the port number to the remote.
        let sga: demi_sgarray_t = self.runtime.sgaalloc(mem::size_of_val(&port))?;
        let ptr: *mut u8 = sga.sga_segs[0].sgaseg_buf as *mut u8;
        let len: usize = sga.sga_segs[0].sgaseg_len as usize;
        let slice: &mut [u8] = unsafe { slice::from_raw_parts_mut(ptr, len) };
        slice.copy_from_slice(&port.to_ne_bytes());

        // Push the port number.
        let qt: QToken = self.catmem.push(catmem_qd, &sga)?;

        // Wait for push to complete.
        while !self.runtime.has_completed(qt)? {
            if let Err(e) = yielder.yield_once().await {
                return Err(e);
            }
        }
        // Free the scatter-gather array.
        self.runtime.sgafree(sga)?;

        // Retrieve operation result and check if it is what we expect.
        let qr: demi_qresult_t = self.runtime.remove_coroutine_and_get_result(qt)?;
        match qr.qr_opcode {
            // We expect a successful completion for previous push().
            demi_opcode_t::DEMI_OPC_PUSH => Ok(new_qd),
            // We may get some error.
            demi_opcode_t::DEMI_OPC_FAILED => {
                let cause: String = format!(
                    "failed to establish connection (qd={:?}, qt={:?}, errno={:?})",
                    qr.qr_qd, qt, qr.qr_ret
                );
                error!("create_pipe(): {:?}", &cause);
                Err(Fail::new(qr.qr_ret as i32, &cause))
            },
            // We do not expect anything else.
            _ => {
                // The following statement is unreachable because we have issued a pop operation.
                // If we successfully complete a different operation, something really bad happen in the scheduler.
                unreachable!("unexpected operation on control duplex pipe")
            },
        }
    }

    async fn send_connection_request(
        &mut self,
        connect_qd: QDesc,
        request_id: &RequestId,
        yielder: &Yielder,
    ) -> Result<(), Fail> {
        // Create a message containing the magic number.
        let sga: demi_sgarray_t = self.send_connect_id(request_id)?;

        // Send to server.
        let qt: QToken = self.catmem.push(connect_qd, &sga)?;
        trace!("Send connection request qtoken={:?}", qt);
        // Yield until push completes.
        while !self.runtime.has_completed(qt)? {
            if let Err(e) = yielder.yield_once().await {
                return Err(e);
            }
        }
        // Free the message buffer.
        self.runtime.sgafree(sga)?;
        // Get the result of the push.
        let qr: demi_qresult_t = self.runtime.remove_coroutine_and_get_result(qt)?;
        match qr.qr_opcode {
            // We expect a successful completion for previous push().
            demi_opcode_t::DEMI_OPC_PUSH => Ok(()),
            // We may get some error.
            demi_opcode_t::DEMI_OPC_FAILED => {
                let cause: String = format!(
                    "failed to establish connection (qd={:?}, qt={:?}, errno={:?})",
                    qr.qr_qd, qt, qr.qr_ret
                );
                error!("send_connection_request(): {:?}", &cause);
                Err(Fail::new(qr.qr_ret as i32, &cause))
            },
            // We do not expect anything else.
            _ => {
                // The following statement is unreachable because we have issued a pop operation.
                // If we successfully complete a different operation, something really bad happen in the scheduler.
                unreachable!("unexpected operation on control duplex pipe")
            },
        }
    }

    async fn get_port(
        &mut self,
        ipv4: &Ipv4Addr,
        port: u16,
        request_id: &RequestId,
        yielder: &Yielder,
    ) -> Result<u16, Fail> {
        // Issue receive operation to wait for connect request ack.
        let size: usize = mem::size_of::<u16>();
        // Open connection to server.
        let connect_qd: QDesc = match self.catmem.open_pipe(&format_pipe_str(ipv4, port)) {
            Ok(qd) => qd,
            Err(e) => {
                // Interpose error.
                if e.errno == libc::ENOENT {
                    let cause: String = format!("failed to establish connection (ipv4={:?}, port={:?})", ipv4, port);
                    error!("get_port(): {:?}", &cause);
                    return Err(Fail::new(libc::ECONNREFUSED, &cause));
                }

                return Err(e);
            },
        };

        let qt: QToken = self.catmem.pop(connect_qd, Some(size))?;
        trace!("Read port qtoken={:?}", qt);

        loop {
            // Send the connection request to the server.
            self.send_connection_request(connect_qd, request_id, &yielder).await?;

            // Wait on the pop for MAX_ACK_RECEIVED_ATTEMPTS
            for _ in 0..MAX_ACK_RECEIVED_ATTEMPTS {
                match yielder.yield_once().await {
                    Ok(()) if self.runtime.has_completed(qt)? => break,
                    Ok(()) => continue,
                    Err(e) => return Err(e),
                }
            }
            // If we received a port back from the server, then unpack it. Otherwise, send the connection request again.
            if self.runtime.has_completed(qt)? {
                // Re-acquire reference to Catmem libos.
                // Get the result of the pop.
                let qr: demi_qresult_t = self.runtime.remove_coroutine_and_get_result(qt)?;
                match qr.qr_opcode {
                    // We expect a successful completion for previous pop().
                    demi_opcode_t::DEMI_OPC_POP => {
                        // Extract scatter-gather array from operation result.
                        let sga: demi_sgarray_t = unsafe { qr.qr_value.sga };

                        // Extract port number.
                        let port: Result<u16, Fail> = extract_port_number(&sga);
                        self.runtime.sgafree(sga)?;
                        return port;
                    },
                    // We may get some error.
                    demi_opcode_t::DEMI_OPC_FAILED => {
                        // Shut down control duplex pipe as we can open the new pipe now.
                        self.catmem.shutdown(connect_qd)?;

                        let cause: String = format!(
                            "failed to establish connection (qd={:?}, qt={:?}, errno={:?})",
                            qr.qr_qd, qt, qr.qr_ret
                        );
                        error!("get_port(): {:?}", &cause);
                        return Err(Fail::new(qr.qr_ret as i32, &cause));
                    },
                    // We do not expect anything else.
                    _ => {
                        // The following statement is unreachable because we have issued a pop operation.
                        // If we successfully complete a different operation, something really bad happen in the scheduler.
                        unreachable!("unexpected operation on control duplex pipe")
                    },
                }
            }
        }
    }

    // Send an ack through a new Catmem pipe.
    async fn send_ack(&mut self, new_qd: QDesc, request_id: &RequestId, yielder: &Yielder) -> Result<(), Fail> {
        // Create message with magic connect.
        let sga: demi_sgarray_t = self.send_connect_id(request_id)?;
        // Send to server through new pipe.
        let qt: QToken = self.catmem.push(new_qd, &sga)?;
        trace!("Send ack qtoken={:?}", qt);

        // Yield until push completes.
        while !self.runtime.has_completed(qt)? {
            if let Err(e) = yielder.yield_once().await {
                return Err(e);
            }
        }
        // Free the message buffer.
        self.runtime.sgafree(sga)?;
        // Retrieve operation result and check if it is what we expect.
        let qr: demi_qresult_t = self.runtime.remove_coroutine_and_get_result(qt)?;

        match qr.qr_opcode {
            // We expect a successful completion for previous push().
            demi_opcode_t::DEMI_OPC_PUSH => Ok(()),
            // We may get some error.
            demi_opcode_t::DEMI_OPC_FAILED => {
                let cause: String = format!(
                    "failed to establish connection (qd={:?}, qt={:?}, errno={:?})",
                    qr.qr_qd, qt, qr.qr_ret
                );
                error!("send_ack(): {:?}", &cause);
                Err(Fail::new(qr.qr_ret as i32, &cause))
            },
            // We do not expect anything else.
            _ => {
                // The following statement is unreachable because we have issued a pop operation.
                // If we successfully complete a different operation, something really bad happen in the scheduler.
                unreachable!("unexpected operation on control duplex pipe")
            },
        }
    }

    /// Creates a magic connect message.
    pub fn send_connect_id(&mut self, request_id: &RequestId) -> Result<demi_sgarray_t, Fail> {
        let sga: demi_sgarray_t = self.runtime.sgaalloc(mem::size_of_val(&request_id))?;
        let ptr: *mut u64 = sga.sga_segs[0].sgaseg_buf as *mut u64;
        unsafe {
            *ptr = request_id.0;
        }
        Ok(sga)
    }
}

//======================================================================================================================
// Standalone Functions
//======================================================================================================================

/// Extracts port number from connect request ack message.
fn extract_port_number(sga: &demi_sgarray_t) -> Result<u16, Fail> {
    match sga.sga_segs[0].sgaseg_len as usize {
        len if len == 0 => {
            let cause: String = format!("server closed connection");
            error!("extract_port_number(): {:?}", &cause);
            Err(Fail::new(libc::EBADF, &cause))
        },
        len if len == 2 => {
            let data_ptr: *mut u8 = sga.sga_segs[0].sgaseg_buf as *mut u8;
            let slice: &mut [u8] = unsafe { slice::from_raw_parts_mut(data_ptr, len) };
            let array: [u8; 2] = [slice[0], slice[1]];
            Ok(u16::from_ne_bytes(array))
        },
        len => {
            let cause: String = format!("server sent invalid port number (len={:?})", len);
            error!("extract_port_number(): {:?}", &cause);
            Err(Fail::new(libc::EBADMSG, &cause))
        },
    }
}

/// Checks for a magic connect message.
fn get_connect_id(sga: &demi_sgarray_t) -> Result<RequestId, Fail> {
    let len: usize = sga.sga_segs[0].sgaseg_len as usize;
    if len == mem::size_of::<RequestId>() {
        let ptr: *mut u64 = sga.sga_segs[0].sgaseg_buf as *mut u64;
        Ok(RequestId(unsafe { *ptr }))
    } else {
        let cause: String = format!("invalid connect request (len={:?})", len);
        error!("get_connect_id(): {:?}", &cause);
        Err(Fail::new(libc::ECONNREFUSED, &cause))
    }
}

fn format_pipe_str(ip: &Ipv4Addr, port: u16) -> String {
    format!("{}:{}", ip, port)
}
