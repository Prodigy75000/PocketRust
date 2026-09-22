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
//! over. It is worth saying out loud once, though, because "my phone is not
//! buzzing" otherwise has two indistinguishable causes: a core that never asked
//! and a frontend that cannot deliver.

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
    /// Has the frontend ever accepted a state change? A property of the
    /// DEVICE, so it outlives any one cartridge.
    accepted: bool,
    /// Has the "it cannot actually buzz" note been handed out? Also a property
    /// of the device, and deliberately not reset per game: see `stop`.
    announced: bool,
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
    accepted: false,
    announced: false,
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
    // Both effects, same value. See the module comment.
    let a = unsafe { f(0, RETRO_RUMBLE_STRONG, strength) };
    let b = unsafe { f(0, RETRO_RUMBLE_WEAK, strength) };
    if a || b {
        s.accepted = true;
    }
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
    // Only the edge. `accepted` and `announced` describe the DEVICE, which does
    // not change when a cartridge does, so clearing them here would re-ask and
    // re-announce on every load.
    //
    // That is not hypothetical. The owner's smoke tablet, an SM-X400, reports
    // "No vibrator found" and does not carry the vibrator feature at all, so
    // for it the answer is false permanently. Resetting per load would put
    // "this device cannot rumble" on screen every single time Pokemon Pinball
    // is opened, forever, which turns one useful explanation into a nag.
    s.last = false;
}

/// Say once, and only once, that the frontend cannot actually buzz.
///
/// Returns true the first time the motor has been asked to run and nothing
/// accepted it. Deliberately keyed on a real attempt rather than on
/// registration: plenty of frontends hand back an interface, and the only
/// evidence that it does anything is a call that was taken.
pub fn unavailable_notice() -> bool {
    let s = shared();
    if !should_announce(s.announced, s.last, s.accepted) {
        return false;
    }
    s.announced = true;
    true
}

/// The rule, separated from the global so it can be tested.
///
/// Announce only when the motor has genuinely been asked to run and nothing
/// took it, and only ever once. The "asked to run" half is what keeps a player
/// who never reaches a rumbling moment from being told about a limitation they
/// have not hit.
fn should_announce(announced: bool, motor_wanted: bool, accepted: bool) -> bool {
    !announced && motor_wanted && !accepted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_waits_for_a_real_attempt_and_then_fires_once() {
        // Nothing has asked for the motor yet: saying anything here would be
        // telling a player about a limit they have not reached.
        assert!(!should_announce(false, false, false));
        // Asked, and nothing took it. This is the one case worth a message.
        assert!(should_announce(false, true, false));
        // Already said. Repeating it on every cartridge load turns a useful
        // explanation into a nag, and on a device with no vibrator at all it
        // would repeat forever.
        assert!(!should_announce(true, true, false));
        // The frontend can actually buzz, so there is nothing to explain.
        assert!(!should_announce(false, true, true));
    }

    #[test]
    fn the_environment_id_has_no_experimental_bit() {
        // Several interfaces near this one do carry it, and adding it here
        // would send a completely different command: 23 | EXPERIMENTAL is not
        // rumble, and the payload it would be handed is unrelated.
        assert_eq!(RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE, 23);
        assert_ne!(RETRO_ENVIRONMENT_GET_RUMBLE_INTERFACE, 23 | 0x10000);
    }
}
