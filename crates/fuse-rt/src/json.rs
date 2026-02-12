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
                out.push_str(&v.to_string());
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
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            _ => out.push(ch),
        }
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
