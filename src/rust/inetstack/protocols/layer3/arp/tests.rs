// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{
    inetstack::{
        protocols::{
            layer2::{EtherType2, Ethernet2Header},
            layer3::arp::header::{ArpHeader, ArpOperation},
        },
        test_helpers::{self, SharedEngine, SharedTestPhysicalLayer},
        types::MacAddress,
        SharedInetStack,
    },
    runtime::memory::DemiBuffer,
};
use ::anyhow::Result;
use ::futures::FutureExt;
use ::std::{
    collections::{HashMap, VecDeque},
    net::Ipv4Addr,
    time::{Duration, Instant},
};

//======================================================================================================================
// Constants
//======================================================================================================================

/// ARP retry count.
const ARP_RETRY_COUNT: usize = 2;

/// ARP request timeout.
const ARP_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

//======================================================================================================================
// Tests
//======================================================================================================================

/// Tests immediate reply for an ARP request.
#[test]
fn arp_immediate_reply() -> Result<()> {
    let mut now: Instant = Instant::now();
    let local_mac: MacAddress = test_helpers::ALICE_MAC;
    let local_ipv4: Ipv4Addr = test_helpers::ALICE_IPV4;
    let remote_mac: MacAddress = test_helpers::BOB_MAC;
    let remote_ipv4: Ipv4Addr = test_helpers::BOB_IPV4;
    let mut engine: SharedEngine = new_engine(now, &test_helpers::ALICE_CONFIG_PATH)?;

    // Create an ARP query request to the local IP address.
    let buf: DemiBuffer = build_arp_query(&remote_mac, &remote_ipv4, &local_ipv4);

    // Feed it to engine.
    engine.push_frame(buf);

    // Move clock forward and poll the engine.
    now += Duration::from_micros(1);
    engine.advance_clock(now);
    engine.poll();

    // Check if the ARP cache outputs a reply message.
    let mut buffers: VecDeque<DemiBuffer> = engine.pop_all_frames();
    crate::ensure_eq!(buffers.len(), 1);
    let mut pkt: DemiBuffer = buffers.pop_front().unwrap();

    // Sanity check Ethernet header.
    let eth2_header: Ethernet2Header = Ethernet2Header::parse_and_strip(&mut pkt)?;
    crate::ensure_eq!(eth2_header.dst_addr(), remote_mac);
    crate::ensure_eq!(eth2_header.src_addr(), local_mac);
    crate::ensure_eq!(eth2_header.ether_type(), EtherType2::Arp);

    // Sanity check ARP header.
    let arp_header: ArpHeader = ArpHeader::parse_and_consume(pkt)?;
    crate::ensure_eq!(arp_header.get_operation(), ArpOperation::Reply);
    crate::ensure_eq!(arp_header.get_sender_hardware_addr(), local_mac);
    crate::ensure_eq!(arp_header.get_sender_protocol_addr(), local_ipv4);
    crate::ensure_eq!(arp_header.get_destination_protocol_addr(), remote_ipv4);

    Ok(())
}

/// Tests no reply for an ARP request.
#[test]
fn arp_no_reply() -> Result<()> {
    let mut now: Instant = Instant::now();
    let remote_mac: MacAddress = test_helpers::BOB_MAC;
    let remote_ipv4: Ipv4Addr = test_helpers::BOB_IPV4;
    let other_remote_ipv4: Ipv4Addr = test_helpers::CARRIE_IPV4;
    let mut engine: SharedEngine = new_engine(now, &test_helpers::ALICE_CONFIG_PATH)?;

    // Create an ARP query request to a different IP address.
    let buf: DemiBuffer = build_arp_query(&remote_mac, &remote_ipv4, &other_remote_ipv4);

    // Feed it to engine.
    engine.push_frame(buf);

    // Move clock forward and poll the engine.
    now += Duration::from_micros(1);
    engine.advance_clock(now);
    engine.poll();

    // Ensure that no reply message is output.
    let buffers: VecDeque<DemiBuffer> = engine.pop_all_frames();
    crate::ensure_eq!(buffers.len(), 0);

    Ok(())
}

/// Tests updates on the ARP cache.
#[test]
fn arp_cache_update() -> Result<()> {
    let mut now: Instant = Instant::now();
    let local_mac: MacAddress = test_helpers::BOB_MAC;
    let local_ipv4: Ipv4Addr = test_helpers::BOB_IPV4;
    let other_remote_mac: MacAddress = test_helpers::CARRIE_MAC;
    let other_remote_ipv4: Ipv4Addr = test_helpers::CARRIE_IPV4;
    let mut engine: SharedEngine = new_engine(now, &test_helpers::BOB_CONFIG_PATH)?;

    // Create an ARP query request to the local IP address.
    let buf: DemiBuffer = build_arp_query(&other_remote_mac, &other_remote_ipv4, &local_ipv4);

    // Feed it to engine.
    engine.push_frame(buf);

    // Move clock forward and poll the engine.
    now += Duration::from_micros(1);
    engine.advance_clock(now);
    engine.poll();

    // Check if the ARP cache has been updated.
    let cache: HashMap<Ipv4Addr, MacAddress> = engine.get_transport().export_arp_cache();
    crate::ensure_eq!(cache.get(&other_remote_ipv4), Some(&other_remote_mac));

    // Check if the ARP cache outputs a reply message.
    let mut buffers: VecDeque<DemiBuffer> = engine.pop_all_frames();
    crate::ensure_eq!(buffers.len(), 1);
    let mut first_pkt: DemiBuffer = buffers.pop_front().unwrap();

    // Sanity check Ethernet header.
    let eth2_header: Ethernet2Header = Ethernet2Header::parse_and_strip(&mut first_pkt)?;
    crate::ensure_eq!(eth2_header.dst_addr(), other_remote_mac);
    crate::ensure_eq!(eth2_header.src_addr(), local_mac);
    crate::ensure_eq!(eth2_header.ether_type(), EtherType2::Arp);

    // Sanity check ARP header.
    let arp_header: ArpHeader = ArpHeader::parse_and_consume(first_pkt)?;
    crate::ensure_eq!(arp_header.get_operation(), ArpOperation::Reply);
    crate::ensure_eq!(arp_header.get_sender_hardware_addr(), local_mac);
    crate::ensure_eq!(arp_header.get_sender_protocol_addr(), local_ipv4);
    crate::ensure_eq!(arp_header.get_destination_protocol_addr(), other_remote_ipv4);

    Ok(())
}

#[test]
fn arp_cache_timeout() -> Result<()> {
    use crate::QToken;

    let mut now: Instant = Instant::now();
    let other_remote_ipv4: Ipv4Addr = test_helpers::CARRIE_IPV4;
    let mut engine: SharedEngine = new_engine(now, test_helpers::ALICE_CONFIG_PATH)?;
    let mut inetstack: SharedInetStack = engine.get_transport();
    let coroutine = Box::pin(async move { inetstack.arp_query(other_remote_ipv4).await }.fuse());
    let qt: QToken = engine.get_runtime().clone().insert_coroutine("arp query", coroutine)?;
    engine.poll();
    engine.poll();

    for _ in 0..(ARP_RETRY_COUNT + 1) {
        // Check if the ARP cache outputs a reply message.
        let buffers: VecDeque<DemiBuffer> = engine.pop_all_frames();
        crate::ensure_eq!(buffers.len(), 1);

        // Move clock forward and poll the engine.
        now += ARP_REQUEST_TIMEOUT;
        engine.advance_clock(now);
        engine.poll();
        engine.poll();
    }

    // Check if the ARP cache outputs a reply message.
    let buffers: VecDeque<DemiBuffer> = engine.pop_all_frames();
    crate::ensure_eq!(buffers.len(), 0);

    // Ensure that the ARP query has failed with ETIMEDOUT.
    match engine.wait(qt, Duration::from_secs(0)) {
        Err(err) => crate::ensure_eq!(err.errno, libc::ETIMEDOUT),
        Ok(_) => unreachable!("arp query must fail with ETIMEDOUT"),
    }

    Ok(())
}

//======================================================================================================================
// Test Helpers
//======================================================================================================================

/// Builds an ARP query request.
fn build_arp_query(local_mac: &MacAddress, local_ipv4: &Ipv4Addr, remote_ipv4: &Ipv4Addr) -> DemiBuffer {
    let body: ArpHeader = ArpHeader::new(
        ArpOperation::Request,
        local_mac.clone(),
        local_ipv4.clone(),
        MacAddress::broadcast(),
        remote_ipv4.clone(),
    );
    let mut pkt: DemiBuffer = body.create_and_serialize();
    let eth2_header: Ethernet2Header =
        Ethernet2Header::new(MacAddress::broadcast(), local_mac.clone(), EtherType2::Arp);
    eth2_header.serialize_and_attach(&mut pkt);
    pkt
}

/// Creates a new engine.
fn new_engine(now: Instant, config_path: &str) -> Result<SharedEngine> {
    let layer1_endpoint: SharedTestPhysicalLayer = SharedTestPhysicalLayer::new_test(now);
    Ok(SharedEngine::new(config_path, layer1_endpoint, now)?)
}
