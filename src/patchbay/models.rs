use miniserde::{Deserialize, Serialize};
use std::collections::HashMap;
use miniserde::json::Value;

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
	pub virtual_mic_name: Option<String>,
	pub virtual_mic_description: Option<String>,
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
	pub id: u32,
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
	pub props: HashMap<String, Value>,
	pub ports: Vec<PortRecord>,
}

impl NodeRecord {
	pub fn prop_str(&self, key: &str) -> Option<&str> {
		match self.props.get(key) {
			Some(Value::String(s)) => Some(s.as_str()),
			_ => None,
		}
	}

	pub fn prop_num(&self, key: &str) -> Option<u32> {
		match self.props.get(key) {
			Some(Value::Number(n)) => n.to_string().parse().ok(),
			Some(Value::String(s)) => s.parse().ok(),
			_ => None,
		}
	}

	pub fn matches_prop(&self, key: &str, expected: &str) -> bool {
		self.prop_str(key) == Some(expected)
	}

	pub fn output_ports(&self) -> impl Iterator<Item = &PortRecord> {
		self.ports.iter().filter(|port| port.direction == PortDirection::Output)
	}

	pub fn input_ports(&self) -> impl Iterator<Item = &PortRecord> {
		self.ports.iter().filter(|port| port.direction == PortDirection::Input)
	}

	pub fn is_device(&self) -> bool {
		let has_device_id = self.prop_str("device.id").is_some_and(|value| !value.is_empty());

		let is_audio_device = self
			.prop_str("media.class")
			.is_some_and(|class| class.starts_with("Audio/Sink") || class.starts_with("Audio/Source"));

		has_device_id || is_audio_device
	}
}
