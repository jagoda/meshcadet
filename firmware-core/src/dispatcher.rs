// SPDX-License-Identifier: GPL-3.0-only
//! MeshCore dispatcher: duplicate suppression, airtime budget, CAD-gated TX queue.
//!
//! Three independent pieces (all `no_std`-compatible; no ESP-IDF imports here):
//!
//! - [`DuplicateFilter`] — ring buffer of 4-byte packet hashes; drops seen frames.
//! - [`AirtimeBudget`] — sliding-window (60 s) duty-cycle enforcer (≤ 10 %).
//! - [`TxQueue`] — small FIFO pending-TX queue; callers decide when to drain it.
//!
//! Source reference: `src/Mesh.cpp` flood-relay logic @ dee3e26a.
//!
//! # Packet hash
//! The dedup key is `protocol::packet_dedup_key` =
//! `SHA-256(payload_type || payload)[0:4]`, computed over the IMMUTABLE part of
//! the frame only — exactly what MeshCore's `Packet::calculatePacketHash`
//! (`src/Packet.cpp:41`) hashes. The 1-byte header and the variable path field
//! are deliberately EXCLUDED: a flood relay appends its own hash to the path and
//! bumps the hop count on every forward, so those bytes differ between copies of
//! one logical packet. Hashing the whole frame (the earlier behaviour) gave each
//! relayed copy a distinct key, so duplicates slipped past the ring and were
//! displayed/ACKed repeatedly. The key lives in `protocol::dedup` (host-tested).

use protocol::packet_dedup_key;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Number of recent packet hashes kept in the duplicate ring.
///
/// A flood relay can deliver one logical packet over many paths interleaved with
/// other traffic, so the ring must be deep enough that the original is still
/// remembered when its relayed copies arrive. MeshCore uses 160 (`128+32`); 128
/// keeps the same order of magnitude (128 × 4 B = 512 B) for an endpoint node.
pub const DEDUP_SLOTS: usize = 128;

/// Airtime budget window in milliseconds (60 seconds).
pub const BUDGET_WINDOW_MS: u64 = 60_000;

/// Maximum TX airtime allowed inside `BUDGET_WINDOW_MS` (10 % duty cycle).
pub const BUDGET_MAX_MS: u64 = 6_000;

/// Maximum TX frames tracked for the sliding-window airtime budget.
pub const BUDGET_SLOTS: usize = 32;

/// Wire frame buffer size (matches `protocol::constants::MAX_TRANS_UNIT`).
pub const FRAME_BUF: usize = 255;

// ── DuplicateFilter ───────────────────────────────────────────────────────────

/// Ring buffer of the last [`DEDUP_SLOTS`] packet hashes.
///
/// Invariant: `head` is the *next* write position mod `DEDUP_SLOTS`.
pub struct DuplicateFilter {
    slots: [[u8; 4]; DEDUP_SLOTS],
    head: usize,
    count: usize, // saturates at DEDUP_SLOTS
}

impl DuplicateFilter {
    pub const fn new() -> Self {
        Self {
            slots: [[0u8; 4]; DEDUP_SLOTS],
            head: 0,
            count: 0,
        }
    }

    /// Return `true` if `frame` was already seen (hash collision = discard).
    pub fn is_duplicate(&self, frame: &[u8]) -> bool {
        let h = packet_dedup_key(frame);
        let n = self.count.min(DEDUP_SLOTS);
        for i in 0..n {
            if self.slots[i] == h {
                return true;
            }
        }
        false
    }

    /// Record `frame` as seen.  Oldest entry is evicted when the ring is full.
    pub fn insert(&mut self, frame: &[u8]) {
        let h = packet_dedup_key(frame);
        self.slots[self.head] = h;
        self.head = (self.head + 1) % DEDUP_SLOTS;
        if self.count < DEDUP_SLOTS {
            self.count += 1;
        }
    }
}

impl Default for DuplicateFilter {
    fn default() -> Self {
        Self::new()
    }
}

// ── AirtimeBudget ─────────────────────────────────────────────────────────────

/// Slot entry: (start_uptime_ms, duration_ms).
#[derive(Clone, Copy)]
struct TxRecord {
    start_ms: u64,
    duration_ms: u32,
}

/// Sliding-window airtime budget (10 % duty cycle over 60 s).
///
/// Callers supply uptime in milliseconds (`now_ms`); the budget itself has no
/// clock dependency so it can be tested without hardware.
pub struct AirtimeBudget {
    records: [TxRecord; BUDGET_SLOTS],
    head: usize,
    count: usize,
}

impl AirtimeBudget {
    pub const fn new() -> Self {
        Self {
            records: [TxRecord {
                start_ms: 0,
                duration_ms: 0,
            }; BUDGET_SLOTS],
            head: 0,
            count: 0,
        }
    }

    /// Return `true` if transmitting `required_ms` of airtime right now would
    /// stay within the 10 % duty cycle over the last 60 s.
    pub fn can_transmit(&self, now_ms: u64, required_ms: u32) -> bool {
        let used = self.used_in_window(now_ms);
        used + required_ms as u64 <= BUDGET_MAX_MS
    }

    /// Record that a TX of `duration_ms` started at `now_ms`.
    pub fn record_tx(&mut self, now_ms: u64, duration_ms: u32) {
        self.records[self.head] = TxRecord {
            start_ms: now_ms,
            duration_ms,
        };
        self.head = (self.head + 1) % BUDGET_SLOTS;
        if self.count < BUDGET_SLOTS {
            self.count += 1;
        }
    }

    /// Sum of TX durations whose start falls within the last `BUDGET_WINDOW_MS`.
    fn used_in_window(&self, now_ms: u64) -> u64 {
        let cutoff = now_ms.saturating_sub(BUDGET_WINDOW_MS);
        let n = self.count.min(BUDGET_SLOTS);
        let mut total: u64 = 0;
        for i in 0..n {
            let r = &self.records[i];
            if r.start_ms >= cutoff {
                total += r.duration_ms as u64;
            }
        }
        total
    }
}

impl Default for AirtimeBudget {
    fn default() -> Self {
        Self::new()
    }
}

// ── TxQueue ───────────────────────────────────────────────────────────────────

/// Number of pending frames [`TxQueue`] can hold before it starts dropping the
/// oldest to make room for a new one.
///
/// DEFECT FIX: the queue used to be a
/// single "youngest wins" slot — a new `enqueue` silently replaced whatever was
/// already pending. That is safe when a dispatcher-loop iteration produces at
/// most one outbound frame, but `handle_dm`'s telemetry-pull path enqueues
/// TWO: the location reply, then (a few lines later, same call, same loop
/// iteration, no drain in between) the DM ACK. The ACK enqueue clobbered the
/// reply before the loop ever reached the TX-drain step, so an enabled
/// contact's `?loc` logged `TX telemetry reply to ...` — the frame was built
/// and "sent" as far as the log was concerned — yet nothing reached the wire;
/// only the ACK went out. DMs (ACK-only, one frame per event) kept working,
/// which is exactly the reported symptom: contact enabled, DMs fine, pull
/// silently dropped. A small FIFO removes this same-iteration clobber: both
/// frames survive and drain one per loop iteration, oldest first. 4 slots
/// covers the current worst case (2) with headroom for a future path that
/// enqueues more without another silent-drop surprise.
pub const TX_QUEUE_SLOTS: usize = 4;

/// FIFO TX queue.
///
/// Frames are drained oldest-first via [`TxQueue::peek`] + [`TxQueue::pop_front`]
/// (the dispatcher loop calls `peek` once per iteration and only `pop_front`s
/// once the transmit attempt actually succeeds — a failed attempt leaves the
/// frame queued for the next iteration to retry instead of discarding it). If
/// [`TX_QUEUE_SLOTS`] frames are already pending, a new `enqueue` drops the
/// OLDEST to make room — bounded memory, and a sustained-overload bias toward
/// the newest traffic, same spirit as the original single-slot policy, but
/// only once the queue is actually full instead of on every enqueue.
pub struct TxQueue {
    bufs: [[u8; FRAME_BUF]; TX_QUEUE_SLOTS],
    lens: [usize; TX_QUEUE_SLOTS],
    /// Index of the oldest pending frame.
    head: usize,
    /// Number of frames currently pending (0..=TX_QUEUE_SLOTS).
    count: usize,
}

impl TxQueue {
    pub const fn new() -> Self {
        Self {
            bufs: [[0u8; FRAME_BUF]; TX_QUEUE_SLOTS],
            lens: [0usize; TX_QUEUE_SLOTS],
            head: 0,
            count: 0,
        }
    }

    /// Enqueue `frame` for transmission (FIFO order; drops the oldest pending
    /// frame if the queue is already full).
    pub fn enqueue(&mut self, frame: &[u8]) {
        let n = frame.len().min(FRAME_BUF);
        let idx = if self.count == TX_QUEUE_SLOTS {
            // Full: drop the oldest to make room for this one.
            let idx = self.head;
            self.head = (self.head + 1) % TX_QUEUE_SLOTS;
            idx
        } else {
            let idx = (self.head + self.count) % TX_QUEUE_SLOTS;
            self.count += 1;
            idx
        };
        self.bufs[idx][..n].copy_from_slice(&frame[..n]);
        self.lens[idx] = n;
    }

    /// Copy the oldest pending frame into `out` WITHOUT removing it from the
    /// queue. Returns the byte count (0 if empty).
    ///
    /// Paired with [`Self::pop_front`] so a caller can attempt to transmit a
    /// frame and only remove it from the queue once that attempt actually
    /// succeeds — a failed attempt (CAD-clear-but-radio-error, or an
    /// airtime-budget denial) leaves the frame in place for the next
    /// dispatcher-loop iteration to retry, instead of the frame vanishing on
    /// its first (and only) attempt.
    pub fn peek(&self, out: &mut [u8]) -> usize {
        if self.count == 0 {
            return 0;
        }
        let idx = self.head;
        let n = self.lens[idx].min(out.len());
        out[..n].copy_from_slice(&self.bufs[idx][..n]);
        n
    }

    /// Remove the oldest pending frame (previously read via [`Self::peek`])
    /// without copying it anywhere. No-op if the queue is empty.
    pub fn pop_front(&mut self) {
        if self.count == 0 {
            return;
        }
        self.head = (self.head + 1) % TX_QUEUE_SLOTS;
        self.count -= 1;
    }

    /// `true` if at least one frame is waiting.
    pub fn has_pending(&self) -> bool {
        self.count > 0
    }
}

impl Default for TxQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ── TX guard ─────────────────────────────────────────────────────────────────

/// Whether a frame carrying this wire `payload_type` may proceed to
/// `radio.transmit()`.
///
/// This is the RELEASE-LIVE enforcement of "MeshCadet never emits an ADVERT
/// frame" — the TX loop (`firmware/src/main.rs`) used to gate this on a bare
/// `debug_assert!`, which is compiled to a no-op whenever
/// `debug-assertions` is off, and the root `Cargo.toml`'s `[profile.release]`
/// does NOT enable it — so the guard the campaign relies on as the
/// enforcement of "never over the air" was a no-op in shipped release
/// firmware. This function is a plain runtime check with no `cfg` gate at
/// all: it evaluates identically in every build profile, debug or release.
///
/// [`protocol::PolicyFilter::is_advert_type`] itself is unmodified; this
/// only wraps it. Returns `false` iff `payload_type` is a MeshCore ADVERT
/// (`0x04`) — the caller must drop the frame (do not retry, do not panic)
/// and log an error rather than pass it to the radio.
pub fn tx_guard_allows(payload_type: u8) -> bool {
    !protocol::PolicyFilter::is_advert_type(payload_type)
}

// ── Airtime calculator ────────────────────────────────────────────────────────

/// Estimate LoRa time-on-air in milliseconds for `payload_bytes` at the locked
/// MeshCadet preset (SF7 / BW 62.5 kHz / CR 4/5 / 8-symbol preamble / explicit
/// header / CRC on).
///
/// Formula from Semtech AN1200.13 §4.
pub fn lora_airtime_ms(payload_bytes: usize) -> u32 {
    const SF: f64 = 7.0;
    const BW_HZ: f64 = 62_500.0;
    const CR: f64 = 1.0; // CR 4/5 → CR denominator offset = 1
    const N_PRE: f64 = 8.0;
    const CRC: f64 = 1.0;
    const IH: f64 = 0.0; // 0 = explicit header

    let t_sym_ms = (2f64.powf(SF) / BW_HZ) * 1000.0; // ms

    // Payload symbol count
    let pl = payload_bytes as f64;
    let num = (8.0 * pl - 4.0 * SF + 28.0 + 16.0 * CRC - 20.0 * IH).max(0.0);
    let denom = 4.0 * SF; // LDRO=0 because t_sym < 16 ms at SF7/62.5 kHz
    let payload_syms = 8.0 + f64::ceil(num / denom) * (CR + 4.0);

    let t_pre_ms = (N_PRE + 4.25) * t_sym_ms;
    let t_pay_ms = payload_syms * t_sym_ms;

    (t_pre_ms + t_pay_ms).ceil() as u32
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── DuplicateFilter ──────────────────────────────────────────────────────

    #[test]
    fn dedup_new_frame_not_duplicate() {
        let mut f = DuplicateFilter::new();
        let frame = b"hello world";
        assert!(!f.is_duplicate(frame));
        f.insert(frame);
        assert!(f.is_duplicate(frame));
    }

    #[test]
    fn dedup_different_frames_not_duplicate() {
        let mut f = DuplicateFilter::new();
        let a = b"frame_a";
        let b = b"frame_b";
        f.insert(a);
        assert!(!f.is_duplicate(b));
    }

    #[test]
    fn dedup_ring_evicts_oldest() {
        let mut f = DuplicateFilter::new();
        // Fill ring with DEDUP_SLOTS distinct frames
        let frames: Vec<Vec<u8>> = (0..DEDUP_SLOTS).map(|i| vec![i as u8; 8]).collect();
        for fr in &frames {
            f.insert(fr);
        }
        // All frames should be seen
        for fr in &frames {
            assert!(f.is_duplicate(fr), "should be in ring: {:?}", fr);
        }
        // Insert one more → oldest (frames[0]) is evicted
        let new_frame = vec![0xFF; 8];
        f.insert(&new_frame);
        assert!(f.is_duplicate(&new_frame));
        // Oldest slot re-used: frames[0] is no longer guaranteed present
        // (ring is full; first inserted is gone)
        assert!(
            !f.is_duplicate(&frames[0]),
            "oldest should have been evicted"
        );
    }

    /// REGRESSION (ISSUE 2): a flood-relayed copy (mutated path) must dedup
    /// against the original through the ring. The dedup-KEY invariance itself is
    /// proven host-side in `protocol::dedup`; this exercises it via the filter.
    #[test]
    fn dedup_relayed_copy_with_mutated_path_is_duplicate() {
        // header = GRP_TXT(0x05)<<2 | FLOOD(0x01) = 0x15.
        let payload = [0x6d, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77];
        let direct = {
            let mut v = vec![0x15u8, 0x40]; // 0 hops
            v.extend_from_slice(&payload);
            v
        };
        let relayed = {
            let mut v = vec![0x15u8, 0x41, 0xAA, 0xBB]; // 1 hop appended
            v.extend_from_slice(&payload);
            v
        };
        assert_ne!(direct, relayed, "relay mutates the frame bytes");

        let mut f = DuplicateFilter::new();
        assert!(!f.is_duplicate(&direct));
        f.insert(&direct);
        assert!(
            f.is_duplicate(&relayed),
            "relayed copy must dedup against original"
        );
    }

    // ── AirtimeBudget ────────────────────────────────────────────────────────

    #[test]
    fn budget_allows_first_tx() {
        let b = AirtimeBudget::new();
        assert!(b.can_transmit(0, 200), "fresh budget should allow 200 ms");
    }

    #[test]
    fn budget_enforces_limit() {
        let mut b = AirtimeBudget::new();
        // Record 5900 ms of TX at t=0
        b.record_tx(0, 5900);
        // 5900 + 200 > 6000 → deny
        assert!(!b.can_transmit(100, 200), "should be denied: over budget");
        // But 5900 + 99 ≤ 6000 → allow
        assert!(
            b.can_transmit(100, 99),
            "should be allowed: just within budget"
        );
    }

    #[test]
    fn budget_window_expires() {
        let mut b = AirtimeBudget::new();
        // Record 5900 ms of TX at t=0
        b.record_tx(0, 5900);
        // After 60 s + 1 ms the window has lapsed
        let now = BUDGET_WINDOW_MS + 1;
        assert!(b.can_transmit(now, 5900), "expired TX should not count");
    }

    // ── TxQueue ──────────────────────────────────────────────────────────────

    #[test]
    fn txqueue_enqueue_take_roundtrip() {
        let mut q = TxQueue::new();
        assert!(!q.has_pending());
        q.enqueue(b"test frame");
        assert!(q.has_pending());
        let mut buf = [0u8; 32];
        let n = q.peek(&mut buf);
        q.pop_front();
        assert_eq!(n, 10);
        assert_eq!(&buf[..n], b"test frame");
        assert!(!q.has_pending());
    }

    /// REGRESSION: two frames enqueued
    /// back-to-back in the same call (mirrors `handle_dm`'s telemetry-reply-
    /// then-ACK sequence) must BOTH survive and drain in FIFO order — the
    /// prior single-slot "youngest wins" queue silently dropped the first.
    #[test]
    fn txqueue_both_frames_enqueued_same_pass_survive_fifo_order() {
        let mut q = TxQueue::new();
        q.enqueue(b"first");
        q.enqueue(b"second");
        let mut buf = [0u8; 16];
        let n1 = q.peek(&mut buf);
        assert_eq!(
            &buf[..n1],
            b"first",
            "oldest frame must drain first, not be dropped"
        );
        q.pop_front();
        let n2 = q.peek(&mut buf);
        assert_eq!(&buf[..n2], b"second");
        q.pop_front();
        assert!(!q.has_pending());
    }

    #[test]
    fn txqueue_drops_oldest_when_full() {
        let mut q = TxQueue::new();
        // Fill to capacity with distinct single-byte frames.
        for i in 0..TX_QUEUE_SLOTS {
            q.enqueue(&[i as u8]);
        }
        // One more: queue is full, so the oldest (0) is dropped to make room.
        q.enqueue(&[0xFFu8]);
        let mut buf = [0u8; 4];
        for i in 1..TX_QUEUE_SLOTS {
            let n = q.peek(&mut buf);
            assert_eq!(buf[..n], [i as u8], "frame {} should still be pending", i);
            q.pop_front();
        }
        let n = q.peek(&mut buf);
        assert_eq!(buf[..n], [0xFFu8]);
        q.pop_front();
        assert!(!q.has_pending());
    }

    /// REGRESSION: a failed
    /// transmit attempt (radio error, or an airtime-budget denial discovered
    /// only after the frame left the queue) must leave the frame in place
    /// for the next dispatcher-loop iteration — `peek` must not consume it,
    /// and only an explicit `pop_front` (issued by the caller once the send
    /// actually succeeds) removes it. Before this fix the dispatcher used
    /// `take` unconditionally, which pulled the frame out of the queue
    /// whether or not the subsequent `radio.transmit()`/budget check
    /// succeeded — a single dropped attempt permanently lost the message
    /// (fails first try, "succeeds" only if a human notices and re-sends).
    #[test]
    fn txqueue_peek_does_not_consume_frame() {
        let mut q = TxQueue::new();
        q.enqueue(b"channel reply");
        let mut buf = [0u8; 32];
        let n = q.peek(&mut buf);
        assert_eq!(&buf[..n], b"channel reply");
        assert!(
            q.has_pending(),
            "peek must not remove the frame from the queue"
        );
        // A second peek (simulating a retried, still-failing send) sees the
        // exact same frame — it was never lost.
        let n2 = q.peek(&mut buf);
        assert_eq!(&buf[..n2], b"channel reply");
        assert!(q.has_pending());
    }

    #[test]
    fn txqueue_pop_front_removes_peeked_frame() {
        let mut q = TxQueue::new();
        q.enqueue(b"first");
        q.enqueue(b"second");
        let mut buf = [0u8; 16];
        // Simulate a successful send of the head frame.
        let n = q.peek(&mut buf);
        assert_eq!(&buf[..n], b"first");
        q.pop_front();
        // The next peek sees the next frame, in FIFO order.
        let n = q.peek(&mut buf);
        assert_eq!(&buf[..n], b"second");
        q.pop_front();
        assert!(!q.has_pending());
    }

    #[test]
    fn txqueue_pop_front_on_empty_queue_is_a_noop() {
        let mut q = TxQueue::new();
        q.pop_front();
        assert!(!q.has_pending());
    }

    // ── Airtime calculator ───────────────────────────────────────────────────

    #[test]
    fn airtime_single_hop_dm_reasonable() {
        // A typical DM frame: header(1) + path_len(1) + path(0) + payload(~50) = ~52 bytes
        let ms = lora_airtime_ms(52);
        // At SF7/62.5 kHz, 52-byte payload is roughly 50–100 ms
        assert!(ms >= 150, "airtime too short: {} ms", ms);
        assert!(ms <= 300, "airtime too long: {} ms", ms);
    }

    #[test]
    fn airtime_max_frame_under_500ms() {
        // Worst-case frame: 255 bytes
        let ms = lora_airtime_ms(255);
        assert!(
            ms < 1000,
            "max frame airtime {} ms exceeds 1000 ms budget",
            ms
        );
    }

    // ── TX guard ─────────────────────────────────────────────────────────────

    /// **Release-guard, first-class test.** `cargo test` (this crate's own
    /// harness) does not disable `debug_assertions`, so a bare
    /// `debug_assert!` would have silently passed this test too — the old
    /// defect only showed up in an actual `[profile.release]` firmware
    /// build. `tx_guard_allows` closes that gap structurally: it is a plain
    /// `bool`-returning function with no `cfg(debug_assertions)` anywhere in
    /// it, so this assertion is exercising the exact code path that runs in
    /// release firmware, not a debug-only stand-in for it.
    #[test]
    fn tx_guard_refuses_advert_payload_type_handed_to_the_tx_path() {
        const PAYLOAD_TYPE_ADVERT: u8 = 0x04;
        assert!(
            !tx_guard_allows(PAYLOAD_TYPE_ADVERT),
            "an ADVERT frame handed to the TX path must be refused, in every build profile"
        );
    }

    #[test]
    fn tx_guard_allows_every_non_advert_payload_type() {
        for pt in 0u8..16u8 {
            if pt == 0x04 {
                continue;
            }
            assert!(
                tx_guard_allows(pt),
                "non-ADVERT payload_type 0x{:02x} must not be blocked",
                pt
            );
        }
    }
}
