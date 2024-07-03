// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

//======================================================================================================================
// Imports
//======================================================================================================================

use crate::{
    demi_sgarray_t,
    demikernel::config::Config,
    inetstack::protocols::layer4::Layer4Endpoint,
    runtime::{
        fail::Fail,
        memory::{
            DemiBuffer,
            MemoryRuntime,
        },
        network::{
            socket::option::SocketOption,
            transport::NetworkTransport,
            types::MacAddress,
        },
        poll_yield,
        SharedDemiRuntime,
        SharedObject,
    },
    timer,
};
use ::socket2::{
    Domain,
    Type,
};
use protocols::{
    layer1::PhysicalLayer,
    layer2::SharedLayer2Endpoint,
    layer3::{
        SharedArpPeer,
        SharedLayer3Endpoint,
    },
    layer4::Socket,
};

use ::futures::FutureExt;
#[cfg(test)]
use ::std::{
    collections::HashMap,
    hash::RandomState,
    net::Ipv4Addr,
    time::Duration,
};
use ::std::{
    fmt::Debug,
    net::{
        SocketAddr,
        SocketAddrV4,
    },
    ops::{
        Deref,
        DerefMut,
    },
};

//======================================================================================================================
// Exports
//======================================================================================================================

#[cfg(test)]
pub mod test_helpers;

pub mod options;
pub mod protocols;

//======================================================================================================================
// Constants
//======================================================================================================================

const MAX_RECV_ITERS: usize = 2;

//======================================================================================================================
// Structures
//======================================================================================================================

/// Representation of a network stack designed for a network interface that expects raw ethernet frames.
pub struct InetStack {
    #[cfg(test)]
    layer2_endpoint: SharedLayer2Endpoint,
    #[cfg(test)]
    layer3_endpoint: SharedLayer3Endpoint,
    // Layer 4 endpoint. This layer include our network protocols: UDP and TCP.
    layer4_endpoint: Layer4Endpoint,
    // Routing table. This table holds the rules for routing incoming packets.
    // TODO: Implement routing table
    runtime: SharedDemiRuntime,
}

#[derive(Clone)]
pub struct SharedInetStack(SharedObject<InetStack>);

//======================================================================================================================
// Associated Functions
//======================================================================================================================

impl SharedInetStack {
    pub fn new(
        config: &Config,
        runtime: SharedDemiRuntime,
        layer1_endpoint: Box<dyn PhysicalLayer>,
    ) -> Result<Self, Fail> {
        SharedInetStack::new_test(config, runtime, layer1_endpoint)
    }

    pub fn new_test(
        config: &Config,
        mut runtime: SharedDemiRuntime,
        layer1_endpoint: Box<dyn PhysicalLayer>,
    ) -> Result<Self, Fail> {
        let rng_seed: [u8; 32] = [0; 32];
        let layer2_endpoint: SharedLayer2Endpoint = SharedLayer2Endpoint::new(config, layer1_endpoint)?;
        let arp: SharedArpPeer = SharedArpPeer::new(&config, runtime.clone(), layer2_endpoint.clone())?;
        let layer3_endpoint: SharedLayer3Endpoint =
            SharedLayer3Endpoint::new(config, runtime.clone(), arp.clone(), layer2_endpoint.clone(), rng_seed)?;
        let layer4_endpoint: Layer4Endpoint =
            Layer4Endpoint::new(config, runtime.clone(), arp.clone(), layer3_endpoint.clone(), rng_seed)?;
        let me: Self = Self(SharedObject::new(InetStack {
            #[cfg(test)]
            layer2_endpoint,
            #[cfg(test)]
            layer3_endpoint,
            layer4_endpoint,
            runtime: runtime.clone(),
        }));
        runtime.insert_background_coroutine("bgc::inetstack::poll_recv", Box::pin(me.clone().poll().fuse()))?;
        Ok(me)
    }

    /// Scheduler will poll all futures that are ready to make progress.
    /// Then ask the runtime to receive new data which we will forward to the engine to parse and
    /// route to the correct protocol.
    pub async fn poll(mut self) {
        timer!("inetstack::poll");
        loop {
            for _ in 0..MAX_RECV_ITERS {
                timer!("inetstack::poll_bg_work::for::receive");

                // TODO: Log dropped and processed packets.
                if let Err(e) = self.layer4_endpoint.receive() {
                    warn!("Error while receiving packet: {:?}", e);
                }
            }
            poll_yield().await;
        }
    }

    /// Generally these functions are for testing.
    #[cfg(test)]
    pub fn get_ip_addr(&self) -> Ipv4Addr {
        self.layer3_endpoint.get_local_addr()
    }

    #[cfg(test)]
    pub fn get_physical_layer<P: PhysicalLayer>(&mut self) -> &mut P {
        self.layer2_endpoint.get_physical_layer()
    }

    #[cfg(test)]
    /// Schedule a ping.
    pub async fn ping(&mut self, addr: Ipv4Addr, timeout: Option<Duration>) -> Result<Duration, Fail> {
        self.layer4_endpoint.ping(addr, timeout).await
    }

    #[cfg(test)]
    pub async fn arp_query(&mut self, addr: Ipv4Addr) -> Result<MacAddress, Fail> {
        self.layer4_endpoint.arp_query(addr).await
    }

    #[cfg(test)]
    pub fn export_arp_cache(&self) -> HashMap<Ipv4Addr, MacAddress, RandomState> {
        self.layer4_endpoint.export_arp_cache()
    }

    pub fn receive(&mut self) -> Result<(), Fail> {
        self.layer4_endpoint.receive()
    }
}

//======================================================================================================================
// Trait Implementation
//======================================================================================================================

impl Deref for SharedInetStack {
    type Target = InetStack;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl DerefMut for SharedInetStack {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}

/// Memory Runtime Trait Implementation for Inetstack. This implementation dispatches to each lower level and finally
/// to the physical layer implementation.
impl MemoryRuntime for SharedInetStack {
    /// Casts a [DPDKBuf] into an [demi_sgarray_t].
    fn into_sgarray(&self, buf: DemiBuffer) -> Result<demi_sgarray_t, Fail> {
        self.layer4_endpoint.into_sgarray(buf)
    }

    /// Allocates a [demi_sgarray_t].
    fn sgaalloc(&self, size: usize) -> Result<demi_sgarray_t, Fail> {
        self.layer4_endpoint.sgaalloc(size)
    }

    /// Releases a [demi_sgarray_t].
    fn sgafree(&self, sga: demi_sgarray_t) -> Result<(), Fail> {
        self.layer4_endpoint.sgafree(sga)
    }

    /// Clones a [demi_sgarray_t].
    fn clone_sgarray(&self, sga: &demi_sgarray_t) -> Result<DemiBuffer, Fail> {
        self.layer4_endpoint.clone_sgarray(sga)
    }
}

impl NetworkTransport for SharedInetStack {
    // Socket data structure used by upper level libOS to identify this socket.
    type SocketDescriptor = Socket;

    ///
    /// **Brief**
    ///
    /// Creates an endpoint for communication and returns a file descriptor that
    /// refers to that endpoint. The file descriptor returned by a successful
    /// call will be the lowest numbered file descriptor not currently open for
    /// the process.
    ///
    /// The domain argument specifies a communication domain; this selects the
    /// protocol family which will be used for communication. These families are
    /// defined in the libc crate. Currently, the following families are supported:
    ///
    /// - AF_INET Internet Protocol Version 4 (IPv4)
    ///
    /// **Return Vale**
    ///
    /// Upon successful completion, a file descriptor for the newly created
    /// socket is returned. Upon failure, `Fail` is returned instead.
    ///
    fn socket(&mut self, domain: Domain, typ: Type) -> Result<Self::SocketDescriptor, Fail> {
        self.layer4_endpoint.socket(domain, typ)
    }

    /// Set an SO_* option on the socket.
    fn set_socket_option(&mut self, sd: &mut Self::SocketDescriptor, option: SocketOption) -> Result<(), Fail> {
        self.layer4_endpoint.set_socket_option(sd, option)
    }

    /// Gets an SO_* option on the socket. The option should be passed in as [option] and the value is returned in
    /// [option].
    fn get_socket_option(
        &mut self,
        sd: &mut Self::SocketDescriptor,
        option: SocketOption,
    ) -> Result<SocketOption, Fail> {
        self.layer4_endpoint.get_socket_option(sd, option)
    }

    fn getpeername(&mut self, sd: &mut Self::SocketDescriptor) -> Result<SocketAddrV4, Fail> {
        self.layer4_endpoint.getpeername(sd)
    }

    ///
    /// **Brief**
    ///
    /// Binds the socket referred to by `qd` to the local endpoint specified by
    /// `local`.
    ///
    /// **Return Value**
    ///
    /// Upon successful completion, `Ok(())` is returned. Upon failure, `Fail` is
    /// returned instead.
    ///
    fn bind(&mut self, sd: &mut Self::SocketDescriptor, local: SocketAddr) -> Result<(), Fail> {
        self.layer4_endpoint.bind(sd, local)
    }

    ///
    /// **Brief**
    ///
    /// Marks the socket referred to by `qd` as a socket that will be used to
    /// accept incoming connection requests using [accept](Self::accept). The `qd` should
    /// refer to a socket of type `SOCK_STREAM`. The `backlog` argument defines
    /// the maximum length to which the queue of pending connections for `qd`
    /// may grow. If a connection request arrives when the queue is full, the
    /// client may receive an error with an indication that the connection was
    /// refused.
    ///
    /// **Return Value**
    ///
    /// Upon successful completion, `Ok(())` is returned. Upon failure, `Fail` is
    /// returned instead.
    ///
    fn listen(&mut self, sd: &mut Self::SocketDescriptor, backlog: usize) -> Result<(), Fail> {
        self.layer4_endpoint.listen(sd, backlog)
    }

    ///
    /// **Brief**
    ///
    /// Accepts an incoming connection request on the queue of pending
    /// connections for the listening socket referred to by `qd`.
    ///
    /// **Return Value**
    ///
    /// Upon successful completion, a queue token is returned. This token can be
    /// used to wait for a connection request to arrive. Upon failure, `Fail` is
    /// returned instead.
    ///
    async fn accept(&mut self, sd: &mut Self::SocketDescriptor) -> Result<(Self::SocketDescriptor, SocketAddr), Fail> {
        self.layer4_endpoint.accept(sd).await
    }

    ///
    /// **Brief**
    ///
    /// Connects the socket referred to by `qd` to the remote endpoint specified by `remote`.
    ///
    /// **Return Value**
    ///
    /// Upon successful completion, a queue token is returned. This token can be
    /// used to push and pop data to/from the queue that connects the local and
    /// remote endpoints. Upon failure, `Fail` is
    /// returned instead.
    ///
    async fn connect(&mut self, sd: &mut Self::SocketDescriptor, remote: SocketAddr) -> Result<(), Fail> {
        self.layer4_endpoint.connect(sd, remote).await
    }

    ///
    /// **Brief**
    ///
    /// Asynchronously closes a connection referred to by `qd`.
    ///
    /// **Return Value**
    ///
    /// Upon successful completion, `Ok(())` is returned. This qtoken can be used to wait until the close
    /// completes shutting down the connection. Upon failure, `Fail` is returned instead.
    ///
    async fn close(&mut self, sd: &mut Self::SocketDescriptor) -> Result<(), Fail> {
        self.layer4_endpoint.close(sd).await
    }

    /// Forcibly close a socket. This should only be used on clean up.
    fn hard_close(&mut self, sd: &mut Self::SocketDescriptor) -> Result<(), Fail> {
        self.layer4_endpoint.hard_close(sd)
    }

    /// Pushes a buffer to a TCP socket.
    async fn push(
        &mut self,
        sd: &mut Self::SocketDescriptor,
        buf: &mut DemiBuffer,
        addr: Option<SocketAddr>,
    ) -> Result<(), Fail> {
        self.layer4_endpoint.push(sd, buf, addr).await
    }

    /// Create a pop request to write data from IO connection represented by `qd` into a buffer
    /// allocated by the application.
    async fn pop(
        &mut self,
        sd: &mut Self::SocketDescriptor,
        size: usize,
    ) -> Result<(Option<SocketAddr>, DemiBuffer), Fail> {
        self.layer4_endpoint.pop(sd, size).await
    }

    fn get_runtime(&self) -> &SharedDemiRuntime {
        &self.runtime
    }
}

impl Debug for Socket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Socket::Tcp(socket) => socket.fmt(f),
            Socket::Udp(socket) => socket.fmt(f),
        }
    }
}
