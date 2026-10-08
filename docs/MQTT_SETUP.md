# MQTT: what goes where?

Think of Mosquitto as a small post office. The ESP32 sends logs to it. Rust collects
the logs, and your dashboard asks Rust to display them. The browser never needs
the broker password.

| Used by | Settings in the root .env | Purpose |
| --- | --- | --- |
| Rust backend | MQTT_HOST, MQTT_USERNAME, MQTT_PASSWORD, MQTT_CA_FILE | Read device logs and TinyML messages |
| ESP32 firmware | DEVICE_ID, DEVICE_MQTT_URL, DEVICE_MQTT_USERNAME, DEVICE_MQTT_PASSWORD, DEVICE_MQTT_CA_FILE | Publish only this board's messages |
| You, in the dashboard | CONSOLE_VIEWER_TOKEN | Unlock Reporting and Remote Console once |

These are three different credentials. Do not put the backend password into a
board or use the viewer token as an MQTT password. A username such as
console-backend is simply an account you create in Mosquitto.

## Existing configured laptop: start here

From the project folder in PowerShell:

```powershell
.\scripts\start-mqtt.ps1
.\scripts\check-mqtt.ps1 -ProbeDevice -ProbeWebSocket
.\scripts\dev.ps1 -WithMqtt -AllowLanDevices
```

The start helper keeps existing accounts unchanged and reuses an already-running
matching broker. Do not run setup-mqtt every time: -ConfigureProject creates new
passwords, which requires rebuilding older firmware.

Open http://localhost:5173. Use CONSOLE_VIEWER_TOKEN from your private .env for
Monitoring access. Never paste it into chat or source code. Then choose a registered
device under Remote Console. An empty list means there is no registered board in
the active database; it is not a USB scan.

The probes send clearly labelled diagnostic messages. A passing probe proves the
broker path, not a flashed board or public Cloudflare connection. It does not
register a fake device or create a fake inference run.

## First-time setup on another Windows machine

Install Mosquitto (including its password tool), OpenSSL, and Python. Make your own
.env from .env.example and replace the machine-specific paths. Generate a long,
random CONSOLE_VIEWER_TOKEN and keep it private. Set the PC's LAN address here:

```powershell
.\scripts\setup-mqtt.ps1 -BrokerAddress 192.168.1.5 -MosquittoDir 'C:\Program Files\mosquitto' -ConfigureProject -EnableWebSockets -StartBroker
```

Replace 192.168.1.5 with the actual PC address reachable by the board.
-ConfigureProject generates separate strong backend/device passwords and writes
only the relevant root .env settings. Without it, the account tool prompts for
passwords instead. The script retains an existing CA and unrelated accounts, makes
private backups, and refuses occupied listener ports before making changes.

Private broker files live in %LOCALAPPDATA%\csv-mqtt, not in a firmware ZIP.
The dedicated config is mosquitto.dashboard.conf:

- TLS MQTT on 8883: username/password authentication and certificate verification.
- Optional WebSocket on 127.0.0.1:9001: local tunnel origin only, same authentication.
- Backend account subscribes to devices/+/logs and tinyml/v1/#.
- Each device account publishes only devices/<its-id>/logs and tinyml/v1/<its-id>/#.
- Anonymous access is disabled.

The helper does not open Windows Firewall, install a service, change an existing
Mosquitto service, or flash an ESP32. Allow 8883 only on your trusted LAN if a board
needs it; never expose the plaintext 9001 listener to the LAN/Internet.
For another install path, pass -MosquittoDir to start-mqtt and -MosquittoPub to
check-mqtt. The current laptop's default executable is F:\Mosquito\mosquitto.exe.

## Put those settings into the ESP32

Both dashboard-managed builds and flash-esp32.ps1 read the root .env.
They turn DEVICE_MQTT_CA_FILE into the certificate content for compilation. Keep
either *_FILE or *_PEM for each certificate, not both. Backend MQTT_* values stay
on the PC; only DEVICE_MQTT_* values belong in board firmware.

1. Confirm WIFI_SSID/WIFI_PASS, DEVICE_ID, and the device MQTT settings in .env.
2. Start Rust and note the firmware/server URL printed by the launcher.
3. Build without writing to the board:

```powershell
.\flash-esp32.ps1 -BuildOnly -ServerUrl 'http://YOUR_PC_LAN_IP:18080'
```

4. When ready, run the normal flash helper with the same -ServerUrl and your
   actual -Port; it asks before flashing. Alternatively deploy a successfully
   rebuilt managed image through an already-working OTA connection.
5. Confirm Wi-Fi connects, OTA registration succeeds, and MQTT logs arrive.
6. Your uploaded application can keep using log::info!, log::warn!, and log::error!.
   Do not install a second global logger.

Editing .env does **not** change an already-flashed binary. Changes to Wi-Fi,
passwords, certificates, or URLs need a rebuild and installation.
Use one identity per board. Do not flash the same credential-bearing image across
a fleet. Treat .bin, ELF and build artifacts as sensitive.

This console forwards Rust application logs, not every USB byte. ROM boot output,
ESP-IDF C logs, println!, and crashes before networking still need a USB monitor.

## Later: keep MQTT working when the PC changes networks

For direct LAN MQTT, use a stable local DNS name/DHCP reservation where practical.
If using a literal IP, a change requires updating the firmware URL AND the server
certificate's subject alternative name. -RenewCertificate can renew the leaf
certificate while keeping the CA; stop only the broker you own first.

For remote MQTT via Cloudflare:

1. Keep the backend on localhost:8883 with its private CA and backend account.
2. Publish mqtt.YOUR_ACTIVE_DOMAIN to http://127.0.0.1:9001 in your named tunnel.
3. In board settings, change DEVICE_MQTT_URL to wss://mqtt.YOUR_ACTIVE_DOMAIN/mqtt.
4. Unset the device private DEVICE_MQTT_CA_FILE/PEM; this public TLS endpoint uses
   the ESP-IDF public CA bundle. Keep broker username/password authentication.
5. Rebuild, install, and verify on an actual ESP32. The device must have correct
   time (SNTP), DNS and outbound HTTPS connectivity.

Do not route raw MQTT/TLS 8883 as an HTTP service. WSS is MQTT inside WebSocket
over TLS; the local plaintext hop stays inside the PC. The browser still connects
to Rust, not directly to this broker. Public WSS/ESP32 has not been hardware-verified
by the local probe. See [Cloudflare setup](CLOUDFLARE_DEV_SETUP.md).

## Recovery and changing accounts

Backups are in protected backup-* and credentials-backup-* folders under csv-mqtt.
Changing passwords requires updating the broker, matching .env settings, restarting
Rust, and rebuilding the board. Restore matching broker/.env backups together.
Never upload the CA private key, client private keys or backups in project archives.

After reboot, run start-mqtt or dev.ps1 -WithMqtt again. The helper starts a hidden
process with logs in csv-mqtt/logs; this dedicated broker is not installed as a
Windows service. To stop it, confirm the PID and executable/config match the
helper's output, then stop that exact process. Do not kill every mosquitto process:
an unrelated/default Windows service may also exist.
