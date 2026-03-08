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
	// if std::env::var_os("PATCHCORD_DEBUG").is_some() {
	// 	eprintln!("[DEBUG] {message}");
	// }
}

pub fn trace(message: &str) {
	if std::env::var_os("PATCHCORD_TRACE").is_some() {
		eprintln!("[TRACE] {message}");
	}
}
