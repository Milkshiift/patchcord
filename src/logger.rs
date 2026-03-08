use std::{
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

#[derive(Copy, Clone)]
enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

struct Logger {
    trace_enabled: bool,
    file: Option<Mutex<File>>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn log_directory() -> PathBuf {
    let mut dir = env::temp_dir();

    if let Some(home) = env::var_os("HOME") {
        dir = PathBuf::from(home).join(".local").join("state");
    }

    if let Some(state_home) = env::var_os("XDG_STATE_HOME") {
        dir = PathBuf::from(state_home);
    }

    dir.join("patchcord")
}

fn get_logger() -> &'static Logger {
    LOGGER.get_or_init(|| {
        let trace_enabled = env::var_os("patchcord_ENABLE_LOG").is_some();

        let file = if trace_enabled {
            let dir = log_directory();
            let _ = fs::create_dir_all(&dir);

            OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("patchcord.log"))
                .ok()
                .map(Mutex::new)
        } else {
            None
        };

        Logger {
            trace_enabled,
            file,
        }
    })
}

pub fn init_logging() {
    let _ = get_logger();
}

fn should_log(level: Level, logger: &Logger) -> bool {
    match level {
        Level::Error | Level::Warn | Level::Info => true,
        Level::Debug | Level::Trace => logger.trace_enabled,
    }
}

fn write_line(level: Level, message: &str) {
    let logger = get_logger();

    if !should_log(level, logger) {
        return;
    }

    let line = format!("[{}] {}", level.as_str(), message);

    eprintln!("{line}");

    if let Some(file) = &logger.file {
        if let Ok(mut file) = file.lock() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

pub fn error(message: &str) {
    write_line(Level::Error, message);
}

pub fn warn(message: &str) {
    write_line(Level::Warn, message);
}

pub fn info(message: &str) {
    write_line(Level::Info, message);
}

pub fn debug(message: &str) {
    write_line(Level::Debug, message);
}

pub fn trace(message: &str) {
    write_line(Level::Trace, message);
}