// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

mod acknowledger;
mod retransmitter;
mod sender;

use self::{
    acknowledger::acknowledger,
    retransmitter::retransmitter,
    sender::sender,
};
use crate::{
    async_timer,
    inetstack::protocols::layer4::tcp::established::ctrlblk::SharedControlBlock,
    runtime::{
        network::NetworkRuntime,
        QDesc,
    },
};
use ::futures::{
    channel::mpsc,
    pin_mut,
    FutureExt,
};

pub async fn background<N: NetworkRuntime>(cb: SharedControlBlock<N>, _dead_socket_tx: mpsc::UnboundedSender<QDesc>) {
    let acknowledger = async_timer!("tcp::established::background::acknowledger", acknowledger(cb.clone())).fuse();
    pin_mut!(acknowledger);

    let retransmitter = async_timer!("tcp::established::background::retransmitter", retransmitter(cb.clone())).fuse();
    pin_mut!(retransmitter);

    let sender = async_timer!("tcp::established::background::sender", sender(cb.clone())).fuse();
    pin_mut!(sender);

    let mut cb2: SharedControlBlock<N> = cb.clone();
    let receiver = async_timer!("tcp::established::background::receiver", cb2.poll()).fuse();
    pin_mut!(receiver);

    let r = futures::select_biased! {
        r = receiver => r,
        r = acknowledger => r,
        r = retransmitter => r,
        r = sender => r,
    };
    error!("Connection terminated: {:?}", r);
}
