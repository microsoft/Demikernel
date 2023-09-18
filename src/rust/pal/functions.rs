// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use crate::pal::data_structures::{
    SockAddrIn,
    SockAddrIn6,
};

const NUM_OCTETS_IN_IPV4: usize = 16;

#[cfg(feature = "catnip-libos")]
const NUM_OCTETS_IN_IPV4: usize = 4;

#[cfg(feature = "catnip-libos")]
const NUM_SIN_ZERO_BYTES: usize = 8;

#[cfg(all(feature = "catnip-libos", target_os = "windows"))]
use windows::Win32::Foundation::CHAR;

#[cfg(all(feature = "catnip-libos", target_os = "windows"))]
use windows::Win32::Networking::WinSock::IN_ADDR;

#[cfg(all(feature = "catnip-libos", target_os = "windows"))]
use windows::Win32::Networking::WinSock::IN_ADDR_0;

#[cfg(all(feature = "catnip-libos", target_os = "linux"))]
use libc::in_addr;

//======================================================================================================================
// Windows functions
//======================================================================================================================

#[cfg(all(feature = "catnip-libos", target_os = "windows"))]
pub fn create_sin_addr(octets: &[u8; NUM_OCTETS_IN_IPV4]) -> IN_ADDR {
    IN_ADDR {
        S_un: (IN_ADDR_0 {
            // Always create a big-endian u32 from the given 4 bytes (in big-endian order), regardless of architecture.
            S_addr: u32::from_ne_bytes(*octets),
        }),
    }
}

#[cfg(all(feature = "catnip-libos", target_os = "windows"))]
pub fn create_sin_zero() -> [CHAR; NUM_SIN_ZERO_BYTES] {
    [CHAR(0); 8]
}

#[cfg(target_os = "windows")]
pub fn get_addr_from_sock_addr_in(sock_addr_in: &SockAddrIn) -> u32 {
    unsafe { sock_addr_in.sin_addr.S_un.S_addr }
}

#[cfg(target_os = "windows")]
pub fn get_addr_from_sock_addr_in6(sock_addr_in: &SockAddrIn6) -> [u8; NUM_OCTETS_IN_IPV4] {
    unsafe { sock_addr_in.sin6_addr.u.Byte }
}

#[cfg(target_os = "windows")]
pub fn get_scope_id_from_sock_addr_in6(sock_addr_in: &SockAddrIn6) -> u32 {
    unsafe { sock_addr_in.Anonymous.sin6_scope_id }
}

#[cfg(target_os = "windows")]
use std::net::SocketAddrV4;

#[cfg(target_os = "windows")]
use windows::Win32::Networking::WinSock::SOCKADDR;

#[cfg(target_os = "windows")]
use windows::Win32::Networking::WinSock::SOCKADDR_IN;

#[cfg(target_os = "windows")]
use crate::pal::constants::AF_INET;

#[cfg(target_os = "windows")]
pub fn socketaddrv4_to_sockaddr(addr: &SocketAddrV4) -> SOCKADDR {
    let mut sockaddr_in: SOCKADDR_IN = unsafe { std::mem::zeroed() };
    sockaddr_in.sin_family = AF_INET;
    sockaddr_in.sin_port = addr.port().to_be();
    sockaddr_in.sin_addr.S_un.S_addr = u32::from_be_bytes(addr.ip().octets());
    let sockaddr: SOCKADDR = unsafe { std::mem::transmute(sockaddr_in) };
    sockaddr
}

//======================================================================================================================
// Linux functions
//======================================================================================================================

#[cfg(all(feature = "catnip-libos", target_os = "linux"))]
pub fn create_sin_addr(octets: &[u8; NUM_OCTETS_IN_IPV4]) -> in_addr {
    in_addr {
        // Always create a big-endian u32 from the given 4 bytes (in big-endian order), regardless of architecture.
        s_addr: u32::from_ne_bytes(*octets),
    }
}

#[cfg(all(feature = "catnip-libos", target_os = "linux"))]
pub fn create_sin_zero() -> [u8; NUM_SIN_ZERO_BYTES] {
    [0; 8]
}

#[cfg(target_os = "linux")]
pub fn get_addr_from_sock_addr_in(sock_addr_in: &SockAddrIn) -> u32 {
    sock_addr_in.sin_addr.s_addr
}

#[cfg(target_os = "linux")]
pub fn get_addr_from_sock_addr_in6(sock_addr_in: &SockAddrIn6) -> [u8; NUM_OCTETS_IN_IPV4] {
    sock_addr_in.sin6_addr.s6_addr
}

#[cfg(target_os = "linux")]
pub fn get_scope_id_from_sock_addr_in6(sock_addr_in: &SockAddrIn6) -> u32 {
    sock_addr_in.sin6_scope_id
}
