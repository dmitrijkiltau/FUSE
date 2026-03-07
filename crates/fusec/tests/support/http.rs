#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

#[derive(Debug)]
pub struct ScriptedHttpExchange {
    pub request_line: String,
    pub request_contains: Vec<String>,
    pub response: String,
}

#[derive(Debug)]
pub struct DelayedHttpExchange {
    pub request_line: String,
    pub request_contains: Vec<String>,
    pub response: String,
    pub delay: Duration,
}

pub fn send_http_request_with_retry(port: u16, request: &str) -> HttpResponse {
    send_http_request_with_retry_for(port, request, Duration::from_secs(6))
}

pub fn send_http_request_with_retry_for(
    port: u16,
    request: &str,
    timeout: Duration,
) -> HttpResponse {
    let start = Instant::now();
    loop {
        let last_error = match TcpStream::connect(("127.0.0.1", port)) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                if let Err(err) = stream.write_all(request.as_bytes()) {
                    format!("write failed: {err}")
                } else {
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    let mut raw = String::new();
                    if let Err(err) = stream.read_to_string(&mut raw) {
                        format!("read failed: {err}")
                    } else if raw.trim().is_empty() {
                        "empty response".to_string()
                    } else {
                        return parse_http_response(&raw);
                    }
                }
            }
            Err(err) => format!("connect failed: {err}"),
        };

        if start.elapsed() > timeout {
            panic!(
                "server did not produce a stable response on 127.0.0.1:{port} (last error: {last_error})"
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub fn send_http_request_status_body_with_retry(port: u16, request: &str) -> (u16, String) {
    let response = send_http_request_with_retry(port, request);
    (response.status, response.body)
}

pub fn spawn_scripted_http_server(
    exchanges: Vec<ScriptedHttpExchange>,
) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind scripted upstream server");
    let port = listener.local_addr().expect("scripted upstream addr").port();
    let handle = thread::spawn(move || {
        for exchange in exchanges {
            let (mut stream, _) = listener.accept().expect("accept scripted upstream request");
            let request = read_http_request(&mut stream);
            let first_line = request.lines().next().unwrap_or("");
            assert_eq!(first_line, exchange.request_line, "upstream request line");
            for needle in &exchange.request_contains {
                assert!(
                    request.contains(needle),
                    "upstream request missing `{needle}` in {request}"
                );
            }
            stream
                .write_all(exchange.response.as_bytes())
                .expect("write scripted upstream response");
        }
    });
    (port, handle)
}

pub fn spawn_delayed_http_server(exchange: DelayedHttpExchange) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind delayed upstream server");
    let port = listener.local_addr().expect("delayed upstream addr").port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept delayed upstream request");
        let request = read_http_request(&mut stream);
        let first_line = request.lines().next().unwrap_or("");
        assert_eq!(first_line, exchange.request_line, "delayed upstream request line");
        for needle in &exchange.request_contains {
            assert!(
                request.contains(needle),
                "delayed upstream request missing `{needle}` in {request}"
            );
        }
        thread::sleep(exchange.delay);
        stream
            .write_all(exchange.response.as_bytes())
            .expect("write delayed upstream response");
    });
    (port, handle)
}

pub fn spawn_scripted_https_server(
    exchanges: Vec<ScriptedHttpExchange>,
) -> (u16, String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind scripted upstream tls server");
    let port = listener.local_addr().expect("scripted upstream tls addr").port();
    let (tls_config, cert_pem) = build_test_tls_identity();
    let tls_config = Arc::new(tls_config);
    let handle = thread::spawn(move || {
        for exchange in exchanges {
            let (tcp_stream, _) = listener.accept().expect("accept scripted upstream tls request");
            let _ = tcp_stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = tcp_stream.set_write_timeout(Some(Duration::from_millis(500)));
            let connection = ServerConnection::new(Arc::clone(&tls_config))
                .expect("create scripted upstream tls server connection");
            let mut stream = StreamOwned::new(connection, tcp_stream);
            let request = read_http_request_from(&mut stream);
            let first_line = request.lines().next().unwrap_or("");
            assert_eq!(first_line, exchange.request_line, "upstream tls request line");
            for needle in &exchange.request_contains {
                assert!(
                    request.contains(needle),
                    "upstream tls request missing `{needle}` in {request}"
                );
            }
            stream
                .write_all(exchange.response.as_bytes())
                .expect("write scripted upstream tls response");
            stream.conn.send_close_notify();
            stream.flush().expect("flush scripted upstream tls response");
        }
    });
    (port, cert_pem, handle)
}

pub fn spawn_handshake_only_https_server() -> (u16, String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind handshake-only tls server");
    let port = listener.local_addr().expect("handshake-only tls addr").port();
    let (tls_config, cert_pem) = build_test_tls_identity();
    let tls_config = Arc::new(tls_config);
    let handle = thread::spawn(move || {
        let (mut tcp_stream, _) = listener.accept().expect("accept handshake-only tls request");
        let _ = tcp_stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = tcp_stream.set_write_timeout(Some(Duration::from_millis(500)));
        let mut connection = ServerConnection::new(tls_config).expect("create tls server connection");
        loop {
            match connection.complete_io(&mut tcp_stream) {
                Ok(_) if !connection.is_handshaking() => break,
                Ok(_) => continue,
                Err(_) => return,
            }
        }
        let mut stream = StreamOwned::new(connection, tcp_stream);
        let _ = read_http_request_from(&mut stream);
    });
    (port, cert_pem, handle)
}

fn build_test_tls_identity() -> (ServerConfig, String) {
    let certified = generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
        .expect("generate test tls certificate");
    let cert_pem = certified.cert.pem();
    let cert_chain = vec![certified.cert.der().clone()];
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        certified.key_pair.serialize_der(),
    ));
    (
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)
            .expect("build test tls server config"),
        cert_pem,
    )
}

fn parse_http_response(raw: &str) -> HttpResponse {
    let mut parts = raw.splitn(2, "\r\n\r\n");
    let head = parts.next().unwrap_or("");
    let body = parts.next().unwrap_or("").trim().to_string();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("500")
        .parse::<u16>()
        .unwrap_or(500);
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    HttpResponse {
        status,
        headers,
        body,
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    read_http_request_from(stream)
}

fn read_http_request_from<R: Read>(stream: &mut R) -> String {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 1024];
    let mut expected_len = None;
    loop {
        let read = stream.read(&mut temp).expect("read upstream request");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
        if expected_len.is_none() {
            if let Some(header_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_text = String::from_utf8_lossy(&buffer[..header_end]);
                let content_length = header_text
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.trim().eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                expected_len = Some(header_end + 4 + content_length);
            }
        }
        if let Some(expected_len) = expected_len {
            if buffer.len() >= expected_len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}
