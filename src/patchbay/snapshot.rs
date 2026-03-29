use miniserde::{Deserialize, json::Value};
use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use super::cmd::run_text;
use super::error::{BackendError, Result};
use super::models::{NodeRecord, PortDirection, PortRecord};
use crate::logger;

const NODE_TYPE: &str = "PipeWire:Interface:Node";
const PORT_TYPE: &str = "PipeWire:Interface:Port";
const GRAPH_RETRY_ATTEMPTS: usize = 8;
const GRAPH_RETRY_DELAY: Duration = Duration::from_millis(50);

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
	#[serde(default)]
	direction: Option<String>,
}

#[derive(Debug)]
pub struct PipeWireSnapshot {
	pub nodes: HashMap<u32, NodeRecord>,
}

impl PipeWireSnapshot {
	pub fn collect() -> Result<Self> {
		let mut last_err = None;

		for attempt in 0..GRAPH_RETRY_ATTEMPTS {
			match Self::collect_once() {
				Ok(snapshot) => return Ok(snapshot),
				Err(err) if err.is_transient_snapshot_error() => {
					logger::trace(&format!(
						"[patchbay] transient PipeWire graph read failure (attempt {}/{}): {err}",
						attempt + 1,
						GRAPH_RETRY_ATTEMPTS
					));
					last_err = Some(err);

					if attempt + 1 < GRAPH_RETRY_ATTEMPTS {
						thread::sleep(GRAPH_RETRY_DELAY);
					}
				}
				Err(err) => return Err(err),
			}
		}

		Err(last_err.unwrap_or_else(|| BackendError::Message("failed to collect PipeWire graph".to_string())))
	}

	fn collect_once() -> Result<Self> {
		let dump = run_text("pw-dump", &[])?;
		let objects: Vec<RawPwObject> = miniserde::json::from_str(&dump)?;

		let mut nodes = HashMap::<u32, NodeRecord>::new();
		let mut pending_ports = Vec::<(u32, PortRecord)>::new();

		for object in objects {
			match object.kind.as_str() {
				NODE_TYPE => {
					nodes.insert(
						object.id,
						NodeRecord {
							id: object.id,
							props: object.info.props,
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
							id: object.id,
							direction,
							channel: get_str("audio.channel"),
							port_index: get_str("port.id").or_else(|| get_str("port.index")),
							path: None,
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
				port.path = Some(port.id.to_string());
				node.ports.push(port);
			}
		}

		Ok(Self { nodes })
	}

	pub fn find_virtual_sink(&self, sink_name: &str, sink_description: &str) -> Option<&NodeRecord> {
		self.nodes
			.values()
			.find(|node| {
				node.matches_prop("node.name", sink_name)
					|| node.matches_prop("device.name", sink_name)
					|| node.matches_prop("node.nick", sink_name)
			})
			.or_else(|| {
				self.nodes.values().find(|node| {
					node.matches_prop("node.description", sink_description) || node.matches_prop("device.description", sink_description)
				})
			})
	}
}
