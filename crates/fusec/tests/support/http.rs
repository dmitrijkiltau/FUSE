#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

pub fn send_http_request_with_retry(port: u16, request: &str) -> HttpResponse {
    send_http_request_with_retry_for(port, request, Duration::from_secs(3))
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
