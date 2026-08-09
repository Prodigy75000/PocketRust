//! Headless audio recorder: run a ROM, skip a warm-up, then capture N frames of
//! audio to a 16-bit stereo WAV. Also prints peak/RMS so we have hard numbers.
//!
//!   cargo run --release -p gb-runner --bin rec -- <rom.gb> <warmup> <frames> <out.wav>

use gb_core::{Button, GameBoy, SAMPLE_RATE};
use std::fs::File;
use std::io::{BufWriter, Write};

/// Pulse Start (a quarter of every 16 frames), the same shape `smoke` uses. A
/// game parked on a "press start" screen is often silent there, so measuring
/// its audio means getting past it. Off by default: a recording with input in
/// it is not reproducible frame-for-frame in the way a passive one is.
fn start_pulse(gb: &mut GameBoy, frame: u32, enabled: bool) {
    if enabled {
        gb.set_button(Button::Start, frame % 16 < 4);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rom_path = args.next().expect("usage: rec <rom.gb> <warmup> <frames> <out.wav>");
    let warmup: u32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(0);
    let frames: u32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(300);
    let out = args.next().unwrap_or_else(|| "out.wav".into());

    let rom = std::fs::read(&rom_path).expect("failed to read ROM");
    let mut gb = GameBoy::new(rom);
    println!("Loaded '{}': warmup {warmup}, recording {frames} frames", gb.title());

    let mash = std::env::var("RECSTART").is_ok();
    for f in 0..warmup {
        start_pulse(&mut gb, f, mash);
        gb.step_frame();
        gb.take_audio(); // discard
    }

    let mut samples: Vec<i16> = Vec::new();
    for f in 0..frames {
        start_pulse(&mut gb, warmup + f, mash);
        gb.step_frame();
        samples.extend(gb.take_audio());
    }

    // Report measurable facts (leave the aesthetic judgment to the listener).
    let peak = samples.iter().map(|s| s.unsigned_abs() as u64).max().unwrap_or(0);
    let rms = if samples.is_empty() {
        0.0
    } else {
        let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        (sum / samples.len() as f64).sqrt()
    };
    let nonzero = samples.iter().filter(|&&s| s != 0).count();
    println!(
        "{} stereo frames, peak={peak}/32767, rms={rms:.0}, nonzero={}/{}",
        samples.len() / 2,
        nonzero,
        samples.len()
    );

    write_wav(&out, &samples).expect("write wav");
    println!("Wrote {out}");
}

/// Write a minimal canonical 16-bit PCM stereo WAV.
fn write_wav(path: &str, samples: &[i16]) -> std::io::Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    let channels: u16 = 2;
    let bits: u16 = 16;
    let byte_rate = SAMPLE_RATE * channels as u32 * (bits / 8) as u32;
    let block_align = channels * (bits / 8);
    let data_bytes = (samples.len() * 2) as u32;

    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_bytes).to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?; // PCM chunk size
    w.write_all(&1u16.to_le_bytes())?; // PCM format
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&SAMPLE_RATE.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;
    for &s in samples {
        w.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}
