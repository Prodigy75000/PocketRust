//! A minimal windowed frontend: load a ROM, run it at ~60 fps, draw the screen,
//! and map the keyboard to the Game Boy's buttons.
//!
//!   cargo run --release -p gb-runner -- path/to/rom.gb
//!
//! Controls: arrows = D-pad, Z = A, X = B, Enter = Start, Right-Shift = Select.

mod tcp_link;

use gb_core::{Button, GameBoy, SCREEN_H, SCREEN_W};
use minifb::{Key, Scale, Window, WindowOptions};
use tcp_link::TcpLink;

fn main() {
    // Args: <rom.gb> [--link-listen <port> | --link-connect <host:port>]
    let mut path = None;
    let mut link_listen = None;
    let mut link_connect = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--link-listen" => link_listen = args.next().and_then(|p| p.parse::<u16>().ok()),
            "--link-connect" => link_connect = args.next(),
            other => path = Some(other.to_string()),
        }
    }
    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("usage: gb <rom.gb> [--link-listen <port> | --link-connect <host:port>]");
            std::process::exit(1);
        }
    };
    let rom = std::fs::read(&path).expect("failed to read ROM");
    let mut gb = GameBoy::new(rom.clone());
    println!("Loaded: {}", gb.title());

    // Optionally bring up the link cable over TCP.
    if let Some(port) = link_listen {
        match TcpLink::listen(port) {
            Ok(link) => gb.connect_link(Box::new(link)),
            Err(e) => eprintln!("link listen failed: {e}"),
        }
    } else if let Some(addr) = link_connect {
        match TcpLink::connect(&addr) {
            Ok(link) => gb.connect_link(Box::new(link)),
            Err(e) => eprintln!("link connect failed: {e}"),
        }
    }

    let mut reset_held = false;

    let mut window = Window::new(
        &format!("gb-core \u{2014} {}", gb.title()),
        SCREEN_W,
        SCREEN_H,
        WindowOptions {
            scale: Scale::X4,
            ..WindowOptions::default()
        },
    )
    .expect("failed to open window");
    window.set_target_fps(60);

    let mut buffer = vec![0u32; SCREEN_W * SCREEN_H];

    while window.is_open() && !window.is_key_down(Key::Escape) {
        // Feed input.
        let map = [
            (Key::Right, Button::Right),
            (Key::Left, Button::Left),
            (Key::Up, Button::Up),
            (Key::Down, Button::Down),
            (Key::Z, Button::A),
            (Key::X, Button::B),
            (Key::Enter, Button::Start),
            (Key::RightShift, Button::Select),
        ];
        for (key, button) in map {
            gb.set_button(button, window.is_key_down(key));
        }

        // R resets the machine (rising edge only, so holding it doesn't loop).
        let reset_now = window.is_key_down(Key::R);
        if reset_now && !reset_held {
            let sram = gb.sram().to_vec();
            gb = GameBoy::new(rom.clone());
            if gb.has_battery() {
                gb.load_sram(&sram);
            }
        }
        reset_held = reset_now;

        // Run one frame; the framebuffer is already RGB888.
        buffer.copy_from_slice(gb.step_frame());

        window
            .update_with_buffer(&buffer, SCREEN_W, SCREEN_H)
            .expect("failed to update window");
    }
}
