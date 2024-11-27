// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::inetstack::protocols::*;
use ::std::time::Duration;

//======================================================================================================================
// Constants
//======================================================================================================================

/// Fallback MSS Parameter for TCP
pub const FALLBACK_MSS: usize = 536;

/// Minimum MSS Parameter for TCP
pub const MIN_MSS: usize = FALLBACK_MSS;

/// Maximum MSS Parameter for TCP
pub const MAX_MSS: usize = u16::max_value() as usize;

/// Maximum Segment Lifetime
/// See: https://www.rfc-editor.org/rfc/rfc793.txt
pub const MSL: Duration = Duration::from_secs(2);

/// Delay timeout for TCP ACKs.
/// See: https://www.rfc-editor.org/rfc/rfc5681#section-4.2
pub const TCP_ACK_DELAY_TIMEOUT: Duration = Duration::from_millis(500);

/// Handshake timeout for tcp.
pub const TCP_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);

/// Default MSS Parameter for TCP
///
/// TODO: Auto-Discovery MTU Size
pub const DEFAULT_MSS: usize = 1450;

/// Length of a [crate::memory::DemiBuffer] batch.
///
/// TODO: This Should be Generic
pub const RECEIVE_BATCH_SIZE: usize = 4;

/// Maximum local and remote window scaling factor.
/// See: RFC 1323, Section 2.3.
pub const MAX_WINDOW_SCALE: usize = 14;

// Maximum header size of all possible headers.
pub const MAX_HEADER_SIZE: usize =
    layer4::tcp::MAX_TCP_HEADER_SIZE + layer3::ipv4::IPV4_HEADER_MAX_SIZE as usize + layer2::ETHERNET2_HEADER_SIZE;

pub const MAX_RECV_ITERS: usize = 2;
