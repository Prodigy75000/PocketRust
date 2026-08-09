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

/// Some environment ids carry this bit. It is part of the id, not a modifier:
/// send the bare number and the host does not recognise the command at all.
/// Worse, the bare number is usually somebody else's command. 35 without the
/// bit is `SET_CONTROLLER_INFO`, which takes an array of port descriptors, so
/// announcing achievement support with a plain 35 hands a host a `bool` where it
/// expects that array and it walks off the end of it. That is exactly what
/// crashed the app on the first build of this: the host logged
/// "SET_CONTROLLER_INFO port 0:" and then took a SIGSEGV.
const RETRO_ENVIRONMENT_EXPERIMENTAL: u32 = 0x10000;
const RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS: u32 = 42 | RETRO_ENVIRONMENT_EXPERIMENTAL;
const RETRO_ENVIRONMENT_SET_MEMORY_MAPS: u32 = 36 | RETRO_ENVIRONMENT_EXPERIMENTAL;

const RETRO_MEMDESC_CONST: u64 = 1 << 0;
const RETRO_MEMDESC_SYSTEM_RAM: u64 = 1 << 2;
const RETRO_MEMDESC_SAVE_RAM: u64 = 1 << 3;
const RETRO_MEMDESC_VIDEO_RAM: u64 = 1 << 4;

const RETRO_MEMORY_SAVE_RAM: u32 = 0;
const RETRO_MEMORY_SYSTEM_RAM: u32 = 2;

/// Work RAM a monochrome cartridge can see: banks 0 and 1 only. A Game Boy
/// Color game banks all eight, so it gets the whole 32 KiB.
const DMG_WRAM_LEN: usize = 0x2000;

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
    (*info).library_version = c"0.2.3".as_ptr();
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

    // Offer the link-cable (netpacket) interface here, NOT from retro_load_game.
    //
    // A host that has to defer wiring the callbacks — because a core's netplay
    // state isn't built until retro_init — flushes that deferral straight after
    // retro_init returns. Announcing at load time means the flush point is
    // already behind us, so the host arms a kickstart that nothing ever fires:
    // the session comes up (ICE connected, bridge bound, link pill shown) while
    // the core's send_fn stays null and not one link byte moves. On device that
    // read as the Cable Club attendant refusing a pair that looked connected,
    // and only on the FIRST GB game after an app start — a warm app was masked
    // by the previous game's env-78 already sitting in the host's slot.
    //
    // retro_set_environment runs before retro_init on every load path, which is
    // where the host expects this and where its deferral logic works. CALLBACK
    // is a static, so there is nothing here that needs a loaded game.
    if let Some(env) = cb {
        unsafe {
            env(
                netpacket::RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE,
                &netpacket::CALLBACK as *const _ as *mut c_void,
            );
        }
    }

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
        // A reset rebuilds the cartridge, so the ROM and save-RAM allocations
        // move and their descriptors would otherwise point at freed memory.
        publish_memory_map(s);
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
        // After `s.gb` is populated: the descriptors point into it.
        publish_memory_map(s);
        if let Some(env) = s.env {
            let mut yes = true;
            unsafe {
                env(
                    RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS,
                    &mut yes as *mut bool as *mut c_void,
                );
            }
        }
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

// --- Memory map ---------------------------------------------------------------

#[repr(C)]
struct RetroMemoryDescriptor {
    flags: u64,
    ptr: *mut c_void,
    offset: usize,
    start: usize,
    select: usize,
    disconnect: usize,
    len: usize,
    addrspace: *const c_char,
}

#[repr(C)]
struct RetroMemoryMap {
    descriptors: *const RetroMemoryDescriptor,
    num_descriptors: u32,
}

const fn desc(flags: u64, ptr: *const u8, start: usize, select: usize, len: usize) -> RetroMemoryDescriptor {
    RetroMemoryDescriptor {
        flags,
        ptr: ptr as *mut c_void,
        offset: 0,
        start,
        select,
        disconnect: 0,
        len,
        addrspace: ptr::null(),
    }
}

/// Publish the guest address space to the front-end.
///
/// This is the descriptor table a front-end prefers over the two legacy
/// `retro_get_memory_*` ids: rcheevos builds its region table from it, and the
/// cheat engine and any RAM watch address through it. Modelled on the table
/// gambatte publishes, so a front-end sees the same shape from either core.
///
/// Two things make this safe to hand out as raw pointers. Work RAM, video RAM,
/// OAM and high RAM are inline arrays inside the `GameBoy`, which itself lives
/// inline in the process-global `State`, so their addresses are fixed for the
/// life of the process and survive a reset. The ROM and the cartridge RAM are
/// heap allocations owned by the cartridge, so they move when the machine is
/// rebuilt: that is why this is re-published from `retro_reset` and not just
/// from `retro_load_game`. The front-end deep-copies the table during the call,
/// so the array below can live on the stack.
///
/// Deliberately absent: the switchable ROM bank at 0x4000 and the switchable
/// work-RAM bank at 0xD000 on a Game Boy Color. Both would need a pointer that
/// is only correct until the game next writes a bank register, and a descriptor
/// that silently goes stale is worse than one that was never published. Bank 0
/// of each is fixed, and the Color's banks 1-7 are published as one contiguous
/// block at 0x10000, which is where rcheevos expects to find them.
fn publish_memory_map(s: &mut State) {
    let env = match s.env {
        Some(env) => env,
        None => return,
    };
    let gb = match &s.gb {
        Some(gb) => gb,
        None => return,
    };
    let descs = memory_descriptors(gb);
    let map = RetroMemoryMap {
        descriptors: descs.as_ptr(),
        num_descriptors: descs.len() as u32,
    };
    unsafe {
        env(
            RETRO_ENVIRONMENT_SET_MEMORY_MAPS,
            &map as *const RetroMemoryMap as *mut c_void,
        );
    }
}

/// The descriptor table itself, split out from the environment call so it can be
/// asserted against directly.
fn memory_descriptors(gb: &GameBoy) -> Vec<RetroMemoryDescriptor> {
    let mut descs: Vec<RetroMemoryDescriptor> = Vec::with_capacity(8);
    // Work RAM. Bank 0 is permanently at 0xC000; 0xD000 shows bank 1 on a DMG
    // and on a Color until the game selects another.
    descs.push(desc(RETRO_MEMDESC_SYSTEM_RAM, gb.wram().as_ptr(), 0xC000, 0, 0x1000));
    descs.push(desc(RETRO_MEMDESC_SYSTEM_RAM, gb.wram()[0x1000..].as_ptr(), 0xD000, 0, 0x1000));
    descs.push(desc(RETRO_MEMDESC_SYSTEM_RAM, gb.hram().as_ptr(), 0xFF80, 0, 0x7F));
    descs.push(desc(RETRO_MEMDESC_VIDEO_RAM, gb.vram().as_ptr(), 0x8000, 0, 0x2000));
    descs.push(desc(0, gb.oam().as_ptr(), 0xFE00, 0xFFFF_FFE0, 0xA0));
    descs.push(desc(RETRO_MEMDESC_CONST, gb.rom().as_ptr(), 0x0000, 0, 0x4000));
    if gb.has_battery() && !gb.sram().is_empty() {
        descs.push(desc(
            RETRO_MEMDESC_SAVE_RAM,
            gb.sram().as_ptr(),
            0xA000,
            !0x1FFF,
            gb.sram().len(),
        ));
    }
    if gb.is_cgb() {
        // Work-RAM banks 2-7 as one block above the 16-bit space, which is where
        // rcheevos reads a Game Boy Color's extended work RAM.
        //
        // Banks *2*-7, not 1-7. Bank 1 is already published at 0xD000 above, and
        // rcheevos lays its regions out end to end: cartridge RAM, then the
        // 0xC000-0xDFFF pair, then this block. Starting this one a bank early
        // double-counts bank 1 and shifts every address in the extended region
        // down by 0x1000, so each achievement reads a plausible byte from the
        // wrong bank. Six banks, 0x6000 bytes. Same as gambatte's descriptor.
        descs.push(desc(
            RETRO_MEMDESC_SYSTEM_RAM,
            gb.wram()[0x2000..].as_ptr(),
            0x10000,
            0xFFFF_A000,
            0x6000,
        ));
    }
    descs
}

#[cfg(test)]
mod map_tests {
    use super::*;

    fn bare_rom(cgb: bool) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0143] = if cgb { 0xC0 } else { 0x00 };
        rom[0x0147] = 0x00; // ROM only, no cartridge RAM
        rom
    }

    fn find(descs: &[RetroMemoryDescriptor], start: usize) -> Option<&RetroMemoryDescriptor> {
        descs.iter().find(|d| d.start == start)
    }

    /// The EXPERIMENTAL bit is part of these ids, not a decoration. Sending the
    /// bare number is not a no-op that a host politely ignores: 35 on its own is
    /// `SET_CONTROLLER_INFO`, which expects an array of port descriptors, so
    /// announcing achievement support with a plain 35 hands the host a `bool`
    /// where it expects that array. That crashed the app on the first build of
    /// the memory map, and neither the type system nor a headless harness can
    /// catch it, because the harness is written from the same constant. The wire
    /// values are pinned here so they are checked against the spec and not
    /// against my own copy of the mistake.
    #[test]
    fn experimental_env_ids_carry_the_bit() {
        assert_eq!(RETRO_ENVIRONMENT_SET_MEMORY_MAPS, 0x1_0024);
        assert_eq!(RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS, 0x1_002A);
        assert_ne!(RETRO_ENVIRONMENT_SET_MEMORY_MAPS, 36);
        assert_ne!(RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS, 42);
        // 35 is SET_CONTROLLER_INFO. Never send it from here.
        assert_ne!(RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS, 35);
    }

    /// The Game Boy Color's extended work RAM starts at **bank 2**, because bank
    /// 1 is already published at 0xD000 and rcheevos lays its regions out end to
    /// end. Publishing bank 1 twice shifts every address in the extended region
    /// down by one bank, and the reads still succeed: they just come back with a
    /// plausible byte from the wrong bank, which no smoke test would notice.
    /// This was written the wrong way round once already.
    #[test]
    fn cgb_extended_wram_starts_at_bank_two() {
        let gb = GameBoy::new(bare_rom(true));
        let descs = memory_descriptors(&gb);
        let base = gb.wram().as_ptr() as usize;

        let bank1 = find(&descs, 0xD000).expect("0xD000 descriptor");
        assert_eq!(bank1.ptr as usize - base, 0x1000, "0xD000 must be bank 1");

        let ext = find(&descs, 0x10000).expect("extended work RAM descriptor");
        assert_eq!(
            ext.ptr as usize - base,
            0x2000,
            "extended block must start at bank 2, not bank 1"
        );
        assert_eq!(ext.len, 0x6000, "six banks, not seven");
        assert_eq!(ext.flags & RETRO_MEMDESC_SYSTEM_RAM, RETRO_MEMDESC_SYSTEM_RAM);
    }

    /// A monochrome game has no extended block at all: it only ever sees banks
    /// 0 and 1, both already published in the 16-bit space.
    #[test]
    fn dmg_publishes_no_extended_wram() {
        let gb = GameBoy::new(bare_rom(false));
        let descs = memory_descriptors(&gb);
        assert!(find(&descs, 0x10000).is_none());
        assert!(find(&descs, 0xC000).is_some());
        assert!(find(&descs, 0xD000).is_some());
    }

    /// Cartridge RAM is only published when the cartridge actually has a
    /// battery-backed chip. A descriptor with a null pointer or zero length is
    /// worse than an absent one.
    #[test]
    fn save_ram_is_absent_without_a_battery() {
        let gb = GameBoy::new(bare_rom(false));
        assert!(!gb.has_battery());
        assert!(find(&memory_descriptors(&gb), 0xA000).is_none());
        for d in memory_descriptors(&gb) {
            assert!(!d.ptr.is_null(), "descriptor at 0x{:X} has a null ptr", d.start);
            assert!(d.len > 0, "descriptor at 0x{:X} has zero length", d.start);
        }
    }

    /// Every published region has to fit inside the buffer it points into, or
    /// the front-end reads past the end of our memory.
    #[test]
    fn published_lengths_fit_their_buffers() {
        for cgb in [false, true] {
            let gb = GameBoy::new(bare_rom(cgb));
            let descs = memory_descriptors(&gb);
            let wram = gb.wram().as_ptr() as usize;
            for d in &descs {
                let p = d.ptr as usize;
                if (wram..wram + gb.wram().len()).contains(&p) {
                    assert!(
                        p + d.len <= wram + gb.wram().len(),
                        "descriptor at 0x{:X} runs past the end of work RAM",
                        d.start
                    );
                }
            }
            assert_eq!(find(&descs, 0x8000).unwrap().len, 0x2000);
            assert_eq!(find(&descs, 0xFF80).unwrap().len, 0x7F);
            assert_eq!(find(&descs, 0xFE00).unwrap().len, 0xA0);
        }
    }
}

// --- Guest memory exposure ----------------------------------------------------
//
// Two regions, and they are read by different things. SAVE_RAM is the battery,
// which the front-end persists to `.srm`. SYSTEM_RAM is work RAM, which nothing
// persists but which anything reading guest state needs: RetroAchievements,
// cheats, RAM watch.
//
// Serving only SAVE_RAM used to strand achievements. rcheevos builds its region
// table from these two ids, and a cartridge with no battery (Super Mario Land,
// Tetris, Kirby's Dream Land) answered null to both, leaving it with zero
// regions and every achievement address unvalidatable. Trophy Hub's host then
// sat on "waiting for core memory map..." forever, since it defers the load
// until either this succeeds or SET_MEMORY_MAPS fires, and we never published
// a map either. Games *with* a battery resolved one region and wired up fine,
// which is what made it look intermittent rather than simply missing.

#[no_mangle]
pub extern "C" fn retro_get_memory_data(id: u32) -> *mut c_void {
    with_state(|s| {
        let gb = match &s.gb {
            Some(gb) => gb,
            None => return ptr::null_mut(),
        };
        match id {
            RETRO_MEMORY_SAVE_RAM if gb.has_battery() && !gb.sram().is_empty() => {
                gb.sram().as_ptr() as *mut c_void
            }
            RETRO_MEMORY_SYSTEM_RAM => gb.wram().as_ptr() as *mut c_void,
            _ => ptr::null_mut(),
        }
    })
}

#[no_mangle]
pub extern "C" fn retro_get_memory_size(id: u32) -> usize {
    with_state(|s| {
        let gb = match &s.gb {
            Some(gb) => gb,
            None => return 0,
        };
        match id {
            RETRO_MEMORY_SAVE_RAM if gb.has_battery() => gb.sram().len(),
            // A DMG game only ever addresses banks 0 and 1. Reporting all 32 KiB
            // for it would hand the front-end six banks the game cannot reach,
            // and shift nothing, but rcheevos sizes the Game Boy region at 8 KiB
            // and the Game Boy Color one at 32 KiB, so answer with the size that
            // matches the machine actually running.
            RETRO_MEMORY_SYSTEM_RAM => {
                if gb.is_cgb() {
                    gb.wram().len()
                } else {
                    DMG_WRAM_LEN
                }
            }
            _ => 0,
        }
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
