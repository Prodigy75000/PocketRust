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
/// Read only as a liveness check, never as tilt. See `LIVE_THRESHOLD_G`.
const RETRO_SENSOR_ACCELEROMETER_Z: c_uint = 2;

/// How far the reading must MOVE before we believe a sensor is behind it.
///
/// Liveness is a question about change over time, not about magnitude, and
/// that distinction was bought the expensive way. Registering the interface
/// does not mean a sensor is running: our host answers the environment call
/// `true` unconditionally so Dolphin stays on its motion path, and on Android
/// the listener that fills those values is mounted only while a Wii game is
/// loaded.
///
/// The first attempt here tested the magnitude, on the reasoning that an
/// accelerometer measures the reaction to gravity and so reads about 1g at
/// rest whichever way up it is, and never all zeroes. That is true of the
/// hardware and useless as a test, because the DEAD path does not feed zeroes
/// either: Android's bridge stores `(0, 0, 1)` when it unmounts, deliberately,
/// so the next core sees a sane rest pose instead of the last vigorous shake.
/// A plausible baseline chosen for exactly the reason the check assumed no one
/// would choose one.
///
/// Measured on a tablet: the core announced "using this device's
/// accelerometer" and then read a permanently level device, which is the
/// precise failure the magnitude check was written to prevent.
///
/// Change survives that, because it does not care what the constant is. A real
/// accelerometer in a human hand is never still; a stored baseline never moves
/// at all. It is also self-healing in the right direction: if a genuinely
/// motionless device is misread as dead, the player picks it up and it goes
/// live, which is exactly when tilt starts mattering.
const LIVE_CHANGE_G: f32 = 0.02;

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
    /// The first reading seen since `start`, to compare later ones against.
    first: Option<(f32, f32, f32)>,
    /// Has the reading ever moved? Latched, because once a real feed is proven
    /// the answer cannot change, and a player holding still for a moment must
    /// not be dropped onto the fallback mid-roll.
    live: bool,
    /// Frames since `start`, until the sensor-or-stick question is settled.
    /// Stops counting once it has been answered.
    settling: u32,
    /// Has that answer been handed out yet?
    announced: bool,
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
    first: None,
    live: false,
    settling: 0,
    announced: false,
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

// There is deliberately no `available()` here. The obvious accessor would
// report whether the environment call succeeded, and that is exactly the signal
// that turned out not to mean anything: see `LIVE_THRESHOLD_G`. `settle` is the
// question worth asking, and it is answered from a reading.

/// Turn the accelerometer on. Only ever called for a cartridge that has one:
/// nobody should have their phone's sensors woken up because they loaded
/// Pokemon.
pub fn start() {
    let s = shared();
    if s.running || !s.available {
        return;
    }
    s.running = true;
    s.first = None;
    s.live = false;
    s.settling = 0;
    s.announced = false;
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
    s.first = None;
    s.live = false;
    if let Some(f) = s.iface.set_sensor_state {
        unsafe { f(0, RETRO_SENSOR_ACCELEROMETER_DISABLE, 0) };
    }
}

/// The current tilt in the core's screen coordinates, or `None` when there is
/// no sensor actually feeding us, so the caller can fall back to the stick.
///
/// See the module comment for where the signs come from, and `LIVE_THRESHOLD_G`
/// for why a registered interface is not enough on its own.
pub fn tilt() -> Option<(f32, f32)> {
    let s = shared();
    if !s.running {
        return None;
    }
    let f = s.iface.get_sensor_input?;
    let x = unsafe { f(0, RETRO_SENSOR_ACCELEROMETER_X) };
    let y = unsafe { f(0, RETRO_SENSOR_ACCELEROMETER_Y) };
    let z = unsafe { f(0, RETRO_SENSOR_ACCELEROMETER_Z) };
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return None;
    }
    if !s.live {
        match s.first {
            None => {
                s.first = Some((x, y, z));
                return None;
            }
            Some((fx, fy, fz)) => {
                let moved = (x - fx).abs().max((y - fy).abs()).max((z - fz).abs());
                if moved < LIVE_CHANGE_G {
                    return None;
                }
                s.live = true;
            }
        }
    }
    Some((-x, y))
}

/// Which input the player is really on, returned exactly once, on the frame
/// the question is settled. `Some(true)` is the accelerometer.
///
/// Deliberately not answered at load time. Whether a sensor is real cannot be
/// known from the environment call, only from a reading, and the first reading
/// may be a frame or two behind the game. Announcing at load would mean
/// announcing what was advertised, which is the thing that turned out not to
/// be true.
pub fn settle() -> Option<bool> {
    let s = shared();
    if !s.running || s.announced {
        return None;
    }
    if s.live {
        s.announced = true;
        return Some(true);
    }
    s.settling += 1;
    // Three seconds. Longer than the old one second, because the question is
    // now "has it moved" rather than "is it non-zero", and a player who set the
    // device down while the game booted deserves a moment to pick it up before
    // being told they are on the D-pad.
    if s.settling < 180 {
        return None;
    }
    s.announced = true;
    Some(false)
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
