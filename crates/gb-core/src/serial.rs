//! The serial port (the link cable).
//!
//! Two Game Boys exchange one byte at a time. Whichever side uses its *internal*
//! clock (SC bit 0 = 1) is the master and drives the 8-bit shift; the other side
//! (external clock) is the slave. After 8 bits both SB registers have swapped and
//! both request a serial interrupt.
//!
//! The core is agnostic about how the peer is reached: it talks to a
//! [`LinkCable`] transport. [`LocalLink`] wires two cores together in one
//! process (used for tests and same-machine play); a networked transport
//! implements the same trait.

use crate::save::Cursor;

/// T-cycles for one byte at the normal 8192 Hz shift clock (8 bits / 8192 Hz).
const TRANSFER_CYCLES: i32 = 4096;

/// A link-cable transport. One instance is held by each connected core.
pub trait LinkCable {
    /// Present `byte` on the wire as this side's current SB (what the peer reads
    /// if it clocks a transfer).
    fn set_output(&mut self, byte: u8);
    /// This side is master and finished clocking a byte out: deliver `out` to the
    /// peer and return the byte the peer had presented.
    fn master_exchange(&mut self, out: u8) -> u8;
    /// This side is slave: return a byte if the master clocked one into us since
    /// the last poll.
    fn poll_slave_input(&mut self) -> Option<u8>;
}

pub struct Serial {
    /// SB (0xFF01): the transfer data / shift register.
    pub data: u8,
    /// SC (0xFF02): control. Bit 7 start, bit 1 fast (CGB), bit 0 internal clock.
    control: u8,
    /// Cycles remaining in an active master transfer (<= 0 when idle).
    remaining: i32,
    active: bool,
    pub interrupt: bool,
    /// Bytes sent with no peer attached: captured for test ROMs and debugging.
    pub log: Vec<u8>,
    link: Option<Box<dyn LinkCable>>,
}

impl Serial {
    pub fn new() -> Serial {
        Serial {
            data: 0,
            control: 0,
            remaining: 0,
            active: false,
            interrupt: false,
            log: Vec::new(),
            link: None,
        }
    }

    pub fn connect(&mut self, link: Box<dyn LinkCable>) {
        self.link = Some(link);
    }
    pub fn disconnect(&mut self) {
        self.link = None;
    }
    pub fn is_linked(&self) -> bool {
        self.link.is_some()
    }

    pub fn read_data(&self) -> u8 {
        self.data
    }
    pub fn write_data(&mut self, v: u8) {
        self.data = v;
        if let Some(link) = &mut self.link {
            link.set_output(v);
        }
    }

    pub fn read_control(&self) -> u8 {
        // Bits 6..1 read as 1.
        self.control | 0x7E
    }
    pub fn write_control(&mut self, v: u8) {
        self.control = v;
        if v & 0x80 != 0 && v & 0x01 != 0 {
            // Start requested with internal clock: we are the master.
            self.active = true;
            self.remaining = if v & 0x02 != 0 {
                TRANSFER_CYCLES / 32 // CGB fast clock
            } else {
                TRANSFER_CYCLES
            };
        }
    }

    pub fn step(&mut self, cycles: u32) {
        // As slave: pick up a byte the master clocked into us.
        if self.control & 0x80 != 0 && self.control & 0x01 == 0 {
            if let Some(link) = &mut self.link {
                if let Some(incoming) = link.poll_slave_input() {
                    self.data = incoming;
                    if let Some(link) = &mut self.link {
                        link.set_output(self.data);
                    }
                    self.control &= !0x80;
                    self.interrupt = true;
                }
            }
        }

        if !self.active {
            return;
        }
        self.remaining -= cycles as i32;
        if self.remaining > 0 {
            return;
        }
        // Master transfer complete: exchange with the peer (or open bus).
        self.active = false;
        let out = self.data;
        let received = match &mut self.link {
            Some(link) => link.master_exchange(out),
            None => {
                self.log.push(out); // no peer: behave like a debugger sink
                0xFF
            }
        };
        self.data = received;
        if let Some(link) = &mut self.link {
            link.set_output(self.data);
        }
        self.control &= !0x80;
        self.interrupt = true;
    }

    pub(crate) fn transfer<C: Cursor>(&mut self, c: &mut C) {
        c.u8(&mut self.data);
        c.u8(&mut self.control);
        c.i32(&mut self.remaining);
        c.bool(&mut self.active);
        c.bool(&mut self.interrupt);
        // The link connection and debug log are not part of the save state.
    }
}

// --- Local (in-process) link -------------------------------------------------

use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct Wire {
    /// Each side's currently-presented output byte (its SB).
    output: [u8; 2],
    /// A byte the peer clocked into each side, awaiting pickup.
    input: [Option<u8>; 2],
}

/// One end of a same-process link cable.
pub struct LocalLink {
    wire: Rc<RefCell<Wire>>,
    side: usize,
}

/// Create a connected pair of local link endpoints for two cores.
pub fn local_pair() -> (LocalLink, LocalLink) {
    let wire = Rc::new(RefCell::new(Wire {
        output: [0xFF; 2],
        input: [None; 2],
    }));
    (
        LocalLink {
            wire: wire.clone(),
            side: 0,
        },
        LocalLink { wire, side: 1 },
    )
}

impl LinkCable for LocalLink {
    fn set_output(&mut self, byte: u8) {
        self.wire.borrow_mut().output[self.side] = byte;
    }
    fn master_exchange(&mut self, out: u8) -> u8 {
        let peer = 1 - self.side;
        let mut wire = self.wire.borrow_mut();
        let received = wire.output[peer];
        wire.input[peer] = Some(out); // deliver our byte to the slave
        received
    }
    fn poll_slave_input(&mut self) -> Option<u8> {
        self.wire.borrow_mut().input[self.side].take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_slave_byte_exchange() {
        let (a_link, b_link) = local_pair();
        let mut a = Serial::new();
        let mut b = Serial::new();
        a.connect(Box::new(a_link));
        b.connect(Box::new(b_link));

        // A is master (internal clock), B is slave (external clock). Both load
        // a byte and arm a transfer.
        a.write_data(0xA5);
        b.write_data(0x3C);
        b.write_control(0x80); // start, external clock (slave)
        a.write_control(0x81); // start, internal clock (master)

        // Run enough cycles for the master transfer to complete.
        for _ in 0..(TRANSFER_CYCLES / 4 + 1) {
            a.step(4);
            b.step(4);
        }

        assert_eq!(a.data, 0x3C, "master should receive slave's byte");
        assert_eq!(b.data, 0xA5, "slave should receive master's byte");
        assert!(a.interrupt, "master should raise a serial interrupt");
        assert!(b.interrupt, "slave should raise a serial interrupt");
        assert_eq!(a.read_control() & 0x80, 0, "transfer flag should clear");
        assert_eq!(b.read_control() & 0x80, 0, "transfer flag should clear");
    }
}
