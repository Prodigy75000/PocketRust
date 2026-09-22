//! Count motor transitions while a cartridge runs. Diagnostic only.
use gb_core::GameBoy;
fn main() {
    let mut a = std::env::args().skip(1);
    let rom = std::fs::read(a.next().expect("usage: rumbleprobe <rom> [script]")).unwrap();
    let script = a.next().unwrap_or_default();
    let mut gb = GameBoy::new(rom);
    println!("has_rumble = {}", gb.has_rumble());
    if !script.is_empty() {
        gb_runner::play(&mut gb, &script);
    }
    // Launch the ball, then work the flippers. A pinball table with the ball
    // sitting in the plunger lane is not a test of anything.
    use gb_core::Button;
    gb.set_button(Button::Down, true);
    for _ in 0..90 { gb.step_frame(); }
    gb.set_button(Button::Down, false);

    let (mut on, mut edges, mut frames_on) = (false, 0u32, 0u32);
    for i in 0..3600u32 {
        // Mash both flippers on alternating beats.
        gb.set_button(Button::Left, i % 40 < 8);
        gb.set_button(Button::A, i % 40 < 8);
        gb.set_button(Button::Right, i % 40 >= 20 && i % 40 < 28);
        gb.set_button(Button::B, i % 40 >= 20 && i % 40 < 28);
        gb.step_frame();
        let now = gb.rumble();
        if now != on {
            edges += 1;
            on = now;
        }
        if now {
            frames_on += 1;
        }
    }
    println!("edges = {edges}, frames buzzing = {frames_on} of 3600");
}
