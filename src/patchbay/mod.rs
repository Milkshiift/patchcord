pub mod cmd;
pub mod error;
pub mod models;
pub mod routing;
pub mod snapshot;
pub mod state;

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

pub use error::{BackendError, Result};
pub use models::{ShareableNode, VirtualSinkInfo};

use crate::logger;
use cmd::run_text;
use state::PatchbayState;

static PIPEWIRE_DETECTION_CACHE: OnceLock<Mutex<Option<(bool, Instant)>>> = OnceLock::new();
const PIPEWIRE_DETECTION_CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct PatchbayConfig {
	pub sink_prefix: String,
	pub sink_description: String,
	pub virtual_mic: bool,
	pub virtual_mic_name: Option<String>,
	pub virtual_mic_description: Option<String>,
}

impl Default for PatchbayConfig {
	fn default() -> Self {
		Self {
			sink_prefix: "audio-share".to_string(),
			sink_description: "Virtual Audio Share".to_string(),
			virtual_mic: false,
			virtual_mic_name: None,
			virtual_mic_description: None,
		}
	}
}

pub fn has_pipewire() -> bool {
	let cache = PIPEWIRE_DETECTION_CACHE.get_or_init(|| Mutex::new(None));

	{
		let cached = *lock_unpoisoned(cache);
		if let Some((value, checked_at)) = cached
			&& checked_at.elapsed() < PIPEWIRE_DETECTION_CACHE_TTL
		{
			return value;
		}
	}

	let value = match detect_pipewire() {
		Ok(value) => value,
		Err(err) => {
			logger::warn(&format!("[patchbay] PipeWire detection failed: {err}"));
			false
		}
	};

	*lock_unpoisoned(cache) = Some((value, Instant::now()));
	value
}

pub fn ensure_pipewire() -> Result<()> {
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

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
	match mutex.lock() {
		Ok(guard) => guard,
		Err(poisoned) => poisoned.into_inner(),
	}
}

pub struct AudioSharePatchbay {
	state: PatchbayState,
}

impl Default for AudioSharePatchbay {
	fn default() -> Self {
		Self::new(&PatchbayConfig::default())
	}
}

impl AudioSharePatchbay {
	pub fn new(config: &PatchbayConfig) -> Self {
		if has_pipewire() {
			logger::info("[patchbay] ready");
		} else {
			logger::warn("[patchbay] PipeWire was not detected as the active audio server");
		}

		Self {
			state: PatchbayState::new(config),
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

	pub fn clear_routes(&mut self) -> Result<()> {
		self.state.clear_routes()
	}

	pub fn dispose(&mut self) -> Result<()> {
		self.state.dispose()
	}
}
