mod logger;
mod patchbay;

use std::io::{self, BufRead, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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
	StdinClosed,
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

/// Dispatches JSON requests to the Patchbay logic.
/// Returns Ok(false) if a Dispose request was processed, indicating the loop should exit.
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

/// Spawns the thread responsible for reading JSON-RPC lines from Node.js via Stdin.
fn spawn_stdin_thread(tx: mpsc::Sender<IncomingMessage>) {
	thread::spawn(move || {
		let stdin = io::stdin();
		for line in stdin.lock().lines() {
			match line {
				Ok(text) => {
					if text.trim().is_empty() {
						continue;
					}
					if tx.send(IncomingMessage::Request(text)).is_err() {
						return;
					}
				}
				Err(_) => break,
			}
		}
		let _ = tx.send(IncomingMessage::StdinClosed);
	});
}

/// Spawns `pw-mon` and a monitoring thread.
fn spawn_pw_mon_thread(tx: mpsc::Sender<IncomingMessage>) -> Option<Child> {
	let mut child = Command::new("pw-mon")
		.env("LC_ALL", "C")
		.env("LANG", "C")
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.ok()?;

	let stdout = child.stdout.take()?;

	thread::spawn(move || {
		let reader = io::BufReader::new(stdout);
		let mut last_trigger = Instant::now() - Duration::from_secs(1);

		for line in reader.lines() {
			let Ok(text) = line else { break };

			if text.contains("PipeWire:Interface:Node") || text.contains("PipeWire:Interface:Port") {
				if last_trigger.elapsed() > Duration::from_millis(400) {
					if tx.send(IncomingMessage::GraphChanged).is_err() {
						return;
					}
					last_trigger = Instant::now();
				}
			}
		}
		let _ = tx.send(IncomingMessage::MonitorDied);
	});

	Some(child)
}

fn main() -> io::Result<()> {
	let config = parse_args();
	let mut patchbay = AudioSharePatchbay::new(&config);
	let mut stdout = io::BufWriter::new(io::stdout().lock());

	let (tx, rx) = mpsc::channel();

	spawn_stdin_thread(tx.clone());
	let monitor_child = spawn_pw_mon_thread(tx);

	// Main loop: Listen for requests from Node.js or events from PipeWire
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
					break;
				}
			}
			IncomingMessage::GraphChanged => {
				write_json_line(&mut stdout, &EventMessage { event: "graphChanged" })?;
			}
			IncomingMessage::MonitorDied => {
				write_json_line(&mut stdout, &EventMessage { event: "monitorDied" })?;
			}
			IncomingMessage::StdinClosed => {
				logger::info("[helper] stdin closed, exiting...");
				break;
			}
		}
	}

	if let Some(mut child) = monitor_child {
		let _ = child.kill();
		let _ = child.wait();
	}

	let _ = patchbay.dispose();

	Ok(())
}
