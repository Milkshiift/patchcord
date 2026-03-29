use std::sync::OnceLock;

static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();

pub fn error(message: &str) {
	eprintln!("[ERROR] {message}");
}

pub fn warn(message: &str) {
	eprintln!("[WARN] {message}");
}

pub fn info(message: &str) {
	eprintln!("[INFO] {message}");
}

pub fn debug(message: &str) {
	eprintln!("[DEBUG] {message}");
}

pub fn trace(message: &str) {
	let enabled = *TRACE_ENABLED.get_or_init(|| std::env::var_os("PATCHCORD_TRACE").is_some());
	if enabled {
		eprintln!("[TRACE] {message}");
	}
}
