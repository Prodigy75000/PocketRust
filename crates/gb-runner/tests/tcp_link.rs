//! Validate the TCP link transport over loopback: two endpoints should exchange
//! a byte the same way the in-process LocalLink does.
//!
//! The transport module is compiled into the `gb` binary, so we include it here
//! directly to test it in isolation.

#[path = "../src/tcp_link.rs"]
mod tcp_link;

use gb_core::LinkCable;
use std::net::TcpListener;
use tcp_link::TcpLink;

/// Spin briefly until `f` returns Some, so we don't depend on exact thread timing.
fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
    for _ in 0..200 {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    None
}

#[test]
fn tcp_master_slave_exchange() {
    // Bind an ephemeral port and hand it to a listener thread.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        TcpLink::from_stream(stream).unwrap()
    });

    let mut client = TcpLink::connect(&addr.to_string()).unwrap();
    let mut host = handle.join().unwrap();

    // Host (slave) presents its byte; client (master) clocks a transfer.
    host.set_output(0x3C);
    let received = wait_for(|| {
        let r = client.master_exchange(0xA5);
        if r == 0x3C {
            Some(r)
        } else {
            None
        }
    });
    assert_eq!(received, Some(0x3C), "master should read the slave's byte");

    // The host, as slave, should see the master's byte arrive.
    let slave_in = wait_for(|| host.poll_slave_input());
    assert_eq!(slave_in, Some(0xA5), "slave should receive the master's byte");
}
