// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//! # User Datagram Protocol
//!
//! # References
//!
//! - https://datatracker.ietf.org/doc/html/rfc768.

pub mod datagram;
pub mod peer;
pub mod socket;

#[cfg(test)]
mod tests;

//==============================================================================
// Exports
//==============================================================================

pub use self::{
    datagram::UdpHeader,
    peer::SharedUdpPeer,
};

/// Expose this when building tests, as the network simulator uses it.
#[cfg(test)]
pub use self::datagram::UdpDatagram;
