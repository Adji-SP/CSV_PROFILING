# CSV Profiler + ESP32 OTA Dashboard

This project keeps the existing Flask/Pandas CSV profiler and adds a separate Rust/Axum OTA service for ESP32-S3 firmware.

## Beginner handbook

The illustrated LaTeX module in [`docs/book`](docs/book/README.md) teaches the whole project from first startup through CSV internals, the Axum build pipeline, initial ESP32-S3 flashing, function-based uploaded firmware, OTA deployment, rollback, troubleshooting, and safe extension points. Complete English and Bahasa Indonesia editions are generated alongside their LaTeX sources.

- English: [`csv-profiler-ota-handbook.pdf`](docs/book/csv-profiler-ota-handbook.pdf)
- Bahasa Indonesia: [`csv-profiler-ota-handbook-id.pdf`](docs/book/csv-profiler-ota-handbook-id.pdf)

```text
Browser (:5000)
├── CSV profiling ──> Flask + Pandas (:5000)
└── OTA dashboard ──> Axum + SQLite (:7000) ──> ESP32-S3 over LAN
```

The CSV backend remains in `python/profiler_server.py`; OTA source lives in `ota-server/`. Rust source projects are compiled on the PC. Only the generated ESP-IDF application `.bin` is offered to devices.

## Project structure

```text
CSV_PROFILING/
├── web/                         Dashboard HTML/CSS/JS
├── python/                      Existing Flask CSV service
├── ota-server/                  Axum OTA API and builder
│   └── src/
│       ├── routes/              Projects, builds, firmware, devices
│       ├── models/              Serializable API/database models
│       ├── services/            Builder, ZIP handling, SHA-256
│       ├── config.rs
│       ├── errors.rs
│       ├── state.rs
│       └── storage.rs           SQLite schema and queries
├── firmware-projects/           Extracted trusted project uploads (ignored)
├── firmware-builds/             Generated OTA `.bin` files (ignored)
├── data/                        `ota.db` SQLite database (ignored)
├── examples/
│   ├── esp32-rust-ota-client/ Stable USB-flashed runtime and OTA client
│   ├── uploaded-firmware-basic/
│   └── uploaded-firmware-with-libs/
├── .env.example
├── run-dev.bat                  Windows launcher
└── run-dev.sh                   Linux/macOS launcher
```

## Requirements

### CSV profiler

- Python 3.10+
- Packages in `python/requirements.txt`

```powershell
py -3 -m venv python\.venv
python\.venv\Scripts\python -m pip install -r python\requirements.txt
```

Linux/macOS:

```sh
python3 -m venv python/.venv
python/.venv/bin/pip install -r python/requirements.txt
```

### Rust OTA service and ESP32-S3 builds

- Rust (`rustup`, `cargo`, `rustc`)
- A native C/C++ linker for the host Rust server (Visual Studio Build Tools with Desktop C++ on Windows)
- [ESP Rust prerequisites and `espup`](https://docs.esp-rs.org/book/installation/index.html)
- Espressif Rust toolchain with target `xtensa-esp32s3-espidf`
- [`espflash`](https://github.com/esp-rs/espflash), available on `PATH`
- ESP-IDF host prerequisites required by `esp-idf-sys`

Typical tooling checks:

```sh
cargo --version
rustc +esp --version
espflash --version
rustc +esp --print target-list
```

The final command should include `xtensa-esp32s3-espidf`.

On Windows, `run-dev.bat` uses the normal MSVC Rust toolchain when `link.exe` is available. If MSVC is unavailable but a `stable-x86_64-pc-windows-gnu` toolchain and `gcc` are already installed, it automatically uses that host toolchain for the Axum server.

## Configuration

Copy `.env.example` to `.env` and set the PC's real LAN address:

```env
OTA_PUBLIC_BASE_URL=http://192.168.1.5:7000
```

`localhost` cannot be used by an ESP32: on the device it points back to the ESP32. The OTA server automatically detects a likely LAN address when the variable is absent, but an explicit value is recommended. Relative storage paths are resolved from the repository root.

`OTA_RUNTIME_DIR` selects the stable ESP32 runtime template used for managed
application builds. The default is `examples/esp32-rust-ota-client`.

On native Windows, set `OTA_CARGO_TARGET_DIR` to a writable absolute path no
longer than 10 characters, such as `F:/csv-esp` or `C:/ota`. Forward slashes
keep the value portable through `.env` parsing. `esp-idf-sys`
rejects the much longer Cargo output path beneath an uploaded project's UUID;
Windows `subst` drives do not bypass that check. The server serializes local
firmware builds that share this short Cargo cache.

Also set `OTA_ESP_IDF_TOOLS_DIR` to the short ESP-IDF cache installed for the
device toolchain (for example `F:/csv-idf`) and `OTA_PYTHON_PATH` to a real base
Python executable. Dashboard builds pass these settings to `esp-idf-sys`, so it
does not clone ESP-IDF beneath the uploaded project's long UUID path or invoke
the Microsoft Store Python alias.

For managed application uploads, `WIFI_SSID`, `WIFI_PASS`, `DEVICE_NAME`, and
the optional `DEVICE_ID` are read from the server's `.env` and embedded at
compile time. The password is never written to the build log. Restart the OTA
server after changing these values.

CORS defaults to only `http://localhost:5000` and `http://127.0.0.1:5000`. Add separately served frontend origins to `OTA_ALLOWED_ORIGINS` as a comma-separated list.

## Run locally

Windows:

```bat
run-dev.bat
```

Linux/macOS:

```sh
chmod +x run-dev.sh
./run-dev.sh
```

Or run separately:

```sh
python python/profiler_server.py
cargo run --manifest-path ota-server/Cargo.toml
```

- Dashboard and CSV API: `http://localhost:5000`
- Rust OTA API: `http://localhost:7000`

## Firmware workflow

1. Zip either a managed application containing `Cargo.toml`, `ota-app.toml`, and
   `src/main.rs`, or a trusted standalone ESP-IDF Rust project.
2. Open **OTA Firmware**, select ESP32-S3, enter a version, then choose **Upload & Build**.
3. The service securely extracts the archive to a UUID directory.
4. Managed uploads are copied into the stable runtime as its `device-app` path
   dependency. Standalone projects are built unchanged and must retain their own
   OTA client.
5. `LocalFirmwareBuilder` executes `cargo build --release --target xtensa-esp32s3-espidf` with `RUSTUP_TOOLCHAIN=esp` by default.
6. The newest top-level release ELF is passed to `espflash save-image --chip esp32s3 --format esp-idf`. `--merge` is intentionally omitted so the artifact is an OTA application image, not a complete flash image.
7. The server streams SHA-256 calculation, stores metadata in SQLite, and lists the ready binary.
8. Select a registered device and firmware, then deploy. Deployment is pull-based: the device receives the update metadata on its next `/update` check.

Build stdout, stderr, exit code, and timestamps are retained. Failed compiler output is never hidden.

### Managed application contract

Managed applications use ordinary top-level functions rather than an application
class or trait implementation. Application API version 2 calls:

```rust
pub fn setup(context: &mut AppContext) -> anyhow::Result<State>;
pub fn main(context: AppContext, state: State) -> anyhow::Result<()>;
```

`State` is chosen by the application and can simply be `()`. Put fallible
peripheral initialization and startup validation in `setup`; if it fails on a
new image, the runtime requests rollback. After setup succeeds, the runtime marks
the slot valid, starts the OTA supervisor, and calls the uploaded `main` function.

The file remains `src/main.rs`. In the generated workspace only, the server sets
that file as a library target and disables Cargo's automatic binary discovery so
the stable runtime can call its functions. The uploaded source is not rewritten.

The application can declare normal registry, Git, or bundled local Cargo
dependencies. Dependencies must support `xtensa-esp32s3-espidf`. Local path
dependencies are accepted only when their resolved paths remain inside the ZIP.
ESP-IDF core crates must be compatible with the runtime's pinned versions.

See `examples/uploaded-firmware-basic` and
`examples/uploaded-firmware-with-libs`. The latter demonstrates third-party
libraries in an uploadable application.

## OTA API

| Method | Endpoint | Purpose |
|---|---|---|
| GET | `/api/ota/status` | Service health and public URL |
| GET/POST | `/api/ota/projects` | List/upload ZIP projects |
| POST | `/api/ota/projects/:project_id/build` | Queue a build |
| GET | `/api/ota/builds` | Build history |
| GET | `/api/ota/builds/:build_id` | Status and complete logs |
| GET | `/api/ota/firmware` | Firmware list |
| GET | `/api/ota/firmware/latest?target=esp32s3` | Newest ready target image |
| GET | `/api/ota/firmware/:firmware_id` | Firmware metadata |
| GET | `/api/ota/firmware/:firmware_id/download` | Stream the `.bin` |
| DELETE | `/api/ota/firmware/:firmware_id` | Delete an unreferenced image |
| POST | `/api/ota/devices/register` | Register/update a device |
| POST | `/api/ota/devices/:device_id/heartbeat` | Refresh device state |
| GET | `/api/ota/devices` | Device list |
| POST | `/api/ota/devices/:device_id/deploy` | Set desired firmware |
| GET | `/api/ota/devices/:device_id/update` | Device update check |
| POST | `/api/ota/devices/:device_id/ota-status` | Progress/success/failure callback |

Errors use a structured envelope:

```json
{
  "success": false,
  "error": {
    "code": "BUILD_FAILED",
    "message": "Cargo build failed",
    "details": "Process exited with code 101"
  }
}
```

## Storage and deletion

- SQLite: `data/ota.db`
- Uploaded projects: `firmware-projects/<project UUID>/`
- Firmware: `firmware-builds/firmware-<project>-v<version>-<build>.bin`

Firmware paths are reconstructed inside `firmware-builds` and canonicalized before download/deletion. Firmware referenced by a device or deployment history is rejected with `409 Conflict` rather than silently orphaning records.

## Device client, partitions, and rollback

See [`examples/esp32-rust-ota-client/README.md`](examples/esp32-rust-ota-client/README.md) and [`docs/OTA_GUIDE.md`](docs/OTA_GUIDE.md). The stable runtime streams the HTTP response into the inactive OTA partition in 4 KiB chunks while calculating SHA-256; it does not buffer the firmware in RAM.

An OTA-ready device needs `otadata` and at least `ota_0` / `ota_1`. Flash the partition table and rollback-capable bootloader during initial provisioning; OTA downloads update only the inactive application slot.

## Security boundary

ZIP extension, upload size, extracted size, path components, dependency paths,
and symlinks are validated. Managed composition edits only generated copies.
Commands use argument arrays rather than a shell; artifacts are hashed and file
serving is confined to the build directory.

However, `cargo build` executes `build.rs`, proc macros, and dependency build scripts. **Only build trusted projects on the host.** `FirmwareBuilder` isolates this execution behind a trait so a future `DockerFirmwareBuilder` can sandbox it without changing API routes. SHA-256 detects corruption but does not authenticate publishers; production deployments should add real image signing and device-side signature verification rather than treating a hash as a signature.
