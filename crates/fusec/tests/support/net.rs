use std::net::TcpListener;
use std::sync::OnceLock;

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
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind free port");
    listener.local_addr().expect("missing local addr").port()
}
