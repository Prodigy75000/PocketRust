//! The Audio Processing Unit.
//!
//! Four channels feed a stereo mixer:
//!   CH1 pulse + frequency sweep, CH2 pulse, CH3 programmable wave, CH4 noise.
//!
//! A 512 Hz "frame sequencer" (clocked off a falling edge of bit 12 of the same
//! internal counter that drives DIV) paces the length counters (256 Hz), the
//! volume envelopes (64 Hz) and the sweep unit (128 Hz).
//!
//! We step the whole unit per T-cycle, which makes the channel frequency timers
//! and the frame sequencer exact without any special-casing.

use crate::save::Cursor;

/// Host sample rate we resample down to.
pub const SAMPLE_RATE: u32 = 44_100;
const CPU_HZ: u32 = 4_194_304;

/// Duty patterns for the pulse channels (each bit is one of 8 steps).
const DUTY: [u8; 4] = [0b0000_0001, 0b1000_0001, 0b1000_0111, 0b0111_1110];

/// Bit 12 of the internal counter drives the 512 Hz frame sequencer.
const FRAME_SEQ_BIT: u16 = 1 << 12;

// --- Shared building blocks ---------------------------------------------------

/// Length counter: when enabled, counts down at 256 Hz and disables the channel
/// when it reaches zero.
#[derive(Default)]
struct Length {
    enabled: bool,
    counter: u16,
    max: u16, // 64 for pulse/noise, 256 for wave
}

impl Length {
    fn new(max: u16) -> Length {
        Length {
            enabled: false,
            counter: 0,
            max,
        }
    }
    /// Returns true if the channel should be disabled this tick.
    fn clock(&mut self) -> bool {
        if self.enabled && self.counter > 0 {
            self.counter -= 1;
            self.counter == 0
        } else {
            false
        }
    }
    fn set_from_reg(&mut self, load: u16) {
        self.counter = self.max - load;
    }
    /// On trigger a zero counter reloads to max. If that reload happens while
    /// length is enabled and the next frame step won't clock length, the
    /// counter is immediately clocked once (the trigger extra-length quirk).
    fn trigger(&mut self, extra_clock: bool) {
        if self.counter == 0 {
            self.counter = self.max;
            if self.enabled && extra_clock {
                self.counter -= 1;
            }
        }
    }
}

/// Volume envelope: steps the volume up or down at 64 Hz.
#[derive(Default)]
struct Envelope {
    initial: u8,
    add: bool,
    period: u8,
    volume: u8,
    timer: u8,
}

impl Envelope {
    fn clock(&mut self) {
        if self.period == 0 {
            return;
        }
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = self.period;
            if self.add && self.volume < 15 {
                self.volume += 1;
            } else if !self.add && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }
    fn trigger(&mut self) {
        self.volume = self.initial;
        self.timer = self.period;
    }
    /// The DAC is powered by the top 5 bits of the envelope register.
    fn dac_on(&self) -> bool {
        self.initial > 0 || self.add
    }
    fn write_reg(&mut self, v: u8) {
        self.initial = v >> 4;
        self.add = v & 0x08 != 0;
        self.period = v & 0x07;
    }
    fn read_reg(&self) -> u8 {
        (self.initial << 4) | (self.add as u8) << 3 | self.period
    }
}

// --- Pulse channel (CH1 / CH2) -----------------------------------------------

struct Pulse {
    enabled: bool,
    duty: u8,
    duty_pos: u8,
    freq: u16,
    timer: i32,
    length: Length,
    env: Envelope,
    // Sweep (CH1 only; CH2 leaves has_sweep=false).
    has_sweep: bool,
    sweep_period: u8,
    sweep_negate: bool,
    sweep_shift: u8,
    sweep_timer: u8,
    sweep_enabled: bool,
    sweep_shadow: u16,
    sweep_did_negate: bool,
}

impl Pulse {
    fn new(has_sweep: bool) -> Pulse {
        Pulse {
            enabled: false,
            duty: 0,
            duty_pos: 0,
            freq: 0,
            timer: 0,
            length: Length::new(64),
            env: Envelope::default(),
            has_sweep,
            sweep_period: 0,
            sweep_negate: false,
            sweep_shift: 0,
            sweep_timer: 0,
            sweep_enabled: false,
            sweep_shadow: 0,
            sweep_did_negate: false,
        }
    }

    fn tick(&mut self) {
        self.timer -= 1;
        if self.timer <= 0 {
            self.timer = ((2048 - self.freq as i32) * 4).max(1);
            self.duty_pos = (self.duty_pos + 1) & 7;
        }
    }

    fn clock_length(&mut self) {
        if self.length.clock() {
            self.enabled = false;
        }
    }

    fn clock_sweep(&mut self) {
        if !self.has_sweep {
            return;
        }
        if self.sweep_timer > 0 {
            self.sweep_timer -= 1;
        }
        if self.sweep_timer == 0 {
            self.sweep_timer = if self.sweep_period > 0 { self.sweep_period } else { 8 };
            if self.sweep_enabled && self.sweep_period > 0 {
                let new = self.sweep_calc();
                if new <= 2047 && self.sweep_shift > 0 {
                    self.sweep_shadow = new;
                    self.freq = new;
                    // A second calculation is performed for overflow only.
                    self.sweep_calc();
                }
            }
        }
    }

    /// Compute the next sweep frequency and disable the channel on overflow.
    fn sweep_calc(&mut self) -> u16 {
        let delta = self.sweep_shadow >> self.sweep_shift;
        let new = if self.sweep_negate {
            self.sweep_did_negate = true;
            self.sweep_shadow.wrapping_sub(delta)
        } else {
            self.sweep_shadow + delta
        };
        if new > 2047 {
            self.enabled = false;
        }
        new
    }

    fn trigger(&mut self, extra_clock: bool) {
        self.enabled = true;
        self.length.trigger(extra_clock);
        self.timer = ((2048 - self.freq as i32) * 4).max(1);
        self.env.trigger();
        if !self.env.dac_on() {
            self.enabled = false;
        }
        if self.has_sweep {
            self.sweep_shadow = self.freq;
            self.sweep_timer = if self.sweep_period > 0 { self.sweep_period } else { 8 };
            self.sweep_enabled = self.sweep_period > 0 || self.sweep_shift > 0;
            self.sweep_did_negate = false;
            if self.sweep_shift > 0 {
                self.sweep_calc();
            }
        }
    }

    /// Power-off zeros every register-backed field but keeps the length
    /// counter's current value (DMG behaviour).
    fn power_off(&mut self) {
        self.enabled = false;
        self.duty = 0;
        self.duty_pos = 0;
        self.freq = 0;
        self.timer = 0;
        self.env = Envelope::default();
        self.length.enabled = false;
        self.sweep_period = 0;
        self.sweep_negate = false;
        self.sweep_shift = 0;
        self.sweep_timer = 0;
        self.sweep_enabled = false;
        self.sweep_shadow = 0;
        self.sweep_did_negate = false;
    }

    /// Digital output 0..15 (before the DAC).
    fn output(&self) -> u8 {
        if !self.enabled || !self.env.dac_on() {
            return 0;
        }
        let bit = (DUTY[self.duty as usize] >> (7 - self.duty_pos)) & 1;
        if bit != 0 {
            self.env.volume
        } else {
            0
        }
    }
}

// --- Wave channel (CH3) -------------------------------------------------------

struct Wave {
    enabled: bool,
    dac_on: bool,
    freq: u16,
    timer: i32,
    position: u8,
    volume_code: u8,
    length: Length,
    ram: [u8; 16],
    sample_buffer: u8,
}

impl Wave {
    fn new() -> Wave {
        Wave {
            enabled: false,
            dac_on: false,
            freq: 0,
            timer: 0,
            position: 0,
            volume_code: 0,
            length: Length::new(256),
            ram: [0; 16],
            sample_buffer: 0,
        }
    }

    fn tick(&mut self) {
        self.timer -= 1;
        if self.timer <= 0 {
            self.timer = ((2048 - self.freq as i32) * 2).max(1);
            self.position = (self.position + 1) & 31;
            let byte = self.ram[(self.position / 2) as usize];
            self.sample_buffer = if self.position & 1 == 0 {
                byte >> 4
            } else {
                byte & 0x0F
            };
        }
    }

    fn clock_length(&mut self) {
        if self.length.clock() {
            self.enabled = false;
        }
    }

    fn trigger(&mut self, extra_clock: bool) {
        self.enabled = true;
        self.length.trigger(extra_clock);
        self.timer = ((2048 - self.freq as i32) * 2).max(1);
        self.position = 0;
        if !self.dac_on {
            self.enabled = false;
        }
    }

    fn power_off(&mut self) {
        self.enabled = false;
        self.dac_on = false;
        self.freq = 0;
        self.timer = 0;
        self.position = 0;
        self.volume_code = 0;
        self.sample_buffer = 0;
        self.length.enabled = false;
        // wave RAM and the length counter value survive power-off on DMG
    }

    fn output(&self) -> u8 {
        if !self.enabled || !self.dac_on {
            return 0;
        }
        match self.volume_code {
            0 => 0,
            1 => self.sample_buffer,
            2 => self.sample_buffer >> 1,
            _ => self.sample_buffer >> 2,
        }
    }
}

// --- Noise channel (CH4) ------------------------------------------------------

const NOISE_DIVISOR: [u32; 8] = [8, 16, 32, 48, 64, 80, 96, 112];

struct Noise {
    enabled: bool,
    timer: i32,
    lfsr: u16,
    clock_shift: u8,
    width_7bit: bool,
    divisor_code: u8,
    length: Length,
    env: Envelope,
}

impl Noise {
    fn new() -> Noise {
        Noise {
            enabled: false,
            timer: 0,
            lfsr: 0x7FFF,
            clock_shift: 0,
            width_7bit: false,
            divisor_code: 0,
            length: Length::new(64),
            env: Envelope::default(),
        }
    }

    fn period(&self) -> i32 {
        (NOISE_DIVISOR[self.divisor_code as usize] << self.clock_shift) as i32
    }

    fn tick(&mut self) {
        self.timer -= 1;
        if self.timer <= 0 {
            self.timer = self.period().max(1);
            let xor = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
            self.lfsr >>= 1;
            self.lfsr |= xor << 14;
            if self.width_7bit {
                self.lfsr = (self.lfsr & !(1 << 6)) | (xor << 6);
            }
        }
    }

    fn clock_length(&mut self) {
        if self.length.clock() {
            self.enabled = false;
        }
    }

    fn trigger(&mut self, extra_clock: bool) {
        self.enabled = true;
        self.length.trigger(extra_clock);
        self.timer = self.period().max(1);
        self.lfsr = 0x7FFF;
        self.env.trigger();
        if !self.env.dac_on() {
            self.enabled = false;
        }
    }

    fn power_off(&mut self) {
        self.enabled = false;
        self.timer = 0;
        self.clock_shift = 0;
        self.width_7bit = false;
        self.divisor_code = 0;
        self.env = Envelope::default();
        self.length.enabled = false;
        // LFSR and length counter value survive power-off on DMG
    }

    fn output(&self) -> u8 {
        if !self.enabled || !self.env.dac_on() {
            return 0;
        }
        if self.lfsr & 1 == 0 {
            self.env.volume
        } else {
            0
        }
    }
}

// --- The APU as a whole -------------------------------------------------------

pub struct Apu {
    power: bool,
    ch1: Pulse,
    ch2: Pulse,
    ch3: Wave,
    ch4: Noise,

    nr50: u8, // master volume + VIN
    nr51: u8, // panning

    counter: u16, // mirrors the timer's internal counter for the frame sequencer
    prev_fs_bit: bool,
    frame_step: u8,

    // Fractional accumulator for downsampling CPU_HZ -> SAMPLE_RATE.
    sample_timer: u32,
    pub output: Vec<i16>, // interleaved stereo
}

impl Apu {
    pub fn new() -> Apu {
        Apu {
            power: true,
            ch1: Pulse::new(true),
            ch2: Pulse::new(false),
            ch3: Wave::new(),
            ch4: Noise::new(),
            nr50: 0x77,
            nr51: 0xF3,
            counter: 0xABCC,
            prev_fs_bit: false,
            frame_step: 0,
            sample_timer: 0,
            output: Vec::new(),
        }
    }

    pub fn step(&mut self, cycles: u32) {
        for _ in 0..cycles {
            self.counter = self.counter.wrapping_add(1);
            self.tick_frame_sequencer();

            if self.power {
                self.ch1.tick();
                self.ch2.tick();
                self.ch3.tick();
                self.ch4.tick();
            }

            // Emit a stereo sample at the host rate.
            self.sample_timer += SAMPLE_RATE;
            if self.sample_timer >= CPU_HZ {
                self.sample_timer -= CPU_HZ;
                self.emit_sample();
            }
        }
    }

    /// The frame sequencer advances on the falling edge of counter bit 12.
    fn tick_frame_sequencer(&mut self) {
        let bit = self.counter & FRAME_SEQ_BIT != 0;
        if self.prev_fs_bit && !bit {
            self.advance_frame();
        }
        self.prev_fs_bit = bit;
    }

    /// Called by the MMU when the game writes DIV (0xFF04): the counter resets,
    /// which can itself produce a frame-sequencer falling edge.
    pub fn on_div_reset(&mut self) {
        if self.counter & FRAME_SEQ_BIT != 0 {
            self.advance_frame();
        }
        self.counter = 0;
        self.prev_fs_bit = false;
    }

    fn advance_frame(&mut self) {
        if !self.power {
            return;
        }
        // Steps 0,2,4,6 clock length; 2,6 clock sweep; 7 clocks envelope.
        match self.frame_step {
            0 | 4 => self.clock_lengths(),
            2 | 6 => {
                self.clock_lengths();
                self.ch1.clock_sweep();
            }
            7 => {
                self.ch1.env.clock();
                self.ch2.env.clock();
                self.ch4.env.clock();
            }
            _ => {}
        }
        self.frame_step = (self.frame_step + 1) & 7;
    }

    fn clock_lengths(&mut self) {
        self.ch1.clock_length();
        self.ch2.clock_length();
        self.ch3.clock_length();
        self.ch4.clock_length();
    }

    fn emit_sample(&mut self) {
        let (mut left, mut right) = (0i32, 0i32);
        let chans = [
            (self.ch1.output(), self.ch1.env.dac_on()),
            (self.ch2.output(), self.ch2.env.dac_on()),
            (self.ch3.output(), self.ch3.dac_on),
            (self.ch4.output(), self.ch4.env.dac_on()),
        ];
        for (i, &(digital, dac_on)) in chans.iter().enumerate() {
            // Three cases, and the middle one is easy to lose. A powered DAC
            // maps digital 0..15 to a signed swing, so a channel that is merely
            // *disabled* still sits at the bottom rail (-15) rather than at
            // silence. A channel whose DAC is *off* is disconnected from the
            // mixer entirely and contributes nothing at all.
            //
            // Collapsing those two into "digital 0" is what made a fully quiet
            // APU emit a rock-solid -15 per channel: with all four routed at
            // NR50 volume 7 that is a constant -19200, 59% of full scale, held
            // for as long as the game is silent. Every interruption of the
            // stream (pause, resume, underrun) then stepped to or from it and
            // clicked. Gambatte does the same thing we do here: see
            // `Channel1::update`, `dacIsOn() ? soBaseVol & soMask_ : 0`.
            let sample = if dac_on { digital as i32 * 2 - 15 } else { 0 };
            if self.nr51 & (1 << i) != 0 {
                right += sample;
            }
            if self.nr51 & (1 << (i + 4)) != 0 {
                left += sample;
            }
        }
        let left_vol = ((self.nr50 >> 4) & 7) as i32 + 1;
        let right_vol = (self.nr50 & 7) as i32 + 1;
        // 4 channels * 15 * 8 (volume) -> scale into i16 with headroom.
        let scale = 40;
        self.output.push((left * left_vol * scale) as i16);
        self.output.push((right * right_vol * scale) as i16);
    }

    // --- Register interface (0xFF10..=0xFF3F) -------------------------------

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF10 => self.ch1_sweep_reg() | 0x80,
            0xFF11 => (self.ch1.duty << 6) | 0x3F,
            0xFF12 => self.ch1.env.read_reg(),
            0xFF13 => 0xFF,
            0xFF14 => (self.ch1.length.enabled as u8) << 6 | 0xBF,
            0xFF16 => (self.ch2.duty << 6) | 0x3F,
            0xFF17 => self.ch2.env.read_reg(),
            0xFF18 => 0xFF,
            0xFF19 => (self.ch2.length.enabled as u8) << 6 | 0xBF,
            0xFF1A => (self.ch3.dac_on as u8) << 7 | 0x7F,
            0xFF1B => 0xFF,
            0xFF1C => (self.ch3.volume_code << 5) | 0x9F,
            0xFF1D => 0xFF,
            0xFF1E => (self.ch3.length.enabled as u8) << 6 | 0xBF,
            0xFF20 => 0xFF,
            0xFF21 => self.ch4.env.read_reg(),
            0xFF22 => (self.ch4.clock_shift << 4) | (self.ch4.width_7bit as u8) << 3 | self.ch4.divisor_code,
            0xFF23 => (self.ch4.length.enabled as u8) << 6 | 0xBF,
            0xFF24 => self.nr50,
            0xFF25 => self.nr51,
            0xFF26 => self.read_nr52(),
            0xFF30..=0xFF3F => self.ch3.ram[(addr - 0xFF30) as usize],
            _ => 0xFF,
        }
    }

    fn ch1_sweep_reg(&self) -> u8 {
        (self.ch1.sweep_period << 4) | (self.ch1.sweep_negate as u8) << 3 | self.ch1.sweep_shift
    }

    fn read_nr52(&self) -> u8 {
        let mut v = (self.power as u8) << 7 | 0x70;
        if self.ch1.enabled {
            v |= 1;
        }
        if self.ch2.enabled {
            v |= 2;
        }
        if self.ch3.enabled {
            v |= 4;
        }
        if self.ch4.enabled {
            v |= 8;
        }
        v
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        // NR52 power and wave RAM are always writable; other registers are
        // frozen while the APU is powered off.
        if addr == 0xFF26 {
            self.write_nr52(val);
            return;
        }
        if (0xFF30..=0xFF3F).contains(&addr) {
            self.ch3.ram[(addr - 0xFF30) as usize] = val;
            return;
        }
        if !self.power {
            // On DMG the length registers stay writable while off.
            match addr {
                0xFF11 => self.ch1.length.set_from_reg((val & 0x3F) as u16),
                0xFF16 => self.ch2.length.set_from_reg((val & 0x3F) as u16),
                0xFF1B => self.ch3.length.set_from_reg(val as u16),
                0xFF20 => self.ch4.length.set_from_reg((val & 0x3F) as u16),
                _ => {}
            }
            return;
        }

        match addr {
            // CH1
            0xFF10 => {
                self.ch1.sweep_period = (val >> 4) & 7;
                let neg = val & 0x08 != 0;
                // Clearing negate after it was used mid-sweep disables the channel.
                if self.ch1.sweep_negate && !neg && self.ch1.sweep_did_negate {
                    self.ch1.enabled = false;
                }
                self.ch1.sweep_negate = neg;
                self.ch1.sweep_shift = val & 0x07;
            }
            0xFF11 => {
                self.ch1.duty = val >> 6;
                self.ch1.length.set_from_reg((val & 0x3F) as u16);
            }
            0xFF12 => {
                self.ch1.env.write_reg(val);
                if !self.ch1.env.dac_on() {
                    self.ch1.enabled = false;
                }
            }
            0xFF13 => self.ch1.freq = (self.ch1.freq & 0x700) | val as u16,
            0xFF14 => {
                self.ch1.freq = (self.ch1.freq & 0xFF) | ((val as u16 & 7) << 8);
                let extra = !self.frame_step_clocks_length();
                self.write_length_enable(0, val);
                if val & 0x80 != 0 {
                    self.ch1.trigger(extra);
                }
            }
            // CH2
            0xFF16 => {
                self.ch2.duty = val >> 6;
                self.ch2.length.set_from_reg((val & 0x3F) as u16);
            }
            0xFF17 => {
                self.ch2.env.write_reg(val);
                if !self.ch2.env.dac_on() {
                    self.ch2.enabled = false;
                }
            }
            0xFF18 => self.ch2.freq = (self.ch2.freq & 0x700) | val as u16,
            0xFF19 => {
                self.ch2.freq = (self.ch2.freq & 0xFF) | ((val as u16 & 7) << 8);
                let extra = !self.frame_step_clocks_length();
                self.write_length_enable(1, val);
                if val & 0x80 != 0 {
                    self.ch2.trigger(extra);
                }
            }
            // CH3
            0xFF1A => {
                self.ch3.dac_on = val & 0x80 != 0;
                if !self.ch3.dac_on {
                    self.ch3.enabled = false;
                }
            }
            0xFF1B => self.ch3.length.set_from_reg(val as u16),
            0xFF1C => self.ch3.volume_code = (val >> 5) & 3,
            0xFF1D => self.ch3.freq = (self.ch3.freq & 0x700) | val as u16,
            0xFF1E => {
                self.ch3.freq = (self.ch3.freq & 0xFF) | ((val as u16 & 7) << 8);
                let extra = !self.frame_step_clocks_length();
                self.write_length_enable(2, val);
                if val & 0x80 != 0 {
                    self.ch3.trigger(extra);
                }
            }
            // CH4
            0xFF20 => self.ch4.length.set_from_reg((val & 0x3F) as u16),
            0xFF21 => {
                self.ch4.env.write_reg(val);
                if !self.ch4.env.dac_on() {
                    self.ch4.enabled = false;
                }
            }
            0xFF22 => {
                self.ch4.clock_shift = val >> 4;
                self.ch4.width_7bit = val & 0x08 != 0;
                self.ch4.divisor_code = val & 0x07;
            }
            0xFF23 => {
                let extra = !self.frame_step_clocks_length();
                self.write_length_enable(3, val);
                if val & 0x80 != 0 {
                    self.ch4.trigger(extra);
                }
            }
            0xFF24 => self.nr50 = val,
            0xFF25 => self.nr51 = val,
            _ => {}
        }
    }

    /// Enabling the length counter in certain frame-sequencer steps causes an
    /// immediate extra length clock (the "extra length" quirk).
    fn write_length_enable(&mut self, chan: usize, val: u8) {
        let enable = val & 0x40 != 0;
        let extra_clock = !self.frame_step_clocks_length() && enable;
        let (length, disabled) = match chan {
            0 => (&mut self.ch1.length, &mut self.ch1.enabled),
            1 => (&mut self.ch2.length, &mut self.ch2.enabled),
            2 => (&mut self.ch3.length, &mut self.ch3.enabled),
            _ => (&mut self.ch4.length, &mut self.ch4.enabled),
        };
        let was_enabled = length.enabled;
        length.enabled = enable;
        if extra_clock && !was_enabled && length.counter > 0 {
            length.counter -= 1;
            if length.counter == 0 && !(val & 0x80 != 0) {
                *disabled = false;
            }
        }
    }

    /// True on the frame steps that themselves clock the length counters.
    fn frame_step_clocks_length(&self) -> bool {
        matches!(self.frame_step, 0 | 2 | 4 | 6)
    }

    fn write_nr52(&mut self, val: u8) {
        let on = val & 0x80 != 0;
        if !on && self.power {
            // Powering off zeros every register (but keeps length counters and
            // wave RAM on DMG).
            self.ch1.power_off();
            self.ch2.power_off();
            self.ch3.power_off();
            self.ch4.power_off();
            self.nr50 = 0;
            self.nr51 = 0;
            self.power = false;
        } else if on && !self.power {
            self.power = true;
            self.frame_step = 0;
        }
    }

    pub fn take_output(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.output)
    }

    pub(crate) fn transfer<C: Cursor>(&mut self, c: &mut C) {
        c.bool(&mut self.power);
        self.ch1.transfer(c);
        self.ch2.transfer(c);
        self.ch3.transfer(c);
        self.ch4.transfer(c);
        c.u8(&mut self.nr50);
        c.u8(&mut self.nr51);
        c.u16(&mut self.counter);
        c.bool(&mut self.prev_fs_bit);
        c.u8(&mut self.frame_step);
        c.u32(&mut self.sample_timer);
        // `output` is transient audio and is not part of the save state.
    }
}

impl Length {
    fn transfer<C: Cursor>(&mut self, c: &mut C) {
        c.bool(&mut self.enabled);
        c.u16(&mut self.counter);
        c.u16(&mut self.max);
    }
}

impl Envelope {
    fn transfer<C: Cursor>(&mut self, c: &mut C) {
        c.u8(&mut self.initial);
        c.bool(&mut self.add);
        c.u8(&mut self.period);
        c.u8(&mut self.volume);
        c.u8(&mut self.timer);
    }
}

impl Pulse {
    fn transfer<C: Cursor>(&mut self, c: &mut C) {
        c.bool(&mut self.enabled);
        c.u8(&mut self.duty);
        c.u8(&mut self.duty_pos);
        c.u16(&mut self.freq);
        c.i32(&mut self.timer);
        self.length.transfer(c);
        self.env.transfer(c);
        // has_sweep is structural (fixed per channel), so it is not transferred.
        c.u8(&mut self.sweep_period);
        c.bool(&mut self.sweep_negate);
        c.u8(&mut self.sweep_shift);
        c.u8(&mut self.sweep_timer);
        c.bool(&mut self.sweep_enabled);
        c.u16(&mut self.sweep_shadow);
        c.bool(&mut self.sweep_did_negate);
    }
}

impl Wave {
    fn transfer<C: Cursor>(&mut self, c: &mut C) {
        c.bool(&mut self.enabled);
        c.bool(&mut self.dac_on);
        c.u16(&mut self.freq);
        c.i32(&mut self.timer);
        c.u8(&mut self.position);
        c.u8(&mut self.volume_code);
        self.length.transfer(c);
        c.bytes(&mut self.ram);
        c.u8(&mut self.sample_buffer);
    }
}

impl Noise {
    fn transfer<C: Cursor>(&mut self, c: &mut C) {
        c.bool(&mut self.enabled);
        c.i32(&mut self.timer);
        c.u16(&mut self.lfsr);
        c.u8(&mut self.clock_shift);
        c.bool(&mut self.width_7bit);
        c.u8(&mut self.divisor_code);
        self.length.transfer(c);
        self.env.transfer(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quiet APU must emit true silence, not a DC rail.
    ///
    /// The DAC of a channel is powered by the top 5 bits of its envelope
    /// register, so at reset all four are off, which on hardware disconnects
    /// them from the mixer. If instead they are treated as "digital 0" they each
    /// sit at the bottom of the DAC's swing (-15) and the mixer sums a large
    /// constant: with all four routed by NR51 at NR50 volume 7 that is -19200,
    /// 59% of full scale, held for as long as the game is silent. Nothing about
    /// it is audible on its own, which is why it survived: it only announces
    /// itself as a click when the stream starts or stops, so pausing or resuming
    /// the emulator popped.
    #[test]
    fn a_quiet_apu_emits_silence_not_a_dc_rail() {
        let mut apu = Apu::new();
        // Loudest, everything routed to both sides: the worst case for the bug
        // and the exact configuration Super Mario Land leaves behind.
        apu.write(0xFF24, 0x77);
        apu.write(0xFF25, 0xFF);
        assert!(apu.power, "precondition: APU is powered");
        for ch in [
            apu.ch1.env.dac_on(),
            apu.ch2.env.dac_on(),
            apu.ch3.dac_on,
            apu.ch4.env.dac_on(),
        ] {
            assert!(!ch, "precondition: every DAC is off at reset");
        }

        apu.step(CPU_HZ / 100); // 10 ms, ~441 stereo samples
        let out = apu.take_output();
        assert!(!out.is_empty(), "precondition: samples were produced");
        assert!(
            out.iter().all(|&s| s == 0),
            "quiet APU emitted a DC level: min {:?} max {:?}",
            out.iter().min(),
            out.iter().max()
        );
    }

    /// The counterpart, so the fix above cannot be "return 0 more often": a
    /// channel that is *disabled* while its DAC is still powered does sit at the
    /// bottom rail. Hardware only disconnects on DAC-off, and gambatte models it
    /// the same way (`Channel1::update`: `master_ ? ... : outLow`).
    #[test]
    fn a_powered_dac_still_rails_while_the_channel_is_disabled() {
        let mut apu = Apu::new();
        apu.write(0xFF24, 0x77);
        apu.write(0xFF25, 0xFF);
        // NR12: initial volume 15, no envelope. Powers CH1's DAC without
        // triggering the channel, so it stays disabled.
        apu.write(0xFF12, 0xF0);
        assert!(apu.ch1.env.dac_on(), "precondition: CH1 DAC is on");
        assert!(!apu.ch1.enabled, "precondition: CH1 is still disabled");

        apu.step(CPU_HZ / 100);
        let out = apu.take_output();
        // -15 from CH1 only, times the NR50 volume (7 + 1), times the mixer
        // scale of 40.
        assert!(
            out.iter().all(|&s| s == -15 * 8 * 40),
            "expected the bottom rail from one powered DAC, got min {:?} max {:?}",
            out.iter().min(),
            out.iter().max()
        );
    }
}
