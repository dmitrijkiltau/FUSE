#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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
