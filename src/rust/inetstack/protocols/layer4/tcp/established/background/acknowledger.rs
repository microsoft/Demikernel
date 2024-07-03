// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use crate::{
    collections::async_value::SharedAsyncValue,
    inetstack::protocols::layer4::tcp::established::background::SharedControlBlock,
    runtime::{
        fail::Fail,
        network::NetworkRuntime,
    },
};
use ::futures::never::Never;
use ::std::time::Instant;

pub async fn acknowledger<N: NetworkRuntime>(mut cb: SharedControlBlock<N>) -> Result<Never, Fail> {
    let mut ack_deadline: SharedAsyncValue<Option<Instant>> = cb.get_ack_deadline();
    let mut deadline: Option<Instant> = ack_deadline.get();
    loop {
        // TODO: Implement TCP delayed ACKs, subject to restrictions from RFC 1122
        // - TCP should implement a delayed ACK
        // - The delay must be less than 500ms
        // - For a stream of full-sized segments, there should be an ack for every other segment.
        // TODO: Implement SACKs
        match ack_deadline.wait_for_change_until(deadline).await {
            Ok(value) => {
                deadline = value;
                continue;
            },
            Err(Fail { errno, cause: _ }) if errno == libc::ETIMEDOUT => {
                cb.send_ack();
                deadline = ack_deadline.get();
            },
            Err(_) => {
                unreachable!("either the ack deadline changed or the deadline passed, no other errors are possible!")
            },
        }
    }
}
