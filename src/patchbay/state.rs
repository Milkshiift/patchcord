use std::collections::BTreeSet;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::cmd::{create_link, remove_link, run_text};
use super::ensure_pipewire;
use super::error::{BackendError, Result};
use super::models::{NodeRecord, Route, ShareableNode, VirtualSinkInfo};
use super::routing::map_ports;
use super::snapshot::PipeWireSnapshot;
use crate::logger;

static NEXT_SINK_ID: AtomicU64 = AtomicU64::new(1);

pub struct PatchbayState {
	sink_name: String,
	sink_description: String,
	module_id: Option<u32>,
	routes: BTreeSet<Route>,
}

impl PatchbayState {
	pub fn new() -> Self {
		let unique = NEXT_SINK_ID.fetch_add(1, Ordering::Relaxed);

		Self {
			sink_name: format!("patchcord-screen-share-{}-{unique}", process::id()),
			sink_description: "GoofCord Screen Share".to_string(),
			module_id: None,
			routes: BTreeSet::new(),
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
				let is_app = node.prop("application.name").is_some_and(|v| !v.is_empty())
					|| node.prop("application.process.binary").is_some_and(|v| !v.is_empty());

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

	pub fn route_nodes(&mut self, node_ids: Vec<u32>) -> Result<VirtualSinkInfo> {
		if node_ids.is_empty() {
			return Err(BackendError::Message("at least one node id is required".to_string()));
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
				let Some(output_path) = output.path.as_deref() else { continue; };
				let Some(input_path) = input.path.as_deref() else { continue; };

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
					logger::warn(&format!("[patchbay] failed to link {} -> {}: {err}", route.output_path, route.input_path));
				}
			}
		}

		if active_routes.is_empty() {
			for route in newly_created {
				if let Err(err) = remove_link(&route.output_path, &route.input_path) {
					logger::warn(&format!("[patchbay] failed to roll back {} -> {}: {err}", route.output_path, route.input_path));
				}
			}
			return Err(BackendError::Message("none of the selected nodes could be linked".to_string()));
		}

		let mut retained_stale = BTreeSet::<Route>::new();

		for route in previous_routes.difference(&desired_routes) {
			match remove_link(&route.output_path, &route.input_path) {
				Ok(()) => {}
				Err(err) => {
					logger::warn(&format!("[patchbay] failed to remove stale link {} -> {}: {err}", route.output_path, route.input_path));
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
					logger::warn(&format!("[patchbay] failed to remove link {} -> {}: {err}", route.output_path, route.input_path));
					failed.insert(route);
				}
			}
		}

		self.routes = failed;

		if failures == 0 {
			Ok(())
		} else {
			Err(BackendError::Message(format!("failed to remove {failures} route(s); cleanup will be retried later")))
		}
	}

	pub fn dispose(&mut self) -> Result<()> {
		let mut errors = Vec::<String>::new();

		if let Err(err) = self.clear_routes() {
			errors.push(err.to_string());
		}

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

	fn is_our_virtual_audio_object(&self, node: &NodeRecord) -> bool {
		self.is_our_virtual_sink_node(node) || self.is_our_monitor_node(node)
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

fn quote_module_value(value: &str) -> String {
	let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
	format!("\"{escaped}\"")
}



#[cfg(test)]
mod tests {
	use super::*;

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
		// Ensure PipeWire is actually running before we test
		if let Err(e) = super::super::ensure_pipewire() {
			panic!("Cannot run integration test: PipeWire not detected. ({e})");
		}

		let mut state = PatchbayState::new();

		// 1. Create the virtual sink in the OS
		let info = state.ensure_virtual_sink().expect("Failed to create virtual sink");
		assert!(info.sink_name.starts_with("patchcord-screen-share-"));
		assert!(state.module_id.is_some(), "pactl module_id should be captured");

		// 2. Fetch the live PipeWire graph and verify it exists
		let snapshot = PipeWireSnapshot::collect().expect("Failed to collect snapshot");
		let found_node = snapshot.find_virtual_sink(&info.sink_name, &state.sink_description);
		assert!(found_node.is_some(), "Virtual sink was created via pactl but not found in pw-dump!");

		// 3. Trigger cleanup
		state.dispose().expect("Failed to dispose virtual sink");

		// 4. Fetch the live graph again and verify it's gone
		let snapshot_after = PipeWireSnapshot::collect().expect("Failed to collect snapshot");
		let found_after = snapshot_after.find_virtual_sink(&info.sink_name, &state.sink_description);
		assert!(found_after.is_none(), "Virtual sink should be removed from the system after dispose");
	}

	#[test]
	#[ignore = "Requires a live PipeWire session with at least one audio device"]
	fn test_integration_list_nodes() {
		if super::super::ensure_pipewire().is_err() {
			return; // Skip if no PipeWire
		}

		let state = PatchbayState::new();

		// Include devices so we are guaranteed to find *something* (like the hardware soundcard)
		let nodes = state.list_shareable_nodes(true).expect("Failed to list nodes");

		assert!(!nodes.is_empty(), "Expected to find at least one shareable device/app on the system");

		// Print them out so you can manually inspect the output when testing
		for node in nodes.iter().take(3) {
			println!("Found node: {} (ID: {})", node.display_name, node.id);
		}
	}
}