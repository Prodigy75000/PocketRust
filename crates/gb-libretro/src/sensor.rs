// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Prodigy75000

//! The libretro sensor interface, for MBC7's accelerometer.
//!
//! `RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE` is `25 | RETRO_ENVIRONMENT_EXPERIMENTAL`,
//! which is `$10019`. Checked against the current libretro.h rather than taken
//! from a neighbour, because this command genuinely has two numberings in the
//! wild and the two sources here disagreed: our own host's header comments call
//! `21 | EXPERIMENTAL` the modern value and `25 | EXPERIMENTAL` the
//! pre-canonical one, and upstream libretro.h line 1203 says the opposite.
//!
//! It does not bite, because the host matches both. We send the canonical one
//! so that any other libretro frontend works too.
//!
//! Bare 25 is NOT it, and neither is bare 21: without the experimental bit
//! those are different commands whose payloads are structurally unrelated, and
//! writing a sensor interface into one corrupts whatever was really passed.
//! The host has already been bitten by exactly that, aliasing bare 21 and
//! landing on GET_INPUT_DEVICE_CAPABILITIES, whose payload is a `uint64_t *`.
//!
//! # It is a PULL, unlike the camera
//!
//! The camera pushes frames at us. This is the other shape: the frontend hands
//! over one function and we call it once a frame. So there is nothing to latch
//! and no re-entrancy to worry about, and the whole module is small.
//!
//! # Axes, and why the signs are not obvious
//!
//! The frontend reports **specific force in g**, which is what an accelerometer
//! actually measures: at rest it reads the reaction to gravity, pointing *up*,
//! not gravity itself pointing down. The host's own note records that a phone
//! lying flat reads `+1g` on Z, which confirms that direction.
//!
//! Writing `x_d`, `y_d` for the device's rightward and up-the-screen axes, and
//! `f` for that specific force:
//!
//! ```text
//!   sensor X = f . x_d        sensor Y = f . y_d
//! ```
//!
//! A ball rolls toward the lowered edge, which is along `-f` projected into the
//! screen plane, so in device axes it rolls along `(-X, -Y)`.
//!
//! The core takes SCREEN coordinates, where `+y` is **down** and the device's
//! `y_d` is **up**. That flip cancels one of the two negations and leaves an
//! asymmetric-looking but correct pair:
//!
//! ```text
//!   core x = -sensor X        core y = +sensor Y
//! ```
//!
//! Lowering the right-hand edge gives a negative sensor X and must roll the
//! ball right, which is `+x`. Lowering the near edge gives a positive sensor Y
//! and must roll it toward the player, which is `+y`.
//!
//! This is derived, not measured on a device, and it is the one part of this
//! file worth distrusting: Android's sign convention is the input to the
//! derivation. If Kirby rolls backwards on real hardware, both signs live here
//! and nowhere else.

use std::cell::UnsafeCell;
use std::ffi::{c_uint, c_void};

use crate::retro_environment_t;

/// `25 | RETRO_ENVIRONMENT_EXPERIMENTAL`.
pub const RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE: u32 = 25 | 0x10000;

const RETRO_SENSOR_ACCELEROMETER_ENABLE: c_uint = 0;
const RETRO_SENSOR_ACCELEROMETER_DISABLE: c_uint = 1;

const RETRO_SENSOR_ACCELEROMETER_X: c_uint = 0;
const RETRO_SENSOR_ACCELEROMETER_Y: c_uint = 1;

/// The rate we ask for, in Hz. A hint: the host samples at whatever its own
/// listener runs at and accepts any value here.
const SAMPLE_RATE_HZ: c_uint = 60;

type SetStateFn = Option<unsafe extern "C" fn(c_uint, c_uint, c_uint) -> bool>;
type GetInputFn = Option<unsafe extern "C" fn(c_uint, c_uint) -> f32>;

/// Field order matches libretro.h verbatim.
#[repr(C)]
pub struct retro_sensor_interface {
    pub set_sensor_state: SetStateFn,
    pub get_sensor_input: GetInputFn,
}

struct Shared {
    iface: retro_sensor_interface,
    /// Did the environment call succeed and hand back usable functions?
    available: bool,
    /// Have we asked the frontend to turn the accelerometer on?
    running: bool,
}

struct Global(UnsafeCell<Shared>);

// SAFETY: libretro serialises calls into the core, and we only ever touch this
// from retro_init / retro_run / retro_unload_game. Same reasoning as
// `GlobalState` in lib.rs.
unsafe impl Sync for Global {}

static SHARED: Global = Global(UnsafeCell::new(Shared {
    iface: retro_sensor_interface {
        set_sensor_state: None,
        get_sensor_input: None,
    },
    available: false,
    running: false,
}));

fn shared() -> &'static mut Shared {
    // SAFETY: see `Global`.
    unsafe { &mut *SHARED.0.get() }
}

/// Ask the frontend for a sensor. Safe to call when there is none.
pub fn register(env: retro_environment_t) -> bool {
    let Some(env) = env else { return false };
    let s = shared();
    s.iface.set_sensor_state = None;
    s.iface.get_sensor_input = None;

    let ok = unsafe {
        env(
            RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE,
            &mut s.iface as *mut retro_sensor_interface as *mut c_void,
        )
    };
    // Both halves are needed. A frontend that returns true and leaves
    // `get_sensor_input` null would otherwise read as a working sensor that
    // reports a permanently level console, which is indistinguishable from a
    // player holding it still.
    s.available = ok && s.iface.get_sensor_input.is_some();
    s.available
}

/// Does the frontend have an accelerometer at all?
pub fn available() -> bool {
    shared().available
}

/// Turn the accelerometer on. Only ever called for a cartridge that has one:
/// nobody should have their phone's sensors woken up because they loaded
/// Pokemon.
pub fn start() {
    let s = shared();
    if s.running || !s.available {
        return;
    }
    s.running = true;
    if let Some(f) = s.iface.set_sensor_state {
        // A false return means the frontend cannot give us this rate. We carry
        // on regardless and let the reads decide, because the rate is a hint
        // and refusing to poll over it would turn a slow sensor into no sensor.
        unsafe { f(0, RETRO_SENSOR_ACCELEROMETER_ENABLE, SAMPLE_RATE_HZ) };
    }
}

/// Turn it off again. Idempotent, and called on unload as well as teardown:
/// leaving a sensor running for a game that is no longer loaded costs battery
/// for nothing.
pub fn stop() {
    let s = shared();
    if !s.running {
        return;
    }
    s.running = false;
    if let Some(f) = s.iface.set_sensor_state {
        unsafe { f(0, RETRO_SENSOR_ACCELEROMETER_DISABLE, 0) };
    }
}

/// The current tilt in the core's screen coordinates, or `None` if there is no
/// sensor running. See the module comment for where the signs come from.
pub fn tilt() -> Option<(f32, f32)> {
    let s = shared();
    if !s.running {
        return None;
    }
    let f = s.iface.get_sensor_input?;
    let x = unsafe { f(0, RETRO_SENSOR_ACCELEROMETER_X) };
    let y = unsafe { f(0, RETRO_SENSOR_ACCELEROMETER_Y) };
    // A frontend with the listener not yet delivering returns 0.0, which is
    // "level" and is the right thing to do with it anyway.
    if !x.is_finite() || !y.is_finite() {
        return Some((0.0, 0.0));
    }
    Some((-x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_id_is_the_canonical_one_with_the_experimental_bit() {
        // libretro.h: (25 | RETRO_ENVIRONMENT_EXPERIMENTAL). Bare 25 is not a
        // command at all, and the other numbering seen in the wild, 0x10015,
        // is what our host happens to call canonical. Pinning the number here
        // means a disagreement upstream shows up as a failing test rather than
        // as a sensor that silently never registers.
        assert_eq!(RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE, 0x1_0019);
        assert_ne!(RETRO_ENVIRONMENT_GET_SENSOR_INTERFACE, 25);
    }
}
