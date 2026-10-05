# Local MQTT example for one ESP32-S3 (Windows)

This is a **copyable development example**, not a change to the running
Mosquitto service. Your existing service can keep using port `1883`. This
example starts a second Mosquitto process on TLS port `8883`, because both the
current ESP32 runtime and Rust backend require MQTT over TLS. Keep the broker
terminal open while testing. SQLite remains the database: leave `DATABASE_URL`
unset and Rust uses `data/ota.db`.

## Which setting goes where?

| Place | Purpose | Settings |
| --- | --- | --- |
| Mosquitto | Carries messages | TLS certificate, password file, topic ACL |
| Root `.env` on the PC | Rust subscriber | `MQTT_HOST`, `MQTT_PORT`, `MQTT_USERNAME`, `MQTT_PASSWORD`, `MQTT_CA_FILE` |
| Root `.env` before a dashboard firmware build | Values compiled into the ESP32 | `DEVICE_ID`, `DEVICE_MQTT_URL`, `DEVICE_MQTT_USERNAME`, `DEVICE_MQTT_PASSWORD`, `DEVICE_MQTT_CA_PEM` |
| Monitoring access in the browser | View reports/logs | `CONSOLE_VIEWER_TOKEN`; **never** an ESP32 MQTT password |

The MQTT broker is **not** the OTA server. The ESP32 sends application logs to
the broker; the Rust server reads them and forwards them to the dashboard. The
board must already be able to reach the PC on the same LAN. Cloudflare's HTTP
Tunnel does not forward this raw MQTT listener. A far-away board needs a
separate reachable TLS MQTT broker later.

## 1. Prepare a private folder and certificates

The example uses the installed `F:\Mosquito` tools and an `openssl` on your
PowerShell PATH. Use a **current PC LAN IPv4 address**, not the board's IP or
`localhost`. Find it with `ipconfig`; enter it when prompted. If it changes,
the example certificate will need to be regenerated for the new address.

Run in PowerShell from the repository root:

```powershell
$mqttDir = Join-Path $env:LOCALAPPDATA 'csv-mqtt'
$lanIp = Read-Host 'Current PC LAN IPv4 address'
New-Item -ItemType Directory -Path $mqttDir -Force | Out-Null
openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 365 -subj '/CN=CSV MQTT Local CA' -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign,cRLSign' -keyout "$mqttDir\ca.key" -out "$mqttDir\ca.crt"
openssl req -newkey rsa:2048 -nodes -sha256 -subj "/CN=$lanIp" -keyout "$mqttDir\server.key" -out "$mqttDir\server.csr"
@("subjectAltName=IP:$lanIp", 'extendedKeyUsage=serverAuth', 'basicConstraints=CA:FALSE') | Set-Content -LiteralPath "$mqttDir\server.ext"
openssl x509 -req -in "$mqttDir\server.csr" -CA "$mqttDir\ca.crt" -CAkey "$mqttDir\ca.key" -CAcreateserial -out "$mqttDir\server.crt" -days 365 -sha256 -extfile "$mqttDir\server.ext"
```

Stop if any command reports an error. Keep `ca.key` and `server.key` private;
never commit or upload them. Only `ca.crt` (the public CA certificate) is
shared with clients. This is a local development CA, not a production PKI.

## 2. Create separate broker accounts

The commands below prompt for passwords. Choose **different** passwords and
save them privately; do not paste them into chat or use the `-b` command-line
password option. The first command creates a new password file: **do not rerun
`-c` on an existing file**, because it replaces that file.

```powershell
& 'F:\Mosquito\mosquitto_passwd.exe' -c "$mqttDir\passwords" console-backend
& 'F:\Mosquito\mosquitto_passwd.exe' "$mqttDir\passwords" ESP32-S3-001
```

The ACL example gives the backend read-only access and gives this board write
access only to its own log and TinyML topics. Make a **different account and
ACL topic** for every other board.

## 3. Create and start the separate TLS broker

Copy the sample ACL and replace the path placeholder in the sample config:

```powershell
Copy-Item .\examples\local-mqtt\acl.example "$mqttDir\acl"
$pathForConfig = $mqttDir.Replace('\', '/')
(Get-Content .\examples\local-mqtt\mosquitto.conf.example -Raw).Replace('MQTT_DATA_DIR', $pathForConfig) | Set-Content -LiteralPath "$mqttDir\mosquitto.conf"
& 'F:\Mosquito\mosquitto.exe' --test-config -c "$mqttDir\mosquitto.conf"
& 'F:\Mosquito\mosquitto.exe' -c "$mqttDir\mosquitto.conf" -v
```

The last command occupies the terminal until you press Ctrl+C. The existing
`1883` Windows service is not stopped or reconfigured. In another PowerShell
terminal, check `Test-NetConnection 127.0.0.1 -Port 8883`. Allow inbound `8883`
only on a trusted **Private** Windows network; do not disable the firewall or
forward the port on your router. Campus/hotspot client isolation may still
prevent the ESP32 from reaching your PC.

## 4. Map credentials into your private root `.env`

Do **not** copy the values below into `.env.example`. In the ignored `.env`,
leave `DATABASE_URL` unset and add the following with your own passwords, IP,
and absolute CA path (ordinary `KEY=value` lines, not Markdown links):

```dotenv
CONSOLE_VIEWER_TOKEN=YOUR_OWN_RANDOM_TOKEN_AT_LEAST_32_CHARACTERS
MQTT_HOST=127.0.0.1
MQTT_PORT=8883
MQTT_USERNAME=console-backend
MQTT_PASSWORD=BACKEND_PASSWORD_FROM_STEP_2
MQTT_CA_FILE=C:/Users/YOUR_USER/AppData/Local/csv-mqtt/ca.crt

DEVICE_ID=ESP32-S3-001
DEVICE_MQTT_URL=mqtts://YOUR_PC_LAN_IP:8883
DEVICE_MQTT_USERNAME=ESP32-S3-001
DEVICE_MQTT_PASSWORD=DEVICE_PASSWORD_FROM_STEP_2
```

The ESP32 also needs the **contents** of `ca.crt`, not its path. An easy way to
provide it without editing a multiline `.env` value is to start the OTA server
from the same PowerShell session after setting:

```powershell
$env:DEVICE_MQTT_CA_PEM = Get-Content -LiteralPath "$mqttDir\ca.crt" -Raw
```

The dashboard's Rust build process inherits that variable and compiles the CA
into the firmware. If you prefer to store it in `.env`, use a quoted multiline
PEM value. Never use `ca.key` or `server.key` as `DEVICE_MQTT_CA_PEM`.

After starting the broker, restart Rust so it reads the new `.env`, build a new
managed firmware, and flash or OTA-deploy it. Already-flashed firmware does not
receive new MQTT settings automatically. Open **Monitoring → Remote Console**,
enter `CONSOLE_VIEWER_TOKEN`, and choose the registered board. This shows Rust
application logs, not raw USB/ROM output.

For a direct `flash-esp32.ps1` build, that helper does not read root `.env`:
set all `DEVICE_MQTT_*` variables in its PowerShell session first. Consult
[`docs/remote-console-azure.md`](../../docs/remote-console-azure.md) for the
existing remote-console behavior and security limitations.
