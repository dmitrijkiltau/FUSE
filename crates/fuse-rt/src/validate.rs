pub fn is_email(value: &str) -> bool {
    let mut parts = value.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    if local.is_empty() || domain.is_empty() || parts.next().is_some() {
        return false;
    }
    domain.contains('.')
}

pub fn check_len(len: i64, min: i64, max: i64) -> bool {
    len >= min && len <= max
}

pub fn check_int_range(value: i64, min: i64, max: i64) -> bool {
    value >= min && value <= max
}

pub fn check_float_range(value: f64, min: f64, max: f64) -> bool {
    value >= min && value <= max
}
