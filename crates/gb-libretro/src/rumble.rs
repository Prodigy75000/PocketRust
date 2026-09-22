// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The libretro rumble interface, for MBC5's motor.
//!
//! `RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE` is **23**, with no experimental
//! bit. Unlike the sensor command there is one numbering and no history to it.
//!
//! # The Game Boy's motor has one speed
//!
//! The cartridge sets a bit and the motor spins until it clears it. There is no
//! duty cycle, no strength and nothing to interpolate, so this module is a
//! latch and an edge detector. Any feel, ramping or amplitude curve belongs to
//! whoever owns the actuator, which is the frontend: a phone's vibrator and a
//! gamepad's two motors want different treatment and neither is knowable here.
//!
//! # Why both effects are driven
//!
//! libretro splits rumble into `STRONG` and `WEAK`, which models a gamepad's
//! two motors. The Game Boy has one, so mapping onto either alone would be a
//! guess about the frontend's hardware. Both are set to the same value, which
//! is what a single-motor device wants whichever one it honours, and what a
//! two-motor pad renders as a plain buzz.
//!
//! # A false return is not a failure to handle
//!
//! `set_rumble_state` returning false means the frontend cannot do this effect,
//! and our own host returns exactly that today: its handler is a deliberate
//! stub, because a stub that answers is spec-correct and keeps cores off the
//! branch they take when the interface is missing entirely.
//!
//! So false is expected and must not be treated as an error to retry or give up
//! over. It is simply dropped. This module used to put a line on screen the
//! first time a motor was asked for and nothing took it, because "my phone is
//! not buzzing" has two indistinguishable causes: a core that never asked and a
//! frontend that cannot deliver. **The owner removed it on 2026-09-22**, having
//! looked for it and judged it unnecessary: "I didn't ask for a message, I
//! guess it's good user UX but it's not needed."
//!
//! Worth knowing before reinstating it: Android never renders SET_MESSAGE at
//! all. The host logs it and nothing draws it, so on that client every such
//! notice was only ever a logcat line. A core that needs to tell an Android
//! player something has to draw into the FRAMEBUFFER, which is what the camera
//! diagnostics do.

use std::cell::UnsafeCell;
use std::ffi::{c_uint, c_void};

use crate::retro_environment_t;

/// `RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE`. No experimental bit on this one.
pub const RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE: u32 = 23;

const RETRO_RUMBLE_STRONG: c_uint = 0;
const RETRO_RUMBLE_WEAK: c_uint = 1;

/// The Game Boy's motor is on or off, so "on" is full scale.
const FULL: u16 = 0xFFFF;

type SetStateFn = Option<unsafe extern "C" fn(c_uint, c_uint, u16) -> bool>;

/// Field order matches libretro.h verbatim.
#[repr(C)]
pub struct retro_rumble_interface {
    pub set_rumble_state: SetStateFn,
}

struct Shared {
    iface: retro_rumble_interface,
    /// Did the environment call hand back a usable function?
    available: bool,
    /// What the motor was doing last frame, so only edges are sent.
    last: bool,
}

struct Global(UnsafeCell<Shared>);

// SAFETY: libretro serialises calls into the core, and this is only touched
// from retro_init / retro_run / retro_unload_game. Same reasoning as
// `GlobalState` in lib.rs.
unsafe impl Sync for Global {}

static SHARED: Global = Global(UnsafeCell::new(Shared {
    iface: retro_rumble_interface {
        set_rumble_state: None,
    },
    available: false,
    last: false,
}));

fn shared() -> &'static mut Shared {
    // SAFETY: see `Global`.
    unsafe { &mut *SHARED.0.get() }
}

/// Ask the frontend for rumble. Safe to call when there is none.
pub fn register(env: retro_environment_t) -> bool {
    let Some(env) = env else { return false };
    let s = shared();
    s.iface.set_rumble_state = None;
    let ok = unsafe {
        env(
            RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE,
            &mut s.iface as *mut retro_rumble_interface as *mut c_void,
        )
    };
    s.available = ok && s.iface.set_rumble_state.is_some();
    s.available
}

/// Drive the motor, but only on a change.
///
/// Called every frame with the cartridge's current motor bit. Sending the same
/// state sixty times a second would be correct and wasteful, and on a frontend
/// that restarts its vibrator per call it would also be audibly worse than the
/// real thing.
pub fn set(on: bool) {
    let s = shared();
    if !s.available || on == s.last {
        return;
    }
    s.last = on;
    let Some(f) = s.iface.set_rumble_state else {
        return;
    };
    let strength = if on { FULL } else { 0 };
    // Both effects, same value. See the module comment. The return values say
    // whether the frontend could do it, and are dropped: there is nothing to
    // retry and nothing left to report.
    unsafe { f(0, RETRO_RUMBLE_STRONG, strength) };
    unsafe { f(0, RETRO_RUMBLE_WEAK, strength) };
}

/// Stop the motor and forget the edge.
///
/// Called on unload and reset. A cartridge can be unloaded mid-buzz, and a
/// motor left running for a game that is no longer there is the kind of bug a
/// user is right to be angry about. Resetting `last` matters too: without it a
/// new game that starts quiet would never send its first "off" and would
/// inherit the previous cartridge's edge.
pub fn stop() {
    let s = shared();
    if s.available {
        if let Some(f) = s.iface.set_rumble_state {
            unsafe { f(0, RETRO_RUMBLE_STRONG, 0) };
            unsafe { f(0, RETRO_RUMBLE_WEAK, 0) };
        }
    }
    s.last = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_id_has_no_experimental_bit() {
        // Several interfaces near this one do carry it, and adding it here
        // would send a completely different command: 23 | EXPERIMENTAL is not
        // rumble, and the payload it would be handed is unrelated.
        assert_eq!(RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE, 23);
        assert_ne!(RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE, 23 | 0x10000);
    }
}
