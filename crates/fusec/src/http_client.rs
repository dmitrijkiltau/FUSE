use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::interp::Value;

pub const DEFAULT_TIMEOUT_MS: i64 = 30_000;

const HTTP_RESPONSE_STRUCT_NAME: &str = "http.Response";
const HTTP_ERROR_STRUCT_NAME: &str = "http.Error";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpBuiltin {
    Request,
    Get,
    Post,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpClientRequest {
    pub method: String,
    pub url: String,
    pub body: String,
    pub headers: HashMap<String, String>,
    pub timeout_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpClientResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpClientError {
    pub code: String,
    pub message: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedHttpUrl {
    host: String,
    port: u16,
    target: String,
    host_header: String,
}

pub fn parse_http_builtin_args(
    builtin: HttpBuiltin,
    args: &[Value],
) -> Result<HttpClientRequest, String> {
    match builtin {
        HttpBuiltin::Request => {
            if !(2..=5).contains(&args.len()) {
                return Err("http.request expects 2 to 5 arguments".to_string());
            }
            let method = expect_string_arg(&args[0], "http.request expects method as String")?;
            let url = expect_string_arg(&args[1], "http.request expects url as String")?;
            let body = if let Some(value) = args.get(2) {
                expect_string_arg(value, "http.request expects body as String")?
            } else {
                String::new()
            };
            let headers = if let Some(value) = args.get(3) {
                expect_string_map_arg(value, "http.request expects headers as Map<String, String>")?
            } else {
                HashMap::new()
            };
            let timeout_ms = if let Some(value) = args.get(4) {
                expect_int_arg(value, "http.request expects timeout_ms as Int")?
            } else {
                DEFAULT_TIMEOUT_MS
            };
            Ok(HttpClientRequest {
                method,
                url,
                body,
                headers,
                timeout_ms,
            })
        }
        HttpBuiltin::Get => {
            if !(1..=3).contains(&args.len()) {
                return Err("http.get expects 1 to 3 arguments".to_string());
            }
            let url = expect_string_arg(&args[0], "http.get expects url as String")?;
            let headers = if let Some(value) = args.get(1) {
                expect_string_map_arg(value, "http.get expects headers as Map<String, String>")?
            } else {
                HashMap::new()
            };
            let timeout_ms = if let Some(value) = args.get(2) {
                expect_int_arg(value, "http.get expects timeout_ms as Int")?
            } else {
                DEFAULT_TIMEOUT_MS
            };
            Ok(HttpClientRequest {
                method: "GET".to_string(),
                url,
                body: String::new(),
                headers,
                timeout_ms,
            })
        }
        HttpBuiltin::Post => {
            if !(2..=4).contains(&args.len()) {
                return Err("http.post expects 2 to 4 arguments".to_string());
            }
            let url = expect_string_arg(&args[0], "http.post expects url as String")?;
            let body = expect_string_arg(&args[1], "http.post expects body as String")?;
            let headers = if let Some(value) = args.get(2) {
                expect_string_map_arg(value, "http.post expects headers as Map<String, String>")?
            } else {
                HashMap::new()
            };
            let timeout_ms = if let Some(value) = args.get(3) {
                expect_int_arg(value, "http.post expects timeout_ms as Int")?
            } else {
                DEFAULT_TIMEOUT_MS
            };
            Ok(HttpClientRequest {
                method: "POST".to_string(),
                url,
                body,
                headers,
                timeout_ms,
            })
        }
    }
}

pub fn perform_http_request(request: &HttpClientRequest) -> Result<HttpClientResponse, HttpClientError> {
    let method = normalize_http_method(&request.method).map_err(|message| HttpClientError {
        code: "invalid_request".to_string(),
        message,
        method: request.method.clone(),
        url: request.url.clone(),
        status: None,
        headers: HashMap::new(),
        body: None,
    })?;
    if request.url.trim().starts_with("https://") {
        return Err(HttpClientError {
            code: "unsupported_scheme".to_string(),
            message: "http.* only supports http:// URLs in 0.9.6".to_string(),
            method,
            url: request.url.clone(),
            status: None,
            headers: HashMap::new(),
            body: None,
        });
    }
    let parsed = parse_http_url(&request.url).map_err(|message| HttpClientError {
        code: "invalid_url".to_string(),
        message,
        method: method.clone(),
        url: request.url.clone(),
        status: None,
        headers: HashMap::new(),
        body: None,
    })?;
    let timeout = timeout_duration(request.timeout_ms).map_err(|message| HttpClientError {
        code: "invalid_request".to_string(),
        message,
        method: method.clone(),
        url: request.url.clone(),
        status: None,
        headers: HashMap::new(),
        body: None,
    })?;
    let headers = normalize_headers(&request.headers).map_err(|message| HttpClientError {
        code: "invalid_request".to_string(),
        message,
        method: method.clone(),
        url: request.url.clone(),
        status: None,
        headers: HashMap::new(),
        body: None,
    })?;
    let response = send_http_request(
        &method,
        &request.url,
        &parsed,
        &request.body,
        &headers,
        timeout,
    )?;
    if (200..=299).contains(&response.status) {
        Ok(response)
    } else {
        Err(HttpClientError {
            code: "http_status".to_string(),
            message: format!(
                "{} {} returned status {}",
                method.to_ascii_lowercase(),
                request.url,
                response.status
            ),
            method,
            url: request.url.clone(),
            status: Some(response.status),
            headers: response.headers,
            body: Some(response.body),
        })
    }
}

pub fn http_response_value(
    method: &str,
    url: &str,
    response: HttpClientResponse,
) -> Value {
    let mut fields = HashMap::new();
    fields.insert("method".to_string(), Value::String(method.to_string()));
    fields.insert("url".to_string(), Value::String(url.to_string()));
    fields.insert("status".to_string(), Value::Int(i64::from(response.status)));
    fields.insert("headers".to_string(), string_map_to_value(response.headers));
    fields.insert("body".to_string(), Value::String(response.body));
    Value::Struct {
        name: HTTP_RESPONSE_STRUCT_NAME.to_string(),
        fields,
    }
}

pub fn http_error_value(error: HttpClientError) -> Value {
    let mut fields = HashMap::new();
    fields.insert("code".to_string(), Value::String(error.code));
    fields.insert("message".to_string(), Value::String(error.message));
    fields.insert("method".to_string(), Value::String(error.method));
    fields.insert("url".to_string(), Value::String(error.url));
    fields.insert("status".to_string(), match error.status {
        Some(status) => Value::Int(i64::from(status)),
        None => Value::Null,
    });
    fields.insert("headers".to_string(), string_map_to_value(error.headers));
    fields.insert("body".to_string(), match error.body {
        Some(body) => Value::String(body),
        None => Value::Null,
    });
    Value::Struct {
        name: HTTP_ERROR_STRUCT_NAME.to_string(),
        fields,
    }
}

fn expect_string_arg(value: &Value, message: &str) -> Result<String, String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        _ => Err(message.to_string()),
    }
}

fn expect_string_map_arg(value: &Value, message: &str) -> Result<HashMap<String, String>, String> {
    let Value::Map(items) = value else {
        return Err(message.to_string());
    };
    let mut out = HashMap::with_capacity(items.len());
    for (key, value) in items {
        let Value::String(text) = value else {
            return Err(message.to_string());
        };
        out.insert(key.clone(), text.clone());
    }
    Ok(out)
}

fn expect_int_arg(value: &Value, message: &str) -> Result<i64, String> {
    match value {
        Value::Int(value) => Ok(*value),
        _ => Err(message.to_string()),
    }
}

fn normalize_http_method(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("http.request requires a non-empty method".to_string());
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_ascii_whitespace() || ch.is_ascii_control())
    {
        return Err("http.request rejects whitespace/control characters in method".to_string());
    }
    Ok(trimmed.to_ascii_uppercase())
}

fn parse_http_url(raw: &str) -> Result<ParsedHttpUrl, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("http.* requires a non-empty URL".to_string());
    }
    if trimmed.starts_with("https://") {
        return Err("http.* only supports http:// URLs in 0.9.6".to_string());
    }
    let rest = trimmed
        .strip_prefix("http://")
        .ok_or_else(|| "http.* requires an http:// URL".to_string())?;
    let split_idx = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    let authority = &rest[..split_idx];
    if authority.is_empty() {
        return Err("http.* URL is missing a host".to_string());
    }
    let mut remainder = &rest[split_idx..];
    if let Some((before_fragment, _)) = remainder.split_once('#') {
        remainder = before_fragment;
    }
    let target = if remainder.is_empty() {
        "/".to_string()
    } else if remainder.starts_with('/') {
        remainder.to_string()
    } else {
        format!("/{remainder}")
    };

    let (host, port) = if authority.starts_with('[') {
        let end = authority
            .find(']')
            .ok_or_else(|| "http.* URL has an invalid IPv6 host".to_string())?;
        let host = authority[..=end].to_string();
        let port = if let Some(rest) = authority.get(end + 1..) {
            if rest.is_empty() {
                80
            } else {
                let port = rest
                    .strip_prefix(':')
                    .ok_or_else(|| "http.* URL has an invalid host:port".to_string())?;
                parse_port(port)?
            }
        } else {
            80
        };
        (host, port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || port.is_empty() {
            return Err("http.* URL has an invalid host:port".to_string());
        }
        if port.chars().all(|ch| ch.is_ascii_digit()) {
            (host.to_string(), parse_port(port)?)
        } else {
            (authority.to_string(), 80)
        }
    } else {
        (authority.to_string(), 80)
    };
    if host.is_empty() {
        return Err("http.* URL is missing a host".to_string());
    }

    let host_header = if port == 80 {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    Ok(ParsedHttpUrl {
        host,
        port,
        target,
        host_header,
    })
}

fn parse_port(raw: &str) -> Result<u16, String> {
    raw.parse::<u16>()
        .map_err(|_| format!("http.* URL has an invalid port {raw}"))
}

fn timeout_duration(timeout_ms: i64) -> Result<Option<Duration>, String> {
    if timeout_ms < 0 {
        return Err("http.* timeout_ms must be >= 0".to_string());
    }
    if timeout_ms == 0 {
        return Ok(None);
    }
    let millis = u64::try_from(timeout_ms)
        .map_err(|_| "http.* timeout_ms is out of range".to_string())?;
    Ok(Some(Duration::from_millis(millis)))
}

fn normalize_headers(raw: &HashMap<String, String>) -> Result<BTreeMap<String, String>, String> {
    let mut headers = BTreeMap::new();
    for (name, value) in raw {
        validate_http_header(name, value)?;
        let lower = name.to_ascii_lowercase();
        if matches!(lower.as_str(), "host" | "connection" | "content-length") {
            return Err(format!("http.* manages header {lower} automatically"));
        }
        headers.insert(lower, value.clone());
    }
    Ok(headers)
}

fn validate_http_header(name: &str, value: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("http.* headers require a non-empty name".to_string());
    }
    if name.chars().any(|ch| ch == '\r' || ch == '\n' || ch == ':') {
        return Err("http.* headers reject control characters and ':' in names".to_string());
    }
    if value.chars().any(|ch| ch == '\r' || ch == '\n') {
        return Err("http.* headers reject CR/LF characters in values".to_string());
    }
    Ok(())
}

fn send_http_request(
    method: &str,
    url: &str,
    parsed: &ParsedHttpUrl,
    body: &str,
    headers: &BTreeMap<String, String>,
    timeout: Option<Duration>,
) -> Result<HttpClientResponse, HttpClientError> {
    let address_text = format!("{}:{}", parsed.host, parsed.port);
    let mut addrs = address_text.to_socket_addrs().map_err(|err| HttpClientError {
        code: "network_error".to_string(),
        message: format!("failed to resolve {}: {err}", parsed.host),
        method: method.to_string(),
        url: url.to_string(),
        status: None,
        headers: HashMap::new(),
        body: None,
    })?;
    let mut stream = None;
    let mut last_error = None;
    for addr in addrs.by_ref() {
        let candidate = match timeout {
            Some(duration) => TcpStream::connect_timeout(&addr, duration),
            None => TcpStream::connect(addr),
        };
        match candidate {
            Ok(stream_value) => {
                stream = Some(stream_value);
                break;
            }
            Err(err) => last_error = Some(err),
        }
    }
    let mut stream = stream.ok_or_else(|| io_error_to_http_error(
        method,
        url,
        last_error.unwrap_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no resolved addresses")
        }),
    ))?;
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);

    let body_bytes = body.as_bytes();
    let mut request_bytes = Vec::new();
    request_bytes.extend_from_slice(
        format!(
            "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            parsed.target,
            parsed.host_header,
            body_bytes.len()
        )
        .as_bytes(),
    );
    for (name, value) in headers {
        request_bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    request_bytes.extend_from_slice(b"\r\n");
    request_bytes.extend_from_slice(body_bytes);

    stream
        .write_all(&request_bytes)
        .map_err(|err| io_error_to_http_error(method, url, err))?;
    let _ = stream.shutdown(Shutdown::Write);

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|err| io_error_to_http_error(method, url, err))?;
    parse_http_response(method, url, &response)
}

fn io_error_to_http_error(method: &str, url: &str, err: std::io::Error) -> HttpClientError {
    let code = if matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        "timeout"
    } else {
        "network_error"
    };
    HttpClientError {
        code: code.to_string(),
        message: format!("{} {} failed: {err}", method.to_ascii_lowercase(), url),
        method: method.to_string(),
        url: url.to_string(),
        status: None,
        headers: HashMap::new(),
        body: None,
    }
}

fn parse_http_response(
    method: &str,
    url: &str,
    response: &[u8],
) -> Result<HttpClientResponse, HttpClientError> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| HttpClientError {
            code: "invalid_response".to_string(),
            message: format!("{} {} returned an invalid HTTP response", method.to_ascii_lowercase(), url),
            method: method.to_string(),
            url: url.to_string(),
            status: None,
            headers: HashMap::new(),
            body: None,
        })?;
    let head = String::from_utf8_lossy(&response[..header_end]);
    let body = String::from_utf8_lossy(&response[header_end + 4..]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| HttpClientError {
            code: "invalid_response".to_string(),
            message: format!("{} {} returned an invalid status line", method.to_ascii_lowercase(), url),
            method: method.to_string(),
            url: url.to_string(),
            status: None,
            headers: HashMap::new(),
            body: None,
        })?;
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(HttpClientError {
                code: "invalid_response".to_string(),
                message: format!(
                    "{} {} returned an invalid header line",
                    method.to_ascii_lowercase(),
                    url
                ),
                method: method.to_string(),
                url: url.to_string(),
                status: Some(status),
                headers: HashMap::new(),
                body: Some(body),
            });
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Ok(HttpClientResponse {
        status,
        headers,
        body,
    })
}

fn string_map_to_value(items: HashMap<String, String>) -> Value {
    let mut out = HashMap::with_capacity(items.len());
    for (key, value) in items {
        out.insert(key, Value::String(value));
    }
    Value::Map(out)
}

#[cfg(test)]
mod tests {
    use super::{parse_http_url, parse_http_response};

    #[test]
    fn parse_http_url_supports_default_path_and_query() {
        let parsed = parse_http_url("http://example.com:8080/api/users?limit=1")
            .expect("parse url");
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 8080);
        assert_eq!(parsed.target, "/api/users?limit=1");
        assert_eq!(parsed.host_header, "example.com:8080");
    }

    #[test]
    fn parse_http_url_rejects_https() {
        let err = parse_http_url("https://example.com").expect_err("https rejected");
        assert!(err.contains("http://"), "unexpected error: {err}");
    }

    #[test]
    fn parse_http_response_reads_status_headers_and_body() {
        let raw = b"HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\nX-Test: yes\r\n\r\nok";
        let response = parse_http_response("GET", "http://example.com", raw)
            .expect("parse response");
        assert_eq!(response.status, 201);
        assert_eq!(
            response.headers.get("content-type").map(String::as_str),
            Some("text/plain")
        );
        assert_eq!(response.headers.get("x-test").map(String::as_str), Some("yes"));
        assert_eq!(response.body, "ok");
    }
}
