// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

mod background;
pub mod congestion_control;
mod ctrlblk;
mod rto;
mod sender;

use crate::{
    collections::async_queue::SharedAsyncQueue,
    inetstack::{
        protocols::{
            layer3::SharedArpPeer,
            layer4::tcp::{
                congestion_control::CongestionControlConstructor,
                established::ctrlblk::SharedControlBlock,
                ReceiveQueue,
                SeqNumber,
            },
        },
        MacAddress,
    },
    runtime::{
        fail::Fail,
        memory::DemiBuffer,
        network::{
            config::TcpConfig,
            socket::option::TcpSocketOptions,
            NetworkRuntime,
        },
        QDesc,
        SharedDemiRuntime,
    },
    QToken,
};
use ::futures::{
    channel::mpsc,
    FutureExt,
};
use ::std::{
    net::SocketAddrV4,
    time::Duration,
};

#[derive(Clone)]
pub struct EstablishedSocket<N: NetworkRuntime> {
    pub cb: SharedControlBlock<N>,
    recv_queue: ReceiveQueue,
    // We need this to eventually stop the background task on close.
    #[allow(unused)]
    runtime: SharedDemiRuntime,
    /// The background co-routines handles various tasks, such as retransmission and acknowledging.
    /// We annotate it as unused because the compiler believes that it is never called which is not the case.
    #[allow(unused)]
    background_task_qt: QToken,
}

impl<N: NetworkRuntime> EstablishedSocket<N> {
    pub fn new(
        local: SocketAddrV4,
        remote: SocketAddrV4,
        mut runtime: SharedDemiRuntime,
        transport: N,
        recv_queue: ReceiveQueue,
        ack_queue: SharedAsyncQueue<usize>,
        local_link_addr: MacAddress,
        tcp_config: TcpConfig,
        default_socket_options: TcpSocketOptions,
        arp: SharedArpPeer<N>,
        receiver_seq_no: SeqNumber,
        ack_delay_timeout: Duration,
        receiver_window_size: u32,
        receiver_window_scale: u32,
        sender_seq_no: SeqNumber,
        sender_window_size: u32,
        sender_window_scale: u8,
        sender_mss: usize,
        cc_constructor: CongestionControlConstructor,
        congestion_control_options: Option<congestion_control::Options>,
        dead_socket_tx: mpsc::UnboundedSender<QDesc>,
        socket_queue: Option<SharedAsyncQueue<SocketAddrV4>>,
    ) -> Result<Self, Fail> {
        // TODO: Maybe add the queue descriptor here.
        let cb = SharedControlBlock::new(
            local,
            remote,
            runtime.clone(),
            transport,
            local_link_addr,
            tcp_config,
            default_socket_options,
            arp,
            receiver_seq_no,
            ack_delay_timeout,
            receiver_window_size,
            receiver_window_scale,
            sender_seq_no,
            sender_window_size,
            sender_window_scale,
            sender_mss,
            cc_constructor,
            congestion_control_options,
            recv_queue.clone(),
            ack_queue.clone(),
            socket_queue,
        );
        let qt: QToken = runtime.insert_background_coroutine(
            "bgc::inetstack::tcp::established::background",
            Box::pin(background::background(cb.clone(), dead_socket_tx).fuse()),
        )?;
        Ok(Self {
            cb,
            recv_queue,
            background_task_qt: qt.clone(),
            runtime: runtime.clone(),
        })
    }

    pub fn get_recv_queue(&self) -> ReceiveQueue {
        self.recv_queue.clone()
    }

    pub fn send(&mut self, buf: DemiBuffer) -> Result<(), Fail> {
        self.cb.send(buf)
    }

    pub async fn push(&mut self, nbytes: usize) -> Result<(), Fail> {
        self.cb.push(nbytes).await
    }

    pub async fn pop(&mut self, size: Option<usize>) -> Result<DemiBuffer, Fail> {
        self.cb.pop(size).await
    }

    pub async fn close(&mut self) -> Result<(), Fail> {
        self.cb.close().await
    }

    pub fn remote_mss(&self) -> usize {
        self.cb.remote_mss()
    }

    pub fn current_rto(&self) -> Duration {
        self.cb.rto()
    }

    pub fn endpoints(&self) -> (SocketAddrV4, SocketAddrV4) {
        (self.cb.get_local(), self.cb.get_remote())
    }
}
