use std::collections::HashMap;

pub fn env_key(config: &str, field: &str) -> String {
    format!("{}_{}", to_env_key(config), to_env_key(field))
}

fn to_env_key(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            if !out.ends_with('_') {
                out.push('_');
            }
            prev_lower = false;
            continue;
        }
        let is_upper = ch.is_ascii_uppercase();
        if is_upper && prev_lower {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    out
}

pub fn load_config_file(path: &str) -> Result<HashMap<String, HashMap<String, String>>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(format!("failed to read config file: {err}")),
    };
    Ok(parse_toml_config(&content))
}

fn parse_toml_config(input: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current: Option<String> = None;
    for raw_line in input.lines() {
        let line = strip_comment(raw_line).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let section = line.trim_start_matches('[').trim_end_matches(']').trim();
            if !section.is_empty() {
                current = Some(section.to_string());
                out.entry(section.to_string()).or_default();
            }
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let key = line[..eq].trim();
        let value = line[eq + 1..].trim();
        if key.is_empty() {
            continue;
        }
        let parsed = parse_value(value);
        let section = current.clone().unwrap_or_else(|| "".to_string());
        out.entry(section)
            .or_default()
            .insert(key.to_string(), parsed);
    }
    out
}

fn strip_comment(line: &str) -> String {
    let mut out = String::new();
    let mut in_string = false;
    let mut escape = false;
    for ch in line.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            out.push(ch);
            continue;
        }
        if ch == '#' && !in_string {
            break;
        }
        out.push(ch);
    }
    out
}

fn parse_value(raw: &str) -> String {
    let raw = raw.trim();
    if raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2 {
        return unescape_string(&raw[1..raw.len() - 1]);
    }
    raw.to_string()
}

fn unescape_string(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\"') => out.push('\"'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}
