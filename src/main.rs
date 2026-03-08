mod logger;
mod patchbay;

use std::io::{self, BufRead, Write};

use miniserde::{Deserialize, Serialize};
use patchbay::{has_pipewire, AudioSharePatchbay};

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
struct SuccessResponse<T: Serialize> {
    id: u64,
    result: T,
}

#[derive(Serialize)]
struct ErrorResponse {
    id: u64,
    error: String,
}

fn write_json_line<T: Serialize>(out: &mut impl Write, value: &T) -> io::Result<()> {
    let serialized = miniserde::json::to_string(value);
    out.write_all(serialized.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

fn write_result<T: Serialize, E: ToString>(
    out: &mut impl Write,
    id: u64,
    result: std::result::Result<T, E>,
) -> io::Result<()> {
    match result {
        Ok(value) => write_json_line(out, &SuccessResponse { id, result: value }),
        Err(err) => write_json_line(out, &ErrorResponse { id, error: err.to_string() }),
    }
}

fn handle_request(
    out: &mut impl Write,
    patchbay: &mut AudioSharePatchbay,
    request: Request,
) -> io::Result<bool> {
    match request {
        Request::HasPipeWire { id } => {
            write_json_line(out, &SuccessResponse { id, result: has_pipewire() })?;
        }
        Request::ListShareableNodes {
            id,
            include_devices,
        } => {
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

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut patchbay = AudioSharePatchbay::new();

    for line in stdin.lock().lines() {
        let line = line?;

        if line.trim().is_empty() {
            continue;
        }

        let request = match miniserde::json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(err) => {
                logger::warn(&format!("[helper] invalid request: {:?}", err));
                write_json_line(
                    &mut stdout,
                    &ErrorResponse { id: 0u64, error: format!("invalid request: {:?}", err) },
                )?;
                continue;
            }
        };

        if !handle_request(&mut stdout, &mut patchbay, request)? {
            break;
        }
    }

    Ok(())
}