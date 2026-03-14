# patchcord

A lightweight Rust helper that manages PipeWire virtual audio routing via a JSON-over-stdio protocol. It creates virtual sinks, optionally wraps them as virtual microphones, and links application audio output ports to the virtual sink, enabling per-application audio capture and sharing.    

Made for internal use in [GoofCord](https://github.com/Milkshiift/GoofCord).

## Requirements

- Linux with [PipeWire](https://pipewire.org/) as the active audio server
- PipeWire CLI utilities: `pw-dump`, `pw-link`
- PulseAudio compatibility layer: `pactl` (provided by `pipewire-pulse` on most distros)

## Building

### Stable Rust

```bash
cargo build --release
```

The resulting binary will be at `target/release/patchcord`.

### Nightly Rust
To achieve a smaller binary size and cross-compile to x64 and arm64, you can use the `build-rs.sh` script. The binaries in `dist` are built using this script.

## Usage

```
patchcord [OPTIONS]
```

### Options

| Flag | Description |
|---|---|
| `--sink-prefix <PREFIX>` | Set the virtual sink name prefix |
| `--sink-description <DESC>` | Set the virtual sink's visible description |
| `--virtual-mic` | Automatically create a virtual microphone source wrapping the sink's monitor |
| `--virtual-mic-name <NAME>` | Set the virtual microphone's internal name |
| `--virtual-mic-description <DESC>` | Set the virtual microphone's visible description |
| `-h`, `--help` | Print help information and exit |

### Environment Variables

| Variable | Description |
|---|---|
| `PATCHCORD_TRACE` | Set to any value to enable trace-level log output to stderr |

## Protocol

Patchcord communicates over **stdin/stdout** using newline-delimited JSON. Each request must include an `id` field (unsigned 64-bit integer) and a `method` field. Responses are JSON objects containing either `{ "id", "result" }` on success or `{ "id", "error" }` on failure.

### Methods

#### `hasPipeWire`

Check whether PipeWire is detected as the active audio server.

```json
{"id": 1, "method": "hasPipeWire"}
```

**Response:**
```json
{"id": 1, "result": true}
```

---

#### `listShareableNodes`

List audio nodes (applications and optionally hardware devices) that can be routed to the virtual sink.

```json
{"id": 2, "method": "listShareableNodes", "includeDevices": false}
```

**Response:**
```json
{
  "id": 2,
  "result": [
    {
      "id": 42,
      "displayName": "Firefox",
      "applicationName": "Firefox",
      "nodeName": "Firefox",
      "description": "Firefox",
      "mediaName": "AudioStream",
      "binary": "firefox",
      "processId": 12345,
      "isDevice": false
    }
  ]
}
```

---

#### `ensureVirtualSink`

Create the virtual null sink (and optionally the virtual microphone wrapper) if they don't already exist. Returns sink information.

```json
{"id": 3, "method": "ensureVirtualSink"}
```

**Response:**
```json
{
  "id": 3,
  "result": {
    "sinkName": "audio-share-7890-1",
    "monitorSource": "audio-share-7890-1.monitor",
    "nodeId": 99,
    "virtualMicName": "audio-share-mic-7890-1",
    "virtualMicDescription": "Virtual Audio Share (Virtual Mic)"
  }
}
```

---

#### `routeNodes`

Route one or more application/device nodes to the virtual sink by creating PipeWire links. Stale routes from a previous call are automatically removed.

```json
{"id": 4, "method": "routeNodes", "nodeIds": [42, 55]}
```

**Response:** Same shape as `ensureVirtualSink`.

---

#### `clearRoutes`

Remove all active PipeWire links managed by this session without destroying the virtual sink.

```json
{"id": 5, "method": "clearRoutes"}
```

**Response:**
```json
{"id": 5, "result": null}
```

---

#### `dispose`

Tear down all routes, the virtual microphone, and the virtual sink. The helper exits after responding.

```json
{"id": 6, "method": "dispose"}
```

**Response:**
```json
{"id": 6, "result": null}
```