//! A TCP link-cable transport: implements [`gb_core::LinkCable`] over a socket
//! so two PocketRust instances can play a linked game across the network.
//!
//! Protocol: fixed 2-byte messages `[tag, byte]`.
//!   tag 0 = "I set my SB to `byte`" (the value the peer reads when it clocks).
//!   tag 1 = "I am master and clocked `byte` into you" (peer becomes the slave).
//!
//! A background thread drains the socket into shared state, so the emulation
//! loop never blocks: a master exchange returns the peer's last-known SB
//! immediately and notifies the peer to raise its serial interrupt. This matches
//! how turn-based link games work (the slave presents its byte before the master
//! clocks), and it keeps both machines running at full speed.

use gb_core::LinkCable;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

struct Shared {
    /// The peer's most recently presented SB byte.
    peer_output: u8,
    /// Bytes the peer (as master) clocked into us, awaiting pickup.
    slave_inputs: VecDeque<u8>,
}

pub struct TcpLink {
    stream: TcpStream,
    shared: Arc<Mutex<Shared>>,
}

impl TcpLink {
    /// Wait for an incoming connection on `port` (the "host" side of the link).
    pub fn listen(port: u16) -> std::io::Result<TcpLink> {
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        println!("link: waiting for a peer on port {port}...");
        let (stream, addr) = listener.accept()?;
        println!("link: peer connected from {addr}");
        TcpLink::from_stream(stream)
    }

    /// Connect to a listening peer at `addr` (e.g. "192.168.1.5:5150").
    pub fn connect(addr: &str) -> std::io::Result<TcpLink> {
        let stream = TcpStream::connect(addr)?;
        println!("link: connected to {addr}");
        TcpLink::from_stream(stream)
    }

    pub fn from_stream(stream: TcpStream) -> std::io::Result<TcpLink> {
        stream.set_nodelay(true).ok();
        let shared = Arc::new(Mutex::new(Shared {
            peer_output: 0xFF,
            slave_inputs: VecDeque::new(),
        }));

        // Reader thread: drain incoming messages into the shared state.
        let mut reader = stream.try_clone()?;
        let shared_r = shared.clone();
        std::thread::spawn(move || {
            let mut msg = [0u8; 2];
            while reader.read_exact(&mut msg).is_ok() {
                let mut s = shared_r.lock().unwrap();
                match msg[0] {
                    0 => s.peer_output = msg[1],
                    1 => s.slave_inputs.push_back(msg[1]),
                    _ => {}
                }
            }
            // Socket closed: nothing more will arrive.
        });

        Ok(TcpLink { stream, shared })
    }

    fn send(&mut self, tag: u8, byte: u8) {
        // Best-effort: if the peer has gone, the game just sees open-bus bytes.
        let _ = self.stream.write_all(&[tag, byte]);
    }
}

impl LinkCable for TcpLink {
    fn set_output(&mut self, byte: u8) {
        self.send(0, byte);
    }

    fn master_exchange(&mut self, out: u8) -> u8 {
        self.send(1, out); // tell the peer it just received our byte
        self.shared.lock().unwrap().peer_output
    }

    fn poll_slave_input(&mut self) -> Option<u8> {
        self.shared.lock().unwrap().slave_inputs.pop_front()
    }
}
