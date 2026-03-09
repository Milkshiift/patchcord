use super::models::PortRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRole {
	Left,
	Right,
	Both,
	Unknown,
}

pub fn channel_role(channel: Option<&str>) -> ChannelRole {
	match channel.unwrap_or("").to_ascii_uppercase().as_str() {
		"FL" | "FRONT-LEFT" | "SL" | "SIDE-LEFT" | "RL" | "REAR-LEFT" | "FLC" => ChannelRole::Left,
		"FR" | "FRONT-RIGHT" | "SR" | "SIDE-RIGHT" | "RR" | "REAR-RIGHT" | "FRC" => ChannelRole::Right,
		"FC" | "LFE" | "RC" | "MONO" => ChannelRole::Both,
		_ => ChannelRole::Unknown,
	}
}

pub fn same_port_index(left: &PortRecord, right: &PortRecord) -> bool {
	matches!(
		(left.port_index.as_deref(), right.port_index.as_deref()),
		(Some(a), Some(b)) if a == b
	)
}

pub fn map_ports(outputs: &[PortRecord], inputs: &[PortRecord]) -> Vec<(PortRecord, PortRecord)> {
	if outputs.is_empty() || inputs.is_empty() {
		return Vec::new();
	}

	let left_inputs = inputs
		.iter()
		.filter(|input| channel_role(input.channel.as_deref()) == ChannelRole::Left)
		.collect::<Vec<_>>();

	let right_inputs = inputs
		.iter()
		.filter(|input| channel_role(input.channel.as_deref()) == ChannelRole::Right)
		.collect::<Vec<_>>();

	let mut mapped = Vec::new();

	for (output_index, output) in outputs.iter().enumerate() {
		match channel_role(output.channel.as_deref()) {
			ChannelRole::Left => {
				if !left_inputs.is_empty() {
					for input in &left_inputs {
						mapped.push((output.clone(), (*input).clone()));
					}
				} else if let Some(input) = inputs.iter().find(|input| same_port_index(output, input)) {
					mapped.push((output.clone(), input.clone()));
				} else if let Some(input) = inputs.get(output_index).or_else(|| inputs.first()) {
					mapped.push((output.clone(), input.clone()));
				}
			}
			ChannelRole::Right => {
				if !right_inputs.is_empty() {
					for input in &right_inputs {
						mapped.push((output.clone(), (*input).clone()));
					}
				} else if let Some(input) = inputs.iter().find(|input| same_port_index(output, input)) {
					mapped.push((output.clone(), input.clone()));
				} else if let Some(input) = inputs.get(output_index).or_else(|| inputs.last()) {
					mapped.push((output.clone(), input.clone()));
				}
			}
			ChannelRole::Both => {
				if !left_inputs.is_empty() || !right_inputs.is_empty() {
					for input in left_inputs.iter().chain(right_inputs.iter()) {
						mapped.push((output.clone(), (*input).clone()));
					}
				} else {
					for input in inputs {
						mapped.push((output.clone(), input.clone()));
					}
				}
			}
			ChannelRole::Unknown => {
				if outputs.len() == 1 {
					for input in inputs {
						mapped.push((output.clone(), input.clone()));
					}
				} else if let Some(input) = inputs.iter().find(|input| same_port_index(output, input)) {
					mapped.push((output.clone(), input.clone()));
				} else if let Some(input) = inputs.get(output_index) {
					mapped.push((output.clone(), input.clone()));
				} else if let Some(input) = inputs.last() {
					mapped.push((output.clone(), input.clone()));
				}
			}
		}
	}

	mapped
}
