use std::collections::BTreeSet;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::cmd::{create_link, remove_link, run_text};
use super::error::{BackendError, Result};
use super::models::{NodeRecord, Route, ShareableNode, VirtualSinkInfo};
use super::routing::map_ports;
use super::snapshot::PipeWireSnapshot;
use super::{PatchbayConfig, ensure_pipewire};
use crate::logger;

static NEXT_SINK_ID: AtomicU64 = AtomicU64::new(1);

pub struct PatchbayState {
	sink_name: String,
	sink_description: String,
	module_id: Option<u32>,
	routes: BTreeSet<Route>,

	virtual_mic_name: Option<String>,
	virtual_mic_description: Option<String>,
	remap_module_id: Option<u32>,
}

impl PatchbayState {
	pub fn new(config: &PatchbayConfig) -> Self {
		// Clean up any left-over pactl modules from previous hard crashes
		clean_orphaned_modules(&config.sink_prefix);

		let unique = NEXT_SINK_ID.fetch_add(1, Ordering::Relaxed);

		let virtual_mic_name = if config.virtual_mic || config.virtual_mic_name.is_some() {
			let base_name = config
				.virtual_mic_name
				.clone()
				.unwrap_or_else(|| format!("{}-mic", config.sink_prefix));
			Some(format!("{base_name}-{}-{unique}", process::id()))
		} else {
			None
		};

		let virtual_mic_description = if config.virtual_mic || config.virtual_mic_description.is_some() {
			Some(
				config
					.virtual_mic_description
					.clone()
					.unwrap_or_else(|| format!("{} (Virtual Mic)", config.sink_description)),
			)
		} else {
			None
		};

		Self {
			sink_name: format!("{}-{}-{unique}", config.sink_prefix, process::id()),
			sink_description: config.sink_description.clone(),
			module_id: None,
			routes: BTreeSet::new(),
			virtual_mic_name,
			virtual_mic_description,
			remap_module_id: None,
		}
	}

	pub fn list_shareable_nodes(&self, include_devices: bool) -> Result<Vec<ShareableNode>> {
		ensure_pipewire()?;

		let snapshot = PipeWireSnapshot::collect()?;

		let mut nodes = snapshot
			.nodes
			.values()
			.filter(|node| !self.is_our_virtual_audio_object(node))
			.filter(|node| node.output_ports().any(|port| port.path.is_some()))
			.filter(|node| {
				let is_app = node.prop_str("application.name").is_some_and(|v| !v.is_empty())
					|| node.prop_str("application.process.binary").is_some_and(|v| !v.is_empty());

				if node.is_device() { include_devices } else { is_app }
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

	pub fn ensure_virtual_sink(&mut self) -> Result<VirtualSinkInfo> {
		ensure_pipewire()?;

		let info = if let Some(info) = self.virtual_sink_info()? {
			info
		} else {
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

			let ready_info;
			let deadline = Instant::now() + Duration::from_secs(3);

			loop {
				match self.virtual_sink_info() {
					Ok(Some(info)) => {
						logger::info(&format!("[patchbay] virtual sink ready: {} ({})", info.sink_name, info.node_id));
						ready_info = Some(info);
						break;
					}
					Ok(None) => {}
					Err(err) => {
						logger::trace(&format!("[patchbay] transient error while waiting for sink: {err}"));
					}
				}

				if Instant::now() >= deadline {
					return Err(BackendError::Timeout("virtual sink"));
				}

				// Increased to 200ms to prevent heavy CPU spikes during the pw-dump wait loop
				thread::sleep(Duration::from_millis(200));
			}

			ready_info.unwrap()
		};

		if self.remap_module_id.is_none()
			&& let Some(mic_name) = &self.virtual_mic_name
		{
			let mic_desc = self.virtual_mic_description.as_deref().unwrap_or("Virtual Microphone");
			logger::info(&format!("[patchbay] creating virtual mic wrapper {mic_name}"));

			let master_arg = format!("master={}", info.monitor_source);
			let source_name_arg = format!("source_name={mic_name}");
			let source_properties_arg = format!(
				"source_properties=device.description={} node.description={} node.name={}",
				quote_module_value(mic_desc),
				quote_module_value(mic_desc),
				quote_module_value(mic_name),
			);

			let module_id_text = run_text(
				"pactl",
				&[
					"load-module",
					"module-remap-source",
					master_arg.as_str(),
					source_name_arg.as_str(),
					source_properties_arg.as_str(),
				],
			)?;

			let module_id = module_id_text
				.trim()
				.parse::<u32>()
				.map_err(|err| BackendError::InvalidOutput("pactl", format!("failed to parse remap module id: {err}")))?;

			self.remap_module_id = Some(module_id);
		}

		Ok(info)
	}

	pub fn route_nodes(&mut self, node_ids: Vec<u32>) -> Result<VirtualSinkInfo> {
		if node_ids.is_empty() {
			self.clear_routes()?;
			return self.ensure_virtual_sink();
		}

		let sink_info = self.ensure_virtual_sink()?;
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

		if sink_inputs.len() < 2 {
			return Err(BackendError::Message("virtual sink has no usable stereo input ports".to_string()));
		}

		let previous_routes = self.routes.clone();
		let mut desired_routes = BTreeSet::<Route>::new();

		for node_id in dedupe_node_ids(node_ids) {
			if node_id == sink_node.id {
				logger::warn("[patchbay] refusing to link the virtual sink to itself");
				continue;
			}

			let Some(node) = snapshot.nodes.get(&node_id) else {
				logger::warn(&format!("[patchbay] node {node_id} does not exist"));
				continue;
			};

			if self.is_our_virtual_audio_object(node) {
				logger::warn(&format!("[patchbay] refusing to route helper-owned virtual node {node_id}"));
				continue;
			}

			let outputs = node.output_ports().filter(|port| port.path.is_some()).cloned().collect::<Vec<_>>();

			if outputs.is_empty() {
				logger::debug(&format!("[patchbay] node {node_id} has no usable output ports"));
				continue;
			}

			for (output, input) in map_ports(&outputs, &sink_inputs) {
				let Some(output_path) = output.path.as_deref() else {
					continue;
				};
				let Some(input_path) = input.path.as_deref() else {
					continue;
				};

				desired_routes.insert(Route {
					output_path: output_path.to_string(),
					input_path: input_path.to_string(),
				});
			}
		}

		if desired_routes.is_empty() {
			return Err(BackendError::Message("none of the selected nodes could be linked".to_string()));
		}

		let mut active_routes = previous_routes.intersection(&desired_routes).cloned().collect::<BTreeSet<_>>();
		let mut newly_created = BTreeSet::<Route>::new();

		for route in desired_routes.difference(&previous_routes) {
			match create_link(&route.output_path, &route.input_path) {
				Ok(()) => {
					active_routes.insert(route.clone());
					newly_created.insert(route.clone());
				}
				Err(err) => {
					logger::warn(&format!(
						"[patchbay] failed to link {} -> {}: {err}",
						route.output_path, route.input_path
					));
				}
			}
		}

		if active_routes.is_empty() {
			for route in newly_created {
				if let Err(err) = remove_link(&route.output_path, &route.input_path) {
					logger::warn(&format!(
						"[patchbay] failed to roll back {} -> {}: {err}",
						route.output_path, route.input_path
					));
				}
			}
			return Err(BackendError::Message("none of the selected nodes could be linked".to_string()));
		}

		let mut retained_stale = BTreeSet::<Route>::new();

		for route in previous_routes.difference(&desired_routes) {
			match remove_link(&route.output_path, &route.input_path) {
				Ok(()) => {}
				Err(err) => {
					logger::warn(&format!(
						"[patchbay] failed to remove stale link {} -> {}: {err}",
						route.output_path, route.input_path
					));
					retained_stale.insert(route.clone());
				}
			}
		}

		self.routes = active_routes;
		self.routes.extend(retained_stale);

		Ok(sink_info)
	}

	pub fn clear_routes(&mut self) -> Result<()> {
		if self.routes.is_empty() {
			return Ok(());
		}

		let routes = std::mem::take(&mut self.routes);
		let mut failed = BTreeSet::new();
		let mut failures = 0usize;

		for route in routes {
			match remove_link(&route.output_path, &route.input_path) {
				Ok(()) => {}
				Err(err) => {
					failures += 1;
					logger::warn(&format!(
						"[patchbay] failed to remove link {} -> {}: {err}",
						route.output_path, route.input_path
					));
					failed.insert(route);
				}
			}
		}

		self.routes = failed;

		if failures == 0 {
			Ok(())
		} else {
			Err(BackendError::Message(format!(
				"failed to remove {failures} route(s); cleanup will be retried later"
			)))
		}
	}

	pub fn dispose(&mut self) -> Result<()> {
		let mut errors = Vec::<String>::new();

		if let Err(err) = self.clear_routes() {
			errors.push(err.to_string());
		}

		// Cleanup Virtual Mic first (dependency of the master sink)
		if let Some(module_id) = self.remap_module_id {
			let module_id_text = module_id.to_string();
			match run_text("pactl", &["unload-module", module_id_text.as_str()]) {
				Ok(_) => {
					self.remap_module_id = None;
				}
				Err(err) => {
					logger::warn(&format!("[patchbay] failed to unload virtual mic module {module_id}: {err}"));
					errors.push(format!("failed to unload virtual mic module {module_id}: {err}"));
				}
			}
		}

		// Cleanup Virtual Sink
		if let Some(module_id) = self.module_id {
			let module_id_text = module_id.to_string();
			match run_text("pactl", &["unload-module", module_id_text.as_str()]) {
				Ok(_) => {
					self.module_id = None;
				}
				Err(err) => {
					logger::warn(&format!("[patchbay] failed to unload module {module_id}: {err}"));
					errors.push(format!("failed to unload virtual sink module {module_id}: {err}"));
				}
			}
		}

		if errors.is_empty() {
			Ok(())
		} else {
			Err(BackendError::Message(errors.join("; ")))
		}
	}

	fn virtual_sink_info(&self) -> Result<Option<VirtualSinkInfo>> {
		let snapshot = PipeWireSnapshot::collect()?;
		let Some(node) = snapshot.find_virtual_sink(&self.sink_name, &self.sink_description) else {
			return Ok(None);
		};

		if node.input_ports().filter(|port| port.path.is_some()).count() < 2 {
			return Ok(None);
		}

		Ok(Some(VirtualSinkInfo {
			sink_name: self.sink_name.clone(),
			monitor_source: self.monitor_source_name(),
			node_id: node.id,
			virtual_mic_name: self.virtual_mic_name.clone(),
			virtual_mic_description: self.virtual_mic_description.clone(),
		}))
	}

	fn monitor_source_name(&self) -> String {
		format!("{}.monitor", self.sink_name)
	}

	fn is_our_virtual_sink_node(&self, node: &NodeRecord) -> bool {
		node.matches_prop("node.name", &self.sink_name)
			|| node.matches_prop("device.name", &self.sink_name)
			|| node.matches_prop("node.nick", &self.sink_name)
	}

	fn is_our_monitor_node(&self, node: &NodeRecord) -> bool {
		let monitor_name = self.monitor_source_name();

		node.matches_prop("node.name", &monitor_name)
			|| node.matches_prop("device.name", &monitor_name)
			|| node.matches_prop("node.nick", &monitor_name)
	}

	fn is_our_virtual_mic_node(&self, node: &NodeRecord) -> bool {
		let Some(mic_name) = &self.virtual_mic_name else {
			return false;
		};

		node.matches_prop("node.name", mic_name) || node.matches_prop("device.name", mic_name) || node.matches_prop("node.nick", mic_name)
	}

	fn is_our_virtual_audio_object(&self, node: &NodeRecord) -> bool {
		self.is_our_virtual_sink_node(node) || self.is_our_monitor_node(node) || self.is_our_virtual_mic_node(node)
	}
}

impl Drop for PatchbayState {
	fn drop(&mut self) {
		if let Err(err) = self.dispose() {
			logger::warn(&format!("[patchbay] cleanup failed during drop: {err}"));
		}
	}
}

fn dedupe_node_ids(node_ids: Vec<u32>) -> Vec<u32> {
	let mut seen = BTreeSet::new();
	let mut deduped = Vec::new();

	for node_id in node_ids {
		if seen.insert(node_id) {
			deduped.push(node_id);
		}
	}

	deduped
}

fn to_shareable_node(node: &NodeRecord) -> ShareableNode {
	let application_name = node.prop_str("application.name").map(str::to_string);
	let node_name = node.prop_str("node.name").map(str::to_string);
	let description = node
		.prop_str("node.description")
		.or_else(|| node.prop_str("device.description"))
		.map(str::to_string);
	let media_name = node.prop_str("media.name").map(str::to_string);
	let binary = node.prop_str("application.process.binary").map(str::to_string);
	let process_id = node.prop_num("application.process.id");

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

fn clean_orphaned_modules(prefix: &str) {
	if let Ok(output) = run_text("pactl", &["list", "short", "modules"]) {
		for line in output.lines() {
			if line.contains(prefix) {
				if let Some(id_str) = line.split_whitespace().next() {
					let _ = run_text("pactl", &["unload-module", id_str]);
				}
			}
		}
	}
}

fn quote_module_value(value: &str) -> String {
	let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
	format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::patchbay::PatchbayConfig;

	#[test]
	fn test_quote_module_value() {
		// Standard string
		assert_eq!(quote_module_value("GoofCord Share"), "\"GoofCord Share\"");

		// String with quotes (should be escaped)
		assert_eq!(quote_module_value("My \"App\""), "\"My \\\"App\\\"\"");

		// String with slashes
		assert_eq!(quote_module_value("C:\\App"), "\"C:\\\\App\"");
	}

	#[test]
	fn test_dedupe_node_ids() {
		let input = vec![1, 2, 2, 3, 1, 4];
		let expected = vec![1, 2, 3, 4];
		assert_eq!(dedupe_node_ids(input), expected);
	}

	#[test]
	#[ignore = "Requires a live PipeWire/PulseAudio session"]
	fn test_integration_virtual_sink_lifecycle() {
		if let Err(e) = super::super::ensure_pipewire() {
			panic!("Cannot run integration test: PipeWire not detected. ({e})");
		}

		let config = PatchbayConfig::default();
		let mut state = PatchbayState::new(&config);

		let info = state.ensure_virtual_sink().expect("Failed to create virtual sink");
		assert!(info.sink_name.starts_with(&config.sink_prefix));
		assert!(state.module_id.is_some(), "pactl module_id should be captured");

		let snapshot = PipeWireSnapshot::collect().expect("Failed to collect snapshot");
		let found_node = snapshot.find_virtual_sink(&info.sink_name, &state.sink_description);
		assert!(found_node.is_some(), "Virtual sink was created via pactl but not found in pw-dump!");

		state.dispose().expect("Failed to dispose virtual sink");

		let snapshot_after = PipeWireSnapshot::collect().expect("Failed to collect snapshot");
		let found_after = snapshot_after.find_virtual_sink(&info.sink_name, &state.sink_description);
		assert!(
			found_after.is_none(),
			"Virtual sink should be removed from the system after dispose"
		);
	}

	#[test]
	#[ignore = "Requires a live PipeWire session with at least one audio device"]
	fn test_integration_list_nodes() {
		if super::super::ensure_pipewire().is_err() {
			return;
		}

		let state = PatchbayState::new(&PatchbayConfig::default());
		let nodes = state.list_shareable_nodes(true).expect("Failed to list nodes");

		assert!(
			!nodes.is_empty(),
			"Expected to find at least one shareable device/app on the system"
		);

		for node in nodes.iter().take(3) {
			println!("Found node: {} (ID: {})", node.display_name, node.id);
		}
	}
}