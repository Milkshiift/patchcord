use miniserde::{Deserialize, Serialize};
use std::collections::HashMap;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
	Input,
	Output,
}

impl PortDirection {
	pub fn parse(value: &str) -> Option<Self> {
		match value {
			"in" | "input" => Some(Self::Input),
			"out" | "output" => Some(Self::Output),
			_ => None,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Route {
	pub output_path: String,
	pub input_path: String,
}

#[derive(Debug, Clone)]
pub struct PortRecord {
	pub direction: PortDirection,
	pub channel: Option<String>,
	pub port_index: Option<String>,
	pub path: Option<String>,
	pub port_name: Option<String>,
	pub object_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NodeRecord {
	pub id: u32,
	pub props: HashMap<String, String>,
	pub ports: Vec<PortRecord>,
}

impl NodeRecord {
	pub fn prop(&self, key: &str) -> Option<&str> {
		self.props.get(key).map(String::as_str)
	}

	pub fn matches_prop(&self, key: &str, expected: &str) -> bool {
		self.prop(key) == Some(expected)
	}

	pub fn output_ports(&self) -> impl Iterator<Item = &PortRecord> {
		self.ports.iter().filter(|port| port.direction == PortDirection::Output)
	}

	pub fn input_ports(&self) -> impl Iterator<Item = &PortRecord> {
		self.ports.iter().filter(|port| port.direction == PortDirection::Input)
	}

	pub fn is_device(&self) -> bool {
		let has_device_id = self.prop("device.id").is_some_and(|value| !value.is_empty());

		let is_audio_device = self
			.prop("media.class")
			.is_some_and(|class| class.starts_with("Audio/Sink") || class.starts_with("Audio/Source"));

		has_device_id || is_audio_device
	}
}
