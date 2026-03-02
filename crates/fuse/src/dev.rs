use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fuse_rt::json as rt_json;

use super::{Manifest, RunBackend};

pub fn run_dev(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
    app: Option<&str>,
    backend: Option<RunBackend>,
    strict_architecture: bool,
) -> i32 {
    let reload = match ReloadHub::start() {
        Ok(reload) => reload,
        Err(err) => {
            super::emit_cli_error(&format!("dev error: {err}"));
            return 1;
        }
    };
    let ws_url = reload.ws_url();
    eprintln!("{} live reload websocket: {ws_url}", super::dev_prefix());

    if let Err(err) = super::assets::run_asset_pipeline(manifest, manifest_dir) {
        eprintln!("{} {}", super::dev_prefix(), super::style_error(&err));
    }

    let mut snapshot = build_dev_snapshot(entry, manifest, manifest_dir, deps);
    let mut child = match first_dev_compile_error(entry, deps, strict_architecture) {
        Some(message) => {
            reload.broadcast_compile_error(&message);
            eprintln!(
                "{} compile error; waiting for changes...",
                super::dev_prefix()
            );
            None
        }
        None => match spawn_dev_child(
            entry,
            manifest_dir,
            app,
            backend,
            strict_architecture,
            &ws_url,
        ) {
            Ok(child) => Some(child),
            Err(err) => {
                super::emit_cli_error(&err);
                None
            }
        },
    };
    let mut child_exit_reported = false;

    loop {
        thread::sleep(Duration::from_millis(300));

        if let Some(proc) = child.as_mut() {
            match proc.try_wait() {
                Ok(Some(status)) => {
                    if !child_exit_reported {
                        eprintln!(
                            "{} app exited ({status}); waiting for changes...",
                            super::dev_prefix()
                        );
                        child_exit_reported = true;
                    }
                    child = None;
                }
                Ok(None) => {}
                Err(err) => {
                    if !child_exit_reported {
                        eprintln!("{} failed to poll app process: {err}", super::dev_prefix());
                        child_exit_reported = true;
                    }
                    child = None;
                }
            }
        }

        let next_snapshot = build_dev_snapshot(entry, manifest, manifest_dir, deps);
        if next_snapshot == snapshot {
            continue;
        }

        snapshot = next_snapshot;
        eprintln!("{} change detected, restarting...", super::dev_prefix());
        if let Err(err) = super::assets::run_asset_pipeline(manifest, manifest_dir) {
            eprintln!("{} {}", super::dev_prefix(), super::style_error(&err));
        }
        child_exit_reported = false;
        if let Some(mut proc) = child.take() {
            let _ = proc.kill();
            let _ = proc.wait();
        }
        match first_dev_compile_error(entry, deps, strict_architecture) {
            Some(message) => {
                reload.broadcast_compile_error(&message);
                eprintln!(
                    "{} compile error; waiting for changes...",
                    super::dev_prefix()
                );
            }
            None => match spawn_dev_child(
                entry,
                manifest_dir,
                app,
                backend,
                strict_architecture,
                &ws_url,
            ) {
                Ok(proc) => {
                    child = Some(proc);
                    reload.broadcast_clear_error();
                    reload.broadcast_reload();
                }
                Err(err) => {
                    super::emit_cli_error(&err);
                }
            },
        }
    }
}

fn spawn_dev_child(
    entry: &Path,
    manifest_dir: Option<&Path>,
    app: Option<&str>,
    backend: Option<RunBackend>,
    strict_architecture: bool,
    ws_url: &str,
) -> Result<Child, String> {
    let exe = std::env::current_exe().map_err(|err| format!("dev error: current exe: {err}"))?;
    let mut cmd = Command::new(exe);
    cmd.arg("run");
    if let Some(dir) = manifest_dir {
        cmd.arg("--manifest-path");
        cmd.arg(dir);
    }
    cmd.arg("--file");
    cmd.arg(entry);
    if let Some(name) = app {
        cmd.arg("--app");
        cmd.arg(name);
    }
    if let Some(backend) = backend {
        cmd.arg("--backend");
        cmd.arg(backend.as_str());
    }
    if strict_architecture {
        cmd.arg("--strict-architecture");
    }
    cmd.env("FUSE_DEV_MODE", "1");
    cmd.env("FUSE_DEV_RELOAD_WS_URL", ws_url);
    cmd.spawn()
        .map_err(|err| format!("dev error: failed to start app: {err}"))
}

fn first_dev_compile_error(
    entry: &Path,
    deps: &HashMap<String, PathBuf>,
    strict_architecture: bool,
) -> Option<String> {
    let src = match fs::read_to_string(entry) {
        Ok(src) => src,
        Err(err) => {
            let message = format!("failed to read {}: {err}", entry.display());
            super::emit_cli_error(&message);
            return Some(message);
        }
    };
    let (registry, load_diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    if !load_diags.is_empty() {
        let first = format_diag_summary(&load_diags[0], Some((entry, &src)));
        super::emit_diags_with_fallback(&load_diags, Some((entry, &src)));
        return Some(first);
    }
    let (_analysis, sema_diags) = fusec::sema::analyze_registry_with_options(
        &registry,
        fusec::sema::AnalyzeOptions {
            strict_architecture,
        },
    );
    if !sema_diags.is_empty() {
        let first = format_diag_summary(&sema_diags[0], Some((entry, &src)));
        super::emit_diags_with_fallback(&sema_diags, Some((entry, &src)));
        return Some(first);
    }
    None
}

fn format_diag_summary(diag: &fusec::diag::Diag, fallback: Option<(&Path, &str)>) -> String {
    if let Some(path) = &diag.path {
        if let Ok(src) = fs::read_to_string(path) {
            let (line, col, _) = super::line_info(&src, diag.span.start);
            return format!("{}:{}:{}: {}", path.display(), line, col, diag.message);
        }
        return format!("{}: {}", path.display(), diag.message);
    }
    if let Some((path, src)) = fallback {
        let (line, col, _) = super::line_info(src, diag.span.start);
        return format!("{}:{}:{}: {}", path.display(), line, col, diag.message);
    }
    diag.message.clone()
}

#[derive(Clone, Default, Eq, PartialEq)]
struct DevSnapshot {
    files: BTreeMap<PathBuf, Option<super::FileStamp>>,
}

fn build_dev_snapshot(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
) -> DevSnapshot {
    let files = collect_dev_watch_files(entry, manifest, manifest_dir, deps);
    let mut stamps = BTreeMap::new();
    for file in files {
        stamps.insert(file.clone(), super::file_stamp(&file).ok());
    }
    DevSnapshot { files: stamps }
}

fn collect_dev_watch_files(
    entry: &Path,
    manifest: Option<&Manifest>,
    manifest_dir: Option<&Path>,
    deps: &HashMap<String, PathBuf>,
) -> BTreeSet<PathBuf> {
    let mut out = collect_module_files_for_dev(entry, deps);
    if out.is_empty() {
        out.insert(entry.to_path_buf());
        if let Some(base) = manifest_dir.or_else(|| entry.parent()) {
            super::assets::collect_files_by_extension(base, &["fuse"], &mut out);
        }
    }
    if let Some(base) = manifest_dir.or_else(|| entry.parent()) {
        if let Some(assets) = manifest.and_then(|m| m.assets.as_ref()) {
            if assets.watch != Some(false) {
                if let Some(css) = assets.css.as_ref() {
                    let path = super::assets::resolve_manifest_relative_path(base, css);
                    if path.is_dir() {
                        super::assets::collect_files_by_extension(&path, &["css"], &mut out);
                    } else if path.is_file() {
                        out.insert(path.clone());
                        if let Some(parent) = path.parent() {
                            super::assets::collect_files_by_extension(parent, &["css"], &mut out);
                        }
                    }
                }
            }
        }
    }
    out
}

fn collect_module_files_for_dev(
    entry: &Path,
    deps: &HashMap<String, PathBuf>,
) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let src = match fs::read_to_string(entry) {
        Ok(src) => src,
        Err(_) => return out,
    };
    let (registry, _diags) = fusec::load_program_with_modules_and_deps(entry, &src, deps);
    for unit in registry.modules.values() {
        if unit.path.exists() {
            out.insert(unit.path.clone());
        }
    }
    out
}

struct ReloadHub {
    addr: String,
    clients: Arc<Mutex<Vec<TcpStream>>>,
}

impl ReloadHub {
    fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|err| format!("failed to bind reload websocket: {err}"))?;
        let addr = listener
            .local_addr()
            .map_err(|err| format!("failed to read reload websocket address: {err}"))?;
        let clients = Arc::new(Mutex::new(Vec::new()));
        let thread_clients = Arc::clone(&clients);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                handle_reload_client(stream, &thread_clients);
            }
        });
        Ok(Self {
            addr: addr.to_string(),
            clients,
        })
    }

    fn ws_url(&self) -> String {
        format!("ws://{}/__reload", self.addr)
    }

    fn broadcast_reload(&self) {
        self.broadcast_message("reload");
    }

    fn broadcast_clear_error(&self) {
        let payload = websocket_reload_event_payload("clear_error", None);
        self.broadcast_message(&payload);
    }

    fn broadcast_compile_error(&self, message: &str) {
        let payload = websocket_reload_event_payload("compile_error", Some(message));
        self.broadcast_message(&payload);
    }

    fn broadcast_message(&self, payload: &str) {
        let frame = websocket_text_frame(payload);
        let mut clients = match self.clients.lock() {
            Ok(clients) => clients,
            Err(_) => return,
        };
        let mut idx = 0usize;
        while idx < clients.len() {
            if clients[idx].write_all(&frame).is_err() {
                clients.remove(idx);
            } else {
                idx += 1;
            }
        }
    }
}

fn websocket_reload_event_payload(kind: &str, message: Option<&str>) -> String {
    let mut payload = BTreeMap::new();
    payload.insert(
        "type".to_string(),
        rt_json::JsonValue::String(kind.to_string()),
    );
    if let Some(message) = message {
        payload.insert(
            "message".to_string(),
            rt_json::JsonValue::String(message.to_string()),
        );
    }
    rt_json::encode(&rt_json::JsonValue::Object(payload))
}

fn handle_reload_client(mut stream: TcpStream, clients: &Arc<Mutex<Vec<TcpStream>>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let header = match read_http_header(&mut stream) {
        Ok(header) => header,
        Err(_) => return,
    };
    let header = String::from_utf8_lossy(&header);
    let mut lines = header.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if method != "GET" || !path.starts_with("/__reload") {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let mut upgrade = false;
    let mut connection_upgrade = false;
    let mut ws_key = None::<String>;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            match name.as_str() {
                "upgrade" if value.eq_ignore_ascii_case("websocket") => {
                    upgrade = true;
                }
                "connection" if value.to_ascii_lowercase().contains("upgrade") => {
                    connection_upgrade = true;
                }
                "sec-websocket-key" => {
                    ws_key = Some(value.to_string());
                }
                _ => {}
            }
        }
    }
    let Some(ws_key) = ws_key else {
        let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
        return;
    };
    if !upgrade || !connection_upgrade {
        let _ = stream.write_all(b"HTTP/1.1 426 Upgrade Required\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let accept = websocket_accept_value(&ws_key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(response.as_bytes()).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_nonblocking(true);
    if let Ok(mut guard) = clients.lock() {
        guard.push(stream);
    }
}

fn read_http_header(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 1024];
    loop {
        let read = stream.read(&mut temp)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() >= 16 * 1024 {
            break;
        }
    }
    Ok(buffer)
}

fn websocket_accept_value(key: &str) -> String {
    let mut combined = String::new();
    combined.push_str(key.trim());
    combined.push_str("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let digest = super::sha1_digest(combined.as_bytes());
    fuse_rt::bytes::encode_base64(&digest)
}

fn websocket_text_frame(payload: &str) -> Vec<u8> {
    let bytes = payload.as_bytes();
    let mut frame = Vec::with_capacity(bytes.len() + 10);
    frame.push(0x81);
    match bytes.len() {
        len if len <= 125 => frame.push(len as u8),
        len if len <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            frame.push(127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(bytes);
    frame
}
