// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

use crate::{
    collections::{async_queue::SharedAsyncQueue, async_value::SharedAsyncValue},
    inetstack::protocols::layer4::tcp::{
        established::{rto::RtoCalculator, SharedControlBlock},
        header::TcpHeader,
        SeqNumber,
    },
    runtime::{conditional_yield_until, fail::Fail, memory::DemiBuffer},
};
use ::futures::{pin_mut, select_biased, FutureExt};
use ::libc::{EBUSY, EINVAL};
use ::std::{
    fmt,
    time::{Duration, Instant},
};
use futures::never::Never;
use std::cmp;

// Structure of entries on our unacknowledged queue.
// TODO: We currently allocate these on the fly when we add a buffer to the queue.  Would be more efficient to have a
// buffer structure that held everything we need directly, thus avoiding this extra wrapper.
//
pub struct UnackedSegment {
    pub bytes: Option<DemiBuffer>,
    // Set to `None` on retransmission to implement Karn's algorithm.
    pub initial_tx: Option<Instant>,
}

// Hard limit for unsent queue.
// TODO: Remove this.  We should limit the unsent queue by either having a (configurable) send buffer size (in bytes,
// not segments) and rejecting send requests that exceed that, or by limiting the user's send buffer allocations.
const UNSENT_QUEUE_CUTOFF: usize = 1024;

// Minimum size for unacknowledged queue. This number doesn't really matter very much, it just sets the initial size
// of the unacked queue, below which memory allocation is not required.
const MIN_UNACKED_QUEUE_SIZE_FRAMES: usize = 64;

// Minimum size for unsent queue. This number doesn't really matter very much, it just sets the initial size
// of the unacked queue, below which memory allocation is not required.
const MIN_UNSENT_QUEUE_SIZE_FRAMES: usize = 64;

// TODO: Consider moving retransmit timer and congestion control fields out of this structure.
// TODO: Make all public fields in this structure private.
pub struct Sender {
    //
    // Send Sequence Space:
    //
    //                     |<-----------------send window size----------------->|
    //                     |                                                    |
    //                send_unacked               send_next         send_unacked + send window
    //                     v                         v                          v
    // ... ----------------|-------------------------|--------------------------|--------------------------------
    //       acknowledged  |      unacknowledged     |     allowed to send      |  future sequence number space
    //
    // Note: In RFC 793 terminology, send_unacked is SND.UNA, send_next is SND.NXT, and "send window" is SND.WND.
    //

    // Sequence Number of the oldest byte of unacknowledged sent data.  In RFC 793 terms, this is SND.UNA.
    send_unacked: SharedAsyncValue<SeqNumber>,

    // Queue of unacknowledged sent data.  RFC 793 calls this the "retransmission queue".
    unacked_queue: SharedAsyncQueue<UnackedSegment>,

    // Send timers
    // Current retransmission timer expiration time.
    // TODO: Consider storing this directly in the RtoCalculator.
    retransmit_deadline_time_secs: SharedAsyncValue<Option<Instant>>,

    // Retransmission Timeout (RTO) calculator.
    rto_calculator: RtoCalculator,

    // In RFC 793 terms, this is SND.NXT.
    send_next_seq_no: SharedAsyncValue<SeqNumber>,

    // Sequence number of next data to be pushed but not sent. When there is an open window, this is equivalent to
    // send_next_seq_no.
    unsent_next_seq_no: SeqNumber,

    // Sequence number of the FIN, after we should never allocate more sequence numbers.
    fin_seq_no: Option<SeqNumber>,

    // This is the send buffer (user data we do not yet have window to send). If the option is None, then it indicates
    // a FIN. This keeps us from having to allocate an empty Demibuffer to indicate FIN.
    unsent_queue: SharedAsyncQueue<Option<DemiBuffer>>,

    // Available window to send into, as advertised by our peer.  In RFC 793 terms, this is SND.WND.
    send_window: SharedAsyncValue<u32>,
    send_window_last_update_seq: SeqNumber, // SND.WL1
    send_window_last_update_ack: SeqNumber, // SND.WL2

    // RFC 1323: Number of bits to shift advertised window, defaults to zero.
    send_window_scale_shift_bits: u8,

    // Maximum Segment Size currently in use for this connection.
    // TODO: Revisit this once we support path MTU discovery.
    mss: usize,
}

impl fmt::Debug for Sender {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Sender")
            .field("send_unacked", &self.send_unacked)
            .field("send_next", &self.send_next_seq_no)
            .field("send_window", &self.send_window)
            .field("window_scale", &self.send_window_scale_shift_bits)
            .field("mss", &self.mss)
            .finish()
    }
}

impl Sender {
    pub fn new(seq_no: SeqNumber, send_window: u32, send_window_scale_shift_bits: u8, mss: usize) -> Self {
        Self {
            send_unacked: SharedAsyncValue::new(seq_no),
            unacked_queue: SharedAsyncQueue::with_capacity(MIN_UNACKED_QUEUE_SIZE_FRAMES),
            retransmit_deadline_time_secs: SharedAsyncValue::new(None),
            rto_calculator: RtoCalculator::new(),
            send_next_seq_no: SharedAsyncValue::new(seq_no),
            unsent_next_seq_no: seq_no,
            fin_seq_no: None,
            unsent_queue: SharedAsyncQueue::with_capacity(MIN_UNSENT_QUEUE_SIZE_FRAMES),
            send_window: SharedAsyncValue::new(send_window),
            send_window_last_update_seq: seq_no,
            send_window_last_update_ack: seq_no,
            send_window_scale_shift_bits,
            mss,
        }
    }

    // This function sends a packet and waits for it to be acked.
    pub async fn push(&mut self, buf: DemiBuffer) -> Result<(), Fail> {
        // If the user is done sending (i.e. has called close on this connection), then they shouldn't be sending.
        debug_assert!(self.fin_seq_no.is_none());
        // Our API supports send buffers up to usize (variable, depends upon architecture) in size.  While we could
        // allow for larger send buffers, it is simpler and more practical to limit a single send to 1 GiB, which is
        // also the maximum value a TCP can advertise as its receive window (with maximum window scaling).
        // TODO: the below check just limits a single send to 4 GiB, not 1 GiB.  Check this doesn't break anything.
        //
        // Review: Move this check up the stack (i.e. closer to the user)?
        //
        let _: u32 = buf
            .len()
            .try_into()
            .map_err(|_| Fail::new(EINVAL, "buffer too large"))?;

        // TODO: We need to fix this the correct way: limit our send buffer size to the amount we're willing to buffer.
        if self.unsent_queue.len() > UNSENT_QUEUE_CUTOFF {
            return Err(Fail::new(EBUSY, "too many packets to send"));
        }

        // Place the buffer in the unsent queue.
        self.unsent_next_seq_no = self.unsent_next_seq_no + (buf.len() as u32).into();
        self.unsent_queue.push(Some(buf));

        // Wait until the sequnce number of the pushed buffer is acknowledged.
        let mut send_unacked_watched: SharedAsyncValue<SeqNumber> = self.send_unacked.clone();
        let ack_seq_no: SeqNumber = self.unsent_next_seq_no;
        debug_assert!(send_unacked_watched.get() < ack_seq_no);
        while send_unacked_watched.get() < ack_seq_no {
            send_unacked_watched.wait_for_change(None).await?;
        }
        Ok(())
    }

    // Places a FIN marker in the outgoing data stream. No data can be pushed after this.
    pub async fn push_fin_and_wait_for_ack(&mut self) -> Result<(), Fail> {
        debug_assert!(self.fin_seq_no.is_none());
        // TODO: We need to fix this the correct way: limit our send buffer size to the amount we're willing to buffer.
        if self.unsent_queue.len() > UNSENT_QUEUE_CUTOFF {
            return Err(Fail::new(EBUSY, "too many packets to send"));
        }

        self.fin_seq_no = Some(self.unsent_next_seq_no);
        self.unsent_next_seq_no = self.unsent_next_seq_no + 1.into();
        self.unsent_queue.push(None);

        let mut send_unacked_watched: SharedAsyncValue<SeqNumber> = self.send_unacked.clone();
        let fin_ack_num: SeqNumber = self.unsent_next_seq_no;
        while self.send_unacked.get() < fin_ack_num {
            send_unacked_watched.wait_for_change(None).await?;
        }
        Ok(())
    }

    pub async fn background_sender(&mut self, mut cb: SharedControlBlock) -> Result<Never, Fail> {
        loop {
            // Get next bit of unsent data.
            if let Some(buf) = self.unsent_queue.pop(None).await? {
                self.send_buffer(buf, &mut cb).await?;
            } else {
                self.send_fin(&mut cb)?;
                // Exit the loop because we no longer have anything to process
                return Err(Fail::new(libc::ECONNRESET, "Processed and sent FIN"));
            }
        }
    }

    fn send_fin(&mut self, cb: &mut SharedControlBlock) -> Result<(), Fail> {
        let mut header: TcpHeader = cb.tcp_header();
        header.seq_num = self.send_next_seq_no.get();
        debug_assert!(self.fin_seq_no.is_some_and(|s| { s == header.seq_num }));
        header.fin = true;
        cb.emit(header, None);
        // Update SND.NXT.
        self.send_next_seq_no.modify(|s| s + 1.into());

        // Add the FIN to our unacknowledged queue.
        let unacked_segment = UnackedSegment {
            bytes: None,
            initial_tx: Some(cb.get_now()),
        };
        self.unacked_queue.push(unacked_segment);
        // Set the retransmit timer.
        if self.retransmit_deadline_time_secs.get().is_none() {
            let rto: Duration = self.rto_calculator.rto();
            trace!("set retransmit: {:?}", rto);
            self.retransmit_deadline_time_secs.set(Some(cb.get_now() + rto));
        }
        Ok(())
    }

    async fn send_buffer(&mut self, mut buffer: DemiBuffer, cb: &mut SharedControlBlock) -> Result<(), Fail> {
        let mut send_unacked_watched: SharedAsyncValue<SeqNumber> = self.send_unacked.clone();
        let mut cwnd_watched: SharedAsyncValue<u32> = cb.congestion_control_get_cwnd();

        // The limited transmit algorithm may increase the effective size of cwnd by up to 2 * mss.
        let mut ltci_watched: SharedAsyncValue<u32> = cb.congestion_control_get_limited_transmit_cwnd_increase();
        let mut win_sz_watched: SharedAsyncValue<u32> = self.send_window.clone();

        // Try in a loop until we send this segment.
        loop {
            // If we don't have any window size at all, we need to transition to PERSIST mode and
            // repeatedly send window probes until window opens up.
            if win_sz_watched.get() == 0 {
                // Send a window probe (this is a one-byte packet designed to elicit a window update from our peer).
                self.send_window_probe(buffer.split_front(1)?, cb).await?;
            } else {
                // TODO: Nagle's algorithm - We need to coalese small buffers together to send MSS sized packets.
                // TODO: Silly window syndrome - See RFC 1122's discussion of the SWS avoidance algorithm.

                // We have some window, try to send some or all of the segment.
                let _: usize = self.send_segment(&mut buffer, cb);
                // If the buffer is now empty, then we sent all of it.
                if buffer.len() == 0 {
                    return Ok(());
                }
                // Otherwise, wait until something limiting the window changes and then try again to finish sending
                // the segment.
                futures::select_biased! {
                    _ = send_unacked_watched.wait_for_change(None).fuse() => (),
                    _ = self.send_next_seq_no.wait_for_change(None).fuse() => (),
                    _ = win_sz_watched.wait_for_change(None).fuse() => (),
                    _ = cwnd_watched.wait_for_change(None).fuse() => (),
                    _ = ltci_watched.wait_for_change(None).fuse() => (),
                };
            }
        }
    }

    async fn send_window_probe(&mut self, probe: DemiBuffer, cb: &mut SharedControlBlock) -> Result<(), Fail> {
        // Update SND.NXT.
        self.send_next_seq_no.modify(|s| s + SeqNumber::from(1));

        // Add the probe byte (as a new separate buffer) to our unacknowledged queue.
        let unacked_segment = UnackedSegment {
            bytes: Some(probe.clone()),
            initial_tx: Some(cb.get_now()),
        };
        self.unacked_queue.push(unacked_segment);

        // Note that we loop here *forever*, exponentially backing off.
        // TODO: Use the correct PERSIST mode timer here.
        let mut timeout: Duration = Duration::from_secs(1);
        let mut win_sz_watched: SharedAsyncValue<u32> = self.send_window.clone();
        loop {
            // Create packet.
            let mut header: TcpHeader = cb.tcp_header();
            header.seq_num = self.send_next_seq_no.get();
            cb.emit(header, Some(probe.clone()));

            match win_sz_watched.wait_for_change(Some(timeout)).await {
                Ok(_) => return Ok(()),
                Err(Fail { errno, cause: _ }) if errno == libc::ETIMEDOUT => timeout *= 2,
                Err(_) => unreachable!(
                    "either the ack deadline changed or the deadline passed, no other errors are possible!"
                ),
            }
        }
    }

    // Takes a segment and attempts to send it. The buffer must be non-zero length and the function returns the number
    // of bytes sent.
    fn send_segment(&mut self, segment: &mut DemiBuffer, cb: &mut SharedControlBlock) -> usize {
        let buf_len: usize = segment.len();
        debug_assert_ne!(buf_len, 0);
        // Check window size.
        let max_frame_size_bytes: usize = match self.get_open_window_size_bytes(cb) {
            0 => return 0,
            size => size,
        };

        // Split the packet if necessary.
        // TODO: Use a scatter/gather array to coalesce multiple buffers into a single segment.
        let (frame_size_bytes, do_push): (usize, bool) = {
            if buf_len > max_frame_size_bytes {
                // Suppress PSH flag for partial buffers.
                (max_frame_size_bytes, false)
            } else {
                // We can just send the whole packet. Clone it so we can attach headers/retransmit it later.
                (buf_len, true)
            }
        };
        let segment_data: DemiBuffer = segment
            .split_front(frame_size_bytes)
            .expect("Should be able to split within the length of the buffer");

        let segment_data_len: u32 = segment_data.len() as u32;

        let rto: Duration = self.rto_calculator.rto();
        cb.congestion_control_on_send(rto, (self.send_next_seq_no.get() - self.send_unacked.get()).into());

        // Prepare the segment and send it.
        let mut header: TcpHeader = cb.tcp_header();
        header.seq_num = self.send_next_seq_no.get();
        if do_push {
            header.psh = true;
        }
        cb.emit(header, Some(segment_data.clone()));

        // Update SND.NXT.
        self.send_next_seq_no.modify(|s| s + SeqNumber::from(segment_data_len));

        // Put this segment on the unacknowledged list.
        let unacked_segment = UnackedSegment {
            bytes: Some(segment_data),
            initial_tx: Some(cb.get_now()),
        };
        self.unacked_queue.push(unacked_segment);

        // Set the retransmit timer.
        if self.retransmit_deadline_time_secs.get().is_none() {
            let rto: Duration = self.rto_calculator.rto();
            self.retransmit_deadline_time_secs.set(Some(cb.get_now() + rto));
        }
        segment_data_len as usize
    }

    fn get_open_window_size_bytes(&mut self, cb: &mut SharedControlBlock) -> usize {
        // Calculate amount of data in flight (SND.NXT - SND.UNA).
        let send_unacknowledged: SeqNumber = self.send_unacked.get();
        let send_next: SeqNumber = self.send_next_seq_no.get();
        let sent_data: u32 = (send_next - send_unacknowledged).into();

        // Before we get cwnd for the check, we prompt it to shrink it if the connection has been idle.
        cb.congestion_control_on_cwnd_check_before_send();
        let cwnd: SharedAsyncValue<u32> = cb.congestion_control_get_cwnd();

        // The limited transmit algorithm can increase the effective size of cwnd by up to 2MSS.
        let effective_cwnd: u32 = cwnd.get() + cb.congestion_control_get_limited_transmit_cwnd_increase().get();

        let win_sz: u32 = self.send_window.get();

        if Self::has_open_window(win_sz, sent_data, effective_cwnd) {
            Self::calculate_open_window_bytes(win_sz, sent_data, self.mss, effective_cwnd)
        } else {
            0
        }
    }

    fn has_open_window(win_sz: u32, sent_data: u32, effective_cwnd: u32) -> bool {
        win_sz > 0 && win_sz >= sent_data && effective_cwnd >= sent_data
    }

    fn calculate_open_window_bytes(win_sz: u32, sent_data: u32, mss: usize, effective_cwnd: u32) -> usize {
        cmp::min(
            cmp::min((win_sz - sent_data) as usize, mss),
            (effective_cwnd - sent_data) as usize,
        )
    }

    pub async fn background_retransmitter(&mut self, mut cb: SharedControlBlock) -> Result<Never, Fail> {
        // Watch the retransmission deadline.
        let mut rtx_deadline_watched: SharedAsyncValue<Option<Instant>> = self.retransmit_deadline_time_secs.clone();
        // Watch the fast retransmit flag.
        let mut rtx_fast_retransmit_watched: SharedAsyncValue<bool> = cb.congestion_control_watch_retransmit_now_flag();
        loop {
            let rtx_deadline: Option<Instant> = rtx_deadline_watched.get();
            let rtx_fast_retransmit: bool = rtx_fast_retransmit_watched.get();
            if rtx_fast_retransmit {
                // Notify congestion control about fast retransmit.
                cb.congestion_control_on_fast_retransmit();

                // Retransmit earliest unacknowledged segment.
                self.retransmit(&mut cb);
                continue;
            }

            // If either changed, wake up.
            let something_changed = async {
                select_biased!(
                    _ = rtx_deadline_watched.wait_for_change(None).fuse() => (),
                    _ = rtx_fast_retransmit_watched.wait_for_change(None).fuse() => (),
                )
            };
            pin_mut!(something_changed);
            match conditional_yield_until(something_changed, rtx_deadline).await {
                Ok(()) => match self.fin_seq_no {
                    Some(fin_seq_no) if self.send_unacked.get() > fin_seq_no => {
                        return Err(Fail::new(libc::ECONNRESET, "connection closed"));
                    },
                    _ => continue,
                },
                Err(Fail { errno, cause: _ }) if errno == libc::ETIMEDOUT => {
                    // Retransmit timeout.
                    trace!("retransmit wake");
                    // Notify congestion control about RTO.
                    // TODO: Is this the best place for this?
                    // TODO: Why call into ControlBlock to get SND.UNA when congestion_control_on_rto() has access to it?
                    cb.congestion_control_on_rto(self.send_unacked.get());

                    // RFC 6298 Section 5.4: Retransmit earliest unacknowledged segment.
                    self.retransmit(&mut cb);

                    // RFC 6298 Section 5.5: Back off the retransmission timer.
                    self.rto_calculator.back_off();

                    // RFC 6298 Section 5.6: Restart the retransmission timer with the new RTO.
                    let deadline: Instant = cb.get_now() + self.rto_calculator.rto();
                    self.retransmit_deadline_time_secs.set(Some(deadline));
                },
                Err(_) => {
                    unreachable!(
                        "either the retransmit deadline changed or the deadline passed, no other errors are possible!"
                    )
                },
            }
        }
    }

    /// Retransmits the earliest segment that has not (yet) been acknowledged by our peer.
    pub fn retransmit(&mut self, cb: &mut SharedControlBlock) {
        match self.unacked_queue.get_front_mut() {
            Some(segment) => {
                // We're retransmitting this, so we can no longer use an ACK for it as an RTT measurement (as we can't
                // tell if the ACK is for the original or the retransmission).  Remove the transmission timestamp from
                // the entry.
                segment.initial_tx.take();

                // Clone the segment data for retransmission.
                let data: Option<DemiBuffer> = segment.bytes.as_ref().map(|b| b.clone());

                // TODO: Issue #198 Repacketization - we should send a full MSS (and set the FIN flag if applicable).

                // Prepare and send the segment.
                let mut header: TcpHeader = cb.tcp_header();
                header.seq_num = self.send_unacked.get();
                // If data exists, then this is a regular packet, otherwise, its a FIN.
                if data.is_some() {
                    header.psh = true;
                } else {
                    header.fin = true;
                }
                cb.emit(header, data);
            },
            None => (),
        }
    }

    // Process an ack.
    pub fn process_ack(&mut self, header: &TcpHeader, now: Instant) {
        // Start by checking that the ACK acknowledges something new.
        // TODO: Look into removing Watched types.
        let send_unacknowledged: SeqNumber = self.send_unacked.get();

        if send_unacknowledged < header.ack_num {
            // Remove the now acknowledged data from the unacknowledged queue, update the acked sequence number
            // and update the sender window.

            // Convert the difference in sequence numbers into a u32.
            let bytes_acknowledged: u32 = (header.ack_num - self.send_unacked.get()).into();
            // Convert that into a usize for counting bytes to remove from the unacked queue.
            let mut bytes_remaining: usize = bytes_acknowledged as usize;
            // Remove bytes from the unacked queue.
            while bytes_remaining != 0 {
                bytes_remaining = match self.unacked_queue.try_pop() {
                    Some(segment) if segment.bytes.is_none() => self.process_acked_fin(bytes_remaining, header.ack_num),
                    Some(segment) => self.process_acked_segment(bytes_remaining, segment, now),
                    None => {
                        unreachable!("There should be enough data in the unacked_queue for the number of bytes acked")
                    }, // Shouldn't have bytes_remaining with no segments remaining in unacked_queue.
                };
            }

            // Update SND.UNA to SEG.ACK.
            self.send_unacked.set(header.ack_num);

            // Check and update send window if necessary.
            self.update_send_window(header);

            // Reset the retransmit timer if necessary. If there is more data that hasn't been acked, then set to the
            // next segment deadline, otherwise, do not set.
            let retransmit_deadline_time_secs: Option<Instant> = self.update_retransmit_deadline(now);
            #[cfg(debug_assertions)]
            if retransmit_deadline_time_secs.is_none() {
                debug_assert_eq!(self.send_next_seq_no.get(), header.ack_num);
            }
            self.retransmit_deadline_time_secs.set(retransmit_deadline_time_secs);
        } else {
            // Duplicate ACK (doesn't acknowledge anything new).  We can mostly ignore this, except for fast-retransmit.
            // TODO: Implement fast-retransmit.  In which case, we'd increment our dup-ack counter here.
            warn!("process_ack(): received duplicate ack ({:?})", header.ack_num);
        }
    }

    fn process_acked_fin(&mut self, bytes_remaining: usize, ack_num: SeqNumber) -> usize {
        // This buffer is the end-of-send marker.  So we should only have one byte of acknowledged
        // sequence space remaining (corresponding to our FIN).
        debug_assert_eq!(bytes_remaining, 1);

        // Double check that the ack is for the FIN sequence number.
        debug_assert_eq!(
            ack_num,
            self.fin_seq_no
                .map(|s| { s + 1.into() })
                .expect("should have a FIN set")
        );
        0
    }

    fn process_acked_segment(&mut self, bytes_remaining: usize, mut segment: UnackedSegment, now: Instant) -> usize {
        // Add sample for RTO if we have an initial transmit time.
        // Note that in the case of repacketization, an ack for the first byte is enough for the time sample because it still represents the RTO for that single byte.
        // TODO: TCP timestamp support.
        if let Some(initial_tx) = segment.initial_tx {
            self.rto_calculator.add_sample(now - initial_tx);
        }

        let mut data: DemiBuffer = segment
            .bytes
            .take()
            .expect("there should be data because this is not a FIN.");
        if data.len() > bytes_remaining {
            // Put this segment on the unacknowledged list.
            let unacked_segment = UnackedSegment {
                bytes: Some(
                    data.split_back(bytes_remaining)
                        .expect("Should be able to split back because we just checked the length"),
                ),
                initial_tx: None,
            };
            // Leave this segment on the unacknowledged queue.
            self.unacked_queue.push_front(unacked_segment);
            0
        } else {
            bytes_remaining - data.len()
        }
    }

    fn update_retransmit_deadline(&self, now: Instant) -> Option<Instant> {
        match self.unacked_queue.get_front() {
            Some(UnackedSegment {
                bytes: _,
                initial_tx: Some(initial_tx),
            }) => Some(*initial_tx + self.rto_calculator.rto()),
            Some(UnackedSegment {
                bytes: _,
                initial_tx: None,
            }) => Some(now + self.rto_calculator.rto()),
            None => None,
        }
    }

    fn update_send_window(&mut self, header: &TcpHeader) {
        // Make sure the ack num is bigger than the last one that we used to update the send window.
        if self.send_window_last_update_seq < header.seq_num
            || (self.send_window_last_update_seq == header.seq_num
                && self.send_window_last_update_ack <= header.ack_num)
        {
            self.send_window
                .set((header.window_size as u32) << self.send_window_scale_shift_bits);
            self.send_window_last_update_seq = header.seq_num;
            self.send_window_last_update_ack = header.ack_num;

            debug!(
                "Updating window size -> {} (hdr {}, scale {})",
                self.send_window.get(),
                header.window_size,
                self.send_window_scale_shift_bits,
            );
        }
    }

    // Get SD.UNA.
    pub fn get_unacked_seq_no(&self) -> SeqNumber {
        self.send_unacked.get()
    }

    // Get SND.NXT.
    pub fn get_next_seq_no(&self) -> SeqNumber {
        self.send_next_seq_no.get()
    }

    // Get the current estimate of RTO.
    pub fn get_rto(&self) -> Duration {
        self.rto_calculator.rto()
    }
}
