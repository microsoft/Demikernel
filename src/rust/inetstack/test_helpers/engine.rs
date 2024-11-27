// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use crate::{
    demi_sgarray_t,
    demikernel::{config::Config, libos::network::libos::SharedNetworkLibOS},
    inetstack::{test_helpers::SharedTestPhysicalLayer, types::MacAddress, SharedInetStack},
    runtime::{
        fail::Fail,
        memory::{DemiBuffer, MemoryRuntime},
        OperationResult, QDesc, QToken, SharedDemiRuntime, SharedObject,
    },
};
use ::socket2::{Domain, Protocol, Type};
use ::std::{
    collections::{HashMap, VecDeque},
    net::{Ipv4Addr, SocketAddrV4},
    ops::{Deref, DerefMut},
    time::{Duration, Instant},
};

/// A default amount of time to wait on an operation to complete. This was chosen arbitrarily.
pub const TIMEOUT_SECONDS: Duration = Duration::from_secs(256);

/// A representation of the engine that runs our tests. We keep a references to the highest level of abstraction
/// (libos) and the lowest (physical layer).
#[derive(Clone)]
pub struct Engine {
    libos: SharedNetworkLibOS<SharedInetStack>,
    layer1_endpoint: SharedTestPhysicalLayer,
}

pub struct SharedEngine(SharedObject<Engine>);

impl SharedEngine {
    pub fn new(config_path: &str, layer1_endpoint: SharedTestPhysicalLayer, now: Instant) -> Result<Self, Fail> {
        let config: Config = Config::new(config_path.to_string())?;
        let runtime: SharedDemiRuntime = SharedDemiRuntime::new(now);
        let transport: SharedInetStack = SharedInetStack::new_test(&config, runtime.clone(), layer1_endpoint.clone())?;

        Ok(Self(SharedObject::new(Engine {
            libos: SharedNetworkLibOS::<SharedInetStack>::new(runtime, transport),
            layer1_endpoint,
        })))
    }

    pub fn pop_frame(&mut self) -> DemiBuffer {
        self.layer1_endpoint.pop_frame()
    }

    pub fn pop_all_frames(&mut self) -> VecDeque<DemiBuffer> {
        self.layer1_endpoint.pop_all_frames()
    }

    pub fn advance_clock(&mut self, now: Instant) {
        self.libos.get_runtime().advance_clock(now)
    }

    pub fn push_frame(&mut self, bytes: DemiBuffer) {
        // We no longer do processing in this function, so we will not know if the packet is dropped or not.
        self.layer1_endpoint.push_frame(bytes);
        // So poll the scheduler to do the processing.
        self.libos.get_runtime().poll();
        self.libos.get_runtime().poll();
    }

    pub async fn ipv4_ping(&mut self, dest_ipv4_addr: Ipv4Addr, timeout: Option<Duration>) -> Result<Duration, Fail> {
        self.libos.get_transport().ping(dest_ipv4_addr, timeout).await
    }

    pub fn udp_pushto(&mut self, qd: QDesc, buf: DemiBuffer, to: SocketAddrV4) -> Result<QToken, Fail> {
        let data: demi_sgarray_t = self.libos.get_transport().into_sgarray(buf)?;
        self.libos.pushto(qd, &data, to.into())
    }

    pub fn udp_pop(&mut self, qd: QDesc) -> Result<QToken, Fail> {
        self.libos.pop(qd, None)
    }

    pub fn udp_socket(&mut self) -> Result<QDesc, Fail> {
        self.libos.socket(Domain::IPV4, Type::DGRAM, Protocol::UDP)
    }

    pub fn udp_bind(&mut self, socket_fd: QDesc, endpoint: SocketAddrV4) -> Result<(), Fail> {
        self.libos.bind(socket_fd, endpoint.into())
    }

    pub fn udp_close(&mut self, socket_fd: QDesc) -> Result<(), Fail> {
        let qt = self.libos.async_close(socket_fd)?;
        match self.wait(qt, TIMEOUT_SECONDS)? {
            (_, OperationResult::Close) => Ok(()),
            _ => unreachable!("close did not succeed"),
        }
    }

    pub fn tcp_socket(&mut self) -> Result<QDesc, Fail> {
        self.libos.socket(Domain::IPV4, Type::STREAM, Protocol::TCP)
    }

    pub fn tcp_connect(&mut self, socket_fd: QDesc, remote_endpoint: SocketAddrV4) -> Result<QToken, Fail> {
        self.libos.connect(socket_fd, remote_endpoint.into())
    }

    pub fn tcp_bind(&mut self, socket_fd: QDesc, endpoint: SocketAddrV4) -> Result<(), Fail> {
        self.libos.bind(socket_fd, endpoint.into())
    }

    pub fn tcp_accept(&mut self, fd: QDesc) -> Result<QToken, Fail> {
        self.libos.accept(fd)
    }

    pub fn tcp_push(&mut self, socket_fd: QDesc, buf: DemiBuffer) -> Result<QToken, Fail> {
        let data: demi_sgarray_t = self.libos.get_transport().into_sgarray(buf)?;
        self.libos.push(socket_fd, &data)
    }

    pub fn tcp_pop(&mut self, socket_fd: QDesc) -> Result<QToken, Fail> {
        self.libos.pop(socket_fd, None)
    }

    pub fn tcp_async_close(&mut self, socket_fd: QDesc) -> Result<QToken, Fail> {
        self.libos.async_close(socket_fd)
    }

    pub fn tcp_listen(&mut self, socket_fd: QDesc, backlog: usize) -> Result<(), Fail> {
        self.libos.listen(socket_fd, backlog)
    }

    pub async fn arp_query(self, ipv4_addr: Ipv4Addr) -> Result<MacAddress, Fail> {
        self.libos.get_transport().arp_query(ipv4_addr).await
    }

    pub fn export_arp_cache(&self) -> HashMap<Ipv4Addr, MacAddress> {
        self.libos.get_transport().export_arp_cache()
    }

    pub fn poll(&self) {
        self.libos.get_runtime().poll()
    }

    pub fn wait(&self, qt: QToken, timeout: Duration) -> Result<(QDesc, OperationResult), Fail> {
        // First check if the task has already completed.
        if let Some(result) = self.libos.get_runtime().get_completed_task(&qt) {
            return Ok(result);
        }

        // Otherwise, run the scheduler.
        // Put the QToken into a single element array.
        let qt_array: [QToken; 1] = [qt];
        let mut prev: Instant = Instant::now();
        let mut remaining_time: Duration = timeout;

        // Call run_any() until the task finishes.
        loop {
            // Run for one quanta and if one of our queue tokens completed, then return.
            if let Some((offset, qd, qr)) = self.libos.get_runtime().run_any(&qt_array, remaining_time) {
                debug_assert_eq!(offset, 0);
                return Ok((qd, qr));
            }
            let now: Instant = Instant::now();
            let elapsed_time: Duration = now - prev;
            if elapsed_time >= remaining_time {
                break;
            } else {
                remaining_time = remaining_time - elapsed_time;
                prev = now;
            }
        }
        Err(Fail::new(libc::ETIMEDOUT, "wait timed out"))
    }

    pub fn get_runtime(&self) -> SharedDemiRuntime {
        self.libos.get_runtime().clone()
    }

    pub fn get_transport(&self) -> SharedInetStack {
        self.libos.get_transport().clone()
    }
}

impl Deref for SharedEngine {
    type Target = Engine;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SharedEngine {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
