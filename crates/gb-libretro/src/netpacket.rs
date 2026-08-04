//! libretro netpacket interface (env 78) — GameLink over the frontend's netplay.
//!
//! Trophy Hub (and any libretro netplay host) drives cores that implement
//! `RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE`. We hand the frontend a callback
//! struct; on session start it gives us a `send_fn` to push bytes to peers and
//! calls our `receive` for each inbound packet. We bridge that to gb-core's
//! [`LinkCable`] so the Game Boy serial engine exchanges bytes over the network.
//!
//! The wire format and all of its state live in [`gb_core::LinkProto`], which is
//! pure and unit-tested against a model of this transport (`gb-core/src/link.rs`).
//! This file is only plumbing: drain the outbox into `send_fn`, feed inbound
//! packets to `on_packet`, and pump.
//!
//! Netpacket state lives in its own global, separate from the core's `State`,
//! so the serial engine (called while `State` is borrowed) can reach it without
//! re-entrantly borrowing `State`.

use gb_core::{packet_len, LinkCable, LinkProto};
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void};
use std::time::{Duration, Instant};

pub const RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE: u32 = 78;
const RETRO_NETPACKET_RELIABLE: i32 = 1 << 0;
const RETRO_NETPACKET_BROADCAST: u16 = 0xFFFF;

/// How long a master waits for the paired reply before giving up and handing the
/// ROM an open-bus byte. Long enough for a bad mobile round trip, short enough
/// that a dead peer degrades to the game's own link-error path instead of an ANR.
const EXCHANGE_TIMEOUT: Duration = Duration::from_millis(500);

/// Yield this many times before backing off to a real sleep, so a healthy
/// exchange never pays for a timer but a stuck one never pins a core.
const HOT_SPINS: u32 = 2_000;
const BACKOFF: Duration = Duration::from_micros(250);

/// Pump the network every this many `poll_slave_input` calls.
///
/// The serial engine calls that on *every* CPU step while armed as a slave —
/// ~17000 times a frame — so pumping on each one would swamp the frontend with
/// FFI calls. Every 256 steps is roughly 1 kB of cycles, giving the peer's
/// blocked master an answer in well under a millisecond while costing ~68 pumps
/// a frame. Pumping only once a frame (what this core shipped with) makes every
/// paired exchange cost a full frame; see `peer_must_answer_below_frame_granularity`
/// in gb-core.
const POLL_EVERY: u32 = 256;

type SendFn = unsafe extern "C" fn(flags: i32, buf: *const c_void, len: usize, client_id: u16);
type PollReceiveFn = unsafe extern "C" fn();
type StartFn = unsafe extern "C" fn(u16, SendFn, PollReceiveFn);
type ReceiveFn = unsafe extern "C" fn(*const c_void, usize, u16);
type StopFn = unsafe extern "C" fn();
type PollFn = unsafe extern "C" fn();
type ConnectedFn = unsafe extern "C" fn(u16) -> bool;
type DisconnectedFn = unsafe extern "C" fn(u16);

#[repr(C)]
pub struct RetroNetpacketCallback {
    start: Option<StartFn>,
    receive: Option<ReceiveFn>,
    stop: Option<StopFn>,
    poll: Option<PollFn>,
    connected: Option<ConnectedFn>,
    disconnected: Option<DisconnectedFn>,
    protocol_version: *const c_char,
}
// SAFETY: the frontend only reads this struct (function pointers + a static
// string). We share it as a &'static.
unsafe impl Sync for RetroNetpacketCallback {}

/// Bumped from `pocketrust-link-1`: v1 had no request/response pairing, so a
/// master could not tell the slave's pre-transfer SB from an echo of its own
/// byte. A peer still on v1 must refuse the session rather than corrupt a trade.
static PROTOCOL: &[u8] = b"pocketrust-link-2\0";

/// The callback struct handed to the frontend via env 78.
pub static CALLBACK: RetroNetpacketCallback = RetroNetpacketCallback {
    start: Some(np_start),
    receive: Some(np_receive),
    stop: Some(np_stop),
    poll: None,
    connected: None,
    disconnected: None,
    protocol_version: PROTOCOL.as_ptr() as *const c_char,
};

/// What the emulation loop should do about the link on its next frame.
#[derive(Clone, Copy, PartialEq)]
pub enum Pending {
    None,
    Attach,
    Detach,
}

struct NetLink {
    send_fn: Option<SendFn>,
    poll_receive_fn: Option<PollReceiveFn>,
    active: bool,
    pending: Pending,
    proto: LinkProto,
    steps_since_poll: u32,
}

impl NetLink {
    const fn new() -> NetLink {
        NetLink {
            send_fn: None,
            poll_receive_fn: None,
            active: false,
            pending: Pending::None,
            proto: LinkProto::new(),
            steps_since_poll: 0,
        }
    }

    /// Hand one packet to the frontend. Does not re-enter us: the frontend
    /// queues it for delivery, it never calls `receive` from inside `send`.
    fn send_raw(&self, pkt: &[u8]) {
        if let Some(send) = self.send_fn {
            unsafe {
                send(
                    RETRO_NETPACKET_RELIABLE,
                    pkt.as_ptr() as *const c_void,
                    pkt.len(),
                    RETRO_NETPACKET_BROADCAST,
                );
            }
        }
    }
}

struct NetCell(UnsafeCell<NetLink>);
// SAFETY: every access is on the single emulation thread (start/receive/stop
// and the serial engine all run there); the frontend never touches this.
unsafe impl Sync for NetCell {}
static NET: NetCell = NetCell(UnsafeCell::new(NetLink::new()));

fn with_net<R>(f: impl FnOnce(&mut NetLink) -> R) -> R {
    // SAFETY: single-threaded, non-re-entrant access. Callers must never hold
    // this borrow across `poll_receive`, which re-enters us through `np_receive`
    // — every wait loop below is written as short borrow / poll / short borrow.
    unsafe { f(&mut *NET.0.get()) }
}

pub fn is_active() -> bool {
    with_net(|n| n.active)
}

/// Consume the pending link lifecycle action (attach/detach the transport).
pub fn take_pending() -> Pending {
    with_net(|n| std::mem::replace(&mut n.pending, Pending::None))
}

/// Push everything the protocol wants to send. Safe to call with no borrow held;
/// takes one internally and drops it before returning.
fn flush_outbox() {
    with_net(|n| {
        while let Some(pkt) = n.proto.pop_outbound() {
            let len = packet_len(pkt[0]);
            n.send_raw(&pkt[..len]);
        }
    });
}

/// Drain inbound packets: the frontend calls our `receive` for each queued one,
/// then we push any replies those packets generated.
pub fn poll_receive() {
    let f = with_net(|n| n.poll_receive_fn);
    if let Some(poll) = f {
        // The borrow is released before the callback so `np_receive` can take
        // its own. Do not fold this into a single `with_net`.
        unsafe { poll() };
    }
    flush_outbox();
}

// --- Frontend-facing callbacks (fire on the emulation thread) -----------------

unsafe extern "C" fn np_start(_client_id: u16, send_fn: SendFn, poll_receive_fn: PollReceiveFn) {
    with_net(|n| {
        n.send_fn = Some(send_fn);
        n.poll_receive_fn = Some(poll_receive_fn);
        n.active = true;
        n.steps_since_poll = 0;
        n.proto.reset();
        n.pending = Pending::Attach;
    });
}

unsafe extern "C" fn np_receive(buf: *const c_void, len: usize, _client_id: u16) {
    if buf.is_null() || len < 2 {
        return;
    }
    let pkt = std::slice::from_raw_parts(buf as *const u8, len);
    with_net(|n| n.proto.on_packet(pkt));
}

unsafe extern "C" fn np_stop() {
    with_net(|n| {
        n.active = false;
        n.send_fn = None;
        n.poll_receive_fn = None;
        n.proto.reset();
        n.pending = Pending::Detach;
    });
}

/// The [`LinkCable`] the Game Boy serial engine drives; routes to the NET global.
pub struct NetpacketLink;

impl LinkCable for NetpacketLink {
    fn set_output(&mut self, byte: u8) {
        with_net(|n| n.proto.set_output(byte));
        flush_outbox();
    }

    /// Block until the peer's reply carrying *this* exchange's sequence number
    /// arrives. Only a matching reply is accepted, so a late reply to an
    /// abandoned exchange can never be read as this one's answer.
    fn master_exchange(&mut self, out: u8) -> u8 {
        let seq = with_net(|n| n.proto.begin_exchange(out));
        flush_outbox();

        let deadline = Instant::now() + EXCHANGE_TIMEOUT;
        let mut spins: u32 = 0;
        loop {
            // Short borrow / poll / short borrow — see the SAFETY note on with_net.
            poll_receive();
            if let Some(sb) = with_net(|n| n.proto.take_reply(seq)) {
                return sb;
            }
            if !with_net(|n| n.active) {
                break; // session torn down under us
            }
            if Instant::now() >= deadline {
                break;
            }
            // A healthy reply lands within one network round trip, so spin hot
            // at first. Past that the link is in trouble and we are heading for
            // the timeout — back off rather than pin a core for half a second.
            spins += 1;
            if spins < HOT_SPINS {
                std::thread::yield_now();
            } else {
                std::thread::sleep(BACKOFF);
            }
        }

        with_net(|n| n.proto.abandon(seq));
        0xFF // open bus: let the ROM run its own link-error path
    }

    fn poll_slave_input(&mut self) -> Option<u8> {
        // The engine calls this on every step while armed as a slave. Pump on a
        // fraction of those so a peer blocked in `master_exchange` gets its
        // reply without waiting for our next frame.
        let due = with_net(|n| {
            n.steps_since_poll += 1;
            if n.steps_since_poll >= POLL_EVERY {
                n.steps_since_poll = 0;
                true
            } else {
                false
            }
        });
        if due {
            poll_receive();
        }
        with_net(|n| n.proto.poll_slave_input())
    }
}
