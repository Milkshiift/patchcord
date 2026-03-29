mod logger;
mod patchbay;

use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use miniserde::{Deserialize, Serialize};
use patchbay::{AudioSharePatchbay, PatchbayConfig, has_pipewire};

#[derive(Debug, Deserialize)]
struct RequestEnvelope {
	#[serde(default)]
	id: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method")]
enum Request {
	#[serde(rename = "hasPipeWire")]
	HasPipeWire { id: u64 },

	#[serde(rename = "listShareableNodes")]
	ListShareableNodes {
		id: u64,
		#[serde(default, rename = "includeDevices")]
		include_devices: bool,
	},

	#[serde(rename = "ensureVirtualSink")]
	EnsureVirtualSink { id: u64 },

	#[serde(rename = "routeNodes")]
	RouteNodes {
		id: u64,
		#[serde(rename = "nodeIds")]
		node_ids: Vec<u32>,
	},

	#[serde(rename = "clearRoutes")]
	ClearRoutes { id: u64 },

	#[serde(rename = "dispose")]
	Dispose { id: u64 },
}

#[derive(Serialize)]
struct SuccessResponse<T> {
	id: u64,
	result: T,
}

#[derive(Serialize)]
struct ErrorResponse {
	id: u64,
	error: String,
}

#[derive(Serialize)]
struct EventMessage {
	event: &'static str,
}

enum IncomingMessage {
	Request(String),
	GraphChanged,
	MonitorDied,
}

fn write_json_line<T: Serialize>(out: &mut impl Write, value: &T) -> io::Result<()> {
	let serialized = miniserde::json::to_string(value);
	out.write_all(serialized.as_bytes())?;
	out.write_all(b"\n")?;
	out.flush()
}

fn write_result<T: Serialize, E: ToString>(out: &mut impl Write, id: u64, result: Result<T, E>) -> io::Result<()> {
	match result {
		Ok(value) => write_json_line(out, &SuccessResponse { id, result: value }),
		Err(err) => write_json_line(
			out,
			&ErrorResponse {
				id,
				error: err.to_string(),
			},
		),
	}
}

fn handle_request(out: &mut impl Write, patchbay: &mut AudioSharePatchbay, request: Request) -> io::Result<bool> {
	match request {
		Request::HasPipeWire { id } => {
			write_json_line(
				out,
				&SuccessResponse {
					id,
					result: has_pipewire(),
				},
			)?;
		}
		Request::ListShareableNodes { id, include_devices } => {
			write_result(out, id, patchbay.list_shareable_nodes(include_devices))?;
		}
		Request::EnsureVirtualSink { id } => {
			write_result(out, id, patchbay.ensure_virtual_sink())?;
		}
		Request::RouteNodes { id, node_ids } => {
			write_result(out, id, patchbay.route_nodes(node_ids))?;
		}
		Request::ClearRoutes { id } => {
			write_result(out, id, patchbay.clear_routes())?;
		}
		Request::Dispose { id } => {
			write_result(out, id, patchbay.dispose())?;
			return Ok(false);
		}
	}

	Ok(true)
}

fn parse_args() -> PatchbayConfig {
	let mut config = PatchbayConfig::default();
	let mut args = std::env::args().skip(1);

	while let Some(arg) = args.next() {
		match arg.as_str() {
			"--sink-prefix" => {
				if let Some(val) = args.next() {
					config.sink_prefix = val;
				}
			}
			"--sink-description" => {
				if let Some(val) = args.next() {
					config.sink_description = val;
				}
			}
			"--virtual-mic" => {
				config.virtual_mic = true;
			}
			"--virtual-mic-name" => {
				if let Some(val) = args.next() {
					config.virtual_mic_name = Some(val);
				}
			}
			"--virtual-mic-description" => {
				if let Some(val) = args.next() {
					config.virtual_mic_description = Some(val);
				}
			}
			"-h" | "--help" => {
				println!("Usage: audio-share-helper [OPTIONS]");
				std::process::exit(0);
			}
			_ => {
				logger::warn(&format!("[helper] unknown argument ignored: {arg}"));
			}
		}
	}

	config
}

fn spawn_stdin_thread(tx: mpsc::Sender<IncomingMessage>) {
	// Thread 1: Read JSON requests from Node.js
	thread::spawn(move || {
		let stdin = io::stdin();
		for line in stdin.lock().lines() {
			let Ok(line) = line else { break };

			if line.trim().is_empty() {
				continue;
			}
			if tx.send(IncomingMessage::Request(line)).is_err() {
				break;
			}
		}
	});
}

fn spawn_pw_mon_thread(tx: mpsc::Sender<IncomingMessage>) {
	thread::spawn(move || {
		let Ok(mut child) = Command::new("pw-mon")
			.env("LC_ALL", "C")
			.env("LANG", "C")
			.stdout(Stdio::piped())
			.spawn()
		else {
			logger::warn("[helper] pw-mon not found or failed to start");
			let _ = tx.send(IncomingMessage::MonitorDied);
			return;
		};

		if let Some(child_stdout) = child.stdout.take() {
			let reader = io::BufReader::new(child_stdout);
			let mut last_trigger = std::time::Instant::now() - std::time::Duration::from_secs(1);

			for line in reader.lines() {
				let Ok(text) = line else { break };

				if text.contains("PipeWire:Interface:Node") || text.contains("PipeWire:Interface:Port") {
					if last_trigger.elapsed() > std::time::Duration::from_millis(400) {
						let _ = tx.send(IncomingMessage::GraphChanged);
						last_trigger = std::time::Instant::now();
					}
				}
			}
		}

		let _ = child.wait();
		let _ = tx.send(IncomingMessage::MonitorDied);
	});
}

fn main() -> io::Result<()> {
	let config = parse_args();
	let mut patchbay = AudioSharePatchbay::new(&config);
	let mut stdout = io::BufWriter::new(io::stdout().lock());

	let (tx, rx) = mpsc::channel();

	spawn_stdin_thread(tx.clone());
	spawn_pw_mon_thread(tx);

	// Main Loop: Process messages
	for msg in rx {
		match msg {
			IncomingMessage::Request(line) => {
				let request_id = miniserde::json::from_str::<RequestEnvelope>(&line)
					.ok()
					.and_then(|envelope| envelope.id)
					.unwrap_or(0);

				let request = match miniserde::json::from_str::<Request>(&line) {
					Ok(request) => request,
					Err(err) => {
						logger::warn(&format!("[helper] invalid request: {err:?}"));
						write_json_line(
							&mut stdout,
							&ErrorResponse {
								id: request_id,
								error: format!("invalid request: {err:?}"),
							},
						)?;
						continue;
					}
				};

				if !handle_request(&mut stdout, &mut patchbay, request)? {
					break; // Dispose was called, break loop to cleanly exit
				}
			}
			IncomingMessage::GraphChanged => {
				write_json_line(&mut stdout, &EventMessage { event: "graphChanged" })?;
			}
			IncomingMessage::MonitorDied => {
				write_json_line(&mut stdout, &EventMessage { event: "monitorDied" })?;
			}
		}
	}

	Ok(())
}
