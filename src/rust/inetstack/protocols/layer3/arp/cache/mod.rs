// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[cfg(test)]
mod tests;

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{collections::hashttlcache::HashTtlCache, inetstack::types::MacAddress};
use ::std::{
    collections::HashMap,
    net::Ipv4Addr,
    time::{Duration, Instant},
};

//======================================================================================================================
// Constants
//======================================================================================================================

const DEFAULT_DUMMY_MAC_ADDRESS: MacAddress = MacAddress::new([0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc]);

//======================================================================================================================
// Structures
//======================================================================================================================

#[derive(Debug)]
pub struct Record {
    link_addr: MacAddress,
}

///
/// # ARP Cache
/// - TODO: Allow multiple waiters for the same address
/// - TODO: Deregister waiters here when the receiver goes away.
/// - TODO: Implement eviction.
/// - TODO: Implement remove.
/// Cache for IPv4 Addresses. If set to Off, then all packets will be filled with a dummy MAC address.
pub enum ArpCache {
    On(HashTtlCache<Ipv4Addr, Record>),
    Off(MacAddress),
}

//======================================================================================================================
// Associate Functions
//======================================================================================================================

impl ArpCache {
    /// Creates an ARP Cache.
    pub fn new(
        now: Instant,
        default_ttl: Option<Duration>,
        values: Option<&HashMap<Ipv4Addr, MacAddress>>,
        is_enabled: bool,
        dummy_mac: Option<MacAddress>,
    ) -> ArpCache {
        if is_enabled {
            let hash_ttl_cache = HashTtlCache::<Ipv4Addr, Record>::new(now, default_ttl);
            let mut cache: HashTtlCache<Ipv4Addr, Record> = hash_ttl_cache;
            if let Some(values) = values {
                for (&k, &v) in values {
                    if let Some(record) = cache.insert(k, Record { link_addr: v }) {
                        warn!(
                            "Inserting two cache entries with the same address: address={:?} first MAC={:?} second \
                             MAC={:?}",
                            k, record, v
                        );
                    }
                }
            };
            ArpCache::On(cache)
        } else {
            ArpCache::Off(dummy_mac.unwrap_or(DEFAULT_DUMMY_MAC_ADDRESS))
        }
    }

    /// Caches an address resolution.
    pub fn insert(&mut self, ipv4_addr: Ipv4Addr, link_addr: MacAddress) -> Option<MacAddress> {
        match self {
            Self::On(ref mut cache) => {
                let record = Record { link_addr };
                cache.insert(ipv4_addr, record).map(|r| r.link_addr)
            },
            Self::Off(_) => None,
        }
    }

    /// Gets the MAC address of given IPv4 address.
    pub fn get(&self, ipv4_addr: Ipv4Addr) -> Option<&MacAddress> {
        match self {
            Self::On(ref cache) => cache.get(&ipv4_addr).map(|r| &r.link_addr),
            Self::Off(ref mac_addr) => Some(mac_addr),
        }
    }

    /// Clears the ARP cache.
    #[allow(unused)]
    pub fn clear(&mut self) {
        if let Self::On(ref mut cache) = self {
            cache.clear()
        };
    }

    // Exports address resolutions that are stored in the ARP cache.
    #[cfg(test)]
    pub fn export(&self) -> HashMap<Ipv4Addr, MacAddress> {
        let mut map: HashMap<Ipv4Addr, MacAddress> = HashMap::default();
        if let Self::On(ref cache) = self {
            for (k, v) in cache.iter() {
                map.insert(*k, v.link_addr);
            }
        }
        map
    }

    #[cfg(test)]
    pub fn advance_clock(&mut self, now: Instant) {
        if let Self::On(ref mut cache) = self {
            cache.advance_clock(now)
        }
    }
}
