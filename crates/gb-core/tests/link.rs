//! End-to-end link-cable test.
//!
//! Blargg's cpu_instrs ROM prints its result over the serial port as the *master*
//! (internal clock). When a link transport is attached, those bytes must travel
//! through the full `GameBoy -> MMU -> Serial -> LinkCable` path instead of the
//! internal debug log. We attach a recording cable and confirm the real ROM's
//! "Passed" text arrives byte-for-byte over the link.

use gb_core::{GameBoy, LinkCable};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// A link endpoint that just records every byte the master clocks out.
struct Recorder(Rc<RefCell<Vec<u8>>>);

impl LinkCable for Recorder {
    fn set_output(&mut self, _byte: u8) {}
    fn master_exchange(&mut self, out: u8) -> u8 {
        self.0.borrow_mut().push(out);
        0xFF
    }
    fn poll_slave_input(&mut self) -> Option<u8> {
        None
    }
}

fn rom(rel: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/roms")
        .join(rel);
    std::fs::read(path).unwrap()
}

#[test]
fn serial_output_travels_over_the_link() {
    let mut gb = GameBoy::new(rom("cpu_instrs/06.gb"));
    let recorded = Rc::new(RefCell::new(Vec::new()));
    gb.connect_link(Box::new(Recorder(recorded.clone())));
    assert!(gb.link_connected());

    let mut text = String::new();
    for _ in 0..4000 {
        gb.step_frame();
        text = String::from_utf8_lossy(&recorded.borrow()).to_string();
        if text.contains("Passed") || text.contains("Failed") {
            break;
        }
    }

    assert!(
        text.contains("Passed"),
        "expected the ROM's serial output to arrive over the link; got: {text:?}"
    );
}
