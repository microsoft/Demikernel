// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{
    async_timer,
    collections::{async_queue::SharedAsyncQueue, async_value::SharedAsyncValue},
    inetstack::protocols::{
        layer3::SharedLayer3Endpoint,
        layer4::tcp::{
            constants::MSL,
            established::{
                congestion_control::{self, CongestionControlConstructor},
                receiver::Receiver,
                sender::Sender,
            },
            header::TcpHeader,
            SeqNumber,
        },
        MAX_HEADER_SIZE,
    },
    runtime::{
        fail::Fail,
        memory::DemiBuffer,
        network::{config::TcpConfig, socket::option::TcpSocketOptions},
        yield_with_timeout, SharedDemiRuntime, SharedObject,
    },
};
use ::futures::{never::Never, pin_mut, FutureExt};
use ::std::{
    net::{Ipv4Addr, SocketAddrV4},
    ops::{Deref, DerefMut},
    time::{Duration, Instant},
};

//======================================================================================================================
// Structures
//======================================================================================================================

// TCP Connection State.
// Note: This ControlBlock structure is only used after we've reached the ESTABLISHED state, so states LISTEN,
// SYN_RCVD, and SYN_SENT aren't included here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Established,
    FinWait1,
    FinWait2,
    Closing,
    TimeWait,
    CloseWait,
    LastAck,
    Closed,
}

//======================================================================================================================
// Control Block
//======================================================================================================================

/// Transmission control block for representing our TCP connection.
pub struct ControlBlock {
    local: SocketAddrV4,
    remote: SocketAddrV4,

    layer3_endpoint: SharedLayer3Endpoint,
    runtime: SharedDemiRuntime,
    tcp_config: TcpConfig,
    socket_options: TcpSocketOptions,

    // TCP Connection State.
    state: State,

    // Send Sequence Variables from RFC 793.

    // SND.UNA - send unacknowledged
    // SND.NXT - send next
    // SND.WND - send window
    // SND.UP  - send urgent pointer - not implemented
    // SND.WL1 - segment sequence number used for last window update
    // SND.WL2 - segment acknowledgment number used for last window
    //           update
    // ISS     - initial send sequence number

    // Send queues
    // SND.retrasmission_queue - queue of unacknowledged sent data.
    // SND.unsent - queue of unsent data that we do not have the windows for.
    // Previous send variables and queues.
    // TODO: Consider incorporating this directly into ControlBlock.
    sender: Sender,
    // Receive Sequence Variables from RFC 793.

    // RCV.NXT - receive next
    // RCV.WND - receive window
    // RCV.UP  - receive urgent pointer - not implemented
    // IRS     - initial receive sequence number
    // Receive-side state information.  TODO: Consider incorporating this directly into ControlBlock.
    receiver: Receiver,

    // Congestion control trait implementation we're currently using.
    // TODO: Consider switching this to a static implementation to avoid V-table call overhead.
    congestion_control_algorithm: Box<dyn congestion_control::CongestionControl>,
}

#[derive(Clone)]
pub struct SharedControlBlock(SharedObject<ControlBlock>);
//======================================================================================================================

impl SharedControlBlock {
    pub fn new(
        local: SocketAddrV4,
        remote: SocketAddrV4,
        layer3_endpoint: SharedLayer3Endpoint,
        runtime: SharedDemiRuntime,
        tcp_config: TcpConfig,
        default_socket_options: TcpSocketOptions,
        // In RFC 793, this is IRS.
        receive_initial_seq_no: SeqNumber,
        receive_ack_delay_timeout_secs: Duration,
        receive_window_size_frames: u32,
        receive_window_scale_shift_bits: u8,
        // In RFC 793, this ISS.
        sender_initial_seq_no: SeqNumber,
        send_window_size_frames: u32,
        send_window_scale_shift_bits: u8,
        sender_mss: usize,
        congestion_control_algorithm_constructor: CongestionControlConstructor,
        congestion_control_options: Option<congestion_control::Options>,
        mut recv_queue: SharedAsyncQueue<(Ipv4Addr, TcpHeader, DemiBuffer)>,
    ) -> Self {
        let sender: Sender = Sender::new(
            sender_initial_seq_no,
            send_window_size_frames,
            send_window_scale_shift_bits,
            sender_mss,
        );
        let receiver: Receiver = Receiver::new(
            receive_initial_seq_no,
            receive_initial_seq_no,
            receive_ack_delay_timeout_secs,
            receive_window_size_frames,
            receive_window_scale_shift_bits,
        );
        let congestion_control_algorithm =
            congestion_control_algorithm_constructor(sender_mss, sender_initial_seq_no, congestion_control_options);
        let mut self_: Self = Self(SharedObject::<ControlBlock>::new(ControlBlock {
            local,
            remote,
            layer3_endpoint,
            runtime,
            tcp_config,
            socket_options: default_socket_options,
            sender,
            state: State::Established,
            receiver,
            congestion_control_algorithm,
        }));
        trace!("receive_queue size {:?}", recv_queue.len());
        // Process all pending received packets while setting up the connection.
        while let Some((_ipv4_addr, header, data)) = recv_queue.try_pop() {
            self_.receive(header, data);
        }

        self_
    }

    pub fn get_local(&self) -> SocketAddrV4 {
        self.local
    }

    pub fn get_remote(&self) -> SocketAddrV4 {
        self.remote
    }

    pub fn get_now(&self) -> Instant {
        self.runtime.get_now()
    }

    pub fn receive(&mut self, tcp_hdr: TcpHeader, buf: DemiBuffer) {
        debug!(
            "{:?} Connection Receiving {} bytes + {:?}",
            self.state,
            buf.len(),
            tcp_hdr,
        );

        let cb: Self = self.clone();
        let now: Instant = self.runtime.get_now();
        self.receiver.receive(tcp_hdr, buf, cb, now);
    }

    pub fn congestion_control_watch_retransmit_now_flag(&self) -> SharedAsyncValue<bool> {
        self.congestion_control_algorithm.get_retransmit_now_flag()
    }

    pub fn congestion_control_on_fast_retransmit(&mut self) {
        self.congestion_control_algorithm.on_fast_retransmit()
    }

    pub fn congestion_control_on_rto(&mut self, send_unacknowledged: SeqNumber) {
        self.congestion_control_algorithm.on_rto(send_unacknowledged)
    }

    pub fn congestion_control_on_send(&mut self, rto: Duration, num_sent_bytes: u32) {
        self.congestion_control_algorithm.on_send(rto, num_sent_bytes)
    }

    pub fn congestion_control_on_cwnd_check_before_send(&mut self) {
        self.congestion_control_algorithm.on_cwnd_check_before_send()
    }

    pub fn congestion_control_get_cwnd(&self) -> SharedAsyncValue<u32> {
        self.congestion_control_algorithm.get_cwnd()
    }

    pub fn congestion_control_get_limited_transmit_cwnd_increase(&self) -> SharedAsyncValue<u32> {
        self.congestion_control_algorithm.get_limited_transmit_cwnd_increase()
    }

    pub fn process_ack(&mut self, header: &TcpHeader, now: Instant) -> Result<(), Fail> {
        let send_unacknowledged: SeqNumber = self.sender.get_unacked_seq_no();
        let send_next: SeqNumber = self.sender.get_next_seq_no();

        // TODO: Restructure this call into congestion control to either integrate it directly or make it more fine-
        // grained.  It currently duplicates the new/duplicate ack check itself internally, which is inefficient.
        // We should either make separate calls for each case or integrate those cases directly.
        let rto: Duration = self.sender.get_rto();

        self.congestion_control_algorithm
            .on_ack_received(rto, send_unacknowledged, send_next, header.ack_num);

        // Check whether this is an ack for data that we have sent.
        if header.ack_num <= send_next {
            // Does not matter when we get this since the clock will not move between the beginning of packet
            // processing and now without a call to advance_clock.
            self.sender.process_ack(header, now);
        } else {
            // This segment acknowledges data we have yet to send!?  Send an ACK and drop the segment.
            // TODO: See RFC 5961, this could be a Blind Data Injection Attack.
            let cause: String = format!("Received segment acknowledging data we have yet to send!");
            warn!("process_ack(): {}", cause);
            self.send_ack();
            return Err(Fail::new(libc::EBADMSG, &cause));
        }
        Ok(())
    }

    pub fn get_unacked_seq_no(&self) -> SeqNumber {
        self.sender.get_unacked_seq_no()
    }

    /// Fetch a TCP header filling out various values based on our current state.
    /// TODO: Fix the "filling out various values based on our current state" part to actually do that correctly.
    pub fn tcp_header(&self) -> TcpHeader {
        let mut header: TcpHeader = TcpHeader::new(self.local.port(), self.remote.port());
        header.window_size = self.receiver.hdr_window_size();

        // Note that once we reach a synchronized state we always include a valid acknowledgement number.
        header.ack = true;
        header.ack_num = self.receiver.receive_next_seq_no();

        // Return this header.
        header
    }

    /// Send an ACK to our peer, reflecting our current state.
    pub fn send_ack(&mut self) {
        trace!("sending ack");
        let mut header: TcpHeader = self.tcp_header();

        // TODO: Think about moving this to tcp_header() as well.
        let seq_num: SeqNumber = self.sender.get_next_seq_no();
        header.seq_num = seq_num;
        self.emit(header, None);
    }

    /// Transmit this message to our connected peer.
    pub fn emit(&mut self, header: TcpHeader, body: Option<DemiBuffer>) {
        // Only perform this debug print in debug builds.  debug_assertions is compiler set in non-optimized builds.
        let mut pkt = match body {
            Some(body) => {
                debug!("Sending {} bytes + {:?}", body.len(), header);
                body
            },
            _ => {
                debug!("Sending 0 bytes + {:?}", header);
                DemiBuffer::new_with_headroom(0, MAX_HEADER_SIZE as u16)
            },
        };

        // This routine should only ever be called to send TCP segments that contain a valid ACK value.
        debug_assert!(header.ack);

        let remote_ipv4_addr: Ipv4Addr = self.remote.ip().clone();
        header.serialize_and_attach(
            &mut pkt,
            self.local.ip(),
            self.remote.ip(),
            self.tcp_config.get_tx_checksum_offload(),
        );

        // Call lower L3 layer to send the segment.
        if let Err(e) = self
            .layer3_endpoint
            .transmit_tcp_packet_nonblocking(remote_ipv4_addr, pkt)
        {
            warn!("could not emit packet: {:?}", e);
            return;
        }

        // Post-send operations follow.
        // Review: We perform these after the send, in order to keep send latency as low as possible.

        // Since we sent an ACK, cancel any outstanding delayed ACK request.
        self.receiver.set_receive_ack_deadline(None);
    }
    pub async fn push(&mut self, buf: DemiBuffer) -> Result<(), Fail> {
        let cb: Self = self.clone();
        self.sender.push(buf, cb).await
    }

    pub async fn pop(&mut self, size: Option<usize>) -> Result<DemiBuffer, Fail> {
        self.receiver.pop(size).await
    }

    pub fn process_fin(&mut self) {
        let state = match self.state {
            State::Established => State::CloseWait,
            State::FinWait1 => State::Closing,
            State::FinWait2 => State::TimeWait,
            state => unreachable!("Cannot be in any other state at this point: {:?}", state),
        };
        self.state = state;
    }

    pub fn get_state(&self) -> State {
        self.state
    }
    // This coroutine runs the close protocol.
    pub async fn close(&mut self) -> Result<(), Fail> {
        // Assert we are in a valid state and move to new state.
        match self.state {
            State::Established => self.local_close().await,
            State::CloseWait => self.remote_already_closed().await,
            _ => {
                let cause: String = format!("socket is already closing");
                error!("close(): {}", cause);
                Err(Fail::new(libc::EBADF, &cause))
            },
        }
    }

    async fn local_close(&mut self) -> Result<(), Fail> {
        // 1. Start close protocol by setting state and sending FIN.
        self.state = State::FinWait1;
        self.sender.push_fin_and_wait_for_ack().await?;

        // 2. Got ACK to our FIN. Check if we also received a FIN from remote in the meantime.
        let state: State = self.state;
        match state {
            State::FinWait1 => {
                self.state = State::FinWait2;
                // Haven't received a FIN yet from remote, so wait.
                self.receiver.wait_for_fin().await?;
            },
            State::Closing => self.state = State::TimeWait,
            state => unreachable!("Cannot be in any other state at this point: {:?}", state),
        };
        // 3. TIMED_WAIT
        debug_assert_eq!(self.state, State::TimeWait);
        trace!("socket options: {:?}", self.socket_options.get_linger());
        let timeout: Duration = self.socket_options.get_linger().unwrap_or(MSL * 2);
        yield_with_timeout(timeout).await;
        self.state = State::Closed;
        Ok(())
    }

    async fn remote_already_closed(&mut self) -> Result<(), Fail> {
        // 0. Move state forward
        self.state = State::LastAck;
        // 1. Send FIN and wait for ack before closing.
        self.sender.push_fin_and_wait_for_ack().await?;
        self.state = State::Closed;
        Ok(())
    }

    pub async fn background(&self) {
        let acknowledger = async_timer!(
            "tcp::established::background::acknowledger",
            self.clone().background_acknowledger()
        )
        .fuse();
        pin_mut!(acknowledger);

        let retransmitter = async_timer!(
            "tcp::established::background::retransmitter",
            self.clone().background_retransmitter()
        )
        .fuse();
        pin_mut!(retransmitter);

        let sender = async_timer!("tcp::established::background::sender", self.clone().background_sender()).fuse();
        pin_mut!(sender);

        let r = futures::join!(acknowledger, retransmitter, sender);
        error!("Connection terminated: {:?}", r);
    }

    pub async fn background_retransmitter(mut self) -> Result<Never, Fail> {
        let cb: Self = self.clone();
        self.sender.background_retransmitter(cb).await
    }

    pub async fn background_sender(mut self) -> Result<Never, Fail> {
        let cb: Self = self.clone();
        self.sender.background_sender(cb).await
    }

    pub async fn background_acknowledger(mut self) -> Result<Never, Fail> {
        let cb: Self = self.clone();
        self.receiver.acknowledger(cb).await
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl Deref for SharedControlBlock {
    type Target = ControlBlock;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl DerefMut for SharedControlBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}
