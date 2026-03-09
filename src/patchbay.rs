use std::{
	collections::HashMap,
	fmt,
	process::{self, Command},
	sync::{
		OnceLock,
		atomic::{AtomicU64, Ordering},
	},
	thread,
	time::{Duration, Instant},
};

use miniserde::{Deserialize, Serialize, json::Value};

use crate::logger;

static NEXT_SINK_ID: AtomicU64 = AtomicU64::new(1);
static HAS_PIPEWIRE: OnceLock<bool> = OnceLock::new();

const NODE_TYPE: &str = "PipeWire:Interface:Node";
const PORT_TYPE: &str = "PipeWire:Interface:Port";

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareableNode {
	pub id: u32,
	pub display_name: String,
	pub application_name: Option<String>,
	pub node_name: Option<String>,
	pub description: Option<String>,
	pub media_name: Option<String>,
	pub binary: Option<String>,
	pub process_id: Option<u32>,
	pub is_device: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VirtualSinkInfo {
	pub sink_name: String,
	pub monitor_source: String,
	pub node_id: u32,
}

pub fn has_pipewire() -> bool {
	*HAS_PIPEWIRE.get_or_init(|| match detect_pipewire() {
		Ok(value) => value,
		Err(err) => {
			logger::warn(&format!("[patchbay] PipeWire detection failed: {err}"));
			false
		}
	})
}

pub struct AudioSharePatchbay {
	state: PatchbayState,
}

impl Default for AudioSharePatchbay {
	fn default() -> Self {
		Self::new()
	}
}

impl AudioSharePatchbay {
	pub fn new() -> Self {
		if has_pipewire() {
			logger::info("[patchbay] ready");
		} else {
			logger::warn("[patchbay] PipeWire was not detected as the active audio server");
		}

		Self {
			state: PatchbayState::new(),
		}
	}

	pub fn list_shareable_nodes(&self, include_devices: bool) -> Result<Vec<ShareableNode>> {
		self.state.list_shareable_nodes(include_devices)
	}

	pub fn ensure_virtual_sink(&mut self) -> Result<VirtualSinkInfo> {
		self.state.ensure_virtual_sink()
	}

	pub fn route_nodes(&mut self, node_ids: Vec<u32>) -> Result<VirtualSinkInfo> {
		self.state.route_nodes(node_ids)
	}

	pub fn clear_routes(&mut self) {
		self.state.clear_routes();
	}

	pub fn dispose(&mut self) {
		self.state.dispose();
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortDirection {
	Input,
	Output,
}

impl PortDirection {
	fn parse(value: &str) -> Option<Self> {
		match value {
			"in" | "input" => Some(Self::Input),
			"out" | "output" => Some(Self::Output),
			_ => None,
		}
	}
}

#[derive(Debug, Clone)]
struct Route {
	output_path: String,
	input_path: String,
}

#[derive(Debug, Clone)]
struct PortRecord {
	direction: PortDirection,
	channel: Option<String>,
	port_index: Option<String>,
	path: Option<String>,
	port_name: Option<String>,
	object_path: Option<String>,
}

#[derive(Debug, Clone)]
struct NodeRecord {
	id: u32,
	props: HashMap<String, String>,
	max_output_ports: u32,
	ports: Vec<PortRecord>,
}

impl NodeRecord {
	fn prop(&self, key: &str) -> Option<&str> {
		self.props.get(key).map(String::as_str)
	}

	fn output_ports(&self) -> impl Iterator<Item = &PortRecord> {
		self.ports.iter().filter(|port| port.direction == PortDirection::Output)
	}

	fn input_ports(&self) -> impl Iterator<Item = &PortRecord> {
		self.ports.iter().filter(|port| port.direction == PortDirection::Input)
	}

	fn is_device(&self) -> bool {
		let has_device_id = self.prop("device.id").is_some_and(|value| !value.is_empty());
		// Catch Virtual Sinks/Sources that don't have a hardware device.id
		let is_audio_device = self
			.prop("media.class")
			.is_some_and(|c| c.starts_with("Audio/Sink") || c.starts_with("Audio/Source"));

		has_device_id || is_audio_device
	}
}

#[derive(Debug)]
struct PipeWireSnapshot {
	nodes: HashMap<u32, NodeRecord>,
}

impl PipeWireSnapshot {
	fn collect() -> Result<Self> {
		let dump = run_text("pw-dump", &[])?;
		let objects: Vec<RawPwObject> = miniserde::json::from_str(&dump)?;

		let mut nodes = HashMap::<u32, NodeRecord>::new();
		let mut pending_ports = Vec::<(u32, PortRecord)>::new();

		for object in objects {
			match object.kind.as_str() {
				NODE_TYPE => {
					let props = object
						.info
						.props
						.into_iter()
						.filter_map(|(k, v)| {
							let s = match v {
								Value::String(s) => s,
								Value::Number(n) => n.to_string(),
								Value::Bool(b) => b.to_string(),
								_ => return None,
							};
							Some((k, s))
						})
						.collect();

					nodes.insert(
						object.id,
						NodeRecord {
							id: object.id,
							props,
							max_output_ports: object.info.max_output_ports.unwrap_or(0),
							ports: Vec::new(),
						},
					);
				}
				PORT_TYPE => {
					let get_str = |k: &str| -> Option<String> {
						match object.info.props.get(k) {
							Some(Value::String(s)) => Some(s.clone()),
							Some(Value::Number(n)) => Some(n.to_string()),
							_ => None,
						}
					};

					let Some(node_id) = get_str("node.id").and_then(|v| v.parse::<u32>().ok()) else {
						continue;
					};

					let direction = object
						.info
						.direction
						.as_deref()
						.and_then(PortDirection::parse)
						.or_else(|| get_str("port.direction").as_deref().and_then(PortDirection::parse));

					let Some(direction) = direction else {
						continue;
					};

					pending_ports.push((
						node_id,
						PortRecord {
							direction,
							channel: get_str("audio.channel"),
							port_index: get_str("port.id"),
							path: None, // Filled in the second pass
							port_name: get_str("port.name"),
							object_path: get_str("object.path"),
						},
					));
				}
				_ => {}
			}
		}

		for (node_id, mut port) in pending_ports {
			if let Some(node) = nodes.get_mut(&node_id) {
				port.path = fallback_port_path(node, &port);
				node.ports.push(port);
			}
		}

		Ok(Self { nodes })
	}

	fn find_virtual_sink(&self, sink_name: &str, sink_description: &str) -> Option<&NodeRecord> {
		self.nodes
			.values()
			.find(|node| {
				matches_prop(node, "node.name", sink_name)
					|| matches_prop(node, "device.name", sink_name)
					|| matches_prop(node, "node.nick", sink_name)
			})
			.or_else(|| {
				self.nodes.values().find(|node| {
					matches_prop(node, "node.description", sink_description) || matches_prop(node, "device.description", sink_description)
				})
			})
	}
}

struct PatchbayState {
	sink_name: String,
	sink_description: String,
	module_id: Option<u32>,
	routes: Vec<Route>,
}

impl PatchbayState {
	fn new() -> Self {
		let unique = NEXT_SINK_ID.fetch_add(1, Ordering::Relaxed);

		Self {
			sink_name: format!("patchcord-screen-share-{}-{unique}", process::id()),
			sink_description: "Vencord Screen Share".to_string(),
			module_id: None,
			routes: Vec::new(),
		}
	}

	fn list_shareable_nodes(&self, include_devices: bool) -> Result<Vec<ShareableNode>> {
		ensure_pipewire()?;

		let snapshot = PipeWireSnapshot::collect()?;
		let virtual_sink_id = self.virtual_sink_node_id(&snapshot);

		let mut nodes = snapshot
			.nodes
			.values()
			.filter(|node| Some(node.id) != virtual_sink_id)
			.filter(|node| node.max_output_ports > 0 || node.output_ports().next().is_some())
			.filter(|node| {
				let is_app = node.prop("application.name").is_some_and(|v| !v.is_empty())
					|| node.prop("application.process.binary").is_some_and(|v| !v.is_empty());

				if node.is_device() {
					include_devices // If it's a device (Hardware or Virtual Sink), only allow if requested
				} else {
					is_app // For normal nodes, strictly require them to be real applications
				}
			})
			.map(to_shareable_node)
			.collect::<Vec<_>>();

		nodes.sort_by(|left, right| {
			left.display_name
				.to_ascii_lowercase()
				.cmp(&right.display_name.to_ascii_lowercase())
				.then_with(|| left.id.cmp(&right.id))
		});

		Ok(nodes)
	}

	fn ensure_virtual_sink(&mut self) -> Result<VirtualSinkInfo> {
		ensure_pipewire()?;

		if let Some(info) = self.virtual_sink_info()? {
			return Ok(info);
		}

		logger::info(&format!("[patchbay] creating virtual sink {}", self.sink_name));

		let sink_name_arg = format!("sink_name={}", self.sink_name);
		let sink_properties_arg = format!(
			"sink_properties=device.description={} node.description={} node.name={}",
			quote_module_value(&self.sink_description),
			quote_module_value(&self.sink_description),
			quote_module_value(&self.sink_name),
		);

		let module_id_text = run_text(
			"pactl",
			&[
				"load-module",
				"module-null-sink",
				sink_name_arg.as_str(),
				"channels=2",
				"channel_map=front-left,front-right",
				sink_properties_arg.as_str(),
			],
		)?;

		let module_id = module_id_text
			.trim()
			.parse::<u32>()
			.map_err(|err| BackendError::InvalidOutput("pactl", format!("failed to parse module id: {err}")))?;

		self.module_id = Some(module_id);

		let deadline = Instant::now() + Duration::from_secs(3);

		loop {
			// Catch and ignore transient JSON errors during active graph rebuilding
			match self.virtual_sink_info() {
				Ok(Some(info)) => {
					logger::info(&format!("[patchbay] virtual sink ready: {} ({})", info.sink_name, info.node_id));
					return Ok(info);
				}
				Ok(None) => {}
				Err(err) => {
					logger::trace(&format!("[patchbay] transient error while waiting for sink: {err}"));
				}
			}

			if Instant::now() >= deadline {
				return Err(BackendError::Timeout("virtual sink"));
			}

			thread::sleep(Duration::from_millis(50));
		}
	}

	fn route_nodes(&mut self, node_ids: Vec<u32>) -> Result<VirtualSinkInfo> {
		let sink_info = self.ensure_virtual_sink()?;
		self.clear_routes();

		if node_ids.is_empty() {
			return Err(BackendError::Message("at least one node id is required".to_string()));
		}

		let snapshot = PipeWireSnapshot::collect()?;
		let Some(sink_node) = snapshot.find_virtual_sink(&self.sink_name, &self.sink_description) else {
			return Err(BackendError::Message(
				"virtual sink exists in PulseAudio but was not found in PipeWire".to_string(),
			));
		};

		let sink_inputs = sink_node
			.input_ports()
			.filter(|port| port.path.is_some())
			.cloned()
			.collect::<Vec<_>>();

		if sink_inputs.is_empty() {
			return Err(BackendError::Message("virtual sink has no usable input ports".to_string()));
		}

		let mut created = 0usize;

		for node_id in node_ids {
			if node_id == sink_node.id {
				logger::warn("[patchbay] refusing to link the virtual sink to itself");
				continue;
			}

			let Some(node) = snapshot.nodes.get(&node_id) else {
				logger::warn(&format!("[patchbay] node {node_id} does not exist"));
				continue;
			};

			let outputs = node.output_ports().filter(|port| port.path.is_some()).cloned().collect::<Vec<_>>();

			if outputs.is_empty() {
				logger::debug(&format!("[patchbay] node {node_id} has no usable output ports"));
				continue;
			}

			for (output, input) in map_ports(&outputs, &sink_inputs) {
				let Some(output_path) = output.path.as_deref() else { continue };
				let Some(input_path) = input.path.as_deref() else { continue };

				match create_link(output_path, input_path) {
					Ok(()) => {
						self.routes.push(Route {
							output_path: output_path.to_string(),
							input_path: input_path.to_string(),
						});
						created += 1;
					}
					Err(err) => {
						logger::warn(&format!("[patchbay] failed to link {output_path} -> {input_path}: {err}"));
					}
				}
			}
		}

		if created == 0 {
			return Err(BackendError::Message("none of the selected nodes could be linked".to_string()));
		}

		Ok(sink_info)
	}

	fn clear_routes(&mut self) {
		let routes = std::mem::take(&mut self.routes);

		for route in routes {
			let _ = remove_link(&route.output_path, &route.input_path);
		}
	}

	fn dispose(&mut self) {
		self.clear_routes();

		if let Some(module_id) = self.module_id.take() {
			let module_id_text = module_id.to_string();
			let _ = run_text("pactl", &["unload-module", module_id_text.as_str()]);
		}
	}

	fn virtual_sink_node_id(&self, snapshot: &PipeWireSnapshot) -> Option<u32> {
		snapshot
			.find_virtual_sink(&self.sink_name, &self.sink_description)
			.map(|node| node.id)
	}

	fn virtual_sink_info(&self) -> Result<Option<VirtualSinkInfo>> {
		let snapshot = PipeWireSnapshot::collect()?;
		let Some(node) = snapshot.find_virtual_sink(&self.sink_name, &self.sink_description) else {
			return Ok(None);
		};

		if node.input_ports().count() < 2 {
			return Ok(None);
		}

		Ok(Some(VirtualSinkInfo {
			sink_name: self.sink_name.clone(),
			monitor_source: format!("{}.monitor", self.sink_name),
			node_id: node.id,
		}))
	}
}

impl Drop for PatchbayState {
	fn drop(&mut self) {
		self.dispose();
	}
}

fn ensure_pipewire() -> Result<()> {
	if has_pipewire() { Ok(()) } else { Err(BackendError::Unsupported) }
}

fn detect_pipewire() -> Result<bool> {
	let info = run_text("pactl", &["info"])?;

	let server_name = info
		.lines()
		.find_map(|line| line.strip_prefix("Server Name:"))
		.map(str::trim)
		.ok_or_else(|| BackendError::InvalidOutput("pactl", "missing `Server Name:` line".to_string()))?;

	let lowered = server_name.to_ascii_lowercase();
	logger::trace(&format!("[patchbay] pulse server: {lowered}"));

	Ok(lowered.contains("pipewire"))
}

fn map_ports(outputs: &[PortRecord], inputs: &[PortRecord]) -> Vec<(PortRecord, PortRecord)> {
	if outputs.is_empty() || inputs.is_empty() {
		return Vec::new();
	}

	let mut mapped = Vec::new();

	for output in outputs {
		let out_channel = output.channel.as_deref().unwrap_or("UNK").to_ascii_uppercase();

		for input in inputs {
			let in_channel = input.channel.as_deref().unwrap_or("UNK").to_ascii_uppercase();
			let is_left_input = in_channel == "FL" || in_channel == "FRONT-LEFT";
			let is_right_input = in_channel == "FR" || in_channel == "FRONT-RIGHT";

			let channels_match = match out_channel.as_str() {
				"FL" | "SL" | "RL" | "FLC" => is_left_input,  // Map all left speakers to Left Sink
				"FR" | "SR" | "RR" | "FRC" => is_right_input, // Map all right speakers to Right Sink
				"FC" | "LFE" | "RC" | "MONO" => true,         // Mix center/sub/mono into BOTH channels
				"UNK" => {
					if outputs.len() == 1 {
						true // Mono downmix fallback
					} else {
						output.port_index == input.port_index
					}
				}
				_ => true, // Unknown channel mappings default to mixing everywhere rather than silencing audio
			};

			// Backup validation: If input channel is missing entirely, try generic index link fallback
			let is_valid = channels_match || (in_channel == "UNK" && output.port_index == input.port_index);

			if is_valid {
				mapped.push((output.clone(), input.clone()));
			}
		}
	}

	mapped
}

fn to_shareable_node(node: &NodeRecord) -> ShareableNode {
	let application_name = node.prop("application.name").map(str::to_string);
	let node_name = node.prop("node.name").map(str::to_string);
	let description = node
		.prop("node.description")
		.or_else(|| node.prop("device.description"))
		.map(str::to_string);
	let media_name = node.prop("media.name").map(str::to_string);
	let binary = node.prop("application.process.binary").map(str::to_string);
	let process_id = node.prop("application.process.id").and_then(|v| v.parse::<u32>().ok());

	let display_name = description
		.clone()
		.or_else(|| media_name.clone())
		.or_else(|| application_name.clone())
		.or_else(|| node_name.clone())
		.unwrap_or_else(|| format!("Node {}", node.id));

	ShareableNode {
		id: node.id,
		display_name,
		application_name,
		node_name,
		description,
		media_name,
		binary,
		process_id,
		is_device: node.is_device(),
	}
}

fn create_link(output_path: &str, input_path: &str) -> Result<()> {
	logger::trace(&format!("[patchbay] linking {output_path} -> {input_path}"));

	match run_text("pw-link", &["-L", output_path, input_path]) {
		Ok(_) => Ok(()),
		Err(BackendError::CommandFailed(_, stderr)) if stderr.contains("File exists") || stderr.contains("exists") => Ok(()),
		Err(err) => Err(err),
	}
}

fn remove_link(output_path: &str, input_path: &str) -> Result<()> {
	match run_text("pw-link", &["-d", output_path, input_path]) {
		Ok(_) => Ok(()),
		Err(BackendError::CommandFailed(_, stderr))
			if stderr.contains("No such file") || stderr.contains("not found") || stderr.contains("does not exist") =>
		{
			Ok(())
		}
		Err(err) => Err(err),
	}
}

fn fallback_port_path(node: &NodeRecord, port: &PortRecord) -> Option<String> {
	let node_name = node.prop("node.name")?;
	let port_name = port.port_name.as_deref().or(port.object_path.as_deref())?;

	Some(format!("{node_name}:{port_name}"))
}

fn matches_prop(node: &NodeRecord, key: &str, expected: &str) -> bool {
	node.prop(key) == Some(expected)
}

fn quote_module_value(value: &str) -> String {
	let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
	format!("\"{escaped}\"")
}

fn run_text(program: &'static str, args: &[&str]) -> Result<String> {
	let output = Command::new(program)
		.args(args)
		.env("LC_ALL", "C")
		.env("LANG", "C")
		.output()
		.map_err(|source| BackendError::Io(program, source))?;

	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		return Err(BackendError::CommandFailed(program, stderr));
	}

	String::from_utf8(output.stdout).map_err(|err| BackendError::InvalidOutput(program, err.to_string()))
}

#[derive(Debug, Deserialize)]
struct RawPwObject {
	id: u32,
	#[serde(rename = "type")]
	kind: String,
	#[serde(default)]
	info: RawPwInfo,
}

#[derive(Debug, Default, Deserialize)]
struct RawPwInfo {
	#[serde(default)]
	props: HashMap<String, Value>,
	#[serde(default, rename = "max-output-ports")]
	max_output_ports: Option<u32>,
	#[serde(default)]
	direction: Option<String>,
}
