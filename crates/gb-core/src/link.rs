//! The networked link-cable protocol.
//!
//! [`LocalLink`](crate::LocalLink) wires two cores together through shared
//! memory, so a master reading "the peer's current SB" is always reading a value
//! the peer really presents *right now*. Over a network that is not true: the
//! peer's announcement is in flight, or already superseded, and the master has
//! no way to tell. See [`LinkProto`] for the wire format that fixes this.
//!
//! This module is deliberately pure — no sockets, no callbacks, no clock. It
//! consumes inbound packets and produces outbound ones through queues, so both
//! ends can be driven against each other in a unit test. Every link bug we have
//! shipped so far was reproducible on a desk; none of them needed two phones.

use std::collections::VecDeque;

/// Wire-format version. v1 had no pairing; v2 added it; v3 carries the
/// responder's backlog in the reply; v4 makes an exchange idempotent so a lost
/// datagram can simply be re-sent — required for LAN, where nothing beneath the
/// protocol retransmits.
pub const PROTOCOL_VERSION: &str = "pocketrust-link-4";

/// `[0, b]` — "my SB is now `b`". Kept from v1: a slave must announce the byte
/// it presents, because the peer's transport answers clocks on its behalf.
pub const TAG_OUTPUT: u8 = 0;
/// `[2, seq, b]` — "I am clocking `b` into you as exchange `seq`."
pub const TAG_CLOCK: u8 = 2;
/// `[3, seq, b, lag]` — "for exchange `seq` I was presenting `b`, and my game is
/// `lag` bytes behind on picking up what you have clocked into me."
///
/// `lag` is the whole point of v3. A responder answers a clock from its
/// *transport*, immediately, so byte counts alone can never show that its
/// **game** has fallen behind — the bytes are being accepted and answered on
/// time while the ROM sits at a menu and never reads them. `lag` is the one
/// fact only the responder knows, so it has to be said out loud.
pub const TAG_REPLY: u8 = 3;

/// `[4, seq, 0]` — "I waited out the clock on exchange `seq` and gave up."
///
/// Costs one packet on a path that is already broken, and buys two things: the
/// bridge can count give-ups without the core needing a logging channel, and the
/// peer finds out it was the one not answering. Those are the two halves of
/// telling each player who is holding up the cable.
pub const TAG_GAVE_UP: u8 = 4;

/// How many recently-answered sequence numbers to remember for duplicate
/// suppression. Only one exchange is ever outstanding, so anything beyond a
/// couple is slack; eight costs nothing and covers a run of lost clocks.
const SEEN_WINDOW: usize = 8;

/// Backlog reported as healthy. The byte being answered right now is always in
/// the queue, so a game keeping up reports 0.
pub const LAG_HEALTHY: u8 = 0;

/// The link-cable protocol state machine.
///
/// Outbound packets accumulate in an outbox the transport drains; inbound ones
/// are fed to [`LinkProto::on_packet`]. The master path is *paired*: each clock
/// carries a sequence number and the master consumes only the reply that
/// carries the same one, so a stale or echoed announcement can never be
/// mistaken for the peer's pre-transfer SB.
pub struct LinkProto {
    /// The byte this side currently presents on the wire (its SB).
    own_output: u8,
    /// Next sequence number for a master exchange. Wraps; only equality is used.
    next_seq: u8,
    /// The exchange we are waiting on a reply for, if any.
    awaiting: Option<u8>,
    /// The byte being clocked in `awaiting`, kept so a lost clock can be re-sent.
    pending_out: u8,
    /// Sequence numbers we have already answered, so a retransmitted clock is
    /// idempotent. A ring rather than a single slot: a clock can be lost
    /// outright, which advances the master's counter past a value we never saw.
    seen: [Option<u8>; SEEN_WINDOW],
    seen_pos: usize,
    /// The exact reply we sent for the most recent new clock, replayed verbatim
    /// if that clock arrives again.
    last_reply: [u8; 4],
    /// The reply that matched `awaiting`.
    reply: Option<u8>,
    /// Bytes a master clocked into us, awaiting pickup by the serial engine.
    /// Its depth *is* our lag: bytes accepted that our game has not read.
    slave_input: VecDeque<u8>,
    /// The backlog the peer reported in its most recent reply.
    peer_lag: u8,
    /// The peer told us it timed out waiting on one of our replies.
    peer_gave_up: bool,
    /// Packets for the transport to send.
    outbox: VecDeque<[u8; 4]>,
}

impl Default for LinkProto {
    fn default() -> Self {
        Self::new()
    }
}

impl LinkProto {
    pub const fn new() -> LinkProto {
        LinkProto {
            own_output: 0xFF,
            next_seq: 0,
            awaiting: None,
            pending_out: 0xFF,
            seen: [None; SEEN_WINDOW],
            seen_pos: 0,
            last_reply: [0; 4],
            reply: None,
            slave_input: VecDeque::new(),
            peer_lag: LAG_HEALTHY,
            peer_gave_up: false,
            outbox: VecDeque::new(),
        }
    }

    /// Forget all in-flight state. Called when a session starts or stops.
    pub fn reset(&mut self) {
        self.own_output = 0xFF;
        self.next_seq = 0;
        self.awaiting = None;
        self.pending_out = 0xFF;
        self.seen = [None; SEEN_WINDOW];
        self.seen_pos = 0;
        self.last_reply = [0; 4];
        self.reply = None;
        self.slave_input.clear();
        self.peer_lag = LAG_HEALTHY;
        self.peer_gave_up = false;
        self.outbox.clear();
    }

    /// How many clocked bytes our game has not picked up yet, beyond the one
    /// currently being answered. 0 while the ROM keeps up.
    pub fn lag(&self) -> u8 {
        self.slave_input.len().saturating_sub(1).min(255) as u8
    }

    /// The backlog the peer reported in its last reply. 0 while it keeps up.
    pub fn peer_lag(&self) -> u8 {
        self.peer_lag
    }

    /// Present `byte` as this side's SB.
    ///
    /// Purely local, and deliberately so. v1 broadcast every SB write as a
    /// `[0, b]` announcement because that was the only way a master could learn
    /// the peer's byte — which is exactly the race that corrupted trades. Under
    /// pairing the peer's SB reaches us in the reply to our own clock, so the
    /// announcement is dead weight: dropping it halves packet volume during a
    /// transfer burst, and fewer queued packets means shorter replies.
    pub fn set_output(&mut self, byte: u8) {
        self.own_output = byte;
    }

    /// Begin a master exchange of `out`. Returns the sequence number to wait on.
    pub fn begin_exchange(&mut self, out: u8) -> u8 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.awaiting = Some(seq);
        self.pending_out = out;
        self.reply = None;
        self.outbox.push_back([TAG_CLOCK, seq, out, 0]);
        seq
    }

    /// Re-send the clock for the exchange still being awaited.
    ///
    /// LAN netplay is bare UDP — the host's `send_fn` ignores
    /// `RETRO_NETPACKET_RELIABLE` and calls `sendto` — so nothing beneath us
    /// retransmits. The WAN path was insulated by the reliable, ordered WebRTC
    /// channel the bridge relays over; on a LAN a single lost datagram would
    /// otherwise burn the master's whole timeout and hand the ROM open bus in
    /// the middle of a trade. Safe to call as often as the transport likes: the
    /// responder answers a duplicate from cache and never re-queues the byte.
    pub fn retry_exchange(&mut self) {
        if let Some(seq) = self.awaiting {
            self.outbox.push_back([TAG_CLOCK, seq, self.pending_out, 0]);
        }
    }

    fn is_new_clock(&self, seq: u8) -> bool {
        !self.seen.contains(&Some(seq))
    }

    fn mark_seen(&mut self, seq: u8) {
        self.seen[self.seen_pos] = Some(seq);
        self.seen_pos = (self.seen_pos + 1) % SEEN_WINDOW;
    }

    /// Take the peer's reply for `seq`, if it has arrived.
    pub fn take_reply(&mut self, seq: u8) -> Option<u8> {
        if self.awaiting == Some(seq) {
            if let Some(sb) = self.reply.take() {
                self.awaiting = None;
                return Some(sb);
            }
        }
        None
    }

    /// Abandon the exchange `seq` (timed out). Later replies for it are ignored,
    /// and the peer is told, so the side that failed to answer is the side that
    /// finds out about it.
    pub fn abandon(&mut self, seq: u8) {
        if self.awaiting == Some(seq) {
            self.awaiting = None;
            self.reply = None;
            self.outbox.push_back([TAG_GAVE_UP, seq, 0, 0]);
        }
    }

    /// Has the peer told us it gave up waiting on one of our replies? Consumes
    /// the flag, so the UI sees each give-up once.
    pub fn take_peer_gave_up(&mut self) -> bool {
        std::mem::replace(&mut self.peer_gave_up, false)
    }

    /// Feed one inbound packet. Answering a clock happens *here*, in the
    /// transport, not when the peer core next steps: a real cable presents the
    /// slave's SB continuously in hardware, with no software action. Waiting for
    /// the slave's emulation thread to notice would stall the master for a frame
    /// and — if the slave has not armed a transfer — forever.
    pub fn on_packet(&mut self, pkt: &[u8]) {
        match pkt.first().copied() {
            Some(TAG_OUTPUT) if pkt.len() >= 2 => self.own_peer_presents(pkt[1]),
            Some(TAG_CLOCK) if pkt.len() >= 3 => {
                let (seq, byte) = (pkt[1], pkt[2]);
                if self.is_new_clock(seq) {
                    self.mark_seen(seq);
                    // Reply with the byte we are presenting *right now*, before
                    // the serial engine picks the clocked byte up and overwrites
                    // SB — and cache it, because by the time a retransmit lands
                    // the game may have consumed the byte and presented another.
                    let sb = self.own_output;
                    self.slave_input.push_back(byte);
                    self.last_reply = [TAG_REPLY, seq, sb, self.lag()];
                }
                if self.last_reply[0] == TAG_REPLY && self.last_reply[1] == seq {
                    self.outbox.push_back(self.last_reply);
                } else {
                    // A duplicate older than the cache holds. Answer it so the
                    // peer isn't left waiting — the master drops any reply whose
                    // seq it isn't awaiting — but never re-queue the byte.
                    self.outbox.push_back([TAG_REPLY, seq, self.own_output, self.lag()]);
                }
            }
            Some(TAG_REPLY) if pkt.len() >= 3 => {
                let (seq, byte) = (pkt[1], pkt[2]);
                // v3 appends the responder's backlog. Absent on a shorter
                // packet, which we read as "no news" rather than as healthy.
                if let Some(&lag) = pkt.get(3) {
                    self.peer_lag = lag;
                }
                if self.awaiting == Some(seq) {
                    self.reply = Some(byte);
                }
                // A reply for any other seq is stale by definition — drop it.
            }
            Some(TAG_GAVE_UP) => self.peer_gave_up = true,
            _ => {}
        }
    }

    /// A tag-0 announcement. We no longer send these and never act on one: the
    /// paired reply is the only peer SB a master may trust. Accepted and
    /// discarded so a stray v1 packet is inert rather than malformed.
    fn own_peer_presents(&mut self, _byte: u8) {}

    /// Take the next packet the transport should send. Send [`packet_len`]
    /// bytes of it — the array is sized for the largest tag.
    pub fn pop_outbound(&mut self) -> Option<[u8; 4]> {
        self.outbox.pop_front()
    }

    /// A byte a master clocked into us, if any.
    pub fn poll_slave_input(&mut self) -> Option<u8> {
        self.slave_input.pop_front()
    }
}

/// Wire length for a packet with the given tag: tag 0 is 2 bytes (the v1
/// layout, inbound only), a clock is 3, a reply is 4 (it carries `lag`).
pub fn packet_len(tag: u8) -> usize {
    match tag {
        TAG_OUTPUT => 2,
        TAG_REPLY => 4,
        _ => 3, // clock, give-up
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::{LinkCable, Serial};
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A two-ended model of the netpacket wire.
    ///
    /// Packets are queued and only cross when `deliver` runs, which is what the
    /// real transport does — the frontend hands us inbound packets when we pump,
    /// not at the instant the peer sent them. This is the property `LocalLink`
    /// does not have and the reason bugs escaped to two-device smokes.
    #[derive(Default)]
    struct Wire {
        /// Packets in flight toward each side.
        inflight: [VecDeque<Vec<u8>>; 2],
        /// Drop every Nth packet that crosses. 0 = lossless.
        ///
        /// LAN netplay is bare UDP: the host's `send_fn` ignores
        /// `RETRO_NETPACKET_RELIABLE` and calls `sendto` directly. The WAN path
        /// never exposed this because the RfuBridge relays over a reliable,
        /// ordered WebRTC channel. A Game Boy cable cannot lose a byte, so the
        /// protocol has to survive loss on its own.
        drop_every: usize,
        crossed: usize,
        dropped: usize,
    }

    impl Wire {
        fn lossy(drop_every: usize) -> Wire {
            Wire {
                drop_every,
                ..Wire::default()
            }
        }
        fn send(&mut self, from: usize, pkt: Vec<u8>) {
            self.crossed += 1;
            if self.drop_every != 0 && self.crossed % self.drop_every == 0 {
                self.dropped += 1;
                return; // silently lost, exactly like a UDP datagram
            }
            self.inflight[1 - from].push_back(pkt);
        }
        fn drain(&mut self, side: usize) -> Vec<Vec<u8>> {
            self.inflight[side].drain(..).collect()
        }
        /// Is any *exchange* packet stranded? Trailing tag-0 announcements are
        /// fine — they are state, not a transaction — but a clock or a reply
        /// still in flight means someone is blocked until the next frame.
        fn exchange_pending(&self) -> bool {
            self.inflight
                .iter()
                .flatten()
                .any(|p| p[0] == TAG_CLOCK || p[0] == TAG_REPLY)
        }
    }

    // --- The v1 protocol, modelled exactly as shipped -------------------------

    /// `[0, b]` / `[1, b]`, both one-way announcements, no pairing.
    struct V1Endpoint {
        wire: Rc<RefCell<Wire>>,
        side: usize,
        peer_output: Rc<RefCell<[u8; 2]>>,
        slave_input: Rc<RefCell<[VecDeque<u8>; 2]>>,
    }

    impl LinkCable for V1Endpoint {
        fn set_output(&mut self, byte: u8) {
            self.wire.borrow_mut().send(self.side, vec![0, byte]);
        }
        fn master_exchange(&mut self, out: u8) -> u8 {
            self.wire.borrow_mut().send(self.side, vec![1, out]);
            self.peer_output.borrow()[self.side]
        }
        fn poll_slave_input(&mut self) -> Option<u8> {
            self.slave_input.borrow_mut()[self.side].pop_front()
        }
    }

    fn v1_deliver(
        wire: &Rc<RefCell<Wire>>,
        peer_output: &Rc<RefCell<[u8; 2]>>,
        slave_input: &Rc<RefCell<[VecDeque<u8>; 2]>>,
        side: usize,
    ) {
        for pkt in wire.borrow_mut().drain(side) {
            match pkt[0] {
                0 => peer_output.borrow_mut()[side] = pkt[1],
                1 => slave_input.borrow_mut()[side].push_back(pkt[1]),
                _ => {}
            }
        }
    }

    /// A Gen-1 trade is hundreds of byte exchanges where each side's next byte
    /// depends on the last one it received. Sixteen is enough to expose a
    /// protocol that cannot pair a reply to its clock.
    const MASTER_BYTES: [u8; 16] = [
        0x01, 0x02, 0x60, 0xFD, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB,
        0xCC,
    ];
    const SLAVE_BYTES: [u8; 16] = [
        0x02, 0x01, 0xFE, 0x60, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xB1, 0xB2,
        0xB3,
    ];

    /// One byte takes 4096 t-cycles at the normal shift clock, stepped 4 at a
    /// time as the CPU does. 1100 iterations is one transfer plus slack.
    const STEPS_PER_TRANSFER: usize = 1100;

    /// How many transfers the ROM performs between two pumps of the wire.
    ///
    /// This is the number that matters. `netpacket.rs` documents `poll_receive`
    /// as "call once per frame, before stepping the emulator" — but a frame is
    /// 70224 cycles and a byte is 4096, so a Game Boy can complete **seventeen**
    /// transfers before the transport next looks at the network. Gen 1 streams
    /// party data in exactly this kind of tight loop. Four is conservative.
    const EXCHANGES_PER_PUMP: usize = 4;

    /// Load SB on both sides, arm the slave (external clock) and the master
    /// (internal clock), then run one transfer's worth of cycles.
    fn one_transfer(master: &mut Serial, slave: &mut Serial, m_byte: u8, s_byte: u8) {
        slave.write_data(s_byte);
        slave.write_control(0x80);
        master.write_data(m_byte);
        master.write_control(0x81);
        for _ in 0..STEPS_PER_TRANSFER {
            master.step(4);
            slave.step(4);
        }
    }

    /// Run the whole byte sequence over v1, pumping the wire only at frame
    /// boundaries. Returns what each side actually received per exchange.
    fn run_v1(
        master: &mut Serial,
        slave: &mut Serial,
        wire: &Rc<RefCell<Wire>>,
        peer_output: &Rc<RefCell<[u8; 2]>>,
        slave_input: &Rc<RefCell<[VecDeque<u8>; 2]>>,
    ) -> Vec<(u8, u8)> {
        let mut got = Vec::new();
        for chunk in (0..MASTER_BYTES.len()).collect::<Vec<_>>().chunks(EXCHANGES_PER_PUMP) {
            // Frame boundary: this is the only place the transport looks at the
            // network, exactly as the shipped code does.
            v1_deliver(wire, peer_output, slave_input, 0);
            v1_deliver(wire, peer_output, slave_input, 1);
            for &i in chunk {
                one_transfer(master, slave, MASTER_BYTES[i], SLAVE_BYTES[i]);
                got.push((master.data, slave.data));
            }
        }
        got
    }

    /// The field bug, on a desk: over a wire where announcements are not
    /// instantaneous, the v1 master reads whatever `peer_output` happens to hold
    /// — a stale announcement, the initial `0xFF`, or an echo of its own byte.
    ///
    /// This test asserts the *correct* behaviour, so it fails against v1. It is
    /// the repro; `paired_exchange_survives_a_delayed_wire` is the fix.
    #[test]
    #[should_panic(expected = "master must receive the slave's byte")]
    fn v1_announcements_corrupt_the_exchange() {
        let wire = Rc::new(RefCell::new(Wire::default()));
        let peer_output = Rc::new(RefCell::new([0xFFu8; 2]));
        let slave_input = Rc::new(RefCell::new([VecDeque::new(), VecDeque::new()]));

        let mut master = Serial::new();
        let mut slave = Serial::new();
        master.connect(Box::new(V1Endpoint {
            wire: wire.clone(),
            side: 0,
            peer_output: peer_output.clone(),
            slave_input: slave_input.clone(),
        }));
        slave.connect(Box::new(V1Endpoint {
            wire: wire.clone(),
            side: 1,
            peer_output: peer_output.clone(),
            slave_input: slave_input.clone(),
        }));

        let got = run_v1(&mut master, &mut slave, &wire, &peer_output, &slave_input);
        for (i, &(got_m, got_s)) in got.iter().enumerate() {
            assert_eq!(
                got_m, SLAVE_BYTES[i],
                "master must receive the slave's byte (exchange {i})"
            );
            assert_eq!(
                got_s, MASTER_BYTES[i],
                "slave must receive the master's byte (exchange {i})"
            );
        }
    }

    /// The same run, recorded rather than asserted: it pins *how* v1 fails, so
    /// the fix can be shown to change this specific behaviour and not just to
    /// make an assertion stop firing.
    #[test]
    fn v1_returns_stale_and_echoed_bytes() {
        let wire = Rc::new(RefCell::new(Wire::default()));
        let peer_output = Rc::new(RefCell::new([0xFFu8; 2]));
        let slave_input = Rc::new(RefCell::new([VecDeque::new(), VecDeque::new()]));

        let mut master = Serial::new();
        let mut slave = Serial::new();
        master.connect(Box::new(V1Endpoint {
            wire: wire.clone(),
            side: 0,
            peer_output: peer_output.clone(),
            slave_input: slave_input.clone(),
        }));
        slave.connect(Box::new(V1Endpoint {
            wire: wire.clone(),
            side: 1,
            peer_output: peer_output.clone(),
            slave_input: slave_input.clone(),
        }));

        let got = run_v1(&mut master, &mut slave, &wire, &peer_output, &slave_input);
        let master_got: Vec<u8> = got.iter().map(|&(m, _)| m).collect();

        // The master never once reads the byte the slave meant to send in the
        // same exchange; it reads the initial 0xFF, then values from earlier
        // exchanges. This is the garbled-Pokémon symptom, on a desk.
        assert_ne!(
            master_got.as_slice(),
            &SLAVE_BYTES[..],
            "v1 must not be accidentally correct here"
        );
        assert_eq!(
            master_got[0], 0xFF,
            "first exchange reads the initial open-bus value"
        );
        let wrong = master_got
            .iter()
            .zip(SLAVE_BYTES.iter())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            wrong >= MASTER_BYTES.len() / 2,
            "expected widespread corruption, got {wrong} wrong of {}",
            MASTER_BYTES.len()
        );
    }

    // --- The v2 paired protocol ----------------------------------------------

    /// Shared state for a pair of `LinkProto`s talking over the model wire.
    struct Pair {
        wire: Rc<RefCell<Wire>>,
        proto: [Rc<RefCell<LinkProto>>; 2],
    }

    impl Pair {
        fn new() -> Pair {
            Pair::lossy(0)
        }
        fn lossy(drop_every: usize) -> Pair {
            Pair {
                wire: Rc::new(RefCell::new(Wire::lossy(drop_every))),
                proto: [
                    Rc::new(RefCell::new(LinkProto::new())),
                    Rc::new(RefCell::new(LinkProto::new())),
                ],
            }
        }
        /// Flush one side's outbox onto the wire.
        fn flush(&self, side: usize) {
            loop {
                let pkt = self.proto[side].borrow_mut().pop_outbound();
                match pkt {
                    Some(p) => {
                        let len = packet_len(p[0]);
                        self.wire.borrow_mut().send(side, p[..len].to_vec());
                    }
                    None => break,
                }
            }
        }
        /// Deliver everything in flight toward `side` into its protocol.
        /// Borrows are taken and dropped per packet — the same shape the real
        /// transport must use, since `on_packet` can enqueue a reply.
        fn deliver(&self, side: usize) {
            for pkt in self.wire.borrow_mut().drain(side) {
                self.proto[side].borrow_mut().on_packet(&pkt);
            }
            self.flush(side);
        }
        fn pump(&self) {
            self.deliver(0);
            self.deliver(1);
        }
        fn endpoint(&self, side: usize) -> ProtoEndpoint {
            ProtoEndpoint {
                pair: PairHandle {
                    wire: self.wire.clone(),
                    proto: [self.proto[0].clone(), self.proto[1].clone()],
                },
                side,
            }
        }
    }

    #[derive(Clone)]
    struct PairHandle {
        wire: Rc<RefCell<Wire>>,
        proto: [Rc<RefCell<LinkProto>>; 2],
    }

    impl PairHandle {
        fn flush(&self, side: usize) {
            loop {
                let pkt = self.proto[side].borrow_mut().pop_outbound();
                match pkt {
                    Some(p) => {
                        let len = packet_len(p[0]);
                        self.wire.borrow_mut().send(side, p[..len].to_vec());
                    }
                    None => break,
                }
            }
        }
        fn deliver(&self, side: usize) {
            for pkt in self.wire.borrow_mut().drain(side) {
                self.proto[side].borrow_mut().on_packet(&pkt);
            }
            self.flush(side);
        }
    }

    /// A [`LinkCable`] over the paired protocol. `master_exchange` blocks —
    /// pumping the wire — until the reply carrying its own sequence number
    /// arrives, which is exactly what the on-device transport does.
    struct ProtoEndpoint {
        pair: PairHandle,
        side: usize,
    }

    impl LinkCable for ProtoEndpoint {
        fn set_output(&mut self, byte: u8) {
            self.pair.proto[self.side].borrow_mut().set_output(byte);
            self.pair.flush(self.side);
        }
        fn master_exchange(&mut self, out: u8) -> u8 {
            let seq = self.pair.proto[self.side].borrow_mut().begin_exchange(out);
            self.pair.flush(self.side);
            for attempt in 0..64 {
                // Peer receives the clock and answers from its transport.
                self.pair.deliver(1 - self.side);
                self.pair.deliver(self.side);
                let got = self.pair.proto[self.side].borrow_mut().take_reply(seq);
                if let Some(sb) = got {
                    return sb;
                }
                // Nothing came back — assume the clock or the reply was lost
                // and re-send. Mirrors the transport's retransmit timer.
                if attempt % 4 == 3 {
                    self.pair.proto[self.side].borrow_mut().retry_exchange();
                    self.pair.flush(self.side);
                }
            }
            self.pair.proto[self.side].borrow_mut().abandon(seq);
            0xFF // let the ROM's own link-error path run
        }
        fn poll_slave_input(&mut self) -> Option<u8> {
            self.pair.proto[self.side].borrow_mut().poll_slave_input()
        }
    }

    /// The identical frame structure as [`run_v1`] — same pump rate, same
    /// number of transfers between pumps. The only difference is the protocol.
    fn run_v2(pair: &Pair, master: &mut Serial, slave: &mut Serial) -> Vec<(u8, u8)> {
        let mut got = Vec::new();
        for chunk in (0..MASTER_BYTES.len()).collect::<Vec<_>>().chunks(EXCHANGES_PER_PUMP) {
            pair.pump();
            for &i in chunk {
                one_transfer(master, slave, MASTER_BYTES[i], SLAVE_BYTES[i]);
                got.push((master.data, slave.data));
            }
        }
        got
    }

    /// The same delayed wire that breaks v1, with the paired protocol: every
    /// exchange lands the right byte on both sides.
    #[test]
    fn paired_exchange_survives_a_delayed_wire() {
        let pair = Pair::new();
        let mut master = Serial::new();
        let mut slave = Serial::new();
        master.connect(Box::new(pair.endpoint(0)));
        slave.connect(Box::new(pair.endpoint(1)));

        let got = run_v2(&pair, &mut master, &mut slave);
        assert_eq!(got.len(), MASTER_BYTES.len());
        for (i, &(got_m, got_s)) in got.iter().enumerate() {
            assert_eq!(
                got_m, SLAVE_BYTES[i],
                "master must receive the slave's byte (exchange {i})"
            );
            assert_eq!(
                got_s, MASTER_BYTES[i],
                "slave must receive the master's byte (exchange {i})"
            );
        }
    }

    /// LAN is bare UDP with no retransmit anywhere beneath us, so a dropped
    /// clock or reply must not cost a byte. Every exchange still has to land the
    /// right value with one packet in twenty going missing.
    #[test]
    fn paired_exchange_survives_packet_loss() {
        // 1-in-20 is a plausible bad LAN; 1-in-3 is far worse than anything a
        // real network does. Both must be lossless at the byte level, because a
        // trade has no tolerance for a single wrong byte.
        for drop_every in [20usize, 7, 3] {
            let pair = Pair::lossy(drop_every);
            let mut master = Serial::new();
            let mut slave = Serial::new();
            master.connect(Box::new(pair.endpoint(0)));
            slave.connect(Box::new(pair.endpoint(1)));

            let got = run_v2(&pair, &mut master, &mut slave);
            let dropped = pair.wire.borrow().dropped;
            assert!(
                dropped > 0,
                "the test is worthless if nothing was actually dropped (1 in {drop_every})"
            );
            for (i, &(got_m, got_s)) in got.iter().enumerate() {
                assert_eq!(
                    got_m, SLAVE_BYTES[i],
                    "master must receive the slave's byte despite loss \
                     (exchange {i}, 1 in {drop_every} dropped, {dropped} lost)"
                );
                assert_eq!(
                    got_s, MASTER_BYTES[i],
                    "slave must receive the master's byte despite loss \
                     (exchange {i}, 1 in {drop_every} dropped, {dropped} lost)"
                );
            }
        }
    }

    /// A retransmitted clock must not hand the game the same byte twice. This is
    /// the failure retransmit would otherwise introduce: the reply is lost, the
    /// master re-sends, and a naive responder queues the byte a second time —
    /// silently shifting every later byte in a trade by one.
    #[test]
    fn a_retransmitted_clock_is_not_delivered_twice() {
        let mut p = LinkProto::new();
        p.set_output(0x3C);

        p.on_packet(&[TAG_CLOCK, 7, 0xA5]);
        let first = p.pop_outbound().expect("first clock must be answered");

        // The reply was lost; the master sends the identical clock again.
        p.on_packet(&[TAG_CLOCK, 7, 0xA5]);
        let again = p.pop_outbound().expect("a duplicate must still be answered");

        assert_eq!(first, again, "the answer must be identical, not recomputed");
        assert_eq!(p.poll_slave_input(), Some(0xA5));
        assert_eq!(
            p.poll_slave_input(),
            None,
            "the duplicate must not queue a second copy"
        );
    }

    /// The cached answer must be the SB from the original exchange. If the game
    /// has moved on and written a new SB by the time the retransmit lands,
    /// recomputing the reply would answer a stale clock with a fresh byte.
    #[test]
    fn a_duplicate_is_answered_with_the_original_sb() {
        let mut p = LinkProto::new();
        p.set_output(0x11);
        p.on_packet(&[TAG_CLOCK, 3, 0xAA]);
        p.pop_outbound();

        // Game consumes the byte and presents something new.
        assert_eq!(p.poll_slave_input(), Some(0xAA));
        p.set_output(0x99);

        p.on_packet(&[TAG_CLOCK, 3, 0xAA]);
        assert_eq!(
            p.pop_outbound(),
            Some([TAG_REPLY, 3, 0x11, LAG_HEALTHY]),
            "must replay the original SB, not the byte presented since"
        );
    }

    /// A reply that arrives for an exchange the master already moved past must
    /// never be consumed as the answer to the current one. This is the specific
    /// confusion that produced Pokémon the owner did not own.
    #[test]
    fn stale_reply_is_not_mistaken_for_the_current_one() {
        let mut p = LinkProto::new();
        let seq_a = p.begin_exchange(0x11);
        p.abandon(seq_a); // timed out
        let seq_b = p.begin_exchange(0x22);
        assert_ne!(seq_a, seq_b);

        // The late reply to A shows up now.
        p.on_packet(&[TAG_REPLY, seq_a, 0xDE]);
        assert_eq!(p.take_reply(seq_b), None, "A's reply must not answer B");

        p.on_packet(&[TAG_REPLY, seq_b, 0xAD]);
        assert_eq!(p.take_reply(seq_b), Some(0xAD));
    }

    /// The transport answers a clock with the byte it was presenting *before*
    /// the clocked byte overwrites SB. Answering afterwards echoes the master's
    /// own byte back, which is what v1 did.
    #[test]
    fn reply_carries_the_pre_transfer_sb() {
        let mut p = LinkProto::new();
        p.set_output(0x3C);
        assert_eq!(p.pop_outbound(), None, "presenting SB is local, not a packet");

        p.on_packet(&[TAG_CLOCK, 7, 0xA5]);

        assert_eq!(
            p.pop_outbound(),
            Some([TAG_REPLY, 7, 0x3C, LAG_HEALTHY]),
            "reply must carry our pre-transfer SB, not the master's byte"
        );
        assert_eq!(p.poll_slave_input(), Some(0xA5));
    }

    /// The signal the UI needs: a responder whose *game* has stopped reading
    /// clocked bytes reports a rising lag, even though its transport is
    /// answering every clock on time. Byte counts cannot see this — which is
    /// why the stall pill could never light up on the side that was actually
    /// behind.
    #[test]
    fn lag_reports_a_game_that_stopped_reading() {
        let mut p = LinkProto::new();
        p.set_output(0x11);

        // Three clocks arrive and the game reads none of them.
        for seq in 0..3u8 {
            p.on_packet(&[TAG_CLOCK, seq, 0xA0 + seq]);
        }
        let lags: Vec<u8> = std::iter::from_fn(|| p.pop_outbound())
            .map(|pkt| pkt[3])
            .collect();
        assert_eq!(lags, vec![0, 1, 2], "lag must climb as the backlog grows");

        // The game catches up; the next reply says so.
        while p.poll_slave_input().is_some() {}
        p.on_packet(&[TAG_CLOCK, 9, 0xFF]);
        assert_eq!(p.pop_outbound().unwrap()[3], LAG_HEALTHY);
    }

    /// The other half of the mirror: the master learns the responder is behind
    /// from the reply, so both devices can name the same culprit at once.
    #[test]
    fn peer_lag_crosses_the_wire() {
        let mut master = LinkProto::new();
        assert_eq!(master.peer_lag(), LAG_HEALTHY);

        let seq = master.begin_exchange(0x5A);
        master.on_packet(&[TAG_REPLY, seq, 0x3C, 4]);

        assert_eq!(master.take_reply(seq), Some(0x3C));
        assert_eq!(
            master.peer_lag(),
            4,
            "master must be able to say the partner is behind"
        );
    }

    /// A healthy round trip through the model wire leaves both sides reporting
    /// no lag, so the pill stays quiet during normal play.
    #[test]
    fn healthy_play_reports_no_lag_on_either_side() {
        let pair = Pair::new();
        let mut master = Serial::new();
        let mut slave = Serial::new();
        master.connect(Box::new(pair.endpoint(0)));
        slave.connect(Box::new(pair.endpoint(1)));

        run_v2(&pair, &mut master, &mut slave);

        assert_eq!(pair.proto[0].borrow().peer_lag(), LAG_HEALTHY);
        assert_eq!(pair.proto[1].borrow().lag(), LAG_HEALTHY);
    }

    /// Pairing alone is not enough.
    ///
    /// A master blocked on a reply can only be answered as fast as the *peer*
    /// looks at the network. With the shipped once-per-frame pump, every
    /// exchange costs a full frame: a Gen-1 party transfer of ~400 bytes would
    /// take over six seconds of frozen emulation. That is the "Please wait!"
    /// wedge, and it survives the protocol fix unless the transport also pumps
    /// below frame granularity.
    ///
    /// This asserts the requirement as a budget rather than a wall-clock time.
    #[test]
    fn peer_must_answer_below_frame_granularity() {
        let pair = Pair::new();
        let mut master = Serial::new();
        let mut slave = Serial::new();
        master.connect(Box::new(pair.endpoint(0)));
        slave.connect(Box::new(pair.endpoint(1)));

        run_v2(&pair, &mut master, &mut slave);

        // `ProtoEndpoint::master_exchange` delivers to the peer inside its wait
        // loop, which is what a sub-frame pump buys us. Every exchange must have
        // resolved without needing the next frame's pump.
        let frames = MASTER_BYTES.len().div_ceil(EXCHANGES_PER_PUMP);
        assert!(
            frames <= 4,
            "the whole sequence must fit in {frames} frames, not one frame per byte"
        );
        assert!(
            !pair.wire.borrow().exchange_pending(),
            "no clock or reply may still be waiting on a frame boundary"
        );
    }

    /// A peer that never answers must not wedge the emulation thread.
    #[test]
    fn unanswered_exchange_gives_up() {
        let mut p = LinkProto::new();
        let seq = p.begin_exchange(0x5A);
        for _ in 0..100 {
            assert_eq!(p.take_reply(seq), None);
        }
        p.abandon(seq);
        // And a reply arriving after we gave up is inert.
        p.on_packet(&[TAG_REPLY, seq, 0x99]);
        assert_eq!(p.take_reply(seq), None);
    }

    /// Giving up is announced, so the peer learns it was the unresponsive one.
    /// Without this the only side that knows the link broke is the side that
    /// was already doing its job.
    #[test]
    fn giving_up_tells_the_peer() {
        let mut master = LinkProto::new();
        let seq = master.begin_exchange(0x5A);
        assert_eq!(master.pop_outbound(), Some([TAG_CLOCK, seq, 0x5A, 0]));
        master.abandon(seq);
        let gave_up = master.pop_outbound().expect("give-up must be announced");
        assert_eq!(gave_up[0], TAG_GAVE_UP);
        assert_eq!(packet_len(gave_up[0]), 3);

        let mut peer = LinkProto::new();
        assert!(!peer.take_peer_gave_up());
        peer.on_packet(&gave_up[..3]);
        assert!(peer.take_peer_gave_up(), "peer must learn it went unanswered");
        assert!(!peer.take_peer_gave_up(), "and only learn it once");
    }

    /// Abandoning an exchange we are not waiting on is silent — no phantom
    /// give-up packet for the peer to misread.
    #[test]
    fn abandoning_an_unheld_exchange_is_silent() {
        let mut p = LinkProto::new();
        let seq = p.begin_exchange(0x11);
        while p.pop_outbound().is_some() {}
        p.on_packet(&[TAG_REPLY, seq, 0x22, LAG_HEALTHY]);
        assert_eq!(p.take_reply(seq), Some(0x22));

        p.abandon(seq); // already resolved
        assert_eq!(p.pop_outbound(), None);
    }
}
