//! libretro front-end ABI for gb-core.
//!
//! This exposes the standard `retro_*` C entry points so the core can be loaded
//! by any libretro host (RetroArch, Trophy Hub's libretro host, ...). The core
//! runs single-threaded, so we keep all state in a thread-local `State`.

#![allow(non_camel_case_types)]
#![allow(clippy::missing_safety_doc)]

mod netpacket;

use gb_core::{Button, Colorize, GameBoy, SCREEN_H, SCREEN_W};
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void, CStr};
use std::ptr;

// --- libretro C types we need -------------------------------------------------

type retro_environment_t = Option<unsafe extern "C" fn(u32, *mut c_void) -> bool>;
type retro_video_refresh_t = Option<unsafe extern "C" fn(*const c_void, u32, u32, usize)>;
type retro_audio_sample_batch_t = Option<unsafe extern "C" fn(*const i16, usize) -> usize>;
type retro_input_poll_t = Option<unsafe extern "C" fn()>;
type retro_input_state_t = Option<unsafe extern "C" fn(u32, u32, u32, u32) -> i16>;

#[repr(C)]
pub struct retro_system_info {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

#[repr(C)]
struct retro_game_geometry {
    base_width: u32,
    base_height: u32,
    max_width: u32,
    max_height: u32,
    aspect_ratio: f32,
}

#[repr(C)]
struct retro_system_timing {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
pub struct retro_system_av_info {
    geometry: retro_game_geometry,
    timing: retro_system_timing,
}

#[repr(C)]
pub struct retro_game_info {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

/// A single front-end-visible core option (SET_VARIABLES / GET_VARIABLE).
#[repr(C)]
struct retro_variable {
    key: *const c_char,
    value: *const c_char,
}

// Environment command + pixel-format constants we use.
const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: u32 = 10;
const RETRO_ENVIRONMENT_GET_VARIABLE: u32 = 15;
const RETRO_ENVIRONMENT_SET_VARIABLES: u32 = 16;
const RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE: u32 = 17;
const RETRO_PIXEL_FORMAT_XRGB8888: i32 = 1;

/// Core-option key for the DMG colorization toggle (Trophy Hub drives this).
const OPT_COLORIZE: &CStr = c"pocketrust_colorize";

// Device + button ids.
const RETRO_DEVICE_JOYPAD: u32 = 1;
const RETRO_DEVICE_ID_JOYPAD_B: u32 = 0;
const RETRO_DEVICE_ID_JOYPAD_SELECT: u32 = 2;
const RETRO_DEVICE_ID_JOYPAD_START: u32 = 3;
const RETRO_DEVICE_ID_JOYPAD_UP: u32 = 4;
const RETRO_DEVICE_ID_JOYPAD_DOWN: u32 = 5;
const RETRO_DEVICE_ID_JOYPAD_LEFT: u32 = 6;
const RETRO_DEVICE_ID_JOYPAD_RIGHT: u32 = 7;
const RETRO_DEVICE_ID_JOYPAD_A: u32 = 8;

const RETRO_MEMORY_SAVE_RAM: u32 = 0;

// --- Core state ---------------------------------------------------------------

struct State {
    gb: Option<GameBoy>,
    rom: Vec<u8>, // kept so retro_reset can rebuild the machine
    frame: Vec<u32>, // XRGB8888, SCREEN_W*SCREEN_H
    /// Set on load; on the next frame we decode the MBC3 RTC out of the SAVE_RAM
    /// buffer the frontend has filled by then (it bypasses `load_sram`).
    restore_rtc: bool,
    env: retro_environment_t,
    video: retro_video_refresh_t,
    audio_batch: retro_audio_sample_batch_t,
    input_poll: retro_input_poll_t,
    input_state: retro_input_state_t,
}

impl State {
    const fn new() -> State {
        State {
            gb: None,
            rom: Vec::new(),
            frame: Vec::new(),
            restore_rtc: false,
            env: None,
            video: None,
            audio_batch: None,
            input_poll: None,
            input_state: None,
        }
    }
}

/// The frontend invokes the core's entry points from more than one thread: it
/// registers callbacks (env, video, audio, input) on its main thread but runs
/// frames on a dedicated emulation thread. So the state must be a single
/// process-global, not thread-local — otherwise `retro_run` sees a fresh empty
/// state with null callbacks (black screen, no audio). This mirrors how C
/// libretro cores keep their state in plain `static`s.
struct GlobalState(UnsafeCell<State>);

// SAFETY: libretro serializes every call into the core — `retro_run`, the
// `retro_set_*` registrations and load/unload never overlap — so there is never
// concurrent access to the single STATE instance.
unsafe impl Sync for GlobalState {}

static STATE: GlobalState = GlobalState(UnsafeCell::new(State::new()));

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    // SAFETY: see `GlobalState` — accesses are serialized by the frontend.
    unsafe { f(&mut *STATE.0.get()) }
}

// --- Required libretro entry points ------------------------------------------

#[no_mangle]
pub extern "C" fn retro_api_version() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn retro_init() {
    with_state(|s| s.frame = vec![0u32; SCREEN_W * SCREEN_H]);
}

#[no_mangle]
pub extern "C" fn retro_deinit() {
    with_state(|s| *s = State::new());
}

#[no_mangle]
pub unsafe extern "C" fn retro_get_system_info(info: *mut retro_system_info) {
    if info.is_null() {
        return;
    }
    (*info).library_name = c"RustGameBoy".as_ptr();
    (*info).library_version = c"0.1.0".as_ptr();
    (*info).valid_extensions = c"gb|gbc".as_ptr();
    (*info).need_fullpath = false;
    (*info).block_extract = false;
}

#[no_mangle]
pub unsafe extern "C" fn retro_get_system_av_info(info: *mut retro_system_av_info) {
    if info.is_null() {
        return;
    }
    (*info).geometry = retro_game_geometry {
        base_width: SCREEN_W as u32,
        base_height: SCREEN_H as u32,
        max_width: SCREEN_W as u32,
        max_height: SCREEN_H as u32,
        aspect_ratio: SCREEN_W as f32 / SCREEN_H as f32,
    };
    (*info).timing = retro_system_timing {
        fps: 4_194_304.0 / 70_224.0, // ~59.727 Hz
        sample_rate: 44_100.0,
    };
}

#[no_mangle]
pub extern "C" fn retro_set_environment(cb: retro_environment_t) {
    with_state(|s| s.env = cb);
    // Advertise our core options to the front-end.
    if let Some(env) = cb {
        let vars = [
            retro_variable {
                key: OPT_COLORIZE.as_ptr(),
                value: c"Colorize GB (DMG) games; auto|off|grayscale".as_ptr(),
            },
            retro_variable {
                key: ptr::null(),
                value: ptr::null(),
            },
        ];
        unsafe {
            env(
                RETRO_ENVIRONMENT_SET_VARIABLES,
                vars.as_ptr() as *mut c_void,
            );
        }
    }
}

/// Read the colorize option from the front-end and apply it to the core.
fn refresh_variables(s: &mut State) {
    let env = match s.env {
        Some(e) => e,
        None => return,
    };
    let mut var = retro_variable {
        key: OPT_COLORIZE.as_ptr(),
        value: ptr::null(),
    };
    let ok = unsafe {
        env(
            RETRO_ENVIRONMENT_GET_VARIABLE,
            &mut var as *mut retro_variable as *mut c_void,
        )
    };
    if ok && !var.value.is_null() {
        let val = unsafe { CStr::from_ptr(var.value) }.to_str().unwrap_or("auto");
        let mode = match val {
            "off" => Colorize::Off,
            "grayscale" => Colorize::Grayscale,
            _ => Colorize::Auto,
        };
        if let Some(gb) = &mut s.gb {
            gb.set_colorization(mode);
        }
    }
}
#[no_mangle]
pub extern "C" fn retro_set_video_refresh(cb: retro_video_refresh_t) {
    with_state(|s| s.video = cb);
}
#[no_mangle]
pub extern "C" fn retro_set_audio_sample(_cb: *const c_void) {}
#[no_mangle]
pub extern "C" fn retro_set_audio_sample_batch(cb: retro_audio_sample_batch_t) {
    with_state(|s| s.audio_batch = cb);
}
#[no_mangle]
pub extern "C" fn retro_set_input_poll(cb: retro_input_poll_t) {
    with_state(|s| s.input_poll = cb);
}
#[no_mangle]
pub extern "C" fn retro_set_input_state(cb: retro_input_state_t) {
    with_state(|s| s.input_state = cb);
}
#[no_mangle]
pub extern "C" fn retro_set_controller_port_device(_port: u32, _device: u32) {}

#[no_mangle]
pub extern "C" fn retro_reset() {
    with_state(|s| {
        if s.rom.is_empty() {
            return;
        }
        // Power-cycle the machine but keep battery-backed save RAM, like a real
        // reset would.
        let sram = s.gb.as_ref().map(|gb| gb.sram().to_vec());
        let mut gb = GameBoy::new(s.rom.clone());
        if let Some(sram) = sram {
            if gb.has_battery() {
                gb.load_sram(&sram);
            }
        }
        s.gb = Some(gb);
        refresh_variables(s); // re-apply the colorize option
    });
}

#[no_mangle]
pub unsafe extern "C" fn retro_load_game(info: *const retro_game_info) -> bool {
    if info.is_null() || (*info).data.is_null() {
        return false;
    }
    let rom = std::slice::from_raw_parts((*info).data as *const u8, (*info).size).to_vec();

    with_state(|s| {
        // Ask the host for 32-bit XRGB8888 video.
        if let Some(env) = s.env {
            let mut fmt = RETRO_PIXEL_FORMAT_XRGB8888;
            env(
                RETRO_ENVIRONMENT_SET_PIXEL_FORMAT,
                &mut fmt as *mut i32 as *mut c_void,
            );
        }
        // Offer the link-cable (netpacket) interface so the host can wire us
        // into its LAN netplay for GameLink. Harmless if the host ignores it.
        if let Some(env) = s.env {
            env(
                netpacket::RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE,
                &netpacket::CALLBACK as *const _ as *mut c_void,
            );
        }

        s.rom = rom.clone();
        let mut gb = GameBoy::new(rom);
        // Default to authentic per-game GBC colorization so DMG games look right
        // out of the box (parity with Gambatte's default-on colorization). A
        // host that exposes the option overrides this in refresh_variables.
        gb.set_colorization(Colorize::Auto);
        s.gb = Some(gb);
        // The frontend loads the .srm into SAVE_RAM after this returns; decode the
        // RTC footer out of it on the first frame (see `restore_rtc` in retro_run).
        s.restore_rtc = true;
        refresh_variables(s); // apply the host's colorize option, if any
    });
    true
}

/// Make the Game Boy's link cable match whether a netpacket session is live.
///
/// Called from `retro_run` (not from the netpacket callback) so it never
/// re-enters the core `State` borrow, and called on **every** frame rather than
/// on a one-shot Attach/Detach edge.
///
/// The edge version dropped sessions. A GameLink session starts when the peers
/// connect, which is ~2 s before `retro_load_game` runs (measured 2026-08-05:
/// netpacket start at 02:17:40.216, load at 02:17:42.105). An Attach consumed in
/// that window found `s.gb == None`, did nothing, and was gone — the cable never
/// attached for the rest of the session, so the Cable Club attendant refused the
/// pair while the app still showed a healthy link. Whether it broke came down to
/// whether one `retro_run` happened to land in the gap, which is exactly the
/// intermittency that was reported.
///
/// Idempotent: reconciling to a state that already holds does nothing.
fn reconcile_netlink() {
    let active = netpacket::is_active();
    with_state(|s| {
        if let Some(gb) = &mut s.gb {
            match (active, gb.link_connected()) {
                (true, false) => gb.connect_link(Box::new(netpacket::NetpacketLink)),
                (false, true) => gb.disconnect_link(),
                _ => {}
            }
        }
    });
}

#[no_mangle]
pub extern "C" fn retro_unload_game() {
    with_state(|s| s.gb = None);
}

#[no_mangle]
pub extern "C" fn retro_run() {
    // Link-cable lifecycle + inbound drain, done outside the State borrow so the
    // netpacket callbacks (which touch their own global) never nest it.
    reconcile_netlink();
    if netpacket::is_active() {
        // Frontend calls our `receive` for each queued inbound byte.
        netpacket::poll_receive();
    }

    with_state(|s| {
        // First frame after a load: the frontend has now filled SAVE_RAM, so pull
        // the MBC3 real-time clock back out of its battery footer.
        if s.restore_rtc {
            if let Some(gb) = &mut s.gb {
                gb.restore_rtc();
            }
            s.restore_rtc = false;
        }

        // Pick up any live change to the colorize option.
        if let Some(env) = s.env {
            let mut updated = false;
            let changed = unsafe {
                env(
                    RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE,
                    &mut updated as *mut bool as *mut c_void,
                )
            };
            if changed && updated {
                refresh_variables(s);
            }
        }

        // Poll input and translate the joypad.
        if let Some(poll) = s.input_poll {
            unsafe { poll() };
        }
        if let (Some(gb), Some(input)) = (&mut s.gb, s.input_state) {
            let pressed = |id: u32| unsafe { input(0, RETRO_DEVICE_JOYPAD, 0, id) != 0 };
            gb.set_button(Button::A, pressed(RETRO_DEVICE_ID_JOYPAD_A));
            gb.set_button(Button::B, pressed(RETRO_DEVICE_ID_JOYPAD_B));
            gb.set_button(Button::Start, pressed(RETRO_DEVICE_ID_JOYPAD_START));
            gb.set_button(Button::Select, pressed(RETRO_DEVICE_ID_JOYPAD_SELECT));
            gb.set_button(Button::Up, pressed(RETRO_DEVICE_ID_JOYPAD_UP));
            gb.set_button(Button::Down, pressed(RETRO_DEVICE_ID_JOYPAD_DOWN));
            gb.set_button(Button::Left, pressed(RETRO_DEVICE_ID_JOYPAD_LEFT));
            gb.set_button(Button::Right, pressed(RETRO_DEVICE_ID_JOYPAD_RIGHT));
        }

        // Run one frame; the core already produces XRGB8888 pixels.
        if let Some(gb) = &mut s.gb {
            s.frame.copy_from_slice(gb.step_frame());
        }
        if let Some(video) = s.video {
            unsafe {
                video(
                    s.frame.as_ptr() as *const c_void,
                    SCREEN_W as u32,
                    SCREEN_H as u32,
                    SCREEN_W * 4,
                );
            }
        }

        // Feed this frame's audio (interleaved stereo i16 @ 44.1 kHz) to the host.
        if let (Some(gb), Some(audio)) = (&mut s.gb, s.audio_batch) {
            let samples = gb.take_audio();
            if !samples.is_empty() {
                unsafe { audio(samples.as_ptr(), samples.len() / 2) };
            }
        }
    });
}

// --- Save RAM (battery) exposure ---------------------------------------------

#[no_mangle]
pub extern "C" fn retro_get_memory_data(id: u32) -> *mut c_void {
    if id != RETRO_MEMORY_SAVE_RAM {
        return ptr::null_mut();
    }
    with_state(|s| match &s.gb {
        Some(gb) if gb.has_battery() && !gb.sram().is_empty() => {
            gb.sram().as_ptr() as *mut c_void
        }
        _ => ptr::null_mut(),
    })
}

#[no_mangle]
pub extern "C" fn retro_get_memory_size(id: u32) -> usize {
    if id != RETRO_MEMORY_SAVE_RAM {
        return 0;
    }
    with_state(|s| match &s.gb {
        Some(gb) if gb.has_battery() => gb.sram().len(),
        _ => 0,
    })
}

// --- Stubs required to satisfy the ABI ---------------------------------------

#[no_mangle]
pub extern "C" fn retro_serialize_size() -> usize {
    // The serialized size is constant for a given cartridge, so this is stable
    // between the front-end's size query and the actual serialize call.
    with_state(|s| s.gb.as_mut().map(|gb| gb.save_state().len()).unwrap_or(0))
}

#[no_mangle]
pub unsafe extern "C" fn retro_serialize(data: *mut c_void, size: usize) -> bool {
    with_state(|s| {
        let gb = match s.gb.as_mut() {
            Some(gb) => gb,
            None => return false,
        };
        let blob = gb.save_state();
        if data.is_null() || blob.len() > size {
            return false;
        }
        std::ptr::copy_nonoverlapping(blob.as_ptr(), data as *mut u8, blob.len());
        true
    })
}

#[no_mangle]
pub unsafe extern "C" fn retro_unserialize(data: *const c_void, size: usize) -> bool {
    with_state(|s| {
        let gb = match s.gb.as_mut() {
            Some(gb) => gb,
            None => return false,
        };
        if data.is_null() {
            return false;
        }
        let slice = std::slice::from_raw_parts(data as *const u8, size);
        let ok = gb.load_state(slice);
        if ok {
            // A save state restores the RTC (and the RAM footer) itself, so a
            // resume-on-launch must not have the pending footer decode overwrite
            // it on the next frame.
            s.restore_rtc = false;
        }
        ok
    })
}
#[no_mangle]
pub extern "C" fn retro_cheat_reset() {}
#[no_mangle]
pub extern "C" fn retro_cheat_set(_index: u32, _enabled: bool, _code: *const c_char) {}
#[no_mangle]
pub unsafe extern "C" fn retro_load_game_special(
    _game_type: u32,
    _info: *const retro_game_info,
    _num: usize,
) -> bool {
    false
}
#[no_mangle]
pub extern "C" fn retro_get_region() -> u32 {
    0 // RETRO_REGION_NTSC
}
