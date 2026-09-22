// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The libretro camera interface, for the Game Boy Camera cartridge.
//!
//! `RETRO_ENVIRONMENT_GET_CAMERA_INTERFACE` is `26 | RETRO_ENVIRONMENT_EXPERIMENTAL`,
//! which is `$1001A`. The bare number is **not** it, and this crate already
//! carries a comment about an environment id sent without the experimental bit
//! taking the whole app down.
//!
//! # Who owns what
//!
//! Ownership is split across one struct, unlike every other interface here:
//!
//! ```text
//!   core sets:      caps, width, height, frame_*, initialized, deinitialized
//!   frontend sets:  start, stop
//! ```
//!
//! So the struct has to outlive the environment call: it is where the frontend
//! leaves the two functions we call later.
//!
//! # Why the frame is latched instead of applied
//!
//! Frames arrive on the `retro_run` thread, but not necessarily *inside* our
//! `retro_run`: a frontend is free to pump them from its own frame loop either
//! side of ours. Touching the core from the callback would then re-enter a
//! `with_state` borrow that is already open, which is undefined behaviour and
//! would show up as something far stranger than a camera bug.
//!
//! So the callback converts and latches, and `retro_run` applies. One slot,
//! newest wins, exactly as the host latches on its side: under load the player
//! gets the freshest frame and a dropped one, rather than a backlog draining in
//! a burst and developing a photograph of something they stopped aiming at.

use std::cell::UnsafeCell;
use std::ffi::{c_uint, c_void};

use crate::retro_environment_t;

/// `26 | RETRO_ENVIRONMENT_EXPERIMENTAL`.
pub const RETRO_ENVIRONMENT_GET_CAMERA_INTERFACE: u32 = 26 | 0x10000;

/// `1 << RETRO_CAMERA_BUFFER_RAW_FRAMEBUFFER`, where that enumerator is 1.
///
/// The OpenGL texture bit is deliberately not set. A software core has no use
/// for a texture, and the host refuses the interface outright rather than
/// accept a format it cannot produce, which is the right call: accepting one
/// would present as a permanently blank viewfinder with nothing in any log.
const RETRO_CAMERA_BUFFER_RAW_FRAMEBUFFER: u64 = 1 << 1;

type StartFn = Option<unsafe extern "C" fn() -> bool>;
type StopFn = Option<unsafe extern "C" fn()>;
type RawFrameFn = Option<unsafe extern "C" fn(*const u32, c_uint, c_uint, usize)>;
type GlFrameFn = Option<unsafe extern "C" fn(c_uint, c_uint, *const f32)>;
type LifetimeFn = Option<unsafe extern "C" fn()>;

/// Field order matches libretro.h verbatim.
#[repr(C)]
pub struct retro_camera_callback {
    pub caps: u64,
    pub width: c_uint,
    pub height: c_uint,
    pub start: StartFn,
    pub stop: StopFn,
    pub frame_raw_framebuffer: RawFrameFn,
    pub frame_opengl_texture: GlFrameFn,
    pub initialized: LifetimeFn,
    pub deinitialized: LifetimeFn,
}

struct Shared {
    cb: retro_camera_callback,
    /// The newest converted frame, waiting for `retro_run` to apply it.
    latched: Option<Vec<u8>>,
    /// Did the environment call succeed? Decides which diagnostic the sensor
    /// shows while no frame has arrived.
    available: bool,
    /// Has a frame genuinely arrived? The host fires `initialized` on the first
    /// real frame rather than at `start`, because on Android `start` cannot
    /// block on a permission prompt and so means only "the lens was asked for".
    live: bool,
    /// Have we asked for the lens? Kept so the lens is never left on.
    running: bool,
}

struct Global(UnsafeCell<Shared>);

// SAFETY: libretro serialises calls into the core, and the host delivers frames
// on the retro_run thread only. Same reasoning as `GlobalState` in lib.rs.
unsafe impl Sync for Global {}

static SHARED: Global = Global(UnsafeCell::new(Shared {
    cb: retro_camera_callback {
        caps: 0,
        width: 0,
        height: 0,
        start: None,
        stop: None,
        frame_raw_framebuffer: None,
        frame_opengl_texture: None,
        initialized: None,
        deinitialized: None,
    },
    latched: None,
    available: false,
    live: false,
    running: false,
}));

fn shared() -> &'static mut Shared {
    // SAFETY: see `Global`.
    unsafe { &mut *SHARED.0.get() }
}

/// Ask the frontend for a camera. Safe to call when there is no camera.
pub fn register(env: retro_environment_t) -> bool {
    let Some(env) = env else { return false };
    let s = shared();
    s.cb.caps = RETRO_CAMERA_BUFFER_RAW_FRAMEBUFFER;
    // A hint only: the frontend delivers what its device can give, and the
    // conversion below copes with a mismatch rather than assuming.
    s.cb.width = gb_core::CAMERA_W as c_uint;
    s.cb.height = gb_core::CAMERA_H as c_uint;
    s.cb.frame_raw_framebuffer = Some(on_raw_frame);
    s.cb.frame_opengl_texture = None;
    s.cb.initialized = Some(on_initialized);
    s.cb.deinitialized = Some(on_deinitialized);
    s.cb.start = None;
    s.cb.stop = None;

    let ok = unsafe {
        env(
            RETRO_ENVIRONMENT_GET_CAMERA_INTERFACE,
            &mut s.cb as *mut retro_camera_callback as *mut c_void,
        )
    };
    s.available = ok && s.cb.start.is_some();
    s.available
}

/// Does the frontend have a camera at all?
pub fn available() -> bool {
    shared().available
}

/// Ask for the lens. Only ever called for a cartridge that has a sensor: a
/// frontend must not be made to raise a camera permission prompt for Pokemon.
pub fn start() {
    let s = shared();
    if s.running || !s.available {
        return;
    }
    if let Some(f) = s.cb.start {
        // On Android this returns true optimistically and means only "the lens
        // was asked for"; a permission prompt cannot be answered on this
        // thread. `initialized` is the edge that means the sensor is really on.
        s.running = unsafe { f() };
    }
}

/// Put the lens away. Idempotent, and called on unload as well as on teardown,
/// because leaving a camera running for a game that is no longer loaded is the
/// kind of bug users are right to be angry about.
pub fn stop() {
    let s = shared();
    if !s.running {
        return;
    }
    s.running = false;
    s.live = false;
    s.latched = None;
    if let Some(f) = s.cb.stop {
        unsafe { f() };
    }
}

/// Take the newest frame, if one has arrived since the last call.
pub fn take_frame() -> Option<Vec<u8>> {
    shared().latched.take()
}

unsafe extern "C" fn on_initialized() {
    shared().live = true;
}

unsafe extern "C" fn on_deinitialized() {
    let s = shared();
    s.live = false;
    s.latched = None;
}

/// Convert an XRGB8888 frame to the sensor's greyscale and latch it.
///
/// `pitch` is in **bytes**, not pixels. Treating it as pixels quarters the
/// stride and shears the picture into a diagonal, which on this cartridge is
/// indistinguishable from the core's own no-frames diagnostic.
unsafe extern "C" fn on_raw_frame(buffer: *const u32, width: c_uint, height: c_uint, pitch: usize) {
    if buffer.is_null() || width == 0 || height == 0 {
        return;
    }
    let (sw, sh) = (width as usize, height as usize);
    let stride = if pitch >= 4 { pitch / 4 } else { sw };

    let mut out = vec![0u8; gb_core::CAMERA_W * gb_core::CAMERA_H];
    for y in 0..gb_core::CAMERA_H {
        // Nearest neighbour. The size is a hint and the host passes through
        // whatever the device gave, so a mismatch has to be survivable rather
        // than assumed away.
        let sy = y * sh / gb_core::CAMERA_H;
        for x in 0..gb_core::CAMERA_W {
            let sx = x * sw / gb_core::CAMERA_W;
            let px = *buffer.add(sy * stride + sx);
            let (r, g, b) = (px >> 16 & 0xFF, px >> 8 & 0xFF, px & 0xFF);
            // Rec. 601 luma in integers. Android sends R=G=B so any weighting
            // would do there, but a frontend delivering real colour should not
            // get a green-heavy picture by accident.
            let luma = (77 * r + 150 * g + 29 * b) >> 8;
            out[y * gb_core::CAMERA_W + x] = luma as u8;
        }
    }
    // One slot, newest wins. See the module comment.
    shared().latched = Some(out);
}
