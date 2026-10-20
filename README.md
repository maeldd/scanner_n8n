<p align="center">
  <img src="docs/banner.jpg" alt="n8n Scanner" width="100%"/>
</p>

<p align="center">
  <strong>Desktop application to connect a Bluetooth barcode scanner to n8n via webhook</strong>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#installation">Installation</a> •
  <a href="#usage">Usage</a> •
  <a href="#n8n-configuration">n8n Configuration</a> •
  <a href="#compilation">Compilation</a> •
  <a href="#license">License</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.85+-orange?logo=rust" alt="Rust"/>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-blue" alt="Platform"/>
  <img src="https://img.shields.io/badge/n8n-webhook-red?logo=n8n" alt="n8n"/>
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License"/>
</p>

---

## About

**n8n Scanner** is a lightweight desktop application written in Rust that captures barcodes from a Bluetooth scanner (like the Netum CS7501) connected via **SPP (Serial Port Profile)** and automatically forwards them to an **n8n** workflow via HTTP webhook.

The application features a modern graphical interface with dark/light themes, a real-time activity log, and scanning statistics.

---

## Features

- 🔌 **Auto-detection** of serial ports (COM on Windows, `/dev/cu.*` on macOS).
- 📡 **Smart serial listening** with raw chunk reading, 50ms timeout, and 100ms idle timeout (no dependency on `\n` terminators).
- 🌐 **Real-time webhook dispatch** to n8n with a structured JSON payload.
- 🎨 **Modern Interface** with an indigo palette, automatic light/dark theme following the OS, and a manual override (Auto → Dark → Light).
- 🌗 **Adaptive app icon**: automatically switches between its dark and light variants to match the OS appearance (macOS/Windows), including live theme changes.
- 📊 **KPI Dashboard**: Total scans, Successes, Failures.
- 📋 **Last scanned code** displayed in large text with 1-click copy.
- 📜 **Activity Log** with color-coded severity levels (Info, Success, Warning, Error, Raw).
- 🔄 **Real-time URL sync**: Edit the webhook URL without stopping the capture process.
- 🧪 **Webhook test button** to validate network connection before scanning.
- ⚡ **Built-in n8n assistant**: Detects if the URL is in test mode (`/webhook-test/`) and offers a 1-click switch to production (`/webhook/`).
- 💾 **Persistence**: User configuration is automatically saved in `scanner_config.json`.
- 🖥️ **Cross-platform**: macOS (native) and Windows (cross-compiled).

---

## Installation

### Pre-compiled Binaries

Download the appropriate binary for your OS from the [Releases](../../releases) page:

| OS | File | Architecture |
|---|---|---|
| **macOS** (Apple Silicon) | `Scanner-n8n-<version>-arm64.dmg` | aarch64 |
| **Windows** | `Scanner-n8n-Setup-<version>-x64.exe` | x86_64 |
| **macOS** (bare binary) | `scanner_n8n` | aarch64 |
| **Windows** (bare binary) | `scanner_n8n.exe` | x86_64 |

### From Source

```bash
# Requirement: Rust 1.85+
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone and compile
git clone https://github.com/<your-username>/scanner_n8n.git
cd scanner_n8n
cargo build --release

# The binary will be located in target/release/scanner_n8n
```

---

## Usage

### 1. Prepare the Scanner

> **Important**: The scanner must be set to **SPP (Serial Port Profile)** mode, NOT HID (keyboard) mode.

For the **Netum CS7501**:
1. Scan the **"Bluetooth SPP Mode"** barcode found in the scanner's manual.
2. Pair the Bluetooth scanner with your computer.
3. The scanner will now appear as a serial port:
   - **macOS**: `/dev/cu.CSbarcodescanner` (or similar)
   - **Windows**: `COM3`, `COM4`, etc.

### 2. Launch the Application

```bash
# From source
cargo run

# Or directly run the binary
./target/release/scanner_n8n      # macOS
scanner_n8n.exe                    # Windows
```

### 3. Configuration

1. **Webhook URL**: Paste your n8n webhook URL (Use the **production** version: `https://your-n8n/webhook/...`).
2. **Serial Port**: Select your scanner's port from the dropdown list.
3. **Baud Rate**: Generally `9600` (default) or `57600`.
4. **Device ID**: A custom identifier to recognize the scanner node in n8n.
5. Click **"▶ Start Capture"**.

Settings are saved automatically:
- **macOS**: `~/Library/Application Support/scanner_n8n/scanner_config.json`
- **Windows**: `%APPDATA%\scanner_n8n\scanner_config.json`
- **Development**: `./scanner_config.json` when present
- **Override**: set `SCANNER_N8N_CONFIG=/path/to/scanner_config.json`

---

## n8n Configuration

### Creating the Workflow

1. In n8n, create a new workflow.
2. Add a **Webhook** node.
3. Set Method to `POST`.
4. Copy the **Production URL** (the one without `-test`).
5. **Activate the workflow** (toggle in the top right corner → Green).
6. Paste the URL into the n8n Scanner application.

### Payload received by n8n

```json
{
  "barcode": "3760123456789",
  "timestamp": "2026-10-19T14:30:00.000Z",
  "device_id": "Netum-CS7501",
  "is_test": null
}
```

### ⚠️ Test vs Production URLs

| Type | URL | When it works |
|---|---|---|
| **Test** | `https://.../webhook-test/<id>` | Only when you click "Listen for test event" in the n8n editor |
| **Production** | `https://.../webhook/<id>` | 24/7, as long as the workflow is activated |

> **If you receive a 404 Error**, ensure you are using the **production** URL (`/webhook/`) and that the workflow is **active**.

---

## Compilation

### macOS (native)

```bash
cargo build --release
# → target/release/scanner_n8n
```

### Windows (cross-compilation from macOS)

```bash
# Install prerequisites (one time only)
rustup target add x86_64-pc-windows-gnu
brew install mingw-w64

# Compile
RUSTC=~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo build \
--release --target x86_64-pc-windows-gnu

# → target/x86_64-pc-windows-gnu/release/scanner_n8n.exe
```

---

## Packaging

### macOS — `.app` + DMG

```bash
./scripts/package-macos.sh
# → dist/Scanner n8n.app
# → dist/Scanner-n8n-<version>-<arch>.dmg
# → dist/Scanner-n8n-<version>-<arch>      (bare binary)
```

- Ad-hoc signature by default. For a Developer ID signature:
  ```bash
  APPLE_SIGNING_IDENTITY="Developer ID Application: ..." ./scripts/package-macos.sh
  ```
- The `.app` uses `assets/icon-dark.icns` as its Finder icon; the running app switches between the dark/light Dock icons automatically.
- Notarization is not automated. A downloaded non-notarized app may require right-click → Open, or `xattr -cr "/Applications/Scanner n8n.app"`.
- The bundle identifier can be overridden with `BUNDLE_ID=...`.

### Windows — installateur NSIS

Prerequisites: the cross-compilation toolchain (see above) and NSIS.

```bash
brew install nsis                # macOS
# or: sudo apt install nsis      # Linux
./scripts/package-windows.sh
# → dist/Scanner-n8n-<version>-x64.exe        (bare binary)
# → dist/Scanner-n8n-Setup-<version>-x64.exe  (installer)
```

The installer installs to `Program Files`, creates Start Menu and desktop shortcuts, and registers itself in **Add or remove programs**. The application icon is embedded directly into `scanner_n8n.exe` by `build.rs` (via the `winresource` crate).

On native Windows, use the MSVC target:

```bash
TARGET=x86_64-pc-windows-msvc ./scripts/package-windows.sh
```

### Automated release

Push a tag such as `v1.1.0`: the GitHub Actions workflow (`.github/workflows/release.yml`) builds the macOS DMG and the Windows installer on native runners and attaches them to the GitHub release.

---

## Tech Stack

| Component | Technology |
|---|---|
| Language | Rust (2024 edition) |
| GUI | [egui](https://github.com/emilk/egui) / [eframe](https://github.com/emilk/egui/tree/master/crates/eframe) 0.29 |
| Serial Port | [serialport-rs](https://github.com/serialport/serialport-rs) 4.3 |
| HTTP | [reqwest](https://github.com/seanmonstar/reqwest) 0.12 (blocking) |
| Serialization | [serde](https://serde.rs/) + serde_json |
| Date/Time | [chrono](https://github.com/chronotope/chrono) 0.4 |

---

## Project Structure

```text
scanner_n8n/
├── .cargo/
│   └── config.toml             # Linker config for Windows cross-compilation
├── .github/
│   └── workflows/
│       └── release.yml         # CI: macOS DMG + Windows installer
├── scripts/
│   ├── package-macos.sh        # Builds .app and .dmg
│   └── package-windows.sh      # Builds .exe and NSIS installer
├── packaging/
│   └── windows/
│       └── installer.nsi       # NSIS installer script
├── src/
│   └── main.rs                 # Main source code (GUI + serial + webhook)
├── docs/
│   └── banner.jpg              # README banner image
├── assets/
│   ├── icon-dark.svg           # Dark icon source
│   ├── icon-light.svg          # Light icon source
│   ├── icon-dark.png           # 1024×1024 dark variant (embedded)
│   ├── icon-light.png          # 1024×1024 light variant (embedded)
│   ├── icon-dark.icns          # macOS icon (dark)
│   ├── icon-light.icns         # macOS icon (light)
│   ├── icon-dark.ico           # Windows icon (dark)
│   ├── icon-light.ico          # Windows icon (light)
│   └── png/{dark,light}/       # PNG sizes 16 → 512 px
├── dist/                       # Generated packages (git-ignored)
├── build.rs                    # Windows icon/metadata embedding
├── Cargo.toml                  # Rust dependencies
├── Cargo.lock                  # Locked dependency versions
├── scanner_config.json         # User configuration (development)
├── CHANGELOG.md                # Version history
├── LICENSE                     # MIT License
└── README.md                   # This file
```

---

## Troubleshooting

<details>
<summary><strong>The scanner does not appear in the port list</strong></summary>

- Ensure the scanner is **turned on** and **paired** via Bluetooth.
- On macOS, it should appear in `System Settings > Bluetooth`.
- Click the **"🔄 Refresh"** button in the application.
</details>

<details>
<summary><strong>Scanner is connected but no scans are showing up</strong></summary>

- Verify the scanner is in **SPP** mode (not HID). In HID mode, it sends characters like a keyboard and bypasses the serial port entirely.
- Scan the "Bluetooth SPP Mode" barcode from the Netum manual.
- The scanner should emit **1 beep** and display a **solid blue** LED.
</details>

<details>
<summary><strong>404 Error when sending to the webhook</strong></summary>

- Use the **production** URL (`/webhook/...`) instead of the test URL (`/webhook-test/...`).
- Verify that the n8n workflow is **activated** (green toggle).
- The app automatically detects test URLs and offers a quick conversion button.
</details>

<details>
<summary><strong>Network connection error</strong></summary>

- Check that your n8n instance is accessible from your network.
- Test the URL in a browser or using `curl`.
- Use the **"⚡ Test Webhook"** button in the app to diagnose connectivity issues.
</details>

---

## License

This project is licensed under the [MIT](LICENSE) License.

---

<p align="center">
  Built with ❤️ in Rust by <a href="https://github.com/<your-username>">@your-username</a>
</p>