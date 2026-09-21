# Remote console and Azure SQL storage

The ESP32 can be far away from your laptop. As long as it has Internet access,
it can send application logs to an MQTT broker. Your browser receives those
logs through the Rust server. You do not need to forward a port on the ESP32's router.

This is a **read-only application console**, not a remote USB serial port or shell.
Rust `log::info!`, `warn!` and `error!` output is forwarded and remains visible over USB.
ROM boot messages, ESP-IDF C output, `println!`, and crashes before networking starts
are not captured. Keep USB monitoring available for initial setup and recovery.
Sensor ingestion and remote command execution are deliberately not implemented.

```mermaid
flowchart LR
    App[Uploaded Rust application] -->|log macros| Runtime[Managed firmware runtime]
    Runtime -->|local output| USB[USB serial monitor]
    Runtime -->|MQTT over TLS, outbound 8883| Broker[Authenticated MQTT broker]
    Broker -->|authorized logs subscription| Rust[Rust Axum console service]
    Rust -->|WSS, read-only authenticated stream| Browser[Remote Console dashboard]
    Rust <-->|TLS SQL connection| DB[Azure Database for PostgreSQL]
```

## What works locally first

Leave `DATABASE_URL`, `MQTT_HOST` and the device MQTT settings unset to retain
SQLite and USB-only firmware logging. Flask CSV routes and OTA routes remain separate.
Start with `run-dev.bat` on Windows or `run-dev.sh` on Linux/macOS.

For the remote console, you need a TLS MQTT broker and separate broker identities:

| Identity | Allowed action |
| --- | --- |
| Each device, for example `ESP32-S3-001` | Publish only `devices/ESP32-S3-001/logs` |
| Console backend | Subscribe only `devices/+/logs` |
| Dashboard operator | Read logs using a console access token; no broker credentials |

Do not allow anonymous MQTT access. Topic/payload matching in the application is an
extra check, **not a replacement for broker authorization**. Never give devices a
wildcard publish permission. Do not retain log messages.

### 1. Configure the Rust service

Copy the relevant commented settings from `.env.example` into the root `.env`.
Generate a random viewer token of at least 32 characters, rather than a memorable password.
For example, `[guid]::NewGuid().ToString('N') + [guid]::NewGuid().ToString('N')`
in PowerShell generates a suitable development token. Share it only with your operators.

```dotenv
CONSOLE_VIEWER_TOKEN=replace-with-your-random-token
MQTT_HOST=your-broker-dns-name
MQTT_PORT=8883
MQTT_USERNAME=console-backend
# For password-authenticated TLS brokers:
# MQTT_PASSWORD=your-backend-password
# For mutual TLS, provide all three file paths:
# MQTT_CA_FILE=D:/certs/broker-root.pem
# MQTT_CERT_FILE=D:/certs/backend.pem
# MQTT_KEY_FILE=D:/certs/backend-key.pem
```

The backend always uses TLS and validates the broker certificate. With no custom CA,
it uses its default trust roots. For mutual TLS, supply the broker's trust roots too.
Keep certificate private keys outside this repository. Restart the backend after
changing configuration. A short/missing viewer token prevents MQTT startup.

### 2. Configure and build each board

Device settings are **compile-time** values. Editing `.env` cannot change firmware
already running on the ESP32. Rebuild and flash, or install a rebuilt image through OTA.

For a direct PowerShell firmware build, set environment variables before invoking
`flash-esp32.ps1`. That script does not load these MQTT settings from `.env` itself:

```powershell
$env:DEVICE_ID = 'ESP32-S3-001'
$env:DEVICE_MQTT_URL = 'mqtts://your-broker-dns-name:8883'
$env:DEVICE_MQTT_USERNAME = 'ESP32-S3-001'
# Only for a private broker CA:
$env:DEVICE_MQTT_CA_PEM = Get-Content -Raw 'D:/certs/broker-root.pem'
# For mutual TLS:
$env:DEVICE_MQTT_CERT_PEM = Get-Content -Raw 'D:/certs/device-001.pem'
$env:DEVICE_MQTT_KEY_PEM = Get-Content -Raw 'D:/certs/device-001-key.pem'
./flash-esp32.ps1 -BuildOnly
```

Use the helper's normal flash workflow when ready. For dashboard builds, the Rust
server loads the root `.env` and build commands inherit those device settings;
restart the server after changing them. Use unique credentials and `DEVICE_ID` per
board. Do not distribute a per-device credential-bearing image to the whole fleet.
Provisioning secrets in protected NVS is a future improvement; this implementation
embeds them in the firmware, so treat `.bin`, ELF and build artifacts as sensitive.

The managed runtime installs the logging bridge; your uploaded function-based
application stays the same. Use normal log macros in your application:

```rust
log::info!("Application started");
log::warn!("Network is temporarily unavailable");
```

Do not initialize another global logger in the uploaded application. Standalone
firmware that bypasses the managed runtime must integrate the module itself.
Wi-Fi ownership remains with the existing manager. SNTP is started for TLS certificate
time checks; ensure the device network permits DNS, NTP and outbound MQTT TLS.

### 3. Open the console

The device must already be registered in the same database used by the console server.
Open **Remote Console**, enter the viewer token, click **Refresh devices**, choose
the board and **Connect**. There are level/text filters, pause, auto-scroll and NDJSON
download controls. Reconnect explicitly after a network interruption.

Tokens are not placed in URLs or local storage. Use HTTPS/WSS outside localhost;
the local HTTP development connection does not encrypt the viewer token.

## SQL: Azure PostgreSQL, not Azure SQL Database

Both are SQL databases, but they are different products. This implementation supports
**SQLite and PostgreSQL**. It does not support Microsoft SQL Server/T-SQL or Cosmos DB.
For Azure, choose **Azure Database for PostgreSQL Flexible Server**.

```dotenv
DATABASE_URL=postgres://ota_user:URL_ENCODED_PASSWORD@your-server.postgres.database.azure.com:5432/ota?sslmode=verify-full
```

Create the database and a dedicated role first. Restrict network access to the backend
and authorized development machines. Configure the current Azure root CA chain if
needed with the PostgreSQL connection option `sslrootcert`; do not disable verification
to work around certificate errors. See [Azure PostgreSQL TLS guidance](https://learn.microsoft.com/en-us/azure/postgresql/security/security-tls-how-to-connect).

The schema lives in `ota-server/migrations/001_initial.sql`. On startup the service
applies version 1 transactionally and records it in `schema_versions`. It adopts the
existing SQLite tables without dropping them. Start one instance for migration before
starting other processes. Back up existing databases first.

Changing `DATABASE_URL` selects another database; **it does not copy your SQLite data**.
The new database starts empty. Re-register test boards there or plan a separately
reviewed data migration. Firmware file paths still refer to local files: sharing a
database does not move artifacts to Azure Blob Storage.

To exercise the same storage test against a disposable PostgreSQL database:

```powershell
$env:TEST_DATABASE_URL = 'postgres://test_user:password@localhost:5432/ota_test'
cd ota-server
cargo test postgres_integration -- --ignored
```

Never point this test at production. It creates the schema and temporary test records.

## Azure MQTT configuration

Azure Event Grid namespaces offer an MQTT broker with X.509 authentication; this
integration uses that standard MQTT interface, not an IoT Hub device endpoint.
Follow [Microsoft's certificate authentication guide](https://learn.microsoft.com/en-us/azure/event-grid/mqtt-client-certificate-authentication)
and [topic-space authorization guide](https://learn.microsoft.com/en-us/azure/event-grid/mqtt-topic-spaces).

1. Create an Event Grid namespace and enable MQTT in a supported region.
2. Register one certificate-backed client per board, plus a separate backend client.
3. Use a device's `DEVICE_ID` as its client authentication name and MQTT username.
4. Grant the device group Publisher access to a topic space template
   `devices/${client.authenticationName}/logs`.
5. Grant only the backend group Subscriber access to a separate `devices/+/logs` topic space.
6. Use the namespace's MQTT hostname in both backend and firmware configuration,
   with the respective client certificates and private keys. No shared device certificate.

Confirm that a device cannot publish to another device's topic before calling the setup
ready. Test certificate expiry/revocation and rotate credentials using your provisioning process.

## Hosting the viewer safely

Set `OTA_CONSOLE_ONLY=true` on a hosted instance. That mounts only status and the two
authenticated console endpoints; it does **not** expose project upload, builds,
deployment, firmware download or device registration. Register devices through the
private/local full service using the same PostgreSQL database. The public viewer
does not make existing LAN-only OTA URLs work over the Internet.

Serve `web/` through an HTTPS web server and reverse-proxy `/api/ota/console/` to Axum,
including WebSocket Upgrade headers. HTTPS pages use their own origin for the OTA API.
Set `OTA_ALLOWED_ORIGINS=https://your-dashboard-domain` exactly. Keep port 7000 private
behind the proxy. `deploy/console.nginx.conf` is an example location configuration.
CSV needs a separately secured Flask upstream; console-only hosting does not publish CSV.

Start with one backend replica. History lives in its memory, not SQL: up to 200 entries
per device, at most 256 devices, and 2,000 entries per browser. Restarting loses history.
The firmware queue holds 64 entries; during outages it drops excess logs rather than
blocking your application. MQTT QoS 0 logs are best-effort, not an audit trail. A `dropped`
field reports firmware queue/publish losses, but cannot detect every network loss.

The viewer token grants access to all registered devices. This is for a small trusted
operator group, not tenant isolation. Add Entra login and per-device authorization before
multi-user production use. Rate-limit connection attempts at the proxy. Never put Wi-Fi
passwords, private keys or customer secrets in log messages.

## Deliberately not deployed yet

No Azure resources are provisioned by these changes. A subscription, region, DNS,
certificates and credentials must be supplied and tested. Blob artifact storage,
isolated cloud build workers, persistent log retention, Entra user authorization and
sensor ingestion are follow-up work. Do not expose the full local build server publicly:
compiling uploaded Rust executes code, including build scripts.

## Design decision

We retain SQLite for offline/local use and add PostgreSQL through the existing storage
layer. This avoids forcing Azure onto local development, but requires testing both SQL
dialects. MQTT separates device connectivity from browsers; WebSocket avoids handing
broker credentials to browsers. The trade-off is an extra broker and ephemeral log
delivery. Revisit this design when durable retention, multiple console replicas, or
per-tenant authorization becomes a requirement.

## Implementation map and verification

| Files | Responsibility |
| --- | --- |
| `ota-server/src/console.rs` | MQTT/TLS ingestion, bounded history, token authentication and WebSocket streaming |
| `ota-server/src/console_tests.rs` | Loopback authentication, origin, history/live delivery and console-only route tests |
| `ota-server/src/storage.rs`, `migrations/001_initial.sql` | SQLite/PostgreSQL queries, versioned schema and storage regression tests |
| `ota-server/src/config.rs`, `state.rs`, `main.rs`, `routes/mod.rs`, `routes/builds.rs` | Configuration and route/storage wiring |
| `web/index.html`, `app.js`, `ota.js`, `console.js` | Navigation, same-origin HTTPS API selection and console UI |
| `examples/esp32-rust-ota-client/src/main.rs`, `remote_console.rs` | SNTP and optional MQTT log forwarding without changing uploaded application structure |
| Cargo manifests and lockfiles | SQL, WebSocket, MQTT/TLS and firmware JSON dependencies |
| `.env.example`, `deploy/console.nginx.conf`, `README.md`, this guide | Configuration, proxy example and operating instructions |

Verified during implementation: backend unit/integration tests (including a temporary
SQLite database and loopback WebSocket server), JavaScript syntax checks, and ESP32-S3
firmware `cargo check --release --target xtensa-esp32s3-espidf`. The firmware check still
reports upstream ESP-IDF macro configuration warnings. It is not a hardware test.
PostgreSQL integration is an explicitly ignored opt-in test until a disposable database
is supplied. Real MQTT/TLS, Azure deployment and physical ESP32 log delivery remain
to be verified with provisioned credentials and hardware.
