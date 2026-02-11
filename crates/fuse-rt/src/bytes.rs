const BASE64_STD_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.len() % 4 != 0 {
        return Err("invalid base64 length".to_string());
    }

    let bytes = trimmed.as_bytes();
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for (chunk_idx, chunk) in bytes.chunks_exact(4).enumerate() {
        let mut vals = [0u8; 4];
        let mut padding = 0usize;
        for (i, ch) in chunk.iter().copied().enumerate() {
            if ch == b'=' {
                vals[i] = 0;
                padding += 1;
                continue;
            }
            if padding > 0 {
                return Err(format!("invalid base64 padding at chunk {chunk_idx}"));
            }
            vals[i] = decode_base64_char(ch)
                .ok_or_else(|| format!("invalid base64 character '{}'", ch as char))?;
        }

        if padding > 2 {
            return Err(format!("invalid base64 padding at chunk {chunk_idx}"));
        }

        out.push((vals[0] << 2) | (vals[1] >> 4));
        if padding < 2 {
            out.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if padding == 0 {
            out.push((vals[2] << 6) | vals[3]);
        }
    }

    Ok(out)
}

pub fn encode_base64(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);

        out.push(BASE64_STD_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(BASE64_STD_ALPHABET[((b0 & 0x03) << 4 | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_STD_ALPHABET[((b1 & 0x0f) << 2 | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_STD_ALPHABET[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn decode_base64_char(ch: u8) -> Option<u8> {
    match ch {
        b'A'..=b'Z' => Some(ch - b'A'),
        b'a'..=b'z' => Some(ch - b'a' + 26),
        b'0'..=b'9' => Some(ch - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_base64, encode_base64};

    #[test]
    fn roundtrip_base64() {
        let src = b"fuse-runtime";
        let encoded = encode_base64(src);
        assert_eq!(encoded, "ZnVzZS1ydW50aW1l");
        let decoded = decode_base64(&encoded).expect("decode failed");
        assert_eq!(decoded, src);
    }

    #[test]
    fn decode_rejects_malformed_padding() {
        assert!(decode_base64("A=A=").is_err());
    }
}
