// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{
    catpowder::win::{ring::rule::params::XdpRedirectParams, socket::XdpSocket},
    inetstack::protocols::Protocol,
    runtime::libxdp,
};
use ::std::{
    fmt::{self, Formatter},
    mem,
};

//======================================================================================================================
// Structures
//======================================================================================================================

/// A wrapper structure for a XDP rule.
#[repr(C)]
pub struct XdpRule(libxdp::XDP_RULE);

//======================================================================================================================
// Implementations
//======================================================================================================================

impl XdpRule {
    /// Creates a new XDP rule for the target socket which filters all traffic.
    #[allow(dead_code)]
    pub fn new(socket: &XdpSocket) -> Self {
        let redirect: XdpRedirectParams = XdpRedirectParams::new(socket);
        let rule: libxdp::XDP_RULE = unsafe {
            let mut rule: libxdp::XDP_RULE = std::mem::zeroed();
            rule.Match = libxdp::_XDP_MATCH_TYPE_XDP_MATCH_ALL;
            rule.Action = libxdp::_XDP_RULE_ACTION_XDP_PROGRAM_ACTION_REDIRECT;
            // Perform bitwise copy from redirect to rule, as this is a union field.
            rule.__bindgen_anon_1 =
                mem::transmute_copy::<libxdp::XDP_REDIRECT_PARAMS, libxdp::_XDP_RULE__bindgen_ty_1>(redirect.as_ref());

            rule
        };
        Self(rule)
    }

    /// Creates a new XDP rule for the target socket which filters for a specific (protocol, port) combination.
    pub fn new_for_dest(socket: &XdpSocket, protocol: Protocol, port: u16) -> Self {
        let redirect: XdpRedirectParams = XdpRedirectParams::new(socket);
        let rule: libxdp::XDP_RULE = unsafe {
            let mut rule: libxdp::XDP_RULE = std::mem::zeroed();
            rule.Match = match protocol {
                Protocol::Udp => libxdp::_XDP_MATCH_TYPE_XDP_MATCH_UDP_DST,
                Protocol::Tcp => libxdp::_XDP_MATCH_TYPE_XDP_MATCH_TCP_DST,
            };
            rule.Action = libxdp::_XDP_RULE_ACTION_XDP_PROGRAM_ACTION_REDIRECT;
            *rule.Pattern.Port.as_mut() = port;
            // Perform bitwise copy from redirect to rule, as this is a union field.
            *rule.__bindgen_anon_1.Redirect.as_mut() = mem::transmute_copy(redirect.as_ref());

            rule
        };
        Self(rule)
    }
}

//======================================================================================================================
// Trait Implementations
//======================================================================================================================

impl fmt::Debug for XdpRule {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let match_str: &str = match self.0.Match {
            libxdp::_XDP_MATCH_TYPE_XDP_MATCH_UDP_DST => "XDP_MATCH_UDP_DST",
            libxdp::_XDP_MATCH_TYPE_XDP_MATCH_TCP_DST => "XDP_MATCH_TCP_DST",
            libxdp::_XDP_MATCH_TYPE_XDP_MATCH_ALL => "XDP_MATCH_ALL",
            _ => "UNKNOWN",
        };

        let pattern: String = match self.0.Match {
            libxdp::_XDP_MATCH_TYPE_XDP_MATCH_UDP_DST | libxdp::_XDP_MATCH_TYPE_XDP_MATCH_TCP_DST => {
                format!("{{Port={}}}", unsafe { self.0.Pattern.Port.as_ref() })
            },
            libxdp::_XDP_MATCH_TYPE_XDP_MATCH_ALL => format!("{{ALL}}"),
            _ => format!("{{UNKNOWN}}"),
        };

        let action_str: &str = match self.0.Action {
            libxdp::_XDP_RULE_ACTION_XDP_PROGRAM_ACTION_REDIRECT => "XDP_PROGRAM_ACTION_REDIRECT",
            libxdp::_XDP_RULE_ACTION_XDP_PROGRAM_ACTION_EBPF => "XDP_PROGRAM_ACTION_EBPF",
            libxdp::_XDP_RULE_ACTION_XDP_PROGRAM_ACTION_L2FWD => "XDP_PROGRAM_ACTION_L2FWD",
            libxdp::_XDP_RULE_ACTION_XDP_PROGRAM_ACTION_PASS => "XDP_PROGRAM_ACTION_PASS",
            libxdp::_XDP_RULE_ACTION_XDP_PROGRAM_ACTION_DROP => "XDP_PROGRAM_ACTION_DROP",
            _ => "UNKNOWN",
        };

        f.debug_struct("XDP_RULE")
            .field("Match", &match_str)
            .field("Pattern", &pattern)
            .field("Action", &action_str)
            .finish()
    }
}
