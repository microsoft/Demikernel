// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{
    inetstack::protocols::layer4::tcp::{established::congestion_control, established::Receiver, established::Sender},
    runtime::network::{config::TcpConfig, socket::option::TcpSocketOptions},
};
use ::std::net::SocketAddrV4;

//======================================================================================================================
// Structures
//======================================================================================================================

// TCP Connection State.
// Note: This ControlBlock structure is only used after we've reached the ESTABLISHED state, so states LISTEN,
// SYN_RCVD, and SYN_SENT aren't included here.
// This struct has only public members because includes state for both the send and receive path and is accessed by
// both.
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
// Data Structures
//======================================================================================================================

/// Transmission control block for representing our TCP connection.
pub struct ControlBlock {
    pub local: SocketAddrV4,
    pub remote: SocketAddrV4,

    pub tcp_config: TcpConfig,
    pub socket_options: TcpSocketOptions,

    // TCP Connection State.
    pub state: State,

    // TCP send path state.
    pub sender: Sender,
    // TCP receive path state.
    pub receiver: Receiver,

    // Congestion control trait implementation we're currently using.
    // TODO: Consider switching this to a static implementation to avoid V-table call overhead.
    pub congestion_control_algorithm: Box<dyn congestion_control::CongestionControl>,
}

//======================================================================================================================
// Associated Functions
//======================================================================================================================

impl ControlBlock {
    pub fn new(
        local: SocketAddrV4,
        remote: SocketAddrV4,
        tcp_config: TcpConfig,
        socket_options: TcpSocketOptions,
        sender: Sender,
        receiver: Receiver,
        congestion_control_algorithm: Box<dyn congestion_control::CongestionControl>,
    ) -> Self {
        Self {
            local,
            remote,
            tcp_config,
            socket_options,
            state: State::Established,
            sender,
            receiver,
            congestion_control_algorithm,
        }
    }
}
