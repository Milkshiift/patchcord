use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::error::{BackendError, Result};
use crate::logger;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

pub fn run_text(program: &'static str, args: &[&str]) -> Result<String> {
	let mut child = Command::new(program)
		.args(args)
		.env("LC_ALL", "C")
		.env("LANG", "C")
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.map_err(|source| BackendError::Io(program, source))?;

	let stdout = child.stdout.take().ok_or_else(|| BackendError::InvalidOutput(program, "stdout pipe was not captured".to_string()))?;
	let stderr = child.stderr.take().ok_or_else(|| BackendError::InvalidOutput(program, "stderr pipe was not captured".to_string()))?;

	let stdout_thread = thread::spawn(move || -> std::io::Result<Vec<u8>> {
		let mut reader = stdout;
		let mut buffer = Vec::new();
		reader.read_to_end(&mut buffer)?;
		Ok(buffer)
	});

	let stderr_thread = thread::spawn(move || -> std::io::Result<Vec<u8>> {
		let mut reader = stderr;
		let mut buffer = Vec::new();
		reader.read_to_end(&mut buffer)?;
		Ok(buffer)
	});

	let deadline = Instant::now() + COMMAND_TIMEOUT;

	let status = loop {
		match child.try_wait() {
			Ok(Some(status)) => break status,
			Ok(None) => {
				if Instant::now() >= deadline {
					let _ = child.kill();
					let _ = child.wait();
					let _ = stdout_thread.join();
					let _ = stderr_thread.join();
					return Err(BackendError::Timeout(program));
				}
				thread::sleep(Duration::from_millis(10));
			}
			Err(err) => {
				let _ = child.kill();
				let _ = child.wait();
				let _ = stdout_thread.join();
				let _ = stderr_thread.join();
				return Err(BackendError::Io(program, err));
			}
		}
	};

	let stdout = join_reader(program, "stdout", stdout_thread)?;
	let stderr = join_reader(program, "stderr", stderr_thread)?;

	if !status.success() {
		let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
		let message = if stderr.is_empty() {
			format!("exit status {status}")
		} else {
			stderr
		};

		return Err(BackendError::CommandFailed(program, message));
	}

	String::from_utf8(stdout).map_err(|err| BackendError::InvalidOutput(program, err.to_string()))
}

fn join_reader(program: &'static str, stream_name: &str, handle: thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
	match handle.join() {
		Ok(Ok(bytes)) => Ok(bytes),
		Ok(Err(err)) => Err(BackendError::InvalidOutput(program, format!("failed to read {stream_name}: {err}"))),
		Err(_) => Err(BackendError::InvalidOutput(program, format!("{stream_name} reader thread panicked"))),
	}
}

pub fn create_link(output_path: &str, input_path: &str) -> Result<()> {
	logger::trace(&format!("[patchbay] linking {output_path} -> {input_path}"));

	match run_text("pw-link", &["-L", output_path, input_path]) {
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
