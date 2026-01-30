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
