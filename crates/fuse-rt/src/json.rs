use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

pub fn encode(value: &JsonValue) -> String {
    let mut out = String::new();
    encode_value(value, &mut out);
    out
}

fn encode_value(value: &JsonValue, out: &mut String) {
    match value {
        JsonValue::Null => out.push_str("null"),
        JsonValue::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
        JsonValue::Number(v) => {
            if v.is_finite() {
                // Fast path: if the value is a whole number within i64 range,
                // write it as an integer (no decimal point, no scientific
                // notation, no heap allocation beyond what `out` already has).
                let i = *v as i64;
                if i as f64 == *v {
                    encode_i64(i, out);
                } else {
                    out.push_str(&v.to_string());
                }
            } else {
                out.push_str("null");
            }
        }
        JsonValue::String(v) => encode_string(v, out),
        JsonValue::Array(items) => {
            out.push('[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                encode_value(item, out);
            }
            out.push(']');
        }
        JsonValue::Object(map) => {
            out.push('{');
            for (idx, (key, value)) in map.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                encode_string(key, out);
                out.push(':');
                encode_value(value, out);
            }
            out.push('}');
        }
    }
}

fn encode_string(value: &str, out: &mut String) {
    // Pre-reserve: at minimum we need len + 2 for the surrounding quotes.
    out.reserve(value.len() + 2);
    out.push('"');
    // Byte-level scan: bulk-copy unescaped segments and only emit escape
    // sequences for the 7 characters that JSON requires escaping.  All
    // multi-byte UTF-8 sequences have bytes >= 0x80 and are never among the
    // 7 special ASCII bytes, so scanning byte-by-byte is safe and correct.
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let escaped = match bytes[i] {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0x08 => "\\b",
            0x0C => "\\f",
            _ => {
                i += 1;
                continue;
            }
        };
        if start < i {
            out.push_str(&value[start..i]);
        }
        out.push_str(escaped);
        i += 1;
        start = i;
    }
    if start < bytes.len() {
        out.push_str(&value[start..]);
    }
    out.push('"');
}

pub fn decode(input: &str) -> Result<JsonValue, String> {
    let mut parser = Parser::new(input);
    let value = parser.parse_value()?;
    parser.skip_ws();
    if parser.eof() {
        Ok(value)
    } else {
        Err("trailing characters".to_string())
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn skip_ws(&mut self) {
        while let Some(ch) = self.peek() {
            if ch == b' ' || ch == b'\n' || ch == b'\t' || ch == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_value(&mut self) -> Result<JsonValue, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'n') => self.parse_null(),
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'"') => self.parse_string().map(JsonValue::String),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(_) => Err("unexpected character".to_string()),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue, String> {
        if self.consume_bytes(b"null") {
            Ok(JsonValue::Null)
        } else {
            Err("invalid null".to_string())
        }
    }

    fn parse_bool(&mut self) -> Result<JsonValue, String> {
        if self.consume_bytes(b"true") {
            Ok(JsonValue::Bool(true))
        } else if self.consume_bytes(b"false") {
            Ok(JsonValue::Bool(false))
        } else {
            Err("invalid bool".to_string())
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        let slice = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "invalid number".to_string())?;
        let value = slice
            .parse::<f64>()
            .map_err(|_| "invalid number".to_string())?;
        Ok(JsonValue::Number(value))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        if self.bump() != Some(b'"') {
            return Err("expected string".to_string());
        }
        let mut out = String::new();
        while let Some(ch) = self.bump() {
            match ch {
                b'"' => return Ok(out),
                b'\\' => {
                    let esc = self.bump().ok_or_else(|| "invalid escape".to_string())?;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let code = self.parse_hex4()?;
                            if let Some(ch) = std::char::from_u32(code) {
                                out.push(ch);
                            } else {
                                return Err("invalid unicode escape".to_string());
                            }
                        }
                        _ => return Err("invalid escape".to_string()),
                    }
                }
                _ => out.push(ch as char),
            }
        }
        Err("unterminated string".to_string())
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let ch = self
                .bump()
                .ok_or_else(|| "invalid unicode escape".to_string())?;
            value = value * 16
                + match ch {
                    b'0'..=b'9' => (ch - b'0') as u32,
                    b'a'..=b'f' => (ch - b'a' + 10) as u32,
                    b'A'..=b'F' => (ch - b'A' + 10) as u32,
                    _ => return Err("invalid unicode escape".to_string()),
                };
        }
        Ok(value)
    }

    fn parse_array(&mut self) -> Result<JsonValue, String> {
        if self.bump() != Some(b'[') {
            return Err("expected [".to_string());
        }
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                break;
            }
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err("expected ',' or ']'".to_string()),
            }
        }
        Ok(JsonValue::Array(items))
    }

    fn parse_object(&mut self) -> Result<JsonValue, String> {
        if self.bump() != Some(b'{') {
            return Err("expected {".to_string());
        }
        let mut map = BTreeMap::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                break;
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.bump() != Some(b':') {
                return Err("expected ':'".to_string());
            }
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err("expected ',' or '}'".to_string()),
            }
        }
        Ok(JsonValue::Object(map))
    }

    fn consume_bytes(&mut self, expected: &[u8]) -> bool {
        if self.bytes.len() < self.pos + expected.len() {
            return false;
        }
        if &self.bytes[self.pos..self.pos + expected.len()] == expected {
            self.pos += expected.len();
            true
        } else {
            false
        }
    }
}

/// Write an i64 into `out` using a fixed 20-byte stack buffer.
/// Avoids a heap allocation for the common case of integer-valued JSON numbers
/// (IDs, counts, status codes, etc.).
fn encode_i64(mut value: i64, out: &mut String) {
    // i64::MIN needs special handling because negating it overflows.
    if value == i64::MIN {
        out.push_str("-9223372036854775808");
        return;
    }
    let negative = value < 0;
    if negative {
        value = -value;
    }
    // Max i64 is 19 digits; 20 bytes is enough for sign + digits.
    let mut buf = [0u8; 20];
    let mut end = buf.len();
    loop {
        end -= 1;
        buf[end] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    if negative {
        end -= 1;
        buf[end] = b'-';
    }
    // SAFETY: buf contains only ASCII digits and an optional '-'.
    out.push_str(unsafe { std::str::from_utf8_unchecked(&buf[end..]) });
}
