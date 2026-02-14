use fuse_rt::json as rt_json;

const ASSET_MAP_ENV: &str = "FUSE_ASSET_MAP";

pub(crate) fn resolve_asset_href(raw: &str) -> String {
    let trimmed = raw.trim();
    let (path, suffix) = split_suffix(trimmed);
    let key = normalize_asset_key(path);
    if key.is_empty() {
        return format!("/{suffix}");
    }
    let base = asset_map_lookup(&key).unwrap_or_else(|| format!("/{key}"));
    if suffix.is_empty() {
        base
    } else {
        format!("{base}{suffix}")
    }
}

fn split_suffix(raw: &str) -> (&str, &str) {
    let mut query = raw.find('?');
    if let Some(hash) = raw.find('#') {
        query = match query {
            Some(existing) if existing < hash => Some(existing),
            _ => Some(hash),
        };
    }
    match query {
        Some(idx) => (&raw[..idx], &raw[idx..]),
        None => (raw, ""),
    }
}

fn normalize_asset_key(raw: &str) -> String {
    raw.trim()
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string()
}

fn asset_map_lookup(key: &str) -> Option<String> {
    let raw = std::env::var(ASSET_MAP_ENV).ok()?;
    let rt_json::JsonValue::Object(entries) = rt_json::decode(&raw).ok()? else {
        return None;
    };
    if let Some(rt_json::JsonValue::String(value)) = entries.get(key) {
        return Some(normalize_href(value));
    }
    let slash_key = format!("/{key}");
    if let Some(rt_json::JsonValue::String(value)) = entries.get(&slash_key) {
        return Some(normalize_href(value));
    }
    None
}

fn normalize_href(raw: &str) -> String {
    if raw.starts_with('/') {
        raw.to_string()
    } else {
        format!("/{raw}")
    }
}
