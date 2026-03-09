use std::fmt;

pub type Result<T> = std::result::Result<T, BackendError>;

#[derive(Debug)]
pub enum BackendError {
	Unsupported,
	Timeout(&'static str),
	Io(&'static str, std::io::Error),
	CommandFailed(&'static str, String),
	InvalidOutput(&'static str, String),
	Json(miniserde::Error),
	Message(String),
}

impl BackendError {
	pub fn is_transient_snapshot_error(&self) -> bool {
		matches!(
			self,
			Self::Json(_) | Self::CommandFailed("pw-dump", _) | Self::InvalidOutput("pw-dump", _) | Self::Timeout("pw-dump")
		)
	}
}

impl fmt::Display for BackendError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Unsupported => write!(f, "PipeWire was not detected as the active audio server"),
			Self::Timeout(thing) => write!(f, "timed out waiting for {thing}"),
			Self::Io(prog, source) => write!(f, "failed to run {prog}: {source}"),
			Self::CommandFailed(prog, stderr) => write!(f, "{prog} failed: {stderr}"),
			Self::InvalidOutput(prog, reason) => write!(f, "invalid output from {prog}: {reason}"),
			Self::Json(err) => write!(f, "failed to parse PipeWire state: {err:?}"),
			Self::Message(message) => write!(f, "{message}"),
		}
	}
}

impl std::error::Error for BackendError {}

impl From<miniserde::Error> for BackendError {
	fn from(value: miniserde::Error) -> Self {
		Self::Json(value)
	}
}
