# ESP32-S3 OTA provisioning and rollback

## Runtime and application ownership

The initially flashed program is the stable runtime in
`examples/esp32-rust-ota-client`. It owns the ESP-IDF event loop, modem, Wi-Fi,
OTA partition access, reporting, and rollback. An uploaded managed application is
a library compiled into that runtime, not a dynamically loaded binary.

The runtime acquires `Peripherals` once, reserves the modem for Wi-Fi, and passes
an application-safe bundle through `AppContext`. Applications expose free
`setup` and `main` functions and must not call `Peripherals::take()` again.

The current Wi-Fi manager supports configured credentials and reconnection. Its
ownership boundary is intentionally ready for later NVS credential management,
SoftAP provisioning, or a captive portal without changing application code.

## Partition layout

OTA requires `otadata` plus two application slots. The example project includes a conservative 4 MiB layout:

```text
nvs      data/nvs
otadata  data/ota
phy_init data/phy
ota_0    app/ota_0
ota_1    app/ota_1
```

The running application may be in either slot. ESP-IDF chooses the inactive slot; application code must not assume that `ota_0` is always active. Do not overwrite an existing product partition table without reconciling its flash size, NVS/data partitions, encryption flags, and application size.

The partition table and bootloader are installed during initial serial provisioning. Dashboard OTA artifacts are application-only images and must be written at offset zero of the selected inactive OTA partition through ESP-IDF's OTA API—not flashed at a hardcoded physical address.

## Rollback lifecycle

Enable `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y` before the initial bootloader build/flash.

1. The old valid app downloads and verifies the new image into the inactive slot.
2. `EspOtaUpdate::complete()` validates the image and selects that slot for next boot.
3. After reboot, the runtime calls the new app's `setup` function for required peripheral, configuration, and storage checks.
4. Only after those checks pass, call `EspOta::mark_running_slot_valid()`.
5. If checks fail, call `mark_running_slot_invalid_and_reboot()` or reset before validation so the rollback-enabled bootloader restores the prior valid slot.
6. Register/report the new `current_version` after validation. The server then clears the desired update.

Test rollback on physical hardware before relying on it. A partition table alone does not provide rollback; the bootloader configuration and application validation call are both required.

## Integrity and authenticity

The dashboard provides SHA-256 metadata. The device computes SHA-256 while streaming and rejects a mismatch before activation. This protects against accidental corruption but not a malicious replacement by someone controlling the server or LAN.

For production, use HTTPS where practical and ESP-IDF Secure Boot / signed application verification with protected device keys. Do not invent a shared-secret signature scheme.

## Network setup

- Bind the server to `0.0.0.0:7000`.
- Set `OTA_PUBLIC_BASE_URL=http://<PC-LAN-IP>:7000`.
- Permit inbound TCP 7000 in the PC firewall for the trusted LAN.
- Ensure client isolation is disabled or otherwise allow the ESP32 to reach the PC.
- Treat plain HTTP as local-development transport only.
