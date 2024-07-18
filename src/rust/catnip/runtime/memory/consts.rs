// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//==============================================================================
// Imports
//==============================================================================

use crate::runtime::libdpdk::{
    RTE_MBUF_DEFAULT_BUF_SIZE,
    RTE_PKTMBUF_HEADROOM,
};

//==============================================================================
// Constants
//==============================================================================

/// Default number of buffers in the body pool.
pub const DEFAULT_BODY_POOL_SIZE: usize = 8192 - 1;

/// Default value for maximum body size.
pub const DEFAULT_MAX_BODY_SIZE: usize = (RTE_MBUF_DEFAULT_BUF_SIZE + RTE_PKTMBUF_HEADROOM) as usize;

/// Default per-thread cache size.
pub const DEFAULT_CACHE_SIZE: usize = 250;
