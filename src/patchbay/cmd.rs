use std::process::Command;

use super::error::{BackendError, Result};
use crate::logger;

pub fn run_text(program: &'static str, args: &[&str]) -> Result<String> {
	let output = Command::new(program)
		.args(args)
		.env("LC_ALL", "C")
		.env("LANG", "C")
		.output()
		.map_err(|source| BackendError::Io(program, source))?;

	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		let message = if stderr.is_empty() {
			format!("exit status {}", output.status)
		} else {
			stderr
		};

		return Err(BackendError::CommandFailed(program, message));
	}

	String::from_utf8(output.stdout).map_err(|err| BackendError::InvalidOutput(program, err.to_string()))
}

pub fn create_link(output_path: &str, input_path: &str) -> Result<()> {
	logger::trace(&format!("[patchbay] linking {output_path} -> {input_path}"));

	match run_text("pw-link", &[output_path, input_path]) {
		Ok(_) => Ok(()),
		Err(BackendError::CommandFailed(_, stderr)) if stderr.to_ascii_lowercase().contains("exists") => Ok(()),
		Err(err) => Err(err),
	}
}

pub fn remove_link(output_path: &str, input_path: &str) -> Result<()> {
	match run_text("pw-link", &["-d", output_path, input_path]) {
		Ok(_) => Ok(()),
		Err(BackendError::CommandFailed(_, stderr)) => {
			let lowered = stderr.to_ascii_lowercase();
			if lowered.contains("no such file") || lowered.contains("not found") || lowered.contains("does not exist") {
				Ok(())
			} else {
				Err(BackendError::CommandFailed("pw-link", stderr))
			}
		}
		Err(err) => Err(err),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	#[ignore = "Requires a Unix-like system"]
	fn test_integration_run_command_success() {
		// Run a safe command that should exist everywhere
		let output = run_text("echo", &["hello", "world"]).expect("Command failed");
		assert_eq!(output.trim(), "hello world");
	}

	#[test]
	#[ignore = "Requires PulseAudio utilities (pactl) installed"]
	fn test_integration_run_pactl_info() {
		let output = run_text("pactl", &["info"]).expect("pactl command failed");

		// Output should contain "Server Name"
		assert!(
			output.contains("Server Name"),
			"pactl info output was missing 'Server Name'. Output: {output}"
		);
	}

	#[test]
	fn test_integration_run_command_failure() {
		// Attempt to run a command that definitely doesn't exist
		let err = run_text("this-command-does-not-exist-12345", &[]).unwrap_err();

		match err {
			BackendError::Io(cmd, _) => assert_eq!(cmd, "this-command-does-not-exist-12345"),
			_ => panic!("Expected IO error for missing executable, got: {:?}", err),
		}
	}
}
