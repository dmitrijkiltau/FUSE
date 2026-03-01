#![allow(dead_code)]

use std::net::TcpListener;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

static PORT_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn can_bind_loopback() -> bool {
    static CAN_BIND: OnceLock<bool> = OnceLock::new();
    *CAN_BIND.get_or_init(|| match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(err) => panic!("failed to probe loopback bind capability: {err}"),
    })
}

pub fn skip_if_loopback_unavailable(test_name: &str) -> bool {
    if can_bind_loopback() {
        return false;
    }
    eprintln!("skipping {test_name}: loopback bind is not permitted in this environment");
    true
}

pub fn find_free_port() -> u16 {
    const PORT_START: u16 = 20_000;
    const PORT_SPAN: u16 = 30_000;
    let pid_offset = (std::process::id() as u16) % PORT_SPAN;
    for _ in 0..PORT_SPAN {
        let seq = PORT_COUNTER.fetch_add(1, Ordering::Relaxed) as u16;
        let candidate = PORT_START + (pid_offset.wrapping_add(seq) % PORT_SPAN);
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", candidate)) {
            drop(listener);
            return candidate;
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind free port");
    listener.local_addr().expect("missing local addr").port()
}
