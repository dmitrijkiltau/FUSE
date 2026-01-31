pub fn format_source(src: &str) -> String {
    if src.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for raw_line in src.split_terminator('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let trimmed = line.trim_end_matches(|c| c == ' ' || c == '\t');
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        let mut indent = String::new();
        let mut rest_index = 0;
        for (idx, ch) in trimmed.char_indices() {
            match ch {
                ' ' => {
                    indent.push(' ');
                    rest_index = idx + ch.len_utf8();
                }
                '\t' => {
                    indent.push(' ');
                    rest_index = idx + ch.len_utf8();
                }
                _ => {
                    rest_index = idx;
                    break;
                }
            }
        }
        let rest = &trimmed[rest_index..];
        out.push_str(&indent);
        out.push_str(rest);
        out.push('\n');
    }
    out
}
