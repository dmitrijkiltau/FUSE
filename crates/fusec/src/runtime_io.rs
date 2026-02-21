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
