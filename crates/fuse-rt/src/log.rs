fn enabled() -> bool {
    matches!(std::env::var("FUSE_LOG").as_deref(), Ok("1"))
}

pub fn info(message: &str) {
    if enabled() {
        eprintln!("[info] {message}");
    }
}

pub fn warn(message: &str) {
    if enabled() {
        eprintln!("[warn] {message}");
    }
}

pub fn error(message: &str) {
    if enabled() {
        eprintln!("[error] {message}");
    }
}
