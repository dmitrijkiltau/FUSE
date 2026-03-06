use std::collections::BTreeMap;
use std::env;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

use fuse_rt::json as rt_json;

use super::{ColorChoice, Command, DiagnosticsFormat};

static COLOR_MODE: AtomicU8 = AtomicU8::new(0);
static DIAGNOSTIC_MODE: AtomicU8 = AtomicU8::new(0);

pub(crate) fn apply_color_choice(choice: ColorChoice) {
    let mode = match choice {
        ColorChoice::Always => 2,
        ColorChoice::Never => 0,
        ColorChoice::Auto => {
            if env::var_os("NO_COLOR").is_some() {
                0
            } else if color_auto_is_tty() {
                1
            } else {
                0
            }
        }
    };
    COLOR_MODE.store(mode, Ordering::Relaxed);
    unsafe {
        env::set_var("FUSE_COLOR", choice.as_env_value());
    }
}

pub(crate) fn apply_diagnostics_format(format: DiagnosticsFormat) {
    let mode = match format {
        DiagnosticsFormat::Text => 0,
        DiagnosticsFormat::Json => 1,
    };
    DIAGNOSTIC_MODE.store(mode, Ordering::Relaxed);
    match format {
        DiagnosticsFormat::Text => unsafe {
            env::remove_var("FUSE_DIAGNOSTICS");
        },
        DiagnosticsFormat::Json => unsafe {
            env::set_var("FUSE_DIAGNOSTICS", format.as_env_value());
        },
    }
}

pub(crate) fn diagnostics_json_enabled() -> bool {
    DIAGNOSTIC_MODE.load(Ordering::Relaxed) == 1
}

fn color_auto_is_tty() -> bool {
    if let Some(force) = env::var_os("FUSE_COLOR_FORCE_TTY") {
        return force == "1";
    }
    std::io::stderr().is_terminal()
}

fn color_enabled() -> bool {
    COLOR_MODE.load(Ordering::Relaxed) != 0
}

fn ansi_paint(text: &str, code: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub(crate) fn style_error(text: &str) -> String {
    ansi_paint(text, "31;1")
}

pub(crate) fn style_warning(text: &str) -> String {
    ansi_paint(text, "33;1")
}

pub(crate) fn style_header(text: &str) -> String {
    ansi_paint(text, "36;1")
}

pub(crate) fn emit_cli_error(message: &str) {
    if diagnostics_json_enabled() {
        emit_json_line(cli_message_json("error", message));
        return;
    }
    eprintln!("{}", style_error(&format!("error: {message}")));
}

pub(crate) fn emit_cli_warning(message: &str) {
    if diagnostics_json_enabled() {
        emit_json_line(cli_message_json("warning", message));
        return;
    }
    eprintln!("{}", style_warning(&format!("warning: {message}")));
}

pub(crate) fn dev_prefix() -> String {
    style_header("[dev]")
}

fn command_tag(command: Command) -> Option<&'static str> {
    match command {
        Command::Run => Some("run"),
        Command::Check => Some("check"),
        Command::Build => Some("build"),
        Command::Test => Some("test"),
        Command::Clean => Some("clean"),
        Command::Dev | Command::Fmt | Command::Openapi | Command::Migrate => None,
    }
}

fn command_prefix(command: Command) -> Option<String> {
    command_tag(command).map(|tag| style_header(&format!("[{tag}]")))
}

pub(crate) fn emit_command_step(command: Command, message: &str) {
    if let Some(tag) = command_tag(command) {
        if diagnostics_json_enabled() {
            emit_json_line(command_step_json(tag, message));
            return;
        }
    }
    if let Some(prefix) = command_prefix(command) {
        eprintln!("{prefix} {message}");
    }
}

pub(crate) fn emit_build_progress(message: &str) {
    emit_command_step(Command::Build, message);
}

pub(crate) fn emit_aot_build_progress(stage: usize, message: &str) {
    emit_build_progress(&format!(
        "aot [{stage}/{AOT_BUILD_PROGRESS_STAGES}] {message}",
        AOT_BUILD_PROGRESS_STAGES = super::AOT_BUILD_PROGRESS_STAGES
    ));
}

pub(crate) fn finalize_command(command: Command, code: i32) -> i32 {
    match code {
        0 => emit_command_step(command, "ok"),
        2 => emit_command_step(command, "validation failed"),
        _ => emit_command_step(command, "failed"),
    }
    code
}

pub(crate) fn emit_usage() {
    if diagnostics_json_enabled() {
        return;
    }
    eprintln!("{}", style_header(super::USAGE));
}

pub(crate) fn emit_json_line(value: rt_json::JsonValue) {
    eprintln!("{}", rt_json::encode(&value));
}

fn cli_message_json(level: &str, message: &str) -> rt_json::JsonValue {
    let mut object = BTreeMap::new();
    object.insert(
        "kind".to_string(),
        rt_json::JsonValue::String("cli_message".to_string()),
    );
    object.insert(
        "level".to_string(),
        rt_json::JsonValue::String(level.to_string()),
    );
    object.insert(
        "message".to_string(),
        rt_json::JsonValue::String(message.to_string()),
    );
    rt_json::JsonValue::Object(object)
}

fn command_step_json(command: &str, message: &str) -> rt_json::JsonValue {
    let mut object = BTreeMap::new();
    object.insert(
        "kind".to_string(),
        rt_json::JsonValue::String("command_step".to_string()),
    );
    object.insert(
        "command".to_string(),
        rt_json::JsonValue::String(command.to_string()),
    );
    object.insert(
        "message".to_string(),
        rt_json::JsonValue::String(message.to_string()),
    );
    rt_json::JsonValue::Object(object)
}

pub(crate) fn emit_diags(diags: &[fusec::diag::Diag]) {
    emit_diags_with_fallback(diags, None);
}

pub(crate) fn emit_diags_with_fallback(
    diags: &[fusec::diag::Diag],
    fallback: Option<(&Path, &str)>,
) {
    for diag in diags {
        emit_diag(diag, fallback);
    }
}

fn emit_diag(diag: &fusec::diag::Diag, fallback: Option<(&Path, &str)>) {
    if diagnostics_json_enabled() {
        emit_json_line(fusec::diag_render::diagnostic_json_value(diag, fallback));
        return;
    }
    let style = fusec::diag_render::TextDiagnosticStyle {
        error_label: style_error("error"),
        warning_label: style_warning("warning"),
        caret: style_error("^"),
        include_fallback_path: true,
    };
    fusec::diag_render::emit_diag_text(diag, fallback, &style);
}

pub(crate) fn line_info(src: &str, offset: usize) -> (usize, usize, &str) {
    fusec::diag_render::line_info(src, offset)
}
