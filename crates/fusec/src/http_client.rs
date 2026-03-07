use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::interp::Value;

pub const DEFAULT_TIMEOUT_MS: i64 = 30_000;

const HTTP_RESPONSE_STRUCT_NAME: &str = "http.Response";
const HTTP_ERROR_STRUCT_NAME: &str = "http.Error";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpScheme {
    Http,
    Https,
}

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
    scheme: HttpScheme,
    host: String,
    port: u16,
    target: String,
    host_header: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ParsedHttpUrlError {
    InvalidUrl(String),
    UnsupportedScheme(String),
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

#[allow(dead_code)]
pub fn perform_http_request(request: &HttpClientRequest) -> Result<HttpClientResponse, HttpClientError> {
    perform_http_request_with_runtime("unknown", request)
}

pub fn perform_http_request_with_runtime(
    runtime: &str,
    request: &HttpClientRequest,
) -> Result<HttpClientResponse, HttpClientError> {
    let started = Instant::now();
    let method = normalize_http_method(&request.method).map_err(|message| HttpClientError {
        code: "invalid_request".to_string(),
        message,
        method: request.method.clone(),
        url: request.url.clone(),
        status: None,
        headers: HashMap::new(),
        body: None,
    });
    let method = match method {
        Ok(method) => method,
        Err(error) => {
            crate::observability::emit_http_client_observability(runtime, &request.method, &request.url, None, started.elapsed(), 0, Some(&error.code));
            return Err(error);
        }
    };
    let parsed = parse_http_url(&request.url).map_err(|error| match error {
        ParsedHttpUrlError::InvalidUrl(message) => HttpClientError {
            code: "invalid_url".to_string(),
            message,
            method: method.clone(),
            url: request.url.clone(),
            status: None,
            headers: HashMap::new(),
            body: None,
        },
        ParsedHttpUrlError::UnsupportedScheme(message) => HttpClientError {
            code: "unsupported_scheme".to_string(),
            message,
            method: method.clone(),
            url: request.url.clone(),
            status: None,
            headers: HashMap::new(),
            body: None,
        },
    });
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            crate::observability::emit_http_client_observability(runtime, &method, &request.url, None, started.elapsed(), 0, Some(&error.code));
            return Err(error);
        }
    };
    let timeout = timeout_duration(request.timeout_ms).map_err(|message| HttpClientError {
        code: "invalid_request".to_string(),
        message,
        method: method.clone(),
        url: request.url.clone(),
        status: None,
        headers: HashMap::new(),
        body: None,
    });
    let timeout = match timeout {
        Ok(timeout) => timeout,
        Err(error) => {
            crate::observability::emit_http_client_observability(runtime, &method, &request.url, None, started.elapsed(), 0, Some(&error.code));
            return Err(error);
        }
    };
    let headers = normalize_headers(&request.headers).map_err(|message| HttpClientError {
        code: "invalid_request".to_string(),
        message,
        method: method.clone(),
        url: request.url.clone(),
        status: None,
        headers: HashMap::new(),
        body: None,
    });
    let headers = match headers {
        Ok(headers) => headers,
        Err(error) => {
            crate::observability::emit_http_client_observability(runtime, &method, &request.url, None, started.elapsed(), 0, Some(&error.code));
            return Err(error);
        }
    };
    let response = send_http_request(
        &method,
        &request.url,
        &parsed,
        &request.body,
        &headers,
        timeout,
    );
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            let response_bytes = error.body.as_ref().map_or(0, String::len);
            crate::observability::emit_http_client_observability(
                runtime,
                &method,
                &request.url,
                error.status,
                started.elapsed(),
                response_bytes,
                Some(&error.code),
            );
            return Err(error);
        }
    };
    if (200..=299).contains(&response.status) {
        crate::observability::emit_http_client_observability(
            runtime,
            &method,
            &request.url,
            Some(response.status),
            started.elapsed(),
            response.body.len(),
            None,
        );
        Ok(response)
    } else {
        let error = HttpClientError {
            code: "http_status".to_string(),
            message: format!(
                "{} {} returned status {}",
                method.to_ascii_lowercase(),
                request.url,
                response.status
            ),
            method: method.clone(),
            url: request.url.clone(),
            status: Some(response.status),
            headers: response.headers,
            body: Some(response.body),
        };
        crate::observability::emit_http_client_observability(
            runtime,
            &method,
            &request.url,
            error.status,
            started.elapsed(),
            error.body.as_ref().map_or(0, String::len),
            Some(&error.code),
        );
        Err(error)
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

fn parse_http_url(raw: &str) -> Result<ParsedHttpUrl, ParsedHttpUrlError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ParsedHttpUrlError::InvalidUrl(
            "http.* requires a non-empty URL".to_string(),
        ));
    }
    let (scheme, rest, default_port) = if let Some(rest) = trimmed.strip_prefix("http://") {
        (HttpScheme::Http, rest, 80)
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        (HttpScheme::Https, rest, 443)
    } else if let Some((scheme, _)) = trimmed.split_once("://") {
        return Err(ParsedHttpUrlError::UnsupportedScheme(format!(
            "http.* does not support {scheme}:// URLs"
        )));
    } else {
        return Err(ParsedHttpUrlError::InvalidUrl(
            "http.* requires an http:// or https:// URL".to_string(),
        ));
    };
    let split_idx = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    let authority = &rest[..split_idx];
    if authority.is_empty() {
        return Err(ParsedHttpUrlError::InvalidUrl(
            "http.* URL is missing a host".to_string(),
        ));
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
            .ok_or_else(|| {
                ParsedHttpUrlError::InvalidUrl("http.* URL has an invalid IPv6 host".to_string())
            })?;
        let host = authority[..=end].to_string();
        let port = if let Some(rest) = authority.get(end + 1..) {
            if rest.is_empty() {
                default_port
            } else {
                let port = rest
                    .strip_prefix(':')
                    .ok_or_else(|| {
                        ParsedHttpUrlError::InvalidUrl(
                            "http.* URL has an invalid host:port".to_string(),
                        )
                    })?;
                parse_port(port)?
            }
        } else {
            default_port
        };
        (host, port)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || port.is_empty() {
            return Err(ParsedHttpUrlError::InvalidUrl(
                "http.* URL has an invalid host:port".to_string(),
            ));
        }
        if port.chars().all(|ch| ch.is_ascii_digit()) {
            (host.to_string(), parse_port(port)?)
        } else {
            (authority.to_string(), default_port)
        }
    } else {
        (authority.to_string(), default_port)
    };
    if host.is_empty() {
        return Err(ParsedHttpUrlError::InvalidUrl(
            "http.* URL is missing a host".to_string(),
        ));
    }

    let host_header = if port == default_port {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    Ok(ParsedHttpUrl {
        scheme,
        host,
        port,
        target,
        host_header,
    })
}

fn parse_port(raw: &str) -> Result<u16, ParsedHttpUrlError> {
    raw.parse::<u16>()
        .map_err(|_| ParsedHttpUrlError::InvalidUrl(format!("http.* URL has an invalid port {raw}")))
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

enum HttpConnection {
    Tcp(TcpStream),
    Tls(StreamOwned<ClientConnection, TcpStream>),
}

impl Read for HttpConnection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read(buf),
            Self::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for HttpConnection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.write(buf),
            Self::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

fn send_http_request(
    method: &str,
    url: &str,
    parsed: &ParsedHttpUrl,
    body: &str,
    headers: &BTreeMap<String, String>,
    timeout: Option<Duration>,
) -> Result<HttpClientResponse, HttpClientError> {
    let mut stream = match parsed.scheme {
        HttpScheme::Http => HttpConnection::Tcp(connect_tcp_stream(method, url, parsed, timeout)?),
        HttpScheme::Https => HttpConnection::Tls(connect_tls_stream(method, url, parsed, timeout)?),
    };

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
        .map_err(|err| io_error_to_http_error_for_phase(method, url, "write", timeout, err))?;
    stream
        .flush()
        .map_err(|err| io_error_to_http_error_for_phase(method, url, "write", timeout, err))?;
    if let HttpConnection::Tcp(stream) = &mut stream {
        let _ = stream.shutdown(Shutdown::Write);
    }

    let response = read_http_response_bytes(&mut stream)
        .map_err(|err| io_error_to_http_error_for_phase(method, url, "read", timeout, err))?;
    parse_http_response(method, url, &response)
}

fn connect_tcp_stream(
    method: &str,
    url: &str,
    parsed: &ParsedHttpUrl,
    timeout: Option<Duration>,
) -> Result<TcpStream, HttpClientError> {
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
    let stream = stream.ok_or_else(|| {
        io_error_to_http_error_for_phase(
            method,
            url,
            "connect",
            timeout,
            last_error.unwrap_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "no resolved addresses")
            }),
        )
    })?;
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    Ok(stream)
}

fn connect_tls_stream(
    method: &str,
    url: &str,
    parsed: &ParsedHttpUrl,
    timeout: Option<Duration>,
) -> Result<StreamOwned<ClientConnection, TcpStream>, HttpClientError> {
    let tls_config = build_tls_client_config().map_err(|message| HttpClientError {
        code: "tls_error".to_string(),
        message,
        method: method.to_string(),
        url: url.to_string(),
        status: None,
        headers: HashMap::new(),
        body: None,
    })?;
    let server_name = tls_server_name(&parsed.host).map_err(|message| HttpClientError {
        code: "invalid_url".to_string(),
        message,
        method: method.to_string(),
        url: url.to_string(),
        status: None,
        headers: HashMap::new(),
        body: None,
    })?;
    let mut stream = connect_tcp_stream(method, url, parsed, timeout)?;
    let mut connection = ClientConnection::new(tls_config, server_name).map_err(|err| {
        tls_error_to_http_error(method, url, format!("invalid TLS client state: {err}"))
    })?;
    while connection.is_handshaking() {
        match connection.complete_io(&mut stream) {
            Ok(_) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(io_error_to_http_error_for_phase(
                    method,
                    url,
                    "tls_handshake",
                    timeout,
                    err,
                ));
            }
            Err(err) => {
                return Err(tls_error_to_http_error(
                    method,
                    url,
                    format!("TLS handshake failed: {err}"),
                ));
            }
        }
    }
    Ok(StreamOwned::new(connection, stream))
}

fn build_tls_client_config() -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore::empty();
    let native_certs = rustls_native_certs::load_native_certs();
    let load_errors = native_certs
        .errors
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>();
    let (native_added, _) = roots.add_parsable_certificates(native_certs.certs);
    let extra_roots = load_extra_root_certs_from_env()?;
    let (extra_added, _) = roots.add_parsable_certificates(extra_roots);
    let added = native_added + extra_added;
    if added == 0 {
        if load_errors.is_empty() {
            return Err("failed to load any trusted root certificates".to_string());
        }
        return Err(format!(
            "failed to load any trusted root certificates: {}",
            load_errors.join("; ")
        ));
    }
    let provider = rustls::crypto::ring::default_provider();
    let builder = ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("failed to configure TLS protocol versions: {err}"))?;
    Ok(Arc::new(
        builder
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

fn load_extra_root_certs_from_env(
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    let Ok(path) = std::env::var("FUSE_EXTRA_CA_CERT_FILE") else {
        return Ok(Vec::new());
    };
    let pem = std::fs::read(&path)
        .map_err(|err| format!("failed to read extra CA certificate file {path}: {err}"))?;
    let mut cursor = Cursor::new(pem);
    rustls_pemfile::certs(&mut cursor)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("failed to parse PEM certificates from {path}: {err}"))
}

fn tls_server_name(host: &str) -> Result<ServerName<'static>, String> {
    let unbracketed = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = unbracketed.parse::<std::net::IpAddr>() {
        return Ok(ServerName::IpAddress(ip.into()));
    }
    ServerName::try_from(unbracketed.to_string())
        .map_err(|_| format!("http.* URL has an invalid TLS host {unbracketed}"))
}

fn tls_error_to_http_error(method: &str, url: &str, detail: String) -> HttpClientError {
    HttpClientError {
        code: "tls_error".to_string(),
        message: format!("{} {} {detail}", method.to_ascii_lowercase(), url),
        method: method.to_string(),
        url: url.to_string(),
        status: None,
        headers: HashMap::new(),
        body: None,
    }
}

fn io_error_to_http_error_for_phase(
    method: &str,
    url: &str,
    phase: &str,
    timeout: Option<Duration>,
    err: std::io::Error,
) -> HttpClientError {
    if matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        let timeout_suffix = timeout
            .map(|timeout| format!(" after {}ms", timeout.as_millis()))
            .unwrap_or_default();
        return HttpClientError {
            code: "timeout".to_string(),
            message: format!(
                "{} {} timed out during {}{}",
                method.to_ascii_lowercase(),
                url,
                phase_label(phase),
                timeout_suffix,
            ),
            method: method.to_string(),
            url: url.to_string(),
            status: None,
            headers: HashMap::new(),
            body: None,
        };
    }
    HttpClientError {
        code: "network_error".to_string(),
        message: format!(
            "{} {} {} failed: {err}",
            method.to_ascii_lowercase(),
            url,
            phase_label(phase),
        ),
        method: method.to_string(),
        url: url.to_string(),
        status: None,
        headers: HashMap::new(),
        body: None,
    }
}

fn phase_label(phase: &str) -> &str {
    match phase {
        "connect" => "connect",
        "write" => "write",
        "read" => "read",
        "tls_handshake" => "TLS handshake",
        _ => phase,
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

fn read_http_response_bytes<R: Read>(stream: &mut R) -> std::io::Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut expected_len = None;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&chunk[..read]);
                if expected_len.is_none() {
                    expected_len = expected_http_response_len(&response);
                }
                if let Some(expected_len) = expected_len {
                    if response.len() >= expected_len {
                        break;
                    }
                }
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::UnexpectedEof
                    && expected_http_response_len(&response)
                        .is_some_and(|expected_len| response.len() >= expected_len) =>
            {
                break;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(response)
}

fn expected_http_response_len(response: &[u8]) -> Option<usize> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&response[..header_end]);
    let content_length = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })?;
    Some(header_end + 4 + content_length)
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
    use super::{HttpScheme, ParsedHttpUrlError, parse_http_response, parse_http_url};

    #[test]
    fn parse_http_url_supports_default_path_and_query() {
        let parsed = parse_http_url("http://example.com:8080/api/users?limit=1")
            .expect("parse url");
        assert_eq!(parsed.scheme, HttpScheme::Http);
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 8080);
        assert_eq!(parsed.target, "/api/users?limit=1");
        assert_eq!(parsed.host_header, "example.com:8080");
    }

    #[test]
    fn parse_http_url_supports_https_default_port() {
        let parsed = parse_http_url("https://example.com/api").expect("parse https url");
        assert_eq!(parsed.scheme, HttpScheme::Https);
        assert_eq!(parsed.host, "example.com");
        assert_eq!(parsed.port, 443);
        assert_eq!(parsed.target, "/api");
        assert_eq!(parsed.host_header, "example.com");
    }

    #[test]
    fn parse_http_url_rejects_unsupported_scheme() {
        let err = parse_http_url("ftp://example.com").expect_err("unsupported scheme");
        assert_eq!(
            err,
            ParsedHttpUrlError::UnsupportedScheme(
                "http.* does not support ftp:// URLs".to_string()
            )
        );
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
