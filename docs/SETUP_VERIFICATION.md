# Setup verification — 2026-10-07

## Verified locally

| Check | Result |
| --- | --- |
| Rust backend suite | 31 passed; 2 opt-in PostgreSQL tests excluded from the normal run |
| PostgreSQL integration | Both opt-in tests passed on a disposable PostgreSQL 17.11 cluster |
| Flask URL configuration | 2 tests passed, including default 5000/7000 and 5173/18080 port mapping |
| MQTT configuration editor | 4 tests passed; duplicate settings and multiline-value rejection covered |
| PowerShell helpers | Syntax passed; LAN selection, MQTT URL validation and multiline-environment isolation tests passed |
| Browser scripts | OTA, Reporting and viewer-access JavaScript syntax passed |
| Real Mosquitto | Backend/device TLS authentication, subscriptions, device publish delivery and loopback WebSocket-to-TLS delivery passed |
| Development launcher | Started/stopped Flask and Rust; checked advertised firmware URL and authenticated Reporting MQTT subscription |
| ESP32-S3 example | Release build passed; non-merged OTA application image generated; no flash/deployment performed |

The PostgreSQL test cluster was stopped afterward. The launcher smoke tests stopped
their own Flask/Rust processes. The dedicated MQTT broker remains available separately.
SQLite is still the active project database; existing data was not migrated.

## Firmware artifact

- Target: xtensa-esp32s3-espidf
- Example runtime version used for this build: 0.1.0-verification
- File: C:\ota\xtensa-esp32s3-espidf\release\firmware-ota.bin
- Application size: 1,275,680 bytes
- OTA slot size: 1,966,080 bytes (64.88% used)
- SHA-256: 9dc448e054fcd153a8a944c9f748643934793c8372ac4d5ef10afb9efc10b405
- espflash reported Merge: false.
- Generated SDK configuration enables the CA bundle, MQTT WebSocket and secure WebSocket transports.

This is the example/default application, not a replacement deployment of a user's
uploaded application. The test build embeds the private network/device settings,
so do not publish the image. It was built for the root .env server URL on port 7000;
a development launcher using 18080 is a different endpoint. Rebuild for the intended
server URL and version before installing. Upstream ESP-IDF macro cfg warnings remain;
they did not prevent compilation or image generation.

## Fixed during verification

- SQLite/PostgreSQL selection is explicit and Azure PostgreSQL requires verified TLS.
- LAN development downloads now use the selected adapter AND API port.
- The laptop moved from 192.168.0.105 to 192.168.0.103; private local URLs and the
  broker leaf certificate were updated. Existing MQTT passwords and CA were retained.
- Local private MQTT CA selection no longer loses precedence to ESP-IDF's public CA bundle.
- The USB build helper accepts secure MQTT WebSocket URLs as well as mqtts.
- Visual Studio environment discovery no longer sees multiline certificate/key values.
- Broker certificate renewal no longer requests unrelated Windows audit privileges.

## Changed-file summary

| Area | Files |
| --- | --- |
| Backend | ota-server/src/config/database.rs; ota-server/src/main.rs; ota-server/src/services/builder.rs |
| MQTT tools | scripts/setup-mqtt.ps1; start-mqtt.ps1; provision-mqtt.py; check-mqtt.ps1; check-mqtt.py; env-file.ps1 |
| ESP32 | flash-esp32.ps1; examples/esp32-rust-ota-client/src/remote_console.rs; sdkconfig.defaults |
| Network/browser integration | scripts/dev.ps1; dev-network.ps1; cloudflared/config.example.yml; python/profiler_server.py; web/ota.js; tinyml.js; viewer-access.js |
| Tests | scripts/test-postgres.ps1; test-mqtt-config.py; test-firmware-env.ps1; test-dev-network.ps1; test-cloudflare-config.py |
| Configuration/docs | .env.example; .gitignore; README.md; docs/CLOUDFLARE_DEV_SETUP.md; MQTT_SETUP.md; DATABASE_SETUP.md; remote-console-azure.md; this report |
| Private runtime configuration | Ignored root .env and the dedicated broker files outside the repository; backups retained privately |

## Not yet verified or completed

- Physical ESP32 MQTT/WSS connectivity and OTA installation: no board was flashed.
- Azure deployment: no Azure resource/connection credentials supplied; local PostgreSQL
  tests are not an Azure TLS/networking test.
- Public Cloudflare: connector is connected, but its active configuration is only
  http_status:503. dev.iot-prototype.net and api-dev.iot-prototype.net failed DNS
  checks and have no configured published routes. Confirm the active account domain
  and configure protected dashboard/API routes before publishing.
- Public unattended OTA still needs a separately authenticated device boundary.
  The Monitoring viewer token does not protect firmware builds or deployment writes.

Follow [MQTT setup](MQTT_SETUP.md), [database selection](DATABASE_SETUP.md), and
[Cloudflare setup](CLOUDFLARE_DEV_SETUP.md). An already-flashed ESP32 will retain old
credentials/URLs until a newly built image is installed.
