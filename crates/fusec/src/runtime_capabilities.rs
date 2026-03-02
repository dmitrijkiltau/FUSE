use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};

#[derive(Copy, Clone)]
enum DigestAlgo {
    Sha256,
    Sha512,
}

fn parse_digest_algo(raw: &str) -> Option<DigestAlgo> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "sha256" | "sha-256" => Some(DigestAlgo::Sha256),
        "sha512" | "sha-512" => Some(DigestAlgo::Sha512),
        _ => None,
    }
}

pub(crate) fn time_now_unix_ms() -> Result<i64, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("time.now failed: {err}"))?;
    i64::try_from(now.as_millis()).map_err(|_| "time.now overflow".to_string())
}

pub(crate) fn time_sleep_ms(ms: i64) -> Result<(), String> {
    if ms < 0 {
        return Err("time.sleep expects a non-negative Int".to_string());
    }
    let delay =
        u64::try_from(ms).map_err(|_| "time.sleep expects a non-negative Int".to_string())?;
    thread::sleep(Duration::from_millis(delay));
    Ok(())
}

pub(crate) fn time_format_epoch_ms(epoch_ms: i64, fmt: &str) -> Result<String, String> {
    let secs = epoch_ms.div_euclid(1000);
    let millis = epoch_ms.rem_euclid(1000) as u32;
    let nanos = millis
        .checked_mul(1_000_000)
        .ok_or_else(|| "time.format overflow".to_string())?;
    let dt = DateTime::<Utc>::from_timestamp(secs, nanos)
        .ok_or_else(|| "time.format invalid epoch milliseconds".to_string())?;
    Ok(dt.format(fmt).to_string())
}

pub(crate) fn time_parse_epoch_ms(text: &str, fmt: &str) -> Result<i64, String> {
    if let Ok(dt) = DateTime::parse_from_str(text, fmt) {
        return Ok(dt.timestamp_millis());
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(text, fmt) {
        return Ok(dt.and_utc().timestamp_millis());
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, fmt) {
        let Some(midnight) = date.and_hms_opt(0, 0, 0) else {
            return Err("time.parse invalid date".to_string());
        };
        return Ok(midnight.and_utc().timestamp_millis());
    }
    Err(format!("time.parse failed for format `{fmt}`"))
}

pub(crate) fn crypto_hash(algo: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let Some(algo) = parse_digest_algo(algo) else {
        return Err(format!("crypto.hash unsupported algorithm {algo}"));
    };
    let digest = match algo {
        DigestAlgo::Sha256 => Sha256::digest(data).to_vec(),
        DigestAlgo::Sha512 => Sha512::digest(data).to_vec(),
    };
    Ok(digest)
}

pub(crate) fn crypto_hmac(algo: &str, key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let Some(algo) = parse_digest_algo(algo) else {
        return Err(format!("crypto.hmac unsupported algorithm {algo}"));
    };
    let digest = match algo {
        DigestAlgo::Sha256 => {
            type HmacSha256 = Hmac<Sha256>;
            let mut mac = HmacSha256::new_from_slice(key)
                .map_err(|err| format!("crypto.hmac invalid key: {err}"))?;
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }
        DigestAlgo::Sha512 => {
            type HmacSha512 = Hmac<Sha512>;
            let mut mac = HmacSha512::new_from_slice(key)
                .map_err(|err| format!("crypto.hmac invalid key: {err}"))?;
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }
    };
    Ok(digest)
}

pub(crate) fn crypto_random_bytes(len: i64) -> Result<Vec<u8>, String> {
    if len < 0 {
        return Err("crypto.random_bytes expects a non-negative Int".to_string());
    }
    let len =
        usize::try_from(len).map_err(|_| "crypto.random_bytes length is too large".to_string())?;
    let mut out = vec![0u8; len];
    getrandom::fill(&mut out).map_err(|err| format!("crypto.random_bytes failed: {err}"))?;
    Ok(out)
}

pub(crate) fn crypto_constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= *a ^ *b;
    }
    diff == 0
}
