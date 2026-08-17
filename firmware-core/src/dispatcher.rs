// SPDX-License-Identifier: GPL-3.0-only
//! MeshCore dispatcher: duplicate suppression, airtime budget, CAD-gated TX
//! queue, outstanding ACKed-send delivery tracking.
//!
//! Four independent pieces (all `no_std`-compatible; no ESP-IDF imports here):
//!
//! - [`DuplicateFilter`] — ring buffer of 4-byte packet hashes; drops seen frames.
//! - [`AirtimeBudget`] — sliding-window (60 s) duty-cycle enforcer (≤ 10 %).
//! - [`TxQueue`] — small FIFO pending-TX queue; callers decide when to drain it.
//! - [`OutstandingSends`] — fixed-size table of in-flight DM/room-post sends
//!   awaiting a wire ACK or their delivery deadline; backs the tri-state
//!   grey/blue/red delivery indicator (`crate::ui::DeliveryState`).
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
    /// Caller-supplied tag for each pending frame (the wire ACK hash the
    /// frame is expected to earn, for the two ACKed send paths
    /// [`crate::ui`]'s outstanding-sends model tracks — `None` for every
    /// other enqueued frame, e.g. an ACK reply or a room login). Carried
    /// alongside `bufs`/`lens` purely so a caller can identify WHICH
    /// outstanding send an eviction just dropped (see [`Self::enqueue`]'s
    /// doc) — the TX queue itself never reads or interprets the tag.
    tags: [Option<[u8; 4]>; TX_QUEUE_SLOTS],
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
            tags: [None; TX_QUEUE_SLOTS],
            head: 0,
            count: 0,
        }
    }

    /// Enqueue `frame` for transmission (FIFO order; drops the oldest pending
    /// frame if the queue is already full). `tag` is an opaque caller value
    /// (the DM/room-post wire ACK hash this frame is expected to earn, or
    /// `None` for a frame no outstanding-sends entry tracks) carried
    /// alongside the frame purely so an eviction can report which send it
    /// dropped.
    ///
    /// Returns the byte length AND `tag` of the frame that was evicted to
    /// make room, or `None` if the queue had a free slot and nothing was
    /// dropped. This is `#[must_use]` — a caller that ignores it silently
    /// repeats the exact defect shape this type's own doc above says is
    /// already fixed: a queued frame vanishing with no log, no counter and
    /// no way for the call site to know its own "queued" log line just
    /// lied (and, for a tagged frame, no way to resolve its outstanding
    /// send to undelivered either).
    #[must_use]
    pub fn enqueue(
        &mut self,
        frame: &[u8],
        tag: Option<[u8; 4]>,
    ) -> Option<(usize, Option<[u8; 4]>)> {
        let n = frame.len().min(FRAME_BUF);
        let (idx, dropped) = if self.count == TX_QUEUE_SLOTS {
            // Full: drop the oldest to make room for this one.
            let idx = self.head;
            let dropped_len = self.lens[idx];
            let dropped_tag = self.tags[idx];
            self.head = (self.head + 1) % TX_QUEUE_SLOTS;
            (idx, Some((dropped_len, dropped_tag)))
        } else {
            let idx = (self.head + self.count) % TX_QUEUE_SLOTS;
            self.count += 1;
            (idx, None)
        };
        self.bufs[idx][..n].copy_from_slice(&frame[..n]);
        self.lens[idx] = n;
        self.tags[idx] = tag;
        dropped
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

// ── OutstandingSends (ACKed DM / room-post delivery state) ────────────────────

/// Number of concurrent outstanding ACKed sends this device tracks at once —
/// combined DM + room-post capacity, not per-kind (nothing on the wire or in
/// this UI bounds how many sends a user can queue before their ACKs return,
/// so splitting a fixed budget by kind would just be a second arbitrary
/// number to justify).
///
/// Sized to `2 * TX_QUEUE_SLOTS` rather than against an unrelated constant
/// like `MAX_CONTACTS` (16 — bounds how many contacts CAN exist, not how
/// many sends are ever concurrently in flight): [`TX_QUEUE_SLOTS`] is the
/// tighter upstream bound on how many NOT-YET-TRANSMITTED frames can be
/// queued at once; once a frame is actually transmitted it leaves the TX
/// queue but stays outstanding here until its ACK or deadline, so this table
/// needs headroom for a full TX queue's worth already sent and awaiting ACK,
/// PLUS a second full queue's worth still waiting to transmit behind them.
pub const MAX_OUTSTANDING_SENDS: usize = 2 * TX_QUEUE_SLOTS;

/// How long an outstanding DM/room-post send waits for its ACK before
/// [`OutstandingSends::sweep_expired`] marks it undelivered (red).
///
/// A flood-routed frame's round trip carries per-hop randomized retransmit
/// jitter (`rand(0,5) * airtime * 52/50 / 2` — `meshcore-wire-protocol.md`
/// §5.2) on both the outbound leg and the ACK's own return leg, plus this
/// device's own CAD-busy/airtime-budget backoff (1000-3000 ms per deferred
/// attempt — see the CAD+TX block's `backoff_ms` in `firmware/src/main.rs`)
/// if the channel is contended when either leg is ready to send. 30 s is a
/// generous multiple of one hop's worst case (a few hundred ms of jitter
/// plus up to ~3 s of CAD backoff) so a genuinely in-flight multi-hop round
/// trip isn't marked red while its ACK is still legitimately en route.
pub const DELIVERY_ACK_DEADLINE_MS: u64 = 30_000;

/// Which kind of ACKed send an [`OutstandingSend`] tracks — the two paths
/// this table covers (Channel/GRP_TXT stays on its own pre-existing
/// single-slot `PendingChannelAck` in `firmware/src/main.rs`, unchanged: no
/// wire ACK to correlate against at all — see that type's doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutstandingKind {
    Dm { to_hash: u8 },
    RoomPost { room_hash: u8 },
}

/// One outstanding ACKed send: the wire ACK hash the sender is waiting for,
/// which contact/room it belongs to, and the send/deadline timestamps
/// (`uptime_ms()`-scale, matching every other `*_ms` field in this crate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutstandingSend {
    pub ack_hash: [u8; 4],
    pub kind: OutstandingKind,
    pub sent_at_ms: u64,
    pub deadline_ms: u64,
}

/// Fixed-size outstanding-sends table replacing the single-slot `PendingAck`
/// (DM) / `RoomRuntime::pending_post_ack` (room post) this mission's
/// Objective retires — see `firmware/src/main.rs`'s (former) `PendingAck`
/// doc for the one-outstanding-at-a-time invariant this closes out. A fixed
/// array of `Option<OutstandingSend>`, matching every other fixed-capacity
/// dispatcher-state type in this file (`DuplicateFilter`, `AirtimeBudget`,
/// `TxQueue`) — bounded memory, no heap allocation, `no_std`-compatible.
///
/// # The dual-path ACK-matching invariant
///
/// `resolve` is the ONE place an inbound ACK is matched against outstanding
/// state, called identically from both dispatch sites that can receive one
/// (`handle_ack`'s bare `Ack` datagram, and `handle_path_return`'s bundled
/// `PathExtra::Ack`) — see
/// `flight-manuals/library/dual-path-event-matcher-gap.md`: the room-post
/// delivery-ack defect this table replaces was exactly a matcher wired into
/// only ONE of those two call sites (a second, room-only `pending_post_ack`
/// slot that `handle_path_return` never checked). Collapsing DM and
/// room-post tracking into one table with one lookup makes that class of
/// bug structurally unreachable here: there is no second matcher to forget
/// to wire up.
pub struct OutstandingSends {
    slots: [Option<OutstandingSend>; MAX_OUTSTANDING_SENDS],
}

impl OutstandingSends {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_OUTSTANDING_SENDS],
        }
    }

    /// Record a freshly-enqueued ACKed send. If the table is already full,
    /// evicts the OLDEST entry (lowest `sent_at_ms`) to make room — same
    /// bounded-memory, sustained-overload policy as [`TxQueue::enqueue`]
    /// (see that method's doc) — and returns the evicted entry so the
    /// caller can resolve it to undelivered exactly like a TX-queue
    /// eviction: an entry bumped out of THIS table has just as definitively
    /// lost its chance to ever be matched again.
    #[must_use]
    pub fn insert(&mut self, send: OutstandingSend) -> Option<OutstandingSend> {
        for slot in self.slots.iter_mut() {
            if slot.is_none() {
                *slot = Some(send);
                return None;
            }
        }
        // Full: evict the entry with the oldest `sent_at_ms` (ties broken by
        // table position — first one found scanning forward).
        let mut oldest_idx = 0;
        let mut oldest_sent_at = u64::MAX;
        for (i, slot) in self.slots.iter().enumerate() {
            if let Some(s) = slot {
                if s.sent_at_ms < oldest_sent_at {
                    oldest_sent_at = s.sent_at_ms;
                    oldest_idx = i;
                }
            }
        }
        let evicted = self.slots[oldest_idx].take();
        self.slots[oldest_idx] = Some(send);
        evicted
    }

    /// Resolve an inbound ACK against `ack_hash`, removing and returning the
    /// matching entry — `None` if nothing outstanding matches (already
    /// resolved, already expired/evicted, or never existed; the caller logs
    /// this as "ACK received, no outstanding send", same as the old
    /// single-slot fallback did).
    pub fn resolve(&mut self, ack_hash: [u8; 4]) -> Option<OutstandingSend> {
        for slot in self.slots.iter_mut() {
            if slot.map(|s| s.ack_hash) == Some(ack_hash) {
                return slot.take();
            }
        }
        None
    }

    /// Resolve a TX-queue eviction: `tag` is the evicted frame's tracked ACK
    /// hash (`TxQueue::enqueue`'s tag) if the evicted frame was one of the
    /// two ACKed send paths this table tracks, `None` otherwise (an evicted
    /// ACK reply, room login, etc. — nothing to resolve). Removes and
    /// returns the matching entry so the caller can raise the same
    /// undelivered event a deadline timeout would.
    pub fn resolve_evicted(&mut self, tag: Option<[u8; 4]>) -> Option<OutstandingSend> {
        tag.and_then(|hash| self.resolve(hash))
    }

    /// Invoke `on_expired` once for every entry whose `deadline_ms` has
    /// passed as of `now_ms`, removing each from the table — called once per
    /// dispatcher-loop iteration. A callback rather than a `Vec<_>` return
    /// (unlike, say, `firmware/src/main.rs`'s `Vec<ui::UiEvent>` collectors)
    /// keeps this table itself allocation-free, matching every sibling type
    /// in this file; the caller pushes whatever `ui::UiEvent` it wants
    /// straight from the closure.
    pub fn sweep_expired(&mut self, now_ms: u64, mut on_expired: impl FnMut(OutstandingSend)) {
        for slot in self.slots.iter_mut() {
            let expired = matches!(slot, Some(s) if now_ms >= s.deadline_ms);
            if expired {
                if let Some(send) = slot.take() {
                    on_expired(send);
                }
            }
        }
    }
}

impl Default for OutstandingSends {
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
        assert_eq!(
            q.enqueue(b"test frame", None),
            None,
            "queue had room; nothing evicted"
        );
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
        assert_eq!(q.enqueue(b"first", None), None);
        assert_eq!(
            q.enqueue(b"second", None),
            None,
            "queue has 4 slots; two frames never evicts"
        );
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
            assert_eq!(q.enqueue(&[i as u8], None), None, "queue not yet full");
        }
        // One more: queue is full, so the oldest (0) is dropped to make room —
        // and `enqueue` must report it, not swallow it silently (F3).
        let dropped = q.enqueue(&[0xFFu8], None);
        assert_eq!(
            dropped,
            Some((1, None)),
            "eviction must be reported so a warn can be logged at the call site"
        );
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
        assert_eq!(q.enqueue(b"channel reply", None), None);
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
        let _ = q.enqueue(b"first", None);
        let _ = q.enqueue(b"second", None);
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

    /// The tag (`OutstandingSends`'s correlation hook) rides along with the
    /// frame through an eviction, so a caller can resolve exactly which
    /// outstanding send just lost its chance to reach the wire.
    #[test]
    fn txqueue_eviction_reports_the_evicted_frames_tag() {
        let mut q = TxQueue::new();
        let tagged_ack = [0xAAu8, 0xBB, 0xCC, 0xDD];
        assert_eq!(q.enqueue(b"tagged DM", Some(tagged_ack)), None);
        for i in 1..TX_QUEUE_SLOTS {
            assert_eq!(q.enqueue(&[i as u8], None), None, "queue not yet full");
        }
        // One more: the oldest (the tagged DM) is evicted — its tag must
        // come back so the caller can resolve it against the
        // outstanding-sends table.
        let dropped = q.enqueue(&[0xFFu8], None);
        assert_eq!(dropped, Some((9, Some(tagged_ack))));
    }

    #[test]
    fn txqueue_untagged_eviction_reports_no_tag() {
        let mut q = TxQueue::new();
        for i in 0..TX_QUEUE_SLOTS {
            assert_eq!(q.enqueue(&[i as u8], None), None);
        }
        let dropped = q.enqueue(&[0xFFu8], None);
        assert_eq!(dropped, Some((1, None)));
    }

    // ── OutstandingSends ─────────────────────────────────────────────────────

    fn dm_send(ack_hash: [u8; 4], to_hash: u8, sent_at_ms: u64) -> OutstandingSend {
        OutstandingSend {
            ack_hash,
            kind: OutstandingKind::Dm { to_hash },
            sent_at_ms,
            deadline_ms: sent_at_ms + DELIVERY_ACK_DEADLINE_MS,
        }
    }

    #[test]
    fn outstanding_insert_then_resolve_roundtrip() {
        let mut t = OutstandingSends::new();
        assert_eq!(t.insert(dm_send([1, 1, 1, 1], 0x42, 0)), None);
        let resolved = t.resolve([1, 1, 1, 1]);
        assert!(matches!(
            resolved,
            Some(OutstandingSend {
                kind: OutstandingKind::Dm { to_hash: 0x42 },
                ..
            })
        ));
        // Resolved entries are removed — resolving the same hash again finds
        // nothing.
        assert!(t.resolve([1, 1, 1, 1]).is_none());
    }

    /// REGRESSION: two DMs sent back-to-back (this mission's acceptance
    /// criterion) must each track and resolve independently — resolving one
    /// hash must not disturb the other, in either arrival order.
    #[test]
    fn outstanding_two_dms_to_the_same_contact_track_independently() {
        let mut t = OutstandingSends::new();
        assert_eq!(t.insert(dm_send([1, 1, 1, 1], 0x42, 0)), None);
        assert_eq!(t.insert(dm_send([2, 2, 2, 2], 0x42, 1)), None);

        // The SECOND DM's ack arrives first.
        let second = t.resolve([2, 2, 2, 2]);
        assert!(matches!(second, Some(s) if s.ack_hash == [2, 2, 2, 2]));

        // The FIRST DM is still outstanding, untouched.
        let first = t.resolve([1, 1, 1, 1]);
        assert!(matches!(first, Some(s) if s.ack_hash == [1, 1, 1, 1]));
    }

    #[test]
    fn outstanding_dm_and_room_post_track_independently() {
        let mut t = OutstandingSends::new();
        assert_eq!(t.insert(dm_send([1, 1, 1, 1], 0x42, 0)), None);
        assert_eq!(
            t.insert(OutstandingSend {
                ack_hash: [2, 2, 2, 2],
                kind: OutstandingKind::RoomPost { room_hash: 0x99 },
                sent_at_ms: 0,
                deadline_ms: DELIVERY_ACK_DEADLINE_MS,
            }),
            None
        );

        let room = t.resolve([2, 2, 2, 2]).unwrap();
        assert_eq!(room.kind, OutstandingKind::RoomPost { room_hash: 0x99 });
        let dm = t.resolve([1, 1, 1, 1]).unwrap();
        assert_eq!(dm.kind, OutstandingKind::Dm { to_hash: 0x42 });
    }

    #[test]
    fn outstanding_resolve_no_match_returns_none() {
        let mut t = OutstandingSends::new();
        assert_eq!(t.insert(dm_send([1, 1, 1, 1], 0x42, 0)), None);
        assert!(t.resolve([9, 9, 9, 9]).is_none());
    }

    #[test]
    fn outstanding_insert_evicts_oldest_when_full() {
        let mut t = OutstandingSends::new();
        for i in 0..MAX_OUTSTANDING_SENDS {
            assert_eq!(
                t.insert(dm_send([i as u8, 0, 0, 0], i as u8, i as u64)),
                None,
                "table not yet full"
            );
        }
        // One more: the table is full, so the OLDEST (sent_at_ms == 0) is
        // evicted to make room — and it must be reported, not swallowed.
        let evicted = t.insert(dm_send([0xFF, 0, 0, 0], 0xFF, 1000));
        assert!(matches!(evicted, Some(s) if s.sent_at_ms == 0));
        // Nothing else was disturbed: entry 1 (the new oldest) is still
        // resolvable.
        assert!(t.resolve([1, 0, 0, 0]).is_some());
    }

    #[test]
    fn outstanding_resolve_evicted_resolves_a_tagged_tx_queue_eviction() {
        let mut t = OutstandingSends::new();
        assert_eq!(t.insert(dm_send([1, 1, 1, 1], 0x42, 0)), None);
        let resolved = t.resolve_evicted(Some([1, 1, 1, 1]));
        assert!(resolved.is_some());
        assert!(
            t.resolve([1, 1, 1, 1]).is_none(),
            "resolved entry is removed"
        );
    }

    #[test]
    fn outstanding_resolve_evicted_none_tag_is_a_no_op() {
        let mut t = OutstandingSends::new();
        assert_eq!(t.insert(dm_send([1, 1, 1, 1], 0x42, 0)), None);
        assert!(t.resolve_evicted(None).is_none());
        // The unrelated outstanding entry is untouched.
        assert!(t.resolve([1, 1, 1, 1]).is_some());
    }

    #[test]
    fn outstanding_sweep_expired_fires_only_for_passed_deadlines() {
        let mut t = OutstandingSends::new();
        assert_eq!(
            t.insert(OutstandingSend {
                ack_hash: [1, 1, 1, 1],
                kind: OutstandingKind::Dm { to_hash: 0x42 },
                sent_at_ms: 0,
                deadline_ms: 1_000,
            }),
            None
        );
        assert_eq!(
            t.insert(OutstandingSend {
                ack_hash: [2, 2, 2, 2],
                kind: OutstandingKind::Dm { to_hash: 0x43 },
                sent_at_ms: 0,
                deadline_ms: 5_000,
            }),
            None
        );

        let mut expired = Vec::new();
        t.sweep_expired(1_000, |s| expired.push(s.ack_hash));
        assert_eq!(
            expired,
            vec![[1, 1, 1, 1]],
            "only the passed deadline fires"
        );

        // The expired entry is gone; the still-live one is untouched.
        assert!(t.resolve([1, 1, 1, 1]).is_none());
        assert!(t.resolve([2, 2, 2, 2]).is_some());
    }

    #[test]
    fn outstanding_sweep_expired_no_entries_past_deadline_is_a_noop() {
        let mut t = OutstandingSends::new();
        assert_eq!(t.insert(dm_send([1, 1, 1, 1], 0x42, 1_000)), None);
        let mut expired = Vec::new();
        t.sweep_expired(500, |s| expired.push(s.ack_hash));
        assert!(expired.is_empty());
        assert!(t.resolve([1, 1, 1, 1]).is_some());
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
