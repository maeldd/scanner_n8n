# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.0.0] - 2026-10-19

### 🎉 Initial Release

First stable release of n8n Scanner — a desktop application to connect a Bluetooth barcode scanner to n8n via webhook.

### Added

#### Graphical User Interface
- Native desktop interface built with [egui](https://github.com/emilk/egui) / eframe 0.29.
- **Dark** (default) and **Light** themes with a built-in toggle.
- Modern slate/indigo color palette with rounded corners.
- KPI Dashboard featuring 3 metrics: Total Scans, n8n Successes, Failures.
- "Last scanned code" card with large monospace display, timestamp, and HTTP status.
- **Copy** button to save the last scanned code to the clipboard.
- Real-time activity log with color-coded levels (Info, Success, Warning, Error, Scan, Raw).
- Auto-scroll toggle for the activity log.

#### Serial Capture
- Auto-detection of available serial ports (macOS: `/dev/cu.*`, Windows: `COMx`).
- Raw chunk reading (512 bytes) with a 50ms timeout.
- 100ms idle timeout for scanners that do not send `\r` or `\n` terminators.
- Explicit activation of **DTR** and **RTS** control lines for maximum compatibility.
- Supported baud rates: 9600, 19200, 38400, 57600, 115200.
- Automatic reconnection upon serial link loss.
- `[RAW]` logs for troubleshooting received bytes.

#### n8n Webhook
- Automatic forwarding of scanned codes to an n8n webhook via HTTP POST (JSON).
- Structured payload: `barcode`, `timestamp` (ISO 8601), `device_id`, `is_test`.
- **"⚡ Test Webhook"** button to validate the connection without scanning.
- Smart detection of test URLs (`/webhook-test/`) with a 1-click assistant to switch to production (`/webhook/`).
- Dynamic URL synchronization: edit the URL in real-time without restarting the capture (via `Arc<RwLock>`).
- 5-second HTTP timeout.

#### Configuration & Persistence
- Automatic saving to `scanner_config.json`.
- Persisted settings: webhook URL, device ID, serial port, baud rate, theme.
- Smart serial port fallback: if the saved port no longer exists (e.g., OS change), automatically defaults to the first available port.

#### Cross-platform
- Native **macOS** build (aarch64-apple-darwin).
- **Windows** x64 cross-compilation (x86_64-pc-windows-gnu via MinGW-w64).
- Added `windows_subsystem = "windows"` attribute in release mode to hide the Windows console.
- Cargo configuration for the MinGW linker (`.cargo/config.toml`).

### Tested Compatibility

| Scanner | Mode | OS | Status |
|---|---|---|---|
| Netum CS7501 | Bluetooth SPP | macOS Sequoia | ✅ |
| Netum CS7501 | Bluetooth SPP | Windows 10/11 | ✅ |

### Dependencies

| Crate | Version | Role |
|---|---|---|
| `eframe` | 0.29 | Native GUI framework (egui) |
| `serialport` | 4.3 | Serial port reading |
| `reqwest` | 0.12 | HTTP Client (blocking + JSON) |
| `serde` | 1.0 | Serialization/Deserialization |
| `serde_json` | 1.0 | JSON format formatting |
| `chrono` | 0.4 | Date/Time management |

---

## [Unreleased]

### Planned
- Linux support (`/dev/ttyUSB*` and `/dev/ttyACM*` ports).
- CSV export / scan history logging.
- Audio notifications for successful/failed scans.
- Multi-scanner mode (listen on multiple ports simultaneously).
- App packaging: `.app` (macOS) and `.msi` (Windows).