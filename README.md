# CSV Profiler + ESP32 OTA Dashboard

This project keeps the existing Flask/Pandas CSV profiler and adds a separate Rust/Axum OTA service for ESP32-S3 firmware.

## Remote console and Azure-ready SQL

The dashboard now includes a read-only **Remote Console**: ESP32 Rust application logs
travel over MQTT/TLS to Axum and then over WebSocket to the browser. SQLite remains the
local default; `DB_BACKEND=postgres` with `DATABASE_URL` selects PostgreSQL, including Azure Database
for PostgreSQL. This is not a Microsoft SQL Server driver.

See [setup, device configuration, security boundaries and Azure instructions](docs/remote-console-azure.md).
Cloud resources are not provisioned automatically. The opt-in MQTT setup helper can
generate local broker credentials and save them privately in .env. Sensor
telemetry remains deferred. Set `OTA_CONSOLE_ONLY=true` for a hosted read-only service;
never expose the trusted local firmware compiler publicly.

For the opt-in Windows development setup with a named Cloudflare Tunnel, see
[Cloudflare Tunnel setup](docs/CLOUDFLARE_DEV_SETUP.md). It serves the existing
Flask dashboard on `:5173` and Rust API on `:18080`; the normal `:5000`/`:7000`
launchers remain unchanged. The guide explains public hostname routes, security,
MQTT/WebSocket behavior, and verification.

Start a configured Windows laptop with:

```powershell
.\scripts\dev.ps1 -WithMqtt -AllowLanDevices
```

The launcher prints the current LAN firmware URL with the matching API port.
It does not update already-flashed devices. For setup and credential placement:

- [MQTT: backend password, board password, and viewer token](docs/MQTT_SETUP.md)
- [SQLite locally and Azure PostgreSQL](docs/DATABASE_SETUP.md)

Local SQLite/PostgreSQL tests and real-broker TLS/WebSocket probes pass.
Public Cloudflare routing, Azure credentials and physical ESP32 WSS still need
environment-specific validation; local success is not proof of cloud deployment.

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

For public phone/browser access with an administrator login (no registration),
use [the authenticated gateway setup](docs/PUBLIC_LOGIN.md) and
`scripts/dev-public.ps1`. Public publishing must go through that gateway.

Windows:

```bat
run-dev.bat
```

Linux/macOS:

```sh
chmod +x run-dev.sh
./run-dev.sh
```

### Fixed HTTPS address with Tailscale (Windows)

Tailscale Serve gives this development dashboard one stable HTTPS address while
keeping it private to your Tailscale network. The browser uses one hostname;
Tailscale sends `/` to Flask on port 5173 and sends `/api/ota`, `/api/tinyml`,
and `/health` to Axum on port 18080.

After installing Tailscale and signing in, enable **Serve** once in the
Tailscale admin page. Then run:

```powershell
# One time. This opens a Windows UAC prompt because Tailscale owns the routes.
.\scripts\setup-tailscale-serve.ps1

# Every development session.
.\scripts\dev-tailscale.ps1 -AllowLanDevices
```

The script prints the exact resulting address, shaped like:

```text
https://csv-ota-dashboard.<your-tailnet>.ts.net
```

Only devices signed in to the same Tailscale network can open it. This is
intentional: the OTA build API executes trusted uploaded Rust projects and is
not yet suitable for anonymous public access. The ESP32 itself cannot join this
tailnet, so `-AllowLanDevices` still advertises the current LAN address for
firmware registration and downloads.

If Tailscale reports that Serve is disabled, open the enable URL printed by the
CLI, approve it once, and rerun `setup-tailscale-serve.ps1`. Do not point a
quick tunnel at port 8080: this project has no service on that port.

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

Firmware paths are restricted to `firmware-builds`. Deletion rejects unsafe paths,
symlinks, a device's current/desired image, and active deployments with `409 Conflict`.
Completed historical deployments do not permanently block cleanup: the binary is
removed while a deleted metadata record preserves history. Failed physical cleanup
can be retried without making a partially deleted image available for deployment.

Build requests reserve a project atomically, so a double-click cannot start two
builds of the same upload. If the OTA server stops mid-build, it marks that job
failed on the next startup and leaves its log visible; start a new build to retry.
Run only one full OTA server against a given SQLite database. Multi-instance
workers need a shared job lease before they can safely recover interrupted jobs.

## Device client, partitions, and rollback

See [`examples/esp32-rust-ota-client/README.md`](examples/esp32-rust-ota-client/README.md) and [`docs/OTA_GUIDE.md`](docs/OTA_GUIDE.md). The stable runtime streams the HTTP response into the inactive OTA partition in 4 KiB chunks while calculating SHA-256; it does not buffer the firmware in RAM.

An OTA-ready device needs `otadata` and at least `ota_0` / `ota_1`. Flash the partition table and rollback-capable bootloader during initial provisioning; OTA downloads update only the inactive application slot.

The Devices page can **Cancel queued OTA**. This clears the server's desired image,
so an offline device will not fetch that image when it reconnects. The matching
API is `DELETE /api/ota/devices/{device_id}/update`. Cancellation cannot stop an
image already downloaded or being installed. Registration or heartbeat from a
device running the queued version now completes the deployment even if the
separate success callback was lost.
If a device reported `rebooting` but later registers with a different version,
the server marks that deployment failed and clears the desired image to avoid
an endless retry after rollback or USB recovery. Inspect the device and build
logs before deploying that image again.

To manually revert to an earlier version, select its retained, known-good image
and use the normal Deploy action. This queues an OTA update to that version; it
is different from ESP-IDF's automatic bootloader rollback after a bad boot.
Both require a reachable device. If bad Wi-Fi credentials make the device
offline, cancel the stale queued image first and recover over USB. Rebuilding
after changing `.env` is essential: existing `.bin` files keep the credentials
compiled into them. Give the corrected image a new version before deploying.

## TinyML monitoring

**Monitoring → Reporting** tracks MQTT inference runs separately from CSV reports
and device logs. It provides live results, custom-field charts, classification
and embedded-performance reports, and JSON/CSV downloads. The single
**Monitoring access** panel unlocks Reporting and Remote Console with one viewer
token; registered devices publish with their own MQTT credentials. Until viewer
access is connected, monitoring data and its device selectors remain locked.

Start with [the TinyML guide](docs/TINYML_GUIDE.md), including protocol examples,
Azure/PostgreSQL setup, metric definitions, and troubleshooting. A function-based
firmware example is in [uploaded-tinyml-demo](examples/uploaded-tinyml-demo/README.md).
No trained model or sensor is assumed. Existing firmware must be rebuilt with
the updated runtime before it can publish TinyML telemetry.

## Security boundary

ZIP extension, upload size, extracted size, path components, dependency paths,
and symlinks are validated. Managed composition edits only generated copies.
Commands use argument arrays rather than a shell; artifacts are hashed and file
serving is confined to the build directory.

However, `cargo build` executes `build.rs`, proc macros, and dependency build scripts. **Only build trusted projects on the host.** `FirmwareBuilder` isolates this execution behind a trait so a future `DockerFirmwareBuilder` can sandbox it without changing API routes. SHA-256 detects corruption but does not authenticate publishers; production deployments should add real image signing and device-side signature verification rather than treating a hash as a signature.

**Do not expose the OTA administration routes through a public tunnel yet.**
Upload/build/delete/deploy and device callbacks do not currently have admin or
per-device authentication. The Monitoring viewer token protects only Reporting
and Remote Console, and CORS is not authorization. Uploaded Cargo projects also
run with access to the developer host environment. Keep the full OTA API on a
trusted LAN until authentication and isolated builds are implemented.
