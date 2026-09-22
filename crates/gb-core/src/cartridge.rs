//! Cartridge loading and Memory Bank Controller (MBC) emulation.
//!
//! A Game Boy cartridge is ROM plus (optionally) a bank controller chip and
//! battery-backed RAM. The MBC intercepts writes to the ROM address space and
//! interprets them as bank-switching commands. We support the two most common
//! cases: no MBC (32 KiB flat ROM) and MBC1.

/// The cartridge header lives at 0x0100..=0x014F. We only pull out the fields
/// we actually need to configure the mapper.
#[derive(Debug, Clone)]
pub struct Header {
    pub title: String,
    pub mbc_kind: MbcKind,
    pub rom_banks: usize,
    pub ram_banks: usize,
    pub has_battery: bool,
    /// MBC3 carts with the on-board timer chip (cart types 0x0F / 0x10) carry a
    /// real-time clock. Only these expose the RTC registers.
    pub has_rtc: bool,
    pub cgb_flag: u8,
    /// SGB support flag (0x146): 0x03 means the cart carries SGB commands.
    /// Parsed but currently unused: SGB detection is disabled (see `Mmu::new`),
    /// so mono SGB carts run as plain DMG with our GBC-auto colorization.
    #[allow(dead_code)]
    pub sgb_flag: u8,
    /// Sum of the title bytes (0x134..=0x143); the CGB boot ROM uses this to
    /// pick a colorization palette, and we reuse it for `Colorize::Auto`.
    pub title_checksum: u8,
    /// The 4th title byte (0x137), used to disambiguate colliding checksums.
    pub title_fourth: u8,
    /// Whether the game is Nintendo-published (only those get boot-ROM palettes).
    pub nintendo_licensed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbcKind {
    None,
    Mbc1,
    Mbc2,
    Mbc3,
    Mbc5,
    Huc1,
    Huc3,
    /// The Game Boy Camera's MAC-GBD. A mapper with an image sensor on it.
    Camera,
    /// Kirby Tilt 'n' Tumble's mapper: a two-axis accelerometer and a
    /// serial EEPROM instead of ordinary cartridge RAM.
    Mbc7,
    Unsupported(u8),
}

/// What mapper a cartridge type byte means, and whether it has a battery.
///
/// Split out of `Header::parse` so that it is askable without a whole ROM. The
/// compatibility smoke tester used to carry its own copy of this list, with a
/// comment saying "mirror cartridge.rs", and it went stale the moment the Game
/// Boy Camera was added: it reported a supported cartridge as unsupported. Two
/// lists that have to agree eventually do not.
fn mbc_kind_of(cart_type: u8) -> (MbcKind, bool) {
    match cart_type {
        0x00 => (MbcKind::None, false),
        0x01 => (MbcKind::Mbc1, false),
        0x02 => (MbcKind::Mbc1, false),
        0x03 => (MbcKind::Mbc1, true),
        0x05 => (MbcKind::Mbc2, false),
        0x06 => (MbcKind::Mbc2, true),
        0x0F..=0x13 => (MbcKind::Mbc3, matches!(cart_type, 0x0F | 0x10 | 0x13)),
        0x19..=0x1E => (MbcKind::Mbc5, matches!(cart_type, 0x1B | 0x1E)),
        // The Game Boy Camera: 1 MB ROM, 128 KB battery RAM for the photo
        // album, and an M64282FP sensor reachable through the RAM window.
        // MBC7: tilt sensor plus a 93LC56 EEPROM. The battery is on the
        // EEPROM rather than on RAM, and the header declares NO cartridge
        // RAM at all ($0149 is $00), which is why the save has to be
        // allocated from the mapper rather than from the size byte.
        0x22 => (MbcKind::Mbc7, true),
        0xFC => (MbcKind::Camera, true),
        0xFE => (MbcKind::Huc3, true), // HuC3: RAM + RTC + battery
        0xFF => (MbcKind::Huc1, true), // HuC1: RAM + battery (+ IR)
        other => (MbcKind::Unsupported(other), false),
    }
}


/// Is this cartridge type one the core actually maps to a real mapper?
///
/// The one place to ask. A cartridge we do not map gets treated as if it had no
/// mapper at all, which for a banked ROM means garbage on screen rather than an
/// error, so anything that wants to warn about that needs this answer and must
/// not keep its own copy of it.
pub fn mapper_is_supported(cart_type: u8) -> bool {
    !matches!(mbc_kind_of(cart_type).0, MbcKind::Unsupported(_))
}

impl Header {
    fn parse(rom: &[u8]) -> Header {
        let title = rom[0x0134..=0x0143]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect::<String>();

        let cart_type = rom[0x0147];
        let (mbc_kind, has_battery) = mbc_kind_of(cart_type);
        // Cart types 0x0F (MBC3+TIMER+BATTERY) and 0x10 (MBC3+TIMER+RAM+BATTERY)
        // are the only ones with the RTC crystal.
        let has_rtc = matches!(cart_type, 0x0F | 0x10);

        // ROM size: 32 KiB << N gives the total size, i.e. 2 << N banks of 16 KiB.
        let rom_banks = 2usize << rom[0x0148];

        // RAM size code -> number of 8 KiB banks.
        let ram_banks = match rom[0x0149] {
            0x00 => 0,
            0x01 => 1, // 2 KiB (a partial bank); we round up to one bank
            0x02 => 1,
            0x03 => 4,
            0x04 => 16,
            0x05 => 8,
            _ => 0,
        };

        Header {
            title,
            mbc_kind,
            rom_banks,
            ram_banks,
            has_battery,
            has_rtc,
            cgb_flag: rom[0x0143],
            sgb_flag: rom[0x0146],
            title_checksum: rom[0x0134..=0x0143]
                .iter()
                .fold(0u8, |acc, &b| acc.wrapping_add(b)),
            title_fourth: rom[0x0137],
            // The boot ROM only assigns palettes to Nintendo-published games:
            // old licensee 0x33 -> new licensee (0x144/0x145) must be "01",
            // otherwise old licensee must be 0x01.
            nintendo_licensed: if rom[0x014B] == 0x33 {
                rom[0x0144] == b'0' && rom[0x0145] == b'1'
            } else {
                rom[0x014B] == 0x01
            },
        }
    }
}

pub struct Cartridge {
    pub header: Header,
    rom: Vec<u8>,
    ram: Vec<u8>,
    mbc: Mbc,
}

/// Per-mapper mutable state.
/// The camera half of the Game Boy Camera's mapper.
///
/// The sensor is not on the link port; it is on the cartridge bus, reached by
/// writing a RAM bank number with bit 4 set and then talking to $A000 onwards.
/// So none of the printer's machinery applies here: this is a mapper with an
/// image sensor bolted to it.
///
/// Registers, from the published mapper documentation:
///
/// ```text
///   A000        trigger and status. Writing bit 0 starts a capture; reading
///               bit 0 gives 1 while the hardware is working. Only the low
///               three bits exist; the rest read as 0.
///   A001        sensor gain and edge operation mode
///   A002-A003   exposure time, 16 bit, MSB first
///   A004        output voltage reference, edge enhancement ratio, invert
///   A005        output reference voltage and zero point calibration
///   A006-A035   a 4 by 4 dither matrix, three bytes per element
/// ```
///
/// Everything except $A000 is write only and reads back $00. The whole block is
/// mirrored every $80 bytes.
#[derive(Clone)]
struct Cam {
    /// The low three bits of $A000. Bit 0 is the capture trigger and the busy
    /// flag, which are the same bit.
    trigger: u8,
    /// $A001 to $A035: the sensor configuration and the dither matrix.
    regs: [u8; 0x35],
    /// Wall-clock T-cycles left of an exposure. A capture that never finished
    /// would leave a game spinning on the busy bit forever.
    busy: i32,
    /// What the lens is pointed at: `CAMERA_W * CAMERA_H` greyscale bytes, 0 is
    /// black. `None` means nothing is feeding us light, and captures develop a
    /// self-describing test card instead. See `camera_develop`.
    frame: Option<Vec<u8>>,
    /// Does the frontend have a camera at all?
    ///
    /// This tells apart two failures that otherwise look identical: a
    /// frontend that never implemented the camera interface, and one whose
    /// camera has not produced a frame yet or was refused permission. Both
    /// leave `frame` as `None`, and without this the player and the developer
    /// see the same picture for a bug and for a prompt.
    sensor_available: bool,
}

/// How long a capture is reported as busy.
///
/// The real exposure is set by $A002-$A003 and varies with the light; a
/// hundredth of a second is a plausible middle and, more importantly, is
/// guaranteed to end. Wiring this to the exposure registers belongs with the
/// sensor model, not with the mapper.
const CAMERA_CAPTURE_CYCLES: i32 = (CYCLES_PER_SECOND / 100) as i32;

/// Where the captured image lands in cartridge RAM, in bank 0.
const CAMERA_IMAGE_OFFSET: usize = 0x0100;

/// The sensor is 128 by 128, but the controller throws away the first eight rows
/// and the last eight, so what reaches the cartridge is 128 by 112.
pub const CAMERA_W: usize = 128;
pub const CAMERA_H: usize = 112;

/// What the sensor sees when nothing is feeding it light.
///
/// This is a **diagnostic**, not a placeholder, and the difference matters. A
/// frontend that has not implemented the camera interface leaves the core with
/// no frames, and that failure looks exactly like a broken sensor model: the
/// picture is wrong and the cause is in somebody else's repository. So the
/// no-signal image is deliberately unmistakable rather than plausible.
///
/// There are TWO of these on purpose. A frontend with no camera interface and
/// a frontend whose camera has not produced a frame yet both leave the core
/// with nothing, and those need completely different responses: one is a bug
/// to file against the frontend, the other is a permission prompt to put in
/// front of the player. One image for both means whoever sees it cannot tell
/// which they are looking at, which defeats the point of a diagnostic.
///
/// `no_camera_card` is four hard-edged vertical bars cut by a diagonal.
/// `waiting_for_light_card` is concentric rings: ROUND rather than straight,
/// so the two are distinguishable at a glance and in a blurry photograph of a
/// screen, which is how these are usually reported.
///
/// Both are deliberately unmistakable rather than plausible, and both exercise
/// all four shades and the whole dither matrix.
fn no_camera_card(x: usize, y: usize) -> u8 {
    let bar = (x * 4 / CAMERA_W).min(3);
    let level = [30u8, 100, 170, 240][bar];
    if (x + y) % 32 < 3 {
        255 - level
    } else {
        level
    }
}

fn waiting_for_light_card(x: usize, y: usize) -> u8 {
    let dx = x as i32 - CAMERA_W as i32 / 2;
    let dy = y as i32 - CAMERA_H as i32 / 2;
    let r = ((dx * dx + dy * dy) as f64).sqrt() as usize;
    [240u8, 170, 100, 30][(r / 9) % 4]
}

impl Cam {
    fn new() -> Cam {
        Cam {
            trigger: 0,
            regs: [0; 0x35],
            busy: 0,
            frame: None,
            sensor_available: false,
        }
    }

    fn read(&self, addr: u16) -> u8 {
        // Mirrored every $80 bytes.
        let i = (addr as usize - 0xA000) % 0x80;
        if i == 0 {
            // Only the low three bits are real, and bit 0 is the BUSY flag on
            // the way out even though it is the TRIGGER on the way in. Keeping
            // the bit the game wrote would leave it set forever and the game
            // polls this in a tight loop waiting for it to clear: mask it off
            // and let only the capture decide.
            (self.trigger & 0x06) | if self.busy > 0 { 1 } else { 0 }
        } else {
            // Every other register is write only.
            0x00
        }
    }

    /// Returns true if this write started a capture.
    fn write(&mut self, addr: u16, val: u8) -> bool {
        let i = (addr as usize - 0xA000) % 0x80;
        if i == 0 {
            self.trigger = val & 0x07;
            // Only a write with bit 0 set triggers; anything else is an
            // ordinary write to the register.
            if val & 1 != 0 {
                self.busy = CAMERA_CAPTURE_CYCLES;
                return true;
            }
            return false;
        }
        if i <= 0x35 {
            self.regs[i - 1] = val;
        }
        false
    }

    /// The 16-bit exposure time from $A002 (high) and $A003 (low).
    ///
    /// The game drives this constantly: its own auto-exposure loop hunts for a
    /// level and the BRIGHTNESS slider biases the hunt. Measured on the real
    /// cartridge, brightness at maximum pins it to $FFFF.
    fn exposure(&self) -> u16 {
        u16::from_be_bytes([self.regs[1], self.regs[2]])
    }

    /// The three dither thresholds for a pixel, ascending.
    ///
    /// $A006-$A035 is a 4 by 4 matrix with three bytes per element, so a pixel's
    /// thresholds come from its position modulo four. The three are ascending and
    /// **their spread is the contrast**: measured on the real cartridge a neutral
    /// setting gives $89 $92 $A2 for a cell, and winding contrast up gives
    /// $84 $96 $CA for the same one. The game recomputes the whole matrix when
    /// either slider moves, which is why honouring this one table makes both
    /// sliders do something real without interpreting either of them.
    fn thresholds(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let cell = (y & 3) * 4 + (x & 3);
        let at = 5 + cell * 3; // regs[0] is $A001, so $A006 is regs[5]
        match (self.regs.get(at), self.regs.get(at + 1), self.regs.get(at + 2)) {
            (Some(&a), Some(&b), Some(&c)) => (a, b, c),
            _ => (0x55, 0x80, 0xAA),
        }
    }

    fn tick(&mut self, cycles: u32) {
        if self.busy > 0 {
            self.busy -= cycles as i32;
            if self.busy < 0 {
                self.busy = 0;
            }
        }
    }
}

/// The MBC7's two-axis accelerometer.
///
/// Both axes read `$8000` at rest and before they have ever been latched, and
/// Earth's gravity moves a value by about `$70`. The game latches by writing
/// `$55` to `$Ax0x` and then `$AA` to `$Ax1x`; anything else leaves the last
/// latched reading in place, so a game that forgets to latch keeps reading the
/// same numbers rather than seeing them drift.
#[derive(Clone)]
struct Accel {
    /// What the host says the console is tilted by, in g.
    tilt_x: f32,
    tilt_y: f32,
    /// The latched values the game actually reads.
    x: u16,
    y: u16,
    /// Has `$55` been seen, so that `$AA` will latch?
    armed: bool,
}

/// One g, as the accelerometer reports it.
const ACCEL_G: f32 = 0x70 as f32;
/// Both axes with the console held level.
///
/// NOT $8000, which is the tempting value and the wrong one. $8000 is what the
/// registers read *before the first latch* and after an erase; a latched
/// reading of a level console is $81D0. Getting this wrong is silent: the
/// mapper works, the game polls it once a frame and reads exactly the numbers
/// handed to it, and nothing moves, because every reading is 464 counts from
/// where the game calibrated and 464 counts is four g of impossible tilt.
const ACCEL_REST: u16 = 0x81D0;
/// What the registers read before the first latch, and after an erase.
const ACCEL_ERASED: u16 = 0x8000;

impl Accel {
    fn new() -> Accel {
        Accel {
            tilt_x: 0.0,
            tilt_y: 0.0,
            x: ACCEL_ERASED,
            y: ACCEL_ERASED,
            armed: false,
        }
    }

    fn latch(&mut self) {
        let map = |g: f32| -> u16 {
            // Negated, and measured rather than assumed. The register reads
            // BELOW rest as the console tilts toward the positive screen axes:
            // driving X below $81D0 rolls Kirby right, and Y below it rolls
            // him down. Handing the raw sign straight through would give a
            // core that is demonstrably working and plays backwards.
            let v = ACCEL_REST as f32 - g * ACCEL_G;
            v.clamp(0.0, 65535.0) as u16
        };
        self.x = map(self.tilt_x);
        self.y = map(self.tilt_y);
    }
}

/// The 93LC56 serial EEPROM: 128 words of 16 bits, which is the 256 bytes of
/// save this cartridge has instead of RAM.
///
/// The game bit-bangs it through one byte at `$Ax8x`:
///
/// ```text
///   bit 7  CS    chip select
///   bit 6  CLK   clock; the state machine advances on a RISING edge
///   bit 1  DI    data in, from the game
///   bit 0  DO    data out, to the game
/// ```
///
/// A command is a start bit, then two opcode bits, then eight address bits of
/// which only the low seven are significant, because 128 words need seven. A
/// write follows with sixteen data bits. Opcode `00` is the odd one out: its
/// top two address bits pick between enabling writes, disabling them, and the
/// bulk operations.
///
/// Writes are ignored unless they have been enabled, which is not a detail to
/// skip: the game disables them again after saving, and a mapper that always
/// allowed writes would let a crash scribble on the save.
#[derive(Clone)]
struct Eeprom {
    cs: bool,
    clk: bool,
    /// The last DI the game drove. Kept because the game reads this port back
    /// and rewrites it with one bit changed, so inventing a value here feeds a
    /// bit of our own into its next clock.
    di: bool,
    do_bit: bool,
    /// Bits shifted in since chip select rose.
    shift: u32,
    bits: u8,
    /// What is being done, once the command has been decoded.
    state: EeState,
    addr: usize,
    /// Bits waiting to be shifted out, most significant first.
    out: u32,
    out_bits: u8,
    write_enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EeState {
    /// Waiting for the start bit.
    Idle,
    /// Collecting opcode and address.
    Command,
    /// Collecting the sixteen data bits of a write.
    WriteData,
    /// Shifting a word out.
    Reading,
}

impl Eeprom {
    fn new() -> Eeprom {
        Eeprom {
            cs: false,
            clk: false,
            di: false,
            do_bit: true,
            shift: 0,
            bits: 0,
            state: EeState::Idle,
            addr: 0,
            out: 0,
            out_bits: 0,
            write_enabled: false,
        }
    }

    fn read(&self) -> u8 {
        // CS, CLK and DI read back as written; DO is ours.
        let mut v = 0u8;
        if self.cs {
            v |= 0x80;
        }
        if self.clk {
            v |= 0x40;
        }
        if self.di {
            v |= 0x02;
        }
        if self.do_bit {
            v |= 0x01;
        }
        v
    }

    /// `ram` is the 256 bytes of save, read and written as big-endian words.
    fn write(&mut self, val: u8, ram: &mut [u8]) {
        let cs = val & 0x80 != 0;
        let clk = val & 0x40 != 0;
        let di = val & 0x02 != 0;
        self.di = di;

        if !cs {
            // Chip deselected: abandon whatever was in progress. The enable
            // latch survives, which is what the real part does.
            self.cs = false;
            self.clk = clk;
            self.state = EeState::Idle;
            self.bits = 0;
            self.shift = 0;
            self.do_bit = true;
            return;
        }

        let rising = cs && clk && !self.clk;
        self.cs = cs;
        self.clk = clk;
        if !rising {
            return;
        }

        match self.state {
            EeState::Idle => {
                // Leading zeroes are ignored; a one is the start bit.
                if di {
                    self.state = EeState::Command;
                    self.shift = 0;
                    self.bits = 0;
                }
            }
            EeState::Command => {
                self.shift = (self.shift << 1) | di as u32;
                self.bits += 1;
                if self.bits == 10 {
                    self.decode(ram);
                }
            }
            EeState::WriteData => {
                self.shift = (self.shift << 1) | di as u32;
                self.bits += 1;
                if self.bits == 16 {
                    if self.write_enabled {
                        let w = self.shift as u16;
                        let at = (self.addr & 0x7F) * 2;
                        if at + 1 < ram.len() {
                            ram[at] = (w >> 8) as u8;
                            ram[at + 1] = w as u8;
                        }
                    }
                    self.state = EeState::Idle;
                    self.bits = 0;
                    self.shift = 0;
                    // Ready again, which the game polls for.
                    self.do_bit = true;
                }
            }
            EeState::Reading => {
                self.out_bits = self.out_bits.saturating_sub(1);
                self.do_bit = (self.out >> self.out_bits) & 1 != 0;
                if self.out_bits == 0 {
                    self.state = EeState::Idle;
                }
            }
        }
    }

    /// Ten bits gathered: two of opcode and eight of address.
    fn decode(&mut self, ram: &mut [u8]) {
        let op = (self.shift >> 8) & 0b11;
        let addr = (self.shift & 0xFF) as usize;
        self.addr = addr & 0x7F;
        self.bits = 0;
        self.shift = 0;

        match op {
            0b10 => {
                // READ. A dummy zero leads the word out, which is why the
                // shift register is seventeen bits wide here.
                let at = self.addr * 2;
                let w = if at + 1 < ram.len() {
                    ((ram[at] as u32) << 8) | ram[at + 1] as u32
                } else {
                    0xFFFF
                };
                self.out = w << 1;
                self.out_bits = 17;
                self.do_bit = false;
                self.state = EeState::Reading;
            }
            0b01 => {
                self.state = EeState::WriteData;
                self.do_bit = false; // busy until the word lands
            }
            0b11 => {
                // ERASE one word.
                if self.write_enabled {
                    let at = self.addr * 2;
                    if at + 1 < ram.len() {
                        ram[at] = 0xFF;
                        ram[at + 1] = 0xFF;
                    }
                }
                self.state = EeState::Idle;
                self.do_bit = true;
            }
            _ => {
                // Opcode 00: the top two address bits say which.
                match addr >> 6 {
                    0b11 => self.write_enabled = true,  // EWEN
                    0b00 => self.write_enabled = false, // EWDS
                    0b10 => {
                        // ERAL, erase the whole chip.
                        if self.write_enabled {
                            ram.fill(0xFF);
                        }
                    }
                    _ => {
                        // WRAL, write the whole chip. Rare enough that taking
                        // the data and ignoring it is honest: no game we have
                        // uses it, and silently doing nothing is better than
                        // silently doing it wrong.
                        self.state = EeState::WriteData;
                        return;
                    }
                }
                self.state = EeState::Idle;
                self.do_bit = true;
            }
        }
    }
}

enum Mbc {
    None,
    Mbc1 {
        ram_enabled: bool,
        rom_bank: u8, // low 5 bits
        ram_bank: u8, // 2 bits: RAM bank OR upper ROM bank bits
        /// false = ROM banking mode (0), true = RAM banking mode (1).
        banking_mode: bool,
    },
    Mbc2 {
        ram_enabled: bool,
        rom_bank: u8, // 4 bits (1..15)
    },
    Mbc3 {
        ram_enabled: bool,
        rom_bank: u8,  // 7 bits
        ram_bank: u8,  // 0x00-0x03 select a RAM bank; 0x08-0x0C select an RTC register
        has_rtc: bool, // whether this cart has the timer chip
        rtc: Rtc,
    },
    Mbc5 {
        ram_enabled: bool,
        rom_bank: u16, // 9 bits
        ram_bank: u8,  // 4 bits
    },
    Huc1 {
        /// true = the 0xA000 window is the IR port; false = cartridge RAM.
        ir_mode: bool,
        rom_bank: u8, // 6 bits
        ram_bank: u8, // 2 bits
    },
    Huc3 {
        rom_bank: u8, // 7 bits
        ram_bank: u8, // 4 bits
        huc3: Huc3,   // mode register + RTC/config command MCU
    },
    Mbc7 {
        /// MBC7 has TWO enables and wants both: $0A to $0000-$1FFF and $40 to
        /// $4000-$5FFF. One alone leaves the register block dead.
        ram_enable_1: bool,
        ram_enable_2: bool,
        rom_bank: u8, // 7 bits
        accel: Accel,
        eeprom: Eeprom,
    },
    Camera {
        ram_enabled: bool,
        rom_bank: u8, // 6 bits, $00-$3F
        /// $00-$0F picks a RAM bank; **bit 4 set** swaps the whole $A000 window
        /// for the camera registers instead.
        ram_bank: u8,
        cam: Cam,
    },
}

/// Wall-clock T-cycles per real second. The RTC crystal runs at real time, so
/// this is the DMG clock rate and is *not* affected by CGB double-speed (the MMU
/// feeds the RTC the wall-clock-rate cycle count).
const CYCLES_PER_SECOND: u32 = 4_194_304;

/// Bytes appended to the battery RAM to persist the RTC across power cycles.
/// Layout: b"PRTC" + version + S + M + H + days(u16 LE) + flags + sub(u32 LE).
const RTC_FOOTER_LEN: usize = 15;
const RTC_FOOTER_MAGIC: &[u8; 4] = b"PRTC";

/// The MBC3 real-time clock.
///
/// Deterministic by design: it advances from emulated cycles, never the host
/// wall clock, so two networked peers (or a replay) see byte-identical clock
/// state — a hard requirement for this core's netplay / save-state guarantees.
/// The consequence is that real time elapsed while the machine is powered off is
/// not counted; the clock resumes from where it was saved.
///
/// The chip keeps a live counter plus a *latched* snapshot. The game latches
/// (write 0x00 then 0x01 to 0x6000-0x7FFF) to freeze the current time into the
/// snapshot, then reads the snapshot back through the 0xA000 window.
struct Rtc {
    seconds: u8, // 0..59
    minutes: u8, // 0..59
    hours: u8,   // 0..23
    days: u16,   // 9-bit day counter (0..511)
    halted: bool,
    day_carry: bool,
    /// Sub-second accumulator, in wall-clock T-cycles.
    sub: u32,
    /// Latched register snapshot the game reads: [S, M, H, day-low, day-high].
    latch: [u8; 5],
    /// Last value written to the latch register, to detect the 0->1 sequence.
    latch_last: u8,
}

impl Rtc {
    fn new() -> Rtc {
        Rtc {
            seconds: 0,
            minutes: 0,
            hours: 0,
            days: 0,
            halted: false,
            day_carry: false,
            sub: 0,
            latch: [0; 5],
            latch_last: 0xFF,
        }
    }

    /// Advance by `cycles` wall-clock T-cycles. Returns true if at least one whole
    /// second elapsed (so the battery footer needs refreshing).
    fn tick(&mut self, cycles: u32) -> bool {
        if self.halted {
            return false;
        }
        self.sub += cycles;
        let mut advanced = false;
        while self.sub >= CYCLES_PER_SECOND {
            self.sub -= CYCLES_PER_SECOND;
            self.advance_second();
            advanced = true;
        }
        advanced
    }

    fn advance_second(&mut self) {
        self.seconds += 1;
        if self.seconds < 60 {
            return;
        }
        self.seconds = 0;
        self.minutes += 1;
        if self.minutes < 60 {
            return;
        }
        self.minutes = 0;
        self.hours += 1;
        if self.hours < 24 {
            return;
        }
        self.hours = 0;
        self.days += 1;
        if self.days >= 512 {
            self.days = 0;
            self.day_carry = true; // sticky until the game clears bit 7 of DH
        }
    }

    /// Build the five live register bytes (S, M, H, DL, DH).
    fn live_regs(&self) -> [u8; 5] {
        let dh = ((self.day_carry as u8) << 7)
            | ((self.halted as u8) << 6)
            | ((self.days >> 8) as u8 & 0x01);
        [
            self.seconds,
            self.minutes,
            self.hours,
            (self.days & 0xFF) as u8,
            dh,
        ]
    }

    /// Handle a write to the latch register (0x6000-0x7FFF): a 0x00 then 0x01
    /// sequence copies the live time into the latched snapshot.
    fn write_latch(&mut self, val: u8) {
        if self.latch_last == 0 && val == 1 {
            self.latch = self.live_regs();
        }
        self.latch_last = val;
    }

    /// Read a latched RTC register by index (0=S, 1=M, 2=H, 3=DL, 4=DH). Unused
    /// bits read back as 0.
    fn read_reg(&self, index: u8) -> u8 {
        match index {
            0 => self.latch[0] & 0x3F,
            1 => self.latch[1] & 0x3F,
            2 => self.latch[2] & 0x1F,
            3 => self.latch[3],
            4 => self.latch[4] & 0xC1,
            _ => 0xFF,
        }
    }

    /// Write a live RTC register by index. Writing seconds also resets the
    /// sub-second prescaler, as on hardware.
    fn write_reg(&mut self, index: u8, val: u8) {
        match index {
            0 => {
                self.seconds = val & 0x3F;
                self.sub = 0;
            }
            1 => self.minutes = val & 0x3F,
            2 => self.hours = val & 0x1F,
            3 => self.days = (self.days & 0x100) | val as u16,
            4 => {
                self.days = (self.days & 0xFF) | (((val & 0x01) as u16) << 8);
                self.halted = val & 0x40 != 0;
                self.day_carry = val & 0x80 != 0;
            }
            _ => {}
        }
    }

    /// Encode the live time into the battery footer (see [`RTC_FOOTER_LEN`]).
    fn encode_footer(&self) -> [u8; RTC_FOOTER_LEN] {
        let mut f = [0u8; RTC_FOOTER_LEN];
        f[0..4].copy_from_slice(RTC_FOOTER_MAGIC);
        f[4] = 1; // version
        f[5] = self.seconds;
        f[6] = self.minutes;
        f[7] = self.hours;
        f[8..10].copy_from_slice(&self.days.to_le_bytes());
        f[10] = (self.halted as u8) | ((self.day_carry as u8) << 1);
        f[11..15].copy_from_slice(&self.sub.to_le_bytes());
        f
    }

    /// Restore the live time from a battery footer, if it is present and valid.
    /// The latched snapshot is reset to the restored live time.
    fn decode_footer(&mut self, f: &[u8]) {
        if f.len() < RTC_FOOTER_LEN || &f[0..4] != RTC_FOOTER_MAGIC || f[4] != 1 {
            return; // no footer (e.g. an older .srm) — keep the power-on clock
        }
        self.seconds = f[5].min(59);
        self.minutes = f[6].min(59);
        self.hours = f[7].min(23);
        self.days = u16::from_le_bytes([f[8], f[9]]) & 0x1FF;
        self.halted = f[10] & 0x01 != 0;
        self.day_carry = f[10] & 0x02 != 0;
        self.sub = u32::from_le_bytes([f[11], f[12], f[13], f[14]]) % CYCLES_PER_SECOND;
        self.latch = self.live_regs();
    }

    fn transfer<C: crate::save::Cursor>(&mut self, c: &mut C) {
        c.u8(&mut self.seconds);
        c.u8(&mut self.minutes);
        c.u8(&mut self.hours);
        c.u16(&mut self.days);
        c.bool(&mut self.halted);
        c.bool(&mut self.day_carry);
        c.u32(&mut self.sub);
        c.bytes(&mut self.latch);
        c.u8(&mut self.latch_last);
    }
}

/// Wall-clock T-cycles per real minute (the HuC3 clock is minute-resolution).
const CYCLES_PER_MINUTE: u32 = 60 * CYCLES_PER_SECOND;

/// The HuC3 (Hudson) register + real-time-clock controller.
///
/// A write to 0x0000-0x1FFF selects what the 0xA000-0xBFFF window maps to (low
/// nibble): 0x0A = cartridge RAM, 0x0B = write the command mailbox, 0x0C = read
/// the mailbox result, 0x0D = the command semaphore, 0x0E = the IR port. The
/// mailbox is a nibble interface to an MCU that holds the clock: the game writes
/// a command (bits 6-4) + argument (bits 3-0), then requests execution by
/// clearing bit 0 in mode 0x0D. Like the MBC3 RTC this clock is deterministic
/// (cycle-driven), and the hardware exposes it at minute resolution.
struct Huc3 {
    /// 0xA000 window mode: the low nibble of the last 0x0000-0x1FFF write.
    mode: u8,
    command: u8, // last command, bits 2-0 of the mailbox command field
    arg: u8,     // last argument nibble
    result: u8,  // result nibble of the last executed command
    addr: u8,    // register address pointer for read/write commands
    scratch: [u8; 8], // read/write scratch registers 0x00-0x07 (one nibble each)
    minutes: u16, // minutes since day start, 0..1439
    days: u16,    // day counter, 0..4095
    sub: u32,     // sub-minute accumulator, in wall-clock T-cycles
}

impl Huc3 {
    fn new() -> Huc3 {
        Huc3 {
            mode: 0,
            command: 0,
            arg: 0,
            result: 0,
            addr: 0,
            scratch: [0; 8],
            minutes: 0,
            days: 0,
            sub: 0,
        }
    }

    fn tick(&mut self, cycles: u32) {
        self.sub += cycles;
        while self.sub >= CYCLES_PER_MINUTE {
            self.sub -= CYCLES_PER_MINUTE;
            self.minutes += 1;
            if self.minutes >= 1440 {
                self.minutes = 0;
                self.days = (self.days + 1) & 0x0FFF;
            }
        }
    }

    /// Read the MCU register at `addr` (scratch 0x00-0x07 or the live clock
    /// nibbles 0x10-0x15).
    fn read_reg(&self, addr: u8) -> u8 {
        match addr {
            0x00..=0x07 => self.scratch[addr as usize],
            0x10 => (self.minutes & 0xF) as u8,
            0x11 => ((self.minutes >> 4) & 0xF) as u8,
            0x12 => ((self.minutes >> 8) & 0xF) as u8,
            0x13 => (self.days & 0xF) as u8,
            0x14 => ((self.days >> 4) & 0xF) as u8,
            0x15 => ((self.days >> 8) & 0xF) as u8,
            _ => 0,
        }
    }

    /// Execute the mailbox command (triggered by clearing the mode-0x0D bit 0).
    fn execute(&mut self) {
        match self.command {
            1 => {
                // Read register at the pointer, then auto-increment.
                self.result = self.read_reg(self.addr) & 0x0F;
                self.addr = self.addr.wrapping_add(1);
            }
            3 => {
                // Write the argument to the pointer, then auto-increment.
                if (self.addr as usize) < self.scratch.len() {
                    self.scratch[self.addr as usize] = self.arg & 0x0F;
                }
                self.result = self.arg;
                self.addr = self.addr.wrapping_add(1);
            }
            4 => {
                self.addr = (self.addr & 0xF0) | self.arg;
                self.result = self.arg;
            }
            5 => {
                self.addr = (self.addr & 0x0F) | (self.arg << 4);
                self.result = self.arg;
            }
            6 => self.extended(self.arg),
            _ => {}
        }
    }

    fn extended(&mut self, cmd: u8) {
        match cmd {
            0 => {
                // Atomically latch the live clock into the scratch registers.
                self.scratch[0] = (self.minutes & 0xF) as u8;
                self.scratch[1] = ((self.minutes >> 4) & 0xF) as u8;
                self.scratch[2] = ((self.minutes >> 8) & 0xF) as u8;
                self.scratch[3] = (self.days & 0xF) as u8;
                self.scratch[4] = ((self.days >> 4) & 0xF) as u8;
                self.scratch[5] = ((self.days >> 8) & 0xF) as u8;
            }
            1 => {
                // Atomically commit the scratch registers back into the clock.
                self.minutes = (self.scratch[0] as u16)
                    | ((self.scratch[1] as u16) << 4)
                    | ((self.scratch[2] as u16) << 8);
                if self.minutes >= 1440 {
                    self.minutes = 1439;
                }
                self.days = ((self.scratch[3] as u16)
                    | ((self.scratch[4] as u16) << 4)
                    | ((self.scratch[5] as u16) << 8))
                    & 0x0FFF;
            }
            _ => {}
        }
        self.result = 1; // report success / ready
    }

    /// The value read back from the 0xA000 window in a non-RAM mode.
    fn read_window(&self) -> u8 {
        match self.mode {
            0x0B | 0x0C => 0x80 | (self.command << 4) | self.result,
            0x0D => 0x80 | (self.command << 4) | 0x01, // semaphore: always ready
            0x0E => 0x00,                              // IR: no signal
            _ => 0xFF,
        }
    }

    fn transfer<C: crate::save::Cursor>(&mut self, c: &mut C) {
        c.u8(&mut self.mode);
        c.u8(&mut self.command);
        c.u8(&mut self.arg);
        c.u8(&mut self.result);
        c.u8(&mut self.addr);
        c.bytes(&mut self.scratch);
        c.u16(&mut self.minutes);
        c.u16(&mut self.days);
        c.u32(&mut self.sub);
    }
}

impl Cartridge {
    pub fn new(rom: Vec<u8>) -> Cartridge {
        let header = Header::parse(&rom);
        // MBC2 has 512 x 4-bit of built-in RAM (no external banks); everything
        // else uses the header-declared bank count.
        let ram = if header.mbc_kind == MbcKind::Mbc2 {
            vec![0u8; 512]
        } else if header.mbc_kind == MbcKind::Mbc7 {
            // MBC7's save is a 93LC56 EEPROM: 128 words of 16 bits, and the
            // header declares no cartridge RAM at all. Sized exactly, because
            // this is the file the frontend writes out, and every other
            // emulator of this mapper produces 256 bytes. An 8 KiB save padded
            // with zeroes would not load anywhere else.
            //
            // Zeroed, NOT erased-to-ones, and that is measured rather than
            // reasoned. An erased 93LC56 reads $FF, so all-ones looks like the
            // honest hardware answer. It is not what this game wants: filled
            // with $FF, Kirby's file select shows three fabricated saves
            // reading "LEVEL 8-4 255%", and it does not offer to format them.
            // Filled with $00 it shows "NO DATA", which is what somebody
            // opening a new cartridge sees.
            //
            // So the game treats zero as empty and anything else as a save,
            // and a real cartridge cannot have shipped reading $FF. Beyond
            // looking wrong, a fabricated 100%-complete file is exactly the
            // kind of garbage state that false-unlocks achievements.
            vec![0x00u8; 256]
        } else {
            // RTC carts append a footer past the game-visible RAM so the clock
            // rides along in the battery save. The MBC never maps into it.
            let footer = if header.has_rtc { RTC_FOOTER_LEN } else { 0 };
            vec![0u8; header.ram_banks.max(1) * 0x2000 + footer]
        };
        let mbc = match header.mbc_kind {
            MbcKind::None => Mbc::None,
            MbcKind::Mbc1 => Mbc::Mbc1 {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
                banking_mode: false,
            },
            MbcKind::Mbc2 => Mbc::Mbc2 {
                ram_enabled: false,
                rom_bank: 1,
            },
            MbcKind::Mbc3 => Mbc::Mbc3 {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
                has_rtc: header.has_rtc,
                rtc: Rtc::new(),
            },
            MbcKind::Mbc5 => Mbc::Mbc5 {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
            },
            MbcKind::Mbc7 => Mbc::Mbc7 {
                ram_enable_1: false,
                ram_enable_2: false,
                rom_bank: 1,
                accel: Accel::new(),
                eeprom: Eeprom::new(),
            },
            MbcKind::Camera => Mbc::Camera {
                ram_enabled: false,
                rom_bank: 1,
                ram_bank: 0,
                cam: Cam::new(),
            },
            MbcKind::Huc1 => Mbc::Huc1 {
                ir_mode: false,
                rom_bank: 1,
                ram_bank: 0,
            },
            MbcKind::Huc3 => Mbc::Huc3 {
                rom_bank: 1,
                ram_bank: 0,
                huc3: Huc3::new(),
            },
            MbcKind::Unsupported(_) => Mbc::None,
        };
        Cartridge {
            header,
            rom,
            ram,
            mbc,
        }
    }

    /// Read from the ROM region (0x0000..=0x7FFF).
    pub fn read_rom(&self, addr: u16) -> u8 {
        match &self.mbc {
            Mbc::None => *self.rom.get(addr as usize).unwrap_or(&0xFF),
            Mbc::Mbc1 {
                rom_bank,
                ram_bank,
                banking_mode,
                ..
            } => {
                let bank = if addr < 0x4000 {
                    // Bank 0 region. In RAM-banking mode the upper bits can remap
                    // this to bank 0x20/0x40/0x60 on large carts.
                    if *banking_mode {
                        ((*ram_bank as usize) << 5) & (self.header.rom_banks - 1)
                    } else {
                        0
                    }
                } else {
                    // Switchable region. Combine the 5-bit low bank with the 2-bit high.
                    let low = (*rom_bank as usize) & 0x1F;
                    let low = if low == 0 { 1 } else { low }; // bank 0 not selectable here
                    let hi = (*ram_bank as usize) << 5;
                    (hi | low) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            Mbc::Mbc2 { rom_bank, .. } => {
                let bank = if addr < 0x4000 {
                    0
                } else {
                    ((*rom_bank as usize).max(1)) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            Mbc::Mbc3 { rom_bank, .. } => {
                let bank = if addr < 0x4000 {
                    0
                } else {
                    ((*rom_bank as usize).max(1)) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            Mbc::Mbc5 { rom_bank, .. } => {
                // MBC5 can select bank 0 into the switchable region.
                let bank = if addr < 0x4000 {
                    0
                } else {
                    (*rom_bank as usize) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            Mbc::Mbc7 { rom_bank, .. } => {
                // Seven bits. Bank 0 in the high window reads as bank 1, the
                // same as MBC1 and MBC3.
                let bank = if addr < 0x4000 {
                    0
                } else {
                    ((*rom_bank as usize).max(1)) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            // The camera's mapper takes $00-$3F and, unlike MBC1, the
            // documentation says nothing about remapping bank 0, so bank 0 is
            // selectable into the high window the way MBC5 allows.
            Mbc::Camera { rom_bank, .. } => {
                let bank = if addr < 0x4000 {
                    0
                } else {
                    (*rom_bank as usize) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
            // HuC1 and HuC3 bank like an MBC1/MBC3 (bank 0 not selectable high).
            Mbc::Huc1 { rom_bank, .. } | Mbc::Huc3 { rom_bank, .. } => {
                let bank = if addr < 0x4000 {
                    0
                } else {
                    ((*rom_bank as usize).max(1)) & (self.header.rom_banks - 1)
                };
                let offset = bank * 0x4000 + (addr as usize & 0x3FFF);
                *self.rom.get(offset).unwrap_or(&0xFF)
            }
        }
    }

    /// Write to the ROM region: interpreted as an MBC control write.
    pub fn write_rom(&mut self, addr: u16, val: u8) {
        match &mut self.mbc {
            Mbc::None => {}
            Mbc::Mbc7 {
                ram_enable_1,
                ram_enable_2,
                rom_bank,
                ..
            } => match addr {
                // Two separate enables, and the register block at $A000 wants
                // BOTH. This is the one mapper here where a single enable is
                // not sufficient.
                0x0000..=0x1FFF => *ram_enable_1 = val == 0x0A,
                0x2000..=0x3FFF => *rom_bank = val & 0x7F,
                0x4000..=0x5FFF => *ram_enable_2 = val == 0x40,
                _ => {}
            },
            Mbc::Camera {
                ram_enabled,
                rom_bank,
                ram_bank,
                ..
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = val & 0x0F == 0x0A,
                0x2000..=0x3FFF => *rom_bank = val & 0x3F,
                // Bit 4 is kept: it is the register/album switch for the $A000
                // window, not part of the bank number.
                0x4000..=0x5FFF => *ram_bank = val & 0x1F,
                _ => {}
            },
            Mbc::Mbc1 {
                ram_enabled,
                rom_bank,
                ram_bank,
                banking_mode,
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => *rom_bank = val & 0x1F,
                0x4000..=0x5FFF => *ram_bank = val & 0x03,
                0x6000..=0x7FFF => *banking_mode = (val & 0x01) != 0,
                _ => {}
            },
            Mbc::Mbc2 {
                ram_enabled,
                rom_bank,
            } => {
                // One shared register in 0x0000..=0x3FFF: address bit 8 picks
                // which. Clear -> RAM enable; set -> ROM bank (4 bits, min 1).
                if addr < 0x4000 {
                    if addr & 0x0100 == 0 {
                        *ram_enabled = (val & 0x0F) == 0x0A;
                    } else {
                        *rom_bank = (val & 0x0F).max(1);
                    }
                }
            }
            Mbc::Mbc3 {
                ram_enabled,
                rom_bank,
                ram_bank,
                has_rtc,
                rtc,
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x3FFF => *rom_bank = val & 0x7F,
                0x4000..=0x5FFF => *ram_bank = val, // 0x00-0x03 = RAM bank, 0x08-0x0C = RTC register
                0x6000..=0x7FFF => {
                    if *has_rtc {
                        rtc.write_latch(val);
                    }
                }
                _ => {}
            },
            Mbc::Mbc5 {
                ram_enabled,
                rom_bank,
                ram_bank,
            } => match addr {
                0x0000..=0x1FFF => *ram_enabled = (val & 0x0F) == 0x0A,
                0x2000..=0x2FFF => *rom_bank = (*rom_bank & 0x100) | val as u16,
                0x3000..=0x3FFF => *rom_bank = (*rom_bank & 0x0FF) | ((val as u16 & 1) << 8),
                0x4000..=0x5FFF => *ram_bank = val & 0x0F,
                _ => {}
            },
            Mbc::Huc1 {
                ir_mode,
                rom_bank,
                ram_bank,
            } => match addr {
                0x0000..=0x1FFF => *ir_mode = (val & 0x0F) == 0x0E,
                0x2000..=0x3FFF => *rom_bank = val & 0x3F,
                0x4000..=0x5FFF => *ram_bank = val & 0x03,
                _ => {}
            },
            Mbc::Huc3 {
                rom_bank,
                ram_bank,
                huc3,
            } => match addr {
                0x0000..=0x1FFF => huc3.mode = val & 0x0F,
                0x2000..=0x3FFF => *rom_bank = val & 0x7F,
                0x4000..=0x5FFF => *ram_bank = val & 0x0F,
                _ => {}
            },
        }
    }

    /// MBC7's register block at $A000-$BFFF.
    ///
    /// Separate from `write_ram` because the EEPROM writes into the save, and
    /// that cannot be done while the mapper itself is mutably borrowed out of
    /// `self`.
    fn mbc7_write(&mut self, addr: u16, val: u8) {
        let enabled = match &self.mbc {
            Mbc::Mbc7 {
                ram_enable_1,
                ram_enable_2,
                ..
            } => *ram_enable_1 && *ram_enable_2,
            _ => return,
        };
        if !enabled {
            return;
        }

        let reg = (addr >> 4) & 0xF;

        // Destructured so the EEPROM can hold `ram` while the mapper holds
        // `mbc`: they are distinct fields, so the borrow checker allows it,
        // where `self.mbc` and `self.ram` together would not.
        let Cartridge { mbc, ram, .. } = self;
        if let Mbc::Mbc7 { accel, eeprom, .. } = mbc {
            if reg == 0x8 {
                eeprom.write(val, ram);
                return;
            }
            match reg {
                // $55 arms the latch, $AA fires it. Anything else leaves the
                // previous reading alone, so a game that forgets to latch keeps
                // reading the same numbers rather than watching them drift.
                0x0 => {
                    if val == 0x55 {
                        accel.x = ACCEL_ERASED;
                        accel.y = ACCEL_ERASED;
                        accel.armed = true;
                    }
                }
                0x1 => {
                    if val == 0xAA && accel.armed {
                        accel.latch();
                        accel.armed = false;
                    }
                }
                _ => {}
            }
        }
    }

    /// Does this cartridge have a tilt sensor on it?
    pub fn has_tilt(&self) -> bool {
        matches!(self.mbc, Mbc::Mbc7 { .. })
    }

    /// Tell the accelerometer how the console is being held, in g per axis,
    /// in SCREEN coordinates: positive x rolls Kirby right, positive y rolls
    /// him down.
    ///
    /// Stored rather than applied: the game decides when to sample, by writing
    /// $55 then $AA, and reads the values latched at that moment. So a frontend
    /// can push as often as it likes without the reading changing under a game
    /// that is midway through reading it.
    pub fn set_tilt(&mut self, x_g: f32, y_g: f32) -> bool {
        if let Mbc::Mbc7 { accel, .. } = &mut self.mbc {
            accel.tilt_x = x_g;
            accel.tilt_y = y_g;
            true
        } else {
            false
        }
    }

    /// Is the $A000 window showing the camera registers rather than the album?
    fn camera_registers_selected(&self) -> bool {
        matches!(&self.mbc, Mbc::Camera { ram_bank, .. } if ram_bank & 0x10 != 0)
    }

    /// Advance an exposure, and finish one that has run its course.
    fn tick_camera(&mut self, cycles: u32) {
        let finished = match &mut self.mbc {
            Mbc::Camera { cam, .. } => {
                let was = cam.busy;
                cam.tick(cycles);
                was > 0 && cam.busy == 0
            }
            _ => return,
        };
        if finished {
            self.camera_develop();
        }
    }

    /// Write the captured image into the album where the cartridge expects it.
    ///
    /// The sensor model is not built yet, so this is a placeholder gradient: a
    /// picture that is obviously synthetic, so that nobody mistakes it for a
    /// working camera, but which exercises the whole path from trigger to
    /// tiles. The real thing takes a greyscale frame and runs the exposure,
    /// edge enhancement and dither the registers ask for.
    /// Turn what the sensor sees into the 2bpp tiles the cartridge expects.
    ///
    /// Short, and every step is driven by a register the game wrote, which is the
    /// whole point: the in-game brightness and contrast sliders work because they
    /// really are changing the exposure and the dither matrix, not because
    /// anything here interprets them.
    ///
    /// 1. take the frame, or the test card if nothing is feeding us light;
    /// 2. scale it by the exposure time;
    /// 3. dither to four shades against the game's own matrix;
    /// 4. pack into tiles at $0100 of the album.
    ///
    /// NOT modelled yet: the edge-enhancement kernel ($A001 and $A004), the
    /// analogue gain and zero-point calibration ($A005), and inversion. Those
    /// sharpen and bias; without them a photograph is soft but correct.
    fn camera_develop(&mut self) {
        let Mbc::Camera { cam, .. } = &self.mbc else {
            return;
        };

        // Exposure as a gain. The reference is the exposure at which the sensor
        // neither amplifies nor attenuates. It is a tuning constant rather than a
        // documented one, so it lives here where there is one place to change it
        // when photographs come out flat.
        const NOMINAL_EXPOSURE: u32 = 0x3000;
        let gain = cam.exposure().max(1) as u32;

        let mut shades = vec![0u8; CAMERA_W * CAMERA_H];
        for y in 0..CAMERA_H {
            for x in 0..CAMERA_W {
                let raw = match (&cam.frame, cam.sensor_available) {
                    (Some(f), _) => f[y * CAMERA_W + x],
                    (None, true) => waiting_for_light_card(x, y),
                    (None, false) => no_camera_card(x, y),
                } as u32;
                let lit = (raw * gain / NOMINAL_EXPOSURE).min(255) as u8;

                // More light means less ink, so this runs the opposite way round
                // from the ascending thresholds.
                let (t0, t1, t2) = cam.thresholds(x, y);
                shades[y * CAMERA_W + x] = if lit < t0 {
                    3
                } else if lit < t1 {
                    2
                } else if lit < t2 {
                    1
                } else {
                    0
                };
            }
        }

        // Pack into 8x8 tiles, the same 2bpp layout the PPU reads.
        let tiles_across = CAMERA_W / 8;
        for ty in 0..CAMERA_H / 8 {
            for tx in 0..tiles_across {
                let tile = ty * tiles_across + tx;
                for row in 0..8 {
                    let mut lo = 0u8;
                    let mut hi = 0u8;
                    for col in 0..8 {
                        let v = shades[(ty * 8 + row) * CAMERA_W + tx * 8 + col];
                        let bit = 7 - col;
                        lo |= (v & 1) << bit;
                        hi |= ((v >> 1) & 1) << bit;
                    }
                    let at = CAMERA_IMAGE_OFFSET + tile * 16 + row * 2;
                    if at + 1 < self.ram.len() {
                        self.ram[at] = lo;
                        self.ram[at + 1] = hi;
                    }
                }
            }
        }
    }

    /// Point the Game Boy Camera at something.
    ///
    /// `gray` is `CAMERA_W * CAMERA_H` bytes, one per pixel, 0 black. It must be
    /// UNMIRRORED and in sensor orientation: a frontend that mirrors its preview,
    /// as phone front cameras conventionally do, must hand over the unmirrored
    /// frame or every photograph with text in it develops backwards, and it
    /// develops backwards in a PRINT, which is the artifact people keep.
    ///
    /// Returns false if this cartridge has no camera, or the frame is the wrong
    /// size.
    pub fn set_camera_frame(&mut self, gray: &[u8]) -> bool {
        if gray.len() != CAMERA_W * CAMERA_H {
            return false;
        }
        match &mut self.mbc {
            Mbc::Camera { cam, .. } => {
                cam.frame = Some(gray.to_vec());
                true
            }
            _ => false,
        }
    }

    /// Tell the core whether the frontend has a camera at all.
    ///
    /// Call it with true once a camera interface has been obtained, whether or
    /// not a frame has arrived. It only chooses which diagnostic the sensor sees
    /// while no frame is available, and it is worth setting because "this
    /// frontend cannot do cameras" and "no picture has arrived yet" want
    /// different responses from whoever is looking at the screen.
    pub fn set_camera_available(&mut self, available: bool) {
        if let Mbc::Camera { cam, .. } = &mut self.mbc {
            cam.sensor_available = available;
        }
    }

    /// Whether this cartridge has an image sensor on it at all.
    pub fn has_camera(&self) -> bool {
        matches!(self.mbc, Mbc::Camera { .. })
    }

    /// Whether cartridge RAM is currently readable/writable.
    fn ram_enabled(&self) -> bool {
        match &self.mbc {
            Mbc::None => true,
            Mbc::Mbc1 { ram_enabled, .. }
            | Mbc::Mbc2 { ram_enabled, .. }
            | Mbc::Mbc3 { ram_enabled, .. }
            | Mbc::Mbc5 { ram_enabled, .. } => *ram_enabled,
            // MBC7 wants both of its enables before the register block
            // answers at all.
            Mbc::Mbc7 {
                ram_enable_1,
                ram_enable_2,
                ..
            } => *ram_enable_1 && *ram_enable_2,
            // The camera's REGISTERS are always reachable; only the photo
            // album behind them needs enabling. The register case never gets
            // this far, so this is only ever asked about RAM.
            Mbc::Camera { ram_enabled, .. } => *ram_enabled,
            // HuC1 RAM is reachable whenever the window isn't in IR mode.
            Mbc::Huc1 { ir_mode, .. } => !*ir_mode,
            // HuC3 RAM is reachable in mode 0x0A (handled before this gate).
            Mbc::Huc3 { huc3, .. } => huc3.mode == 0x0A,
        }
    }

    /// Read from cartridge RAM (0xA000..=0xBFFF).
    pub fn read_ram(&self, addr: u16) -> u8 {
        // The camera's registers take over the whole window when bit 4 of the
        // RAM bank is set, and they answer whether or not RAM is enabled.
        if self.camera_registers_selected() {
            if let Mbc::Camera { cam, .. } = &self.mbc {
                return cam.read(addr);
            }
        }
        // MBC7's whole $A000-$BFFF window is registers, not memory, and which
        // register is chosen by the SECOND nibble of the address: the block is
        // mirrored every sixteen bytes across the window.
        if let Mbc::Mbc7 {
            accel,
            eeprom,
            ram_enable_1,
            ram_enable_2,
            ..
        } = &self.mbc
        {
            if !(*ram_enable_1 && *ram_enable_2) {
                return 0xFF;
            }
            return match (addr >> 4) & 0xF {
                0x2 => accel.x as u8,
                0x3 => (accel.x >> 8) as u8,
                0x4 => accel.y as u8,
                0x5 => (accel.y >> 8) as u8,
                0x6 => 0x00,
                0x8 => eeprom.read(),
                _ => 0xFF,
            };
        }
        // HuC1: the IR window reads back "no signal"; otherwise it is RAM.
        if let Mbc::Huc1 { ir_mode: true, .. } = &self.mbc {
            return 0xC0;
        }
        // HuC3: a non-RAM window returns the command mailbox / semaphore / IR.
        if let Mbc::Huc3 { huc3, ram_bank, .. } = &self.mbc {
            if huc3.mode != 0x00 && huc3.mode != 0x0A {
                return huc3.read_window();
            }
            let banks = self.header.ram_banks.max(1);
            let idx = (*ram_bank as usize % banks) * 0x2000 + (addr as usize & 0x1FFF);
            return self.ram.get(idx).copied().unwrap_or(0xFF);
        }
        // The camera's mapper gates WRITES only: "reading and register writes
        // are always enabled". Gating reads as well is what made the viewfinder
        // black, because a disabled read returns $FF, which is both bitplanes
        // set, which is colour 3.
        if !self.ram_enabled() && !matches!(self.mbc, Mbc::Camera { .. }) {
            return 0xFF;
        }
        // MBC3: a selected RTC register (0x08-0x0C) reads the latched clock.
        if let Mbc::Mbc3 {
            ram_bank,
            has_rtc: true,
            rtc,
            ..
        } = &self.mbc
        {
            if (0x08..=0x0C).contains(ram_bank) {
                return rtc.read_reg(*ram_bank - 0x08);
            }
        }
        let idx = self.ram_offset(addr);
        let val = self.ram.get(idx).copied().unwrap_or(0xFF);
        // MBC2 RAM is 4-bit: the upper nibble reads back as 1s.
        if matches!(self.mbc, Mbc::Mbc2 { .. }) {
            val | 0xF0
        } else {
            val
        }
    }

    /// Write to cartridge RAM (0xA000..=0xBFFF).
    pub fn write_ram(&mut self, addr: u16, val: u8) {
        if self.camera_registers_selected() {
            if let Mbc::Camera { cam, .. } = &mut self.mbc {
                cam.write(addr, val);
            }
            return;
        }
        if let Mbc::Mbc7 { .. } = &self.mbc {
            self.mbc7_write(addr, val);
            return;
        }
        // HuC1: an IR-mode write drives the LED (ignored); RAM otherwise.
        if let Mbc::Huc1 { ir_mode, .. } = &self.mbc {
            if *ir_mode {
                return;
            }
            // fall through to the generic RAM write below
        }
        // HuC3: RAM in mode 0x0A, else the command-mailbox / semaphore interface.
        if matches!(self.mbc, Mbc::Huc3 { .. }) {
            self.huc3_write(addr, val);
            return;
        }
        if !self.ram_enabled() {
            return;
        }
        // MBC3: a selected RTC register (0x08-0x0C) writes the live clock. Refresh
        // the battery footer so a plain .srm save keeps the new time.
        let rtc_footer = if let Mbc::Mbc3 {
            ram_bank,
            has_rtc: true,
            rtc,
            ..
        } = &mut self.mbc
        {
            if (0x08..=0x0C).contains(&*ram_bank) {
                rtc.write_reg(*ram_bank - 0x08, val);
                Some(rtc.encode_footer())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(footer) = rtc_footer {
            self.write_rtc_footer(&footer);
            return;
        }
        let idx = self.ram_offset(addr);
        let val = if matches!(self.mbc, Mbc::Mbc2 { .. }) {
            val & 0x0F
        } else {
            val
        };
        if let Some(cell) = self.ram.get_mut(idx) {
            *cell = val;
        }
    }

    fn ram_offset(&self, addr: u16) -> usize {
        // MBC2's 512-half-byte RAM lives at 0xA000..=0xA1FF and echoes upward.
        if matches!(self.mbc, Mbc::Mbc2 { .. }) {
            return addr as usize & 0x1FF;
        }
        let local = addr as usize & 0x1FFF;
        let bank = match &self.mbc {
            Mbc::Mbc1 {
                ram_bank,
                banking_mode,
                ..
            } => {
                if *banking_mode {
                    *ram_bank as usize
                } else {
                    0
                }
            }
            Mbc::Mbc3 { ram_bank, .. } => (*ram_bank & 0x03) as usize,
            Mbc::Mbc5 { ram_bank, .. } => *ram_bank as usize,
            // Bit 4 means "registers", so it is not part of the bank number.
            Mbc::Camera { ram_bank, .. } => (*ram_bank & 0x0F) as usize,
            Mbc::Huc1 { ram_bank, .. } => (*ram_bank & 0x03) as usize,
            Mbc::Huc3 { ram_bank, .. } => (*ram_bank & 0x0F) as usize,
            // MBC7 has no banked cartridge RAM at all: its whole $A000 window
            // is registers, and its 256 save bytes are the EEPROM, indexed by
            // word address rather than by this path.
            Mbc::None | Mbc::Mbc2 { .. } | Mbc::Mbc7 { .. } => 0, // MBC2 handled above
        };
        // Guard against carts that report no RAM banks.
        let banks = self.header.ram_banks.max(1);
        (bank % banks) * 0x2000 + local
    }

    /// HuC3 write to the 0xA000-0xBFFF window: cartridge RAM in mode 0x0A, else
    /// the command-mailbox (0x0B) / semaphore-commit (0x0D) interface.
    fn huc3_write(&mut self, addr: u16, val: u8) {
        let (mode, bank) = match &self.mbc {
            Mbc::Huc3 { huc3, ram_bank, .. } => (huc3.mode, *ram_bank as usize),
            _ => return,
        };
        match mode {
            0x0A => {
                let banks = self.header.ram_banks.max(1);
                let idx = (bank % banks) * 0x2000 + (addr as usize & 0x1FFF);
                if let Some(cell) = self.ram.get_mut(idx) {
                    *cell = val;
                }
            }
            0x0B => {
                if let Mbc::Huc3 { huc3, .. } = &mut self.mbc {
                    huc3.command = (val >> 4) & 0x07;
                    huc3.arg = val & 0x0F;
                }
            }
            0x0D => {
                // Clearing bit 0 requests execution of the mailbox command.
                if val & 0x01 == 0 {
                    if let Mbc::Huc3 { huc3, .. } = &mut self.mbc {
                        huc3.execute();
                    }
                }
            }
            _ => {} // IR (0x0E) LED and read-only modes: nothing to write
        }
    }

    pub fn title(&self) -> &str {
        &self.header.title
    }

    /// Transfer the mutable cartridge state (RAM + bank registers). The ROM and
    /// header are static and the MBC variant is fixed by the loaded cartridge,
    /// so only the active variant's registers are serialized.
    pub(crate) fn transfer<C: crate::save::Cursor>(&mut self, c: &mut C) {
        c.bytes(&mut self.ram);
        match &mut self.mbc {
            Mbc::None => {}
            Mbc::Mbc7 {
                ram_enable_1,
                ram_enable_2,
                rom_bank,
                accel,
                eeprom,
            } => {
                c.bool(ram_enable_1);
                c.bool(ram_enable_2);
                c.u8(rom_bank);
                c.u16(&mut accel.x);
                c.u16(&mut accel.y);
                c.bool(&mut accel.armed);
                // The EEPROM's own bit-level state. A save state taken in the
                // middle of a word has to come back in the middle of that word,
                // or the game's next clock edge lands somewhere else entirely.
                c.bool(&mut eeprom.cs);
                c.bool(&mut eeprom.clk);
                c.bool(&mut eeprom.do_bit);
                c.u32(&mut eeprom.shift);
                c.u8(&mut eeprom.bits);
                c.u32(&mut eeprom.out);
                c.u8(&mut eeprom.out_bits);
                c.bool(&mut eeprom.write_enabled);
                let mut st = eeprom.state as u8;
                c.u8(&mut st);
                eeprom.state = match st {
                    1 => EeState::Command,
                    2 => EeState::WriteData,
                    3 => EeState::Reading,
                    _ => EeState::Idle,
                };
            }
            Mbc::Camera {
                ram_enabled,
                rom_bank,
                ram_bank,
                cam,
            } => {
                c.bool(ram_enabled);
                c.u8(rom_bank);
                c.u8(ram_bank);
                c.u8(&mut cam.trigger);
                c.i32(&mut cam.busy);
                for r in cam.regs.iter_mut() {
                    c.u8(r);
                }
            }
            Mbc::Mbc1 {
                ram_enabled,
                rom_bank,
                ram_bank,
                banking_mode,
            } => {
                c.bool(ram_enabled);
                c.u8(rom_bank);
                c.u8(ram_bank);
                c.bool(banking_mode);
            }
            Mbc::Mbc2 {
                ram_enabled,
                rom_bank,
            } => {
                c.bool(ram_enabled);
                c.u8(rom_bank);
            }
            Mbc::Mbc3 {
                ram_enabled,
                rom_bank,
                ram_bank,
                has_rtc,
                rtc,
            } => {
                c.bool(ram_enabled);
                c.u8(rom_bank);
                c.u8(ram_bank);
                if *has_rtc {
                    rtc.transfer(c);
                }
            }
            Mbc::Mbc5 {
                ram_enabled,
                rom_bank,
                ram_bank,
            } => {
                c.bool(ram_enabled);
                c.u16(rom_bank);
                c.u8(ram_bank);
            }
            Mbc::Huc1 {
                ir_mode,
                rom_bank,
                ram_bank,
            } => {
                c.bool(ir_mode);
                c.u8(rom_bank);
                c.u8(ram_bank);
            }
            Mbc::Huc3 {
                rom_bank,
                ram_bank,
                huc3,
            } => {
                c.u8(rom_bank);
                c.u8(ram_bank);
                huc3.transfer(c);
            }
        }
    }

    /// Advance the on-cartridge real-time clock (MBC3 or HuC3) by `cycles`
    /// wall-clock T-cycles. A no-op for every other cartridge. The MMU calls this
    /// each tick with the double-speed-adjusted cycle count so the clock always
    /// tracks real time.
    pub fn tick_rtc(&mut self, cycles: u32) {
        // The camera's exposure runs on the same pulse.
        self.tick_camera(cycles);
        let footer = match &mut self.mbc {
            Mbc::Mbc3 {
                has_rtc: true, rtc, ..
            } => {
                if rtc.tick(cycles) {
                    Some(rtc.encode_footer())
                } else {
                    None
                }
            }
            Mbc::Huc3 { huc3, .. } => {
                huc3.tick(cycles);
                None
            }
            _ => None,
        };
        if let Some(footer) = footer {
            self.write_rtc_footer(&footer);
        }
    }

    /// Byte offset of the RTC battery footer: immediately past the game-visible
    /// RAM. (RTC carts are never MBC2.)
    fn rtc_footer_offset(&self) -> usize {
        self.header.ram_banks.max(1) * 0x2000
    }

    fn write_rtc_footer(&mut self, footer: &[u8]) {
        let off = self.rtc_footer_offset();
        if off + footer.len() <= self.ram.len() {
            self.ram[off..off + footer.len()].copy_from_slice(footer);
        }
    }

    /// After the frontend loads a battery `.srm`, pull the RTC time back out of
    /// its footer (if the save carried one). Call once, right after `load_sram`.
    pub fn restore_rtc_from_footer(&mut self) {
        if !self.header.has_rtc {
            return;
        }
        let off = self.rtc_footer_offset();
        if off + RTC_FOOTER_LEN > self.ram.len() {
            return;
        }
        let footer: [u8; RTC_FOOTER_LEN] = self.ram[off..off + RTC_FOOTER_LEN]
            .try_into()
            .expect("slice is RTC_FOOTER_LEN");
        if let Mbc::Mbc3 {
            has_rtc: true, rtc, ..
        } = &mut self.mbc
        {
            rtc.decode_footer(&footer);
        }
    }

    pub fn has_battery(&self) -> bool {
        self.header.has_battery
    }

    /// Whether the cartridge requests Game Boy Color features (flag 0x80/0xC0).
    pub fn is_cgb(&self) -> bool {
        self.header.cgb_flag & 0x80 != 0
    }

    pub fn title_checksum(&self) -> u8 {
        self.header.title_checksum
    }
    pub fn title_fourth(&self) -> u8 {
        self.header.title_fourth
    }
    pub fn nintendo_licensed(&self) -> bool {
        self.header.nintendo_licensed
    }

    /// The raw cartridge RAM, for battery-save persistence.
    /// The full ROM image as loaded, banks end to end.
    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    pub fn ram(&self) -> &[u8] {
        &self.ram
    }
    pub fn ram_mut(&mut self) -> &mut [u8] {
        &mut self.ram
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two no-signal cards are a CROSS-REPOSITORY contract, described in
    /// docs/CAMERA.md and referenced from TrophyHubLibretroHost's source and
    /// TH-Android's ticket. Changing what they look like silently makes two
    /// other repositories' comments wrong, and nothing over there can catch it.
    ///
    /// So this pins the properties the contract rests on rather than the exact
    /// pixels: one card is ROUND and one is STRAIGHT-WITH-A-DIAGONAL, which is
    /// what lets a reader tell "no camera interface" from "no frame yet" at a
    /// glance and in a blurry photograph of a screen.
    ///
    /// The diagonal is load-bearing and must not be removed as a tidy-up.
    /// Without it the no-camera card is four flat shade bars, which is a
    /// gradient, and a gradient is close to what a legitimate but very
    /// low-contrast capture looks like on a four-shade panel. That would trade
    /// an ambiguity existing only during host bring-up for one a player can hit
    /// in a dark room. (Reasoning from TH-LibretroHost, who talked me out of
    /// removing it.)
    #[test]
    fn the_two_no_signal_cards_stay_tellable_apart() {
        let cx = CAMERA_W / 2;
        let cy = CAMERA_H / 2;

        // Round: mirroring across either axis through the centre changes nothing.
        for (x, y) in [(cx + 20, cy), (cx + 8, cy + 30), (cx + 40, cy + 10)] {
            let v = waiting_for_light_card(x, y);
            assert_eq!(v, waiting_for_light_card(2 * cx - x, y), "rings not symmetric in x");
            assert_eq!(v, waiting_for_light_card(x, 2 * cy - y), "rings not symmetric in y");
        }

        // The no-camera card is not a pure function of x: something varies down
        // a column, which is the diagonal. Flat bars would pass every other
        // check here and fail this one, which is the point.
        let varies_down_a_column = (0..CAMERA_H)
            .map(|y| no_camera_card(7, y))
            .any(|v| v != no_camera_card(7, 0));
        assert!(
            varies_down_a_column,
            "the no-camera card has become flat vertical bars. That is a gradient,              and a gradient looks like a legitimate low-light capture. The diagonal              is load-bearing; see docs/CAMERA.md."
        );

        // And they are not each other.
        let differ = (0..CAMERA_H)
            .step_by(7)
            .flat_map(|y| (0..CAMERA_W).step_by(7).map(move |x| (x, y)))
            .filter(|&(x, y)| no_camera_card(x, y) != waiting_for_light_card(x, y))
            .count();
        assert!(differ > 40, "the two cards have converged on each other");
    }

    /// Both cards must be STATIC, which is the strongest tell of all and the one
    /// needing no agreement about what a diagonal looks like: a real frame moves
    /// when the lens moves, a card never does. "Does the picture respond to the
    /// camera" separates a diagnostic from a mangled capture in one question and
    /// survives the blurry phone photo that shape does not. (TH-LibretroHost's
    /// observation; it is in docs/CAMERA.md.)
    ///
    /// Being pure functions of position is what makes that true, so a card that
    /// grew a frame counter or any other input would quietly break it.
    #[test]
    fn the_cards_are_static() {
        for (x, y) in [(3usize, 5usize), (64, 56), (127, 111)] {
            let a = (no_camera_card(x, y), waiting_for_light_card(x, y));
            for _ in 0..8 {
                assert_eq!(
                    (no_camera_card(x, y), waiting_for_light_card(x, y)),
                    a,
                    "a no-signal card changed between calls; it must be static,                      because 'it moves' is how a real frame is told from a card"
                );
            }
        }
    }

    /// A minimal MBC3 + RAM + TIMER + BATTERY cartridge (type 0x10), 32 KiB ROM,
    /// one 8 KiB RAM bank.
    fn mbc3_rtc_cart() -> Cartridge {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x10; // MBC3+TIMER+RAM+BATTERY
        rom[0x0148] = 0x00; // 32 KiB (2 ROM banks)
        rom[0x0149] = 0x02; // 8 KiB RAM (1 bank)
        Cartridge::new(rom)
    }

    /// Enable RAM/RTC and latch the current time into the readable snapshot.
    fn latch(cart: &mut Cartridge) {
        cart.write_rom(0x6000, 0x00);
        cart.write_rom(0x6000, 0x01);
    }

    /// Read RTC register `reg` (0x08=S .. 0x0C=DH) after a fresh latch.
    fn read_rtc(cart: &mut Cartridge, reg: u8) -> u8 {
        cart.write_rom(0x4000, reg);
        cart.read_ram(0xA000)
    }

    #[test]
    fn rtc_present_only_for_timer_carts() {
        assert!(mbc3_rtc_cart().header.has_rtc);
        // MBC3 without the timer (type 0x13) has no clock.
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x13;
        rom[0x0149] = 0x02;
        assert!(!Cartridge::new(rom).header.has_rtc);
    }

    #[test]
    fn rtc_ticks_seconds_minutes_hours() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A); // enable RAM/RTC
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 0);

        // A snapshot is frozen until the next latch.
        cart.tick_rtc(5 * CYCLES_PER_SECOND);
        assert_eq!(read_rtc(&mut cart, 0x08), 0, "latched value stays put");
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 5);

        // +60 s from 5 s -> 1 min 5 s.
        cart.tick_rtc(60 * CYCLES_PER_SECOND);
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 5);
        assert_eq!(read_rtc(&mut cart, 0x09), 1);
    }

    #[test]
    fn rtc_register_write_sets_live_clock() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x08); // select seconds
        cart.write_ram(0xA000, 30);
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 30);
    }

    #[test]
    fn rtc_halt_stops_the_clock() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x0C); // select DH
        cart.write_ram(0xA000, 0x40); // set HALT
        cart.tick_rtc(100 * CYCLES_PER_SECOND);
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 0, "halted clock does not advance");
    }

    #[test]
    fn rtc_day_counter_carries() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A);
        // Seed 23:59:59 on day 511.
        cart.write_rom(0x4000, 0x08);
        cart.write_ram(0xA000, 59);
        cart.write_rom(0x4000, 0x09);
        cart.write_ram(0xA000, 59);
        cart.write_rom(0x4000, 0x0A);
        cart.write_ram(0xA000, 23);
        cart.write_rom(0x4000, 0x0B);
        cart.write_ram(0xA000, 0xFF); // day low = 255
        cart.write_rom(0x4000, 0x0C);
        cart.write_ram(0xA000, 0x01); // day high bit = 1 -> day 511

        cart.tick_rtc(CYCLES_PER_SECOND); // one more second wraps the day counter
        latch(&mut cart);
        let dh = read_rtc(&mut cart, 0x0C);
        assert_eq!(dh & 0x80, 0x80, "day-carry bit set");
        assert_eq!(dh & 0x01, 0x00, "day high bit wrapped to 0");
        assert_eq!(read_rtc(&mut cart, 0x0B), 0, "day low wrapped to 0");
    }

    #[test]
    fn rtc_persists_through_battery_footer() {
        let mut cart = mbc3_rtc_cart();
        cart.write_rom(0x0000, 0x0A);
        cart.write_rom(0x4000, 0x08);
        cart.write_ram(0xA000, 42); // seconds
        cart.write_rom(0x4000, 0x0A);
        cart.write_ram(0xA000, 7); // hours
        let saved = cart.ram().to_vec();

        // Fresh cart, restore the .srm, and confirm the clock came back.
        let mut cart2 = mbc3_rtc_cart();
        {
            let ram = cart2.ram_mut();
            let n = ram.len().min(saved.len());
            ram[..n].copy_from_slice(&saved[..n]);
        }
        cart2.restore_rtc_from_footer();
        cart2.write_rom(0x0000, 0x0A);
        latch(&mut cart2);
        assert_eq!(read_rtc(&mut cart2, 0x08), 42);
        assert_eq!(read_rtc(&mut cart2, 0x0A), 7);
    }

    #[test]
    fn old_srm_without_footer_leaves_clock_at_power_on() {
        // A battery save from before RTC support (just RAM, no footer) must load
        // without corrupting the clock.
        let mut cart = mbc3_rtc_cart();
        let legacy = vec![0xABu8; 0x2000]; // 8 KiB of RAM, no footer
        {
            let ram = cart.ram_mut();
            let n = ram.len().min(legacy.len());
            ram[..n].copy_from_slice(&legacy[..n]);
        }
        cart.restore_rtc_from_footer();
        cart.write_rom(0x0000, 0x0A);
        latch(&mut cart);
        assert_eq!(read_rtc(&mut cart, 0x08), 0);
    }

    // --- HuC1 / HuC3 --------------------------------------------------------

    fn huc_cart(cart_type: u8) -> Cartridge {
        let mut rom = vec![0u8; 0x10000]; // 4 ROM banks so a bank switch is real
        rom[0x0147] = cart_type;
        rom[0x0148] = 0x01; // 64 KiB (4 banks)
        rom[0x0149] = 0x03; // 32 KiB RAM (4 banks)
                            // Stamp each 16 KiB bank so we can tell which is mapped at 0x4000.
        for b in 0..4 {
            rom[b * 0x4000] = b as u8;
        }
        Cartridge::new(rom)
    }

    #[test]
    fn huc1_detected_and_banks_rom() {
        let mut cart = huc_cart(0xFF);
        assert_eq!(cart.header.mbc_kind, MbcKind::Huc1);
        assert!(cart.header.has_battery);
        cart.write_rom(0x2000, 0x02); // select ROM bank 2
        assert_eq!(cart.read_rom(0x4000), 2, "bank 2 stamp at 0x4000");
        cart.write_rom(0x2000, 0x00); // bank 0 is promoted to 1 in the high slot
        assert_eq!(cart.read_rom(0x4000), 1);
        assert_eq!(cart.read_rom(0x0000), 0, "low slot is always bank 0");
    }

    #[test]
    fn huc1_ram_and_ir_window() {
        let mut cart = huc_cart(0xFF);
        // RAM mode (any non-0x0E write): read/write cartridge RAM.
        cart.write_rom(0x0000, 0x00);
        cart.write_ram(0xA000, 0x5A);
        assert_eq!(cart.read_ram(0xA000), 0x5A);
        // IR mode: the window reads "no signal" and swallows writes.
        cart.write_rom(0x0000, 0x0E);
        assert_eq!(cart.read_ram(0xA000), 0xC0);
        cart.write_ram(0xA000, 0x11); // ignored
        cart.write_rom(0x0000, 0x00); // back to RAM
        assert_eq!(cart.read_ram(0xA000), 0x5A, "RAM survived the IR window");
    }

    #[test]
    fn huc3_detected_and_ram() {
        let mut cart = huc_cart(0xFE);
        assert_eq!(cart.header.mbc_kind, MbcKind::Huc3);
        assert!(cart.header.has_battery);
        cart.write_rom(0x0000, 0x0A); // mode 0x0A = RAM
        cart.write_ram(0xA000, 0x42);
        assert_eq!(cart.read_ram(0xA000), 0x42);
    }

    /// Drive the HuC3 nibble command MCU to set an address pointer, then execute.
    fn huc3_exec(cart: &mut Cartridge, byte: u8) {
        cart.write_rom(0x0000, 0x0B); // command-write mode
        cart.write_ram(0xA000, byte); // load the mailbox
        cart.write_rom(0x0000, 0x0D); // semaphore mode
        cart.write_ram(0xA000, 0x00); // clear bit 0 -> execute
    }

    #[test]
    fn huc3_rtc_reads_running_clock() {
        let mut cart = huc_cart(0xFE);
        // Advance the clock 3 minutes.
        cart.tick_rtc(3 * CYCLES_PER_MINUTE);
        // Latch RTC -> scratch (extended command 0x60), then read scratch nibbles.
        huc3_exec(&mut cart, 0x60); // 6<<4 | 0 : copy RTC to scratch
        huc3_exec(&mut cart, 0x40); // set address low nibble = 0
        huc3_exec(&mut cart, 0x50); // set address high nibble = 0
        // Read three minute nibbles (auto-incrementing pointer) and reassemble.
        let mut minutes = 0u16;
        for shift in [0u8, 4, 8] {
            huc3_exec(&mut cart, 0x10); // read register at pointer, auto-inc
            cart.write_rom(0x0000, 0x0C); // result-read mode
            let nib = (cart.read_ram(0xA000) & 0x0F) as u16;
            minutes |= nib << shift;
        }
        assert_eq!(minutes, 3, "HuC3 clock should read 3 minutes");
    }

    #[test]
    fn huc3_rtc_survives_save_state() {
        let mut cart = huc_cart(0xFE);
        cart.tick_rtc(5 * CYCLES_PER_MINUTE);
        let mut w = crate::save::WriteCursor::new();
        cart.transfer(&mut w);

        let mut cart2 = huc_cart(0xFE);
        let mut r = crate::save::ReadCursor::new(&w.buf);
        cart2.transfer(&mut r);
        assert!(r.ok);
        huc3_exec(&mut cart2, 0x60);
        huc3_exec(&mut cart2, 0x40);
        huc3_exec(&mut cart2, 0x50);
        huc3_exec(&mut cart2, 0x10);
        cart2.write_rom(0x0000, 0x0C);
        assert_eq!(cart2.read_ram(0xA000) & 0x0F, 5, "minutes low nibble restored");
    }
}

#[cfg(test)]
mod mbc7_tests {
    use super::*;

    /// Drive the EEPROM's pins the way the game does: one write per pin state,
    /// clocking each bit in with a low-then-high pair.
    struct Pins {
        ee: Eeprom,
        ram: Vec<u8>,
        cs: bool,
    }

    impl Pins {
        fn new() -> Pins {
            Pins {
                ee: Eeprom::new(),
                ram: vec![0x00; 256],
                cs: false,
            }
        }

        fn poke(&mut self, cs: bool, clk: bool, di: bool) {
            let v = (cs as u8) << 7 | (clk as u8) << 6 | (di as u8) << 1;
            self.ee.write(v, &mut self.ram);
        }

        fn select(&mut self) {
            self.cs = true;
            self.poke(true, false, false);
        }

        fn deselect(&mut self) {
            self.cs = false;
            self.poke(false, false, false);
        }

        /// Clock one bit in, and return the DO bit the part presents after it.
        fn bit(&mut self, di: bool) -> bool {
            self.poke(self.cs, false, di);
            self.poke(self.cs, true, di);
            self.ee.read() & 0x01 != 0
        }

        fn send(&mut self, val: u32, n: u8) {
            for i in (0..n).rev() {
                self.bit((val >> i) & 1 != 0);
            }
        }

        /// Start bit, two opcode bits, eight address bits.
        fn command(&mut self, op: u32, addr: u32) {
            self.select();
            self.bit(true);
            self.send(op, 2);
            self.send(addr, 8);
        }

        fn ewen(&mut self) {
            self.command(0b00, 0b11 << 6);
            self.deselect();
        }

        fn write_word(&mut self, addr: u32, word: u16) {
            self.command(0b01, addr);
            self.send(word as u32, 16);
            self.deselect();
        }

        fn read_word(&mut self, addr: u32) -> u16 {
            self.command(0b10, addr);
            let mut w = 0u16;
            for _ in 0..16 {
                w = (w << 1) | self.bit(false) as u16;
            }
            self.deselect();
            w
        }
    }

    #[test]
    fn eeprom_round_trips_a_word() {
        let mut p = Pins::new();
        p.ewen();
        p.write_word(0x05, 0xBEEF);
        assert_eq!(p.read_word(0x05), 0xBEEF, "a written word must read back");
    }

    #[test]
    fn eeprom_words_are_big_endian_in_the_save() {
        // The save file is shared with other emulators of this mapper, so the
        // byte order inside it is a compatibility contract, not a free choice.
        let mut p = Pins::new();
        p.ewen();
        p.write_word(0x00, 0x1234);
        assert_eq!(&p.ram[0..2], &[0x12, 0x34]);
    }

    #[test]
    fn eeprom_ignores_writes_until_enabled() {
        let mut p = Pins::new();
        // Deliberately no EWEN.
        p.write_word(0x02, 0xAAAA);
        assert_eq!(p.read_word(0x02), 0x0000, "a disabled write must not land");
        p.ewen();
        p.write_word(0x02, 0xAAAA);
        assert_eq!(p.read_word(0x02), 0xAAAA);
    }

    #[test]
    fn eeprom_ewds_disables_again() {
        let mut p = Pins::new();
        p.ewen();
        p.write_word(0x03, 0x1111);
        p.command(0b00, 0b00 << 6); // EWDS
        p.deselect();
        p.write_word(0x03, 0x2222);
        assert_eq!(p.read_word(0x03), 0x1111, "EWDS must re-lock the part");
    }

    #[test]
    fn eeprom_addresses_wrap_at_128_words() {
        // Seven significant address bits, so bit 7 is ignored. Honouring it
        // would index past the 256-byte save.
        let mut p = Pins::new();
        p.ewen();
        p.write_word(0x01, 0xCAFE);
        assert_eq!(p.read_word(0x81), 0xCAFE);
    }

    #[test]
    fn eeprom_erase_sets_a_word_to_ones() {
        let mut p = Pins::new();
        p.ewen();
        p.write_word(0x07, 0x0000);
        p.command(0b11, 0x07); // ERASE
        p.deselect();
        assert_eq!(p.read_word(0x07), 0xFFFF);
    }

    #[test]
    fn accelerometer_reads_centre_until_latched() {
        let mut a = Accel::new();
        a.tilt_x = 1.0;
        // No latch yet: the game must see the erased value, not the new tilt,
        // and the erased value is not the resting one.
        assert_eq!(a.x, ACCEL_ERASED);
        assert_ne!(ACCEL_ERASED, ACCEL_REST);
        a.latch();
        assert_eq!(a.x, ACCEL_REST - 0x70);
    }

    #[test]
    fn accelerometer_tilts_both_ways_and_clamps() {
        let mut a = Accel::new();
        a.tilt_x = -1.0;
        a.tilt_y = 2.0;
        a.latch();
        assert_eq!(a.x, ACCEL_REST + 0x70);
        assert_eq!(a.y, ACCEL_REST - 0xE0);
        // Far past anything physical: must saturate, not wrap. A wrap would
        // read as a hard tilt the other way.
        a.tilt_x = 1000.0;
        a.latch();
        assert_eq!(a.x, 0);
    }
}
