// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

pub mod congestion_control;
pub mod ctrlblk;
mod receiver;
mod rto;
mod sender;

use crate::{
    collections::async_queue::SharedAsyncQueue,
    inetstack::protocols::{
        layer3::SharedLayer3Endpoint,
        layer4::tcp::{
            congestion_control::CongestionControlConstructor, established::ctrlblk::SharedControlBlock,
            header::TcpHeader, SeqNumber,
        },
    },
    runtime::{
        fail::Fail,
        memory::DemiBuffer,
        network::{config::TcpConfig, socket::option::TcpSocketOptions},
        QDesc, SharedDemiRuntime,
    },
    QToken,
};
use ::futures::{channel::mpsc, FutureExt};
use ::std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

#[derive(Clone)]
pub struct EstablishedSocket {
    pub cb: SharedControlBlock,
    // We need this to eventually stop the background task on close.
    #[allow(unused)]
    runtime: SharedDemiRuntime,
    /// The background co-routines handles various tasks, such as retransmission and acknowledging.
    /// We annotate it as unused because the compiler believes that it is never called which is not the case.
    #[allow(unused)]
    background_task_qt: QToken,
}

impl EstablishedSocket {
    pub fn new(
        local: SocketAddrV4,
        remote: SocketAddrV4,
        mut runtime: SharedDemiRuntime,
        layer3_endpoint: SharedLayer3Endpoint,
        recv_queue: SharedAsyncQueue<(Ipv4Addr, TcpHeader, DemiBuffer)>,
        tcp_config: TcpConfig,
        default_socket_options: TcpSocketOptions,
        receiver_seq_no: SeqNumber,
        ack_delay_timeout: Duration,
        receiver_window_size: u32,
        receiver_window_scale: u8,
        sender_seq_no: SeqNumber,
        sender_window_size: u32,
        sender_window_scale: u8,
        sender_mss: usize,
        cc_constructor: CongestionControlConstructor,
        congestion_control_options: Option<congestion_control::Options>,
        dead_socket_tx: mpsc::UnboundedSender<QDesc>,
    ) -> Result<Self, Fail> {
        // TODO: Maybe add the queue descriptor here.
        let cb = SharedControlBlock::new(
            local,
            remote,
            layer3_endpoint,
            runtime.clone(),
            tcp_config,
            default_socket_options,
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
        );

        let cb2: SharedControlBlock = cb.clone();
        let qt: QToken = runtime.insert_background_coroutine(
            "bgc::inetstack::tcp::established::background",
            Box::pin(async move { cb2.background(dead_socket_tx).await }.fuse()),
        )?;
        Ok(Self {
            cb,
            background_task_qt: qt.clone(),
            runtime: runtime.clone(),
        })
    }

    pub async fn push(&mut self, buf: DemiBuffer) -> Result<(), Fail> {
        self.cb.push(buf).await
    }

    pub async fn pop(&mut self, size: Option<usize>) -> Result<DemiBuffer, Fail> {
        self.cb.pop(size).await
    }

    pub async fn close(&mut self) -> Result<(), Fail> {
        self.cb.close().await
    }

    pub fn endpoints(&self) -> (SocketAddrV4, SocketAddrV4) {
        (self.cb.get_local(), self.cb.get_remote())
    }

    pub fn get_cb(&self) -> SharedControlBlock {
        self.cb.clone()
    }
}
