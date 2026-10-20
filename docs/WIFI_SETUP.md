# Wi-Fi setup on your ESP32-S3

You no longer need to rebuild an application just to change Wi-Fi. The board hosts
its own setup website: connect your phone to it, enter your router's Wi-Fi details,
and the board remembers them.

This feature must first be installed in firmware. Restarting the dashboard does
not add it to an older image.

## First installation

Build the new runtime without embedding Wi-Fi credentials:

~~~powershell
.\flash-esp32.ps1 -BuildOnly -ProvisionWifi -Version 3.1.0
~~~

This does **not** flash your board. To build and flash with the normal confirmation
prompt, omit -BuildOnly and specify the board's actual COM port:

~~~powershell
.\flash-esp32.ps1 -ProvisionWifi -Port COM16 -Version 3.1.0
~~~

COM16 is an example; check your own port. Review the partition layout and flash
confirmation. Never erase the whole flash just to change Wi-Fi.

If OTA already works, rebuild your managed application using the updated runtime
and deploy it normally instead. Restart the Rust backend before building so it
reads the new build logic. Your uploaded application does not need setup code:
the shared runtime supplies it.

**Important:** -ProvisionWifi omits the image's Wi-Fi fallback. It does not erase
settings already saved on the board. A board with working saved settings
reconnects to them normally.

## Connect with your phone

1. Open USB serial at boot and record the **Recovery hotspot** name and its random
   **Recovery Wi-Fi password**. These remain the same across restarts and ordinary
   OTA updates. Keep the password private; it grants permission to change Wi-Fi.
2. With no saved Wi-Fi, look for **ESP32-Setup-XXXXXX**. With unavailable saved
   Wi-Fi, wait approximately **60 seconds** for it to appear.
3. Join using its recovery password. Your phone may say **No internet**: choose
   **Stay connected**. This is a setup network, not an internet hotspot.
4. A setup page may appear automatically. Otherwise open **http://192.168.4.1/**
   in your browser. Use HTTP, not HTTPS. Automatic captive-page opening depends
   on the phone; the manual address is always the fallback.
5. Select a **2.4 GHz** network or type a hidden network's exact name. Enter its
   password. For a genuinely open network, check **This network has no password**.
6. Press **Connect & save**. The status page shows the attempt and IP once
   connected. Successful means Wi-Fi association plus DHCP; it does **not**
   claim that internet, OTA or MQTT is reachable.
7. The hotspot remains available for about **20 seconds** after connection, then
   closes. Rejoin your usual Wi-Fi to open the dashboard.

Changing Wi-Fi channels during setup can briefly disconnect your phone. Rejoin the
same setup hotspot and open the address again. A closed browser connection alone
does not prove that the password was accepted: check status or USB serial.

## When Wi-Fi drops later

- Short interruptions trigger automatic retries without clearing credentials.
- After about 60 seconds offline, the protected setup hotspot opens again.
- The board keeps trying the saved network. If the router returns, it reconnects
  and closes the hotspot automatically.
- You can submit different credentials while the previous network is unavailable.
- Failed credentials do not replace the last successfully saved settings.
- **Retry saved network** retries without asking you to retype its password.

Connection attempts have a 25-second limit and a 15-second retry pause. Manual
submissions take priority over background retries. Scanning, radio congestion
and DHCP can slightly affect timings.

This version does not reserve an application GPIO for a physical setup button.
To change networks while the current network still works, make it temporarily
unavailable and use automatic recovery. A board-specific button can be added
later without stealing a pin from uploaded applications.

## What gets saved?

The board's **NVS** partition stores a versioned Wi-Fi record and the recovery
password. New credentials are tested before committing. Two alternating record
keys preserve the previous valid record if power is lost while saving.

Neither the Wi-Fi password nor setup password is returned by the setup API,
sent to SQLite, or forwarded through MQTT logs. Recovery details are printed
only over USB serial.

Startup precedence:

1. Valid saved board settings.
2. Optional WIFI_SSID / WIFI_PASS compiled into the image for first-boot migration.
3. Setup hotspot when neither is available.

Once fallback credentials connect successfully, they are saved too. A later image
containing another SSID does not overwrite saved settings. Those two build-time
settings can now be omitted from .env.

Normal application-only OTA changes ota_0 / ota_1, not NVS. Keep the partition
layout and settings schema compatible across versions. Whole-flash erasure,
NVS erasure, incompatible repartitioning, or older firmware that ignores this
manager can change that behaviour.

**NVS is not encrypted at rest by this patch.** Provision ESP-IDF NVS encryption
for production according to your hardware/security plan. No eFuses or irreversible
flash-encryption settings are changed automatically.

## IP addresses, MQTT, and the database

The router assigns the board's IP through DHCP. Existing registration/heartbeat
requests update its address in SQLite when the board reaches the Rust server.

The page changes Wi-Fi, **not OTA/MQTT endpoints or credentials**. Use stable,
internet-reachable service hostnames. A private broker at 10.x.x.x is still
unreachable from another network even when Wi-Fi setup succeeds. Wi-Fi and MQTT
passwords are different credentials.

The page is local to the board: it does not depend on Flask, Rust, SQLite,
Cloudflare, or internet access. Uploaded apps still use the same network handle
and function-based API. An MQTT initialization failure leaves USB logging and
local Wi-Fi setup available; it does not claim that remote logging is working.

## Source map

Files below are relative to examples/esp32-rust-ota-client/crates/wifi-manager:

| File | Responsibility |
|---|---|
| src/manager.rs | Independent Wi-Fi task, retry, AP fallback and validation |
| src/storage.rs | NVS records and per-board recovery password |
| src/settings.rs | Credential validation, retry policy and bounded DNS parser |
| src/portal.rs | HTTP API, captive DNS, interface/origin/request checks |
| web/index.html | Network selection and password form |
| web/status.html | Progress, errors, retry, IP and success feedback |
| web/setup.js, web/setup.css | Offline browser behaviour and mobile layout |

Portal API, available only through the setup hotspot:

| Method | Path | Purpose |
|---|---|---|
| GET | /api/status | Status, networks and session nonce; no passwords |
| POST | /api/scan | Refresh visible networks |
| POST | /api/connect | Test JSON ssid/password; save after connection |
| POST | /api/retry | Retry the saved network |

POST requests require JSON, a current X-Setup-Token header, a bounded body and the
portal's origin. Station-side requests are rejected using socket-address checks.
Pages have no external fonts/scripts, browser password storage or CDN dependency.
The portal closes when connected; do not expose it through a public tunnel.

This version supports open and WPA2-personal-compatible networks, not enterprise
Wi-Fi or hotel/campus web-login networks. Avoid a router using the same
192.168.4.0/24 subnet as the setup hotspot.

## Verification

Host tests (PowerShell):

~~~powershell
$env:CARGO_TARGET_DIR = "$PWD/data/wifi-tests"
cargo +stable-x86_64-pc-windows-msvc test --manifest-path examples/esp32-rust-ota-client/Cargo.toml -p wifi-manager --target x86_64-pc-windows-msvc --offline
$env:PLAYWRIGHT_CHANNEL = "msedge"
node scripts/test-wifi-portal.cjs
~~~

The browser test uses mocked APIs, not an ESP32. It uses the existing Playwright
installation in data/browser-check/node_modules, or PLAYWRIGHT_MODULE if set.

Before relying on a remote board, verify on real hardware:

- First provisioning, manual address, and phone captive detection.
- Wrong password followed by a correct one, including special characters.
- Router loss, fallback hotspot, and router restoration.
- Successful setup, reboot and application-only OTA with different build-time
  Wi-Fi values: saved board credentials must win.
- Power loss while saving: the previous valid record remains recoverable.
- Requests through the normal LAN cannot access setup endpoints.
- Wi-Fi connected but internet/MQTT/OTA unavailable is not labelled fully online.

Radio, power-loss and hardware tests cannot be established by a successful desktop
test or compile alone.
