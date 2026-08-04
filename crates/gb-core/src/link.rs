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

/// Wire-format version. Bumped from `pocketrust-link-1`, which had no pairing.
pub const PROTOCOL_VERSION: &str = "pocketrust-link-2";

/// `[0, b]` — "my SB is now `b`". Kept from v1: a slave must announce the byte
/// it presents, because the peer's transport answers clocks on its behalf.
pub const TAG_OUTPUT: u8 = 0;
/// `[2, seq, b]` — "I am clocking `b` into you as exchange `seq`."
pub const TAG_CLOCK: u8 = 2;
/// `[3, seq, b]` — "for exchange `seq`, the byte I was presenting was `b`."
pub const TAG_REPLY: u8 = 3;

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
    /// The reply that matched `awaiting`.
    reply: Option<u8>,
    /// Bytes a master clocked into us, awaiting pickup by the serial engine.
    slave_input: VecDeque<u8>,
    /// Packets for the transport to send.
    outbox: VecDeque<[u8; 3]>,
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
            reply: None,
            slave_input: VecDeque::new(),
            outbox: VecDeque::new(),
        }
    }

    /// Forget all in-flight state. Called when a session starts or stops.
    pub fn reset(&mut self) {
        self.own_output = 0xFF;
        self.next_seq = 0;
        self.awaiting = None;
        self.reply = None;
        self.slave_input.clear();
        self.outbox.clear();
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
        self.reply = None;
        self.outbox.push_back([TAG_CLOCK, seq, out]);
        seq
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

    /// Abandon the exchange `seq` (timed out). Later replies for it are ignored.
    pub fn abandon(&mut self, seq: u8) {
        if self.awaiting == Some(seq) {
            self.awaiting = None;
            self.reply = None;
        }
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
                // Reply with the byte we are presenting *right now*, before the
                // serial engine picks the clocked byte up and overwrites SB.
                self.outbox.push_back([TAG_REPLY, seq, self.own_output]);
                self.slave_input.push_back(byte);
            }
            Some(TAG_REPLY) if pkt.len() >= 3 => {
                let (seq, byte) = (pkt[1], pkt[2]);
                if self.awaiting == Some(seq) {
                    self.reply = Some(byte);
                }
                // A reply for any other seq is stale by definition — drop it.
            }
            _ => {}
        }
    }

    /// A tag-0 announcement. We no longer send these and never act on one: the
    /// paired reply is the only peer SB a master may trust. Accepted and
    /// discarded so a stray v1 packet is inert rather than malformed.
    fn own_peer_presents(&mut self, _byte: u8) {}

    /// Take the next packet the transport should send.
    pub fn pop_outbound(&mut self) -> Option<[u8; 3]> {
        self.outbox.pop_front()
    }

    /// A byte a master clocked into us, if any.
    pub fn poll_slave_input(&mut self) -> Option<u8> {
        self.slave_input.pop_front()
    }
}

/// Wire length for a packet with the given tag. Tag 0 is 2 bytes (v1 layout),
/// tags 2 and 3 are 3.
pub fn packet_len(tag: u8) -> usize {
    if tag == TAG_OUTPUT {
        2
    } else {
        3
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
    }

    impl Wire {
        fn send(&mut self, from: usize, pkt: Vec<u8>) {
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
            Pair {
                wire: Rc::new(RefCell::new(Wire::default())),
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
            for _ in 0..64 {
                // Peer receives the clock and answers from its transport.
                self.pair.deliver(1 - self.side);
                self.pair.deliver(self.side);
                let got = self.pair.proto[self.side].borrow_mut().take_reply(seq);
                if let Some(sb) = got {
                    return sb;
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
            Some([TAG_REPLY, 7, 0x3C]),
            "reply must carry our pre-transfer SB, not the master's byte"
        );
        assert_eq!(p.poll_slave_input(), Some(0xA5));
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
}
