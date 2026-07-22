//! libretro netpacket interface (env 78) — GameLink over the frontend's netplay.
//!
//! Trophy Hub (and any libretro netplay host) drives cores that implement
//! `RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE`. We hand the frontend a callback
//! struct; on session start it gives us a `send_fn` to push bytes to peers and
//! calls our `receive` for each inbound byte. We bridge that to gb-core's
//! [`LinkCable`] so the Game Boy serial engine exchanges bytes over the network,
//! reusing the exact 2-byte protocol the local/TCP transports use:
//!   `[0, b]` = "my SB is now b" (what a peer reads when it clocks a transfer)
//!   `[1, b]` = "I clocked b into you" (peer becomes the slave for this byte)
//!
//! Netpacket state lives in its own global, separate from the core's `State`,
//! so the serial engine (called while `State` is borrowed) can reach it without
//! re-entrantly borrowing `State`.

use gb_core::LinkCable;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::ffi::{c_char, c_void};

pub const RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE: u32 = 78;
const RETRO_NETPACKET_RELIABLE: i32 = 1 << 0;
const RETRO_NETPACKET_BROADCAST: u16 = 0xFFFF;

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

static PROTOCOL: &[u8] = b"pocketrust-link-1\0";

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
    peer_output: u8,
    slave_input: VecDeque<u8>,
}

impl NetLink {
    const fn new() -> NetLink {
        NetLink {
            send_fn: None,
            poll_receive_fn: None,
            active: false,
            pending: Pending::None,
            peer_output: 0xFF,
            slave_input: VecDeque::new(),
        }
    }
    fn send(&self, tag: u8, byte: u8) {
        if let Some(send) = self.send_fn {
            let pkt = [tag, byte];
            unsafe {
                send(
                    RETRO_NETPACKET_RELIABLE,
                    pkt.as_ptr() as *const c_void,
                    2,
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
    // SAFETY: single-threaded, non-re-entrant access (NET is never touched while
    // already borrowed — the serial engine only calls the LinkCable methods).
    unsafe { f(&mut *NET.0.get()) }
}

pub fn is_active() -> bool {
    with_net(|n| n.active)
}

/// Consume the pending link lifecycle action (attach/detach the transport).
pub fn take_pending() -> Pending {
    with_net(|n| std::mem::replace(&mut n.pending, Pending::None))
}

/// Drain inbound packets: the frontend calls our `receive` for each queued one.
/// Call once per frame, before stepping the emulator.
pub fn poll_receive() {
    let f = with_net(|n| n.poll_receive_fn);
    if let Some(poll) = f {
        unsafe { poll() };
    }
}

// --- Frontend-facing callbacks (fire on the emulation thread) -----------------

unsafe extern "C" fn np_start(_client_id: u16, send_fn: SendFn, poll_receive_fn: PollReceiveFn) {
    with_net(|n| {
        n.send_fn = Some(send_fn);
        n.poll_receive_fn = Some(poll_receive_fn);
        n.active = true;
        n.peer_output = 0xFF;
        n.slave_input.clear();
        n.pending = Pending::Attach;
    });
}

unsafe extern "C" fn np_receive(buf: *const c_void, len: usize, _client_id: u16) {
    if buf.is_null() || len < 2 {
        return;
    }
    let p = buf as *const u8;
    let tag = *p;
    let byte = *p.add(1);
    with_net(|n| match tag {
        0 => n.peer_output = byte,
        1 => n.slave_input.push_back(byte),
        _ => {}
    });
}

unsafe extern "C" fn np_stop() {
    with_net(|n| {
        n.active = false;
        n.send_fn = None;
        n.poll_receive_fn = None;
        n.slave_input.clear();
        n.pending = Pending::Detach;
    });
}

/// The [`LinkCable`] the Game Boy serial engine drives; routes to the NET global.
pub struct NetpacketLink;

impl LinkCable for NetpacketLink {
    fn set_output(&mut self, byte: u8) {
        with_net(|n| n.send(0, byte));
    }
    fn master_exchange(&mut self, out: u8) -> u8 {
        with_net(|n| {
            n.send(1, out);
            n.peer_output
        })
    }
    fn poll_slave_input(&mut self) -> Option<u8> {
        with_net(|n| n.slave_input.pop_front())
    }
}
