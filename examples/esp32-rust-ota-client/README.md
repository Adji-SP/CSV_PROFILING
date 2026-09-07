# ESP32-S3 managed firmware runtime

This project is the stable, initially USB-flashed ESP32 program. It owns ESP-IDF
startup, Wi-Fi, OTA download, status reporting, and rollback. Product behavior is
provided by the `device-app` Cargo dependency.

```text
src/main.rs
├── crates/application-api     shared AppContext and application contract
├── crates/wifi-manager        single owner of modem/Wi-Fi/reconnection
├── crates/ota-runtime         registration, polling, streaming OTA, SHA-256
└── applications/default-app   application used for initial USB provisioning
```

Every managed dashboard build copies this runtime to an isolated build workspace
and replaces only the `device-app` dependency with the uploaded application. The
result is still one statically linked ESP-IDF application image.

## Configure, build, and flash initially

Use the ESP Rust environment and build-time values. `DEVICE_ID` is optional; when
absent the runtime derives a stable ID from the station MAC address.

```powershell
$env:WIFI_SSID="your-ssid"
$env:WIFI_PASS="your-password"
$env:OTA_SERVER_URL="http://192.168.1.5:7000"
$env:DEVICE_NAME="Sensor node"
cargo +esp run --release --target xtensa-esp32s3-espidf
```

The configured Cargo runner invokes `espflash flash --monitor` with
`partitions.csv`. Review that partition table against the board's actual flash
layout before running the command. Do not use `localhost` in `OTA_SERVER_URL`.

## Application lifecycle

The runtime calls the selected application's plain `setup` and `main` functions.
On the first boot of an unverified OTA image, a failed `setup` rejects that slot
and requests rollback. Successful setup marks the image valid before the runtime
starts the OTA supervisor and calls the long-running application `main` function.
Put fallible peripheral construction and startup checks in `setup`.

The Wi-Fi manager owns the modem, event loop, and NVS-backed ESP Wi-Fi service.
Applications receive a read-only network state handle and a bundle of available
ESP32-S3 application peripherals through `AppContext`. They must not call
`Peripherals::take()` themselves.

## Managed upload examples

- `../uploaded-firmware-basic` is the minimal ZIP-ready application.
- `../uploaded-firmware-with-libs` demonstrates `heapless`, `serde`, and
  `serde_json` dependencies.

Keep `Cargo.toml`, `ota-app.toml`, and `src/main.rs` at the ZIP root. The server patches
the declared `firmware-app-api` placeholder only in its generated build copy.

## Partitions and rollback

The supplied 4 MiB example has `nvs`, `otadata`, `phy_init`, `ota_0`, and
`ota_1`. ESP-IDF selects the inactive OTA slot; the client streams in 4 KiB chunks
and verifies SHA-256 before activation.

`sdkconfig.defaults` enables application rollback. Test rollback on physical
hardware. SHA-256 provides transfer integrity, not publisher authentication;
production devices should add HTTPS, Secure Boot, and authenticated image signing.
