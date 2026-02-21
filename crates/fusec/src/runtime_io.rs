use std::io::{self, BufRead, IsTerminal, Write};

pub(crate) fn read_input_line(prompt: &str) -> Result<String, String> {
    if !prompt.is_empty() {
        print!("{prompt}");
        io::stdout()
            .flush()
            .map_err(|err| format!("input prompt flush failed: {err}"))?;
    }

    let stdin = io::stdin();
    let non_interactive = !stdin.is_terminal();
    let mut line = String::new();
    let read = stdin
        .lock()
        .read_line(&mut line)
        .map_err(|err| format!("input read failed: {err}"))?;
    if read == 0 {
        if non_interactive {
            return Err("input requires stdin data in non-interactive mode".to_string());
        }
        return Err("input aborted: reached EOF".to_string());
    }

    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    Ok(line)
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum RuntimeColorChoice {
    Auto,
    Always,
    Never,
}

impl RuntimeColorChoice {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

pub(crate) fn format_log_text_line(level: &str, message: &str) -> String {
    format!("[{}] {}", style_log_level(level), message)
}

fn style_log_level(level: &str) -> String {
    let code = match level {
        "TRACE" => "90",
        "DEBUG" => "36",
        "INFO" => "32",
        "WARN" => "33;1",
        "ERROR" => "31;1",
        _ => return level.to_string(),
    };
    ansi_paint(level, code)
}

fn ansi_paint(text: &str, code: &str) -> String {
    if runtime_color_enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn runtime_color_enabled() -> bool {
    match runtime_color_choice() {
        RuntimeColorChoice::Always => true,
        RuntimeColorChoice::Never => false,
        RuntimeColorChoice::Auto => {
            if std::env::var_os("NO_COLOR").is_some() {
                false
            } else {
                stderr_is_tty()
            }
        }
    }
}

fn runtime_color_choice() -> RuntimeColorChoice {
    std::env::var("FUSE_COLOR")
        .ok()
        .and_then(|raw| RuntimeColorChoice::parse(&raw))
        .unwrap_or(RuntimeColorChoice::Auto)
}

fn stderr_is_tty() -> bool {
    if let Some(force) = std::env::var_os("FUSE_COLOR_FORCE_TTY") {
        return force == "1";
    }
    io::stderr().is_terminal()
}
