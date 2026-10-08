# Cloudflare development setup (Windows)

Start locally first. Cloudflare gives your laptop stable public hostnames; it does
not replace Flask, Rust, the database, or Mosquitto. The laptop must stay awake.

The ordinary run-dev.bat/run-dev.sh launchers still use Flask :5000 and Rust :7000.
The optional development profile below uses :5173 and :18080.

## 1. Start the local services

From the repository root in PowerShell:

```powershell
.\scripts\dev.ps1 -WithMqtt -AllowLanDevices
```

This assumes Python dependencies and the MQTT setup are already installed.
For a new machine, follow [MQTT setup](MQTT_SETUP.md) first. Python uses
python/.venv; install python/requirements.txt there. Host Cargo builds the API;
the ESP toolchain is only needed to build board firmware.

Open http://localhost:5173. Keep the launcher terminal open. Ctrl+C stops only the
Flask and Rust processes it created. Their output is in data/dev-profile/<run-id>/.
Mosquitto and the installed cloudflared service are managed separately.

With -AllowLanDevices, Rust listens on LAN interfaces and the launcher selects the
current PC address with port 18080 for firmware downloads and new managed builds.
It does not edit .env. If two networks are active, pass -LanAddress with the address
reachable by the ESP32. Restrict Windows Firewall to trusted networks.

Already-flashed firmware keeps its old server address. For example, firmware built
for port 7000 will not magically start using 18080. Keep the API on its existing port
with -ApiPort 7000, or rebuild the firmware with the printed server URL.
Without -AllowLanDevices, this is a browser-only loopback profile.

Useful checks:

```powershell
.\scripts\dev.ps1 -CheckOnly -AllowLanDevices
.\scripts\check-dev.ps1 -LocalOnly
.\scripts\check-mqtt.ps1 -ProbeDevice -ProbeWebSocket
```

Do not run a second OTA server at the same time. The launcher refuses occupied ports
and running executables instead of killing them. Use a different -ApiPort only after
stopping the old project launcher. Changing the port also requires changing the
Cloudflare API origin below.

## 2. Protect the dashboard and API before publishing

The full CSV and OTA services do not yet have general administrator authentication.
OTA builds execute uploaded Rust, including build.rs. **Never expose the full API
to anonymous Internet visitors.** The Monitoring viewer token protects reporting
and logs only; it does not protect builds, firmware downloads, or device writes.

Create [Cloudflare Access applications and allow policies](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/self-hosted-public-app/)
for BOTH the dashboard hostname and the API hostname. Allow only your trusted
operator identities. Do not add a broad bypass policy. Keep origin ports private
when using the tunnel alone. This repository does not create account policies.

Browser requests include credentials, and Rust allows them only for configured
origins. Authenticate to the API hostname as well as the dashboard hostname.
For cross-origin requests, configure Access to respond to OPTIONS preflight for
the exact dashboard origin, with credentials enabled, methods GET/HEAD/POST/DELETE/OPTIONS
and headers Authorization/Content-Type/Accept. Actual requests must remain protected.
Responding at Access avoids sending an unauthenticated preflight to the example's
origin JWT validator. See [Access CORS guidance](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/cors/).
A redirect to login is not an API response.

An ESP32 cannot complete an interactive Access login. A separate authenticated
device route/policy and firmware integration are still required before public OTA
can be called ready. Do not bypass Access on the entire API just to connect a board.
Keep OTA on trusted LAN until that boundary is implemented and tested.

## 3. Configure your existing named tunnel

Use a domain that is actually active in your Cloudflare account. These are the
project's requested example names, not proof that they exist:

| Public hostname | Service URL on this PC | Protection |
| --- | --- | --- |
| dev.iot-prototype.net | http://127.0.0.1:5173 | Cloudflare Access |
| api-dev.iot-prototype.net | http://127.0.0.1:18080 | Cloudflare Access |
| mqtt.iot-prototype.net | http://127.0.0.1:9001 | MQTT authentication and per-device ACL |

Add these **Published application routes** to the existing tunnel in Cloudflare.
Use your active domain if different, and pass matching launcher settings:

```powershell
.\scripts\dev.ps1 -WithMqtt -PublicFrontendHost dev.YOUR_DOMAIN -PublicApiUrl https://api-dev.YOUR_DOMAIN
```

The MQTT route carries MQTT over WebSocket; it is not a raw TCP 8883 route.
The local 9001 listener must stay loopback-only. Do not put an interactive Access
login on this hostname: the firmware speaks MQTT, not browser login. Mosquitto
still authenticates every MQTT connection and restricts each device's topics.
See [MQTT setup](MQTT_SETUP.md) before switching board configuration to WSS.

A connected connector is only one part of setup. It also needs hostname routes and
DNS records. The [Cloudflare tunnel guide](https://developers.cloudflare.com/tunnel/get-started/)
explains that mapping. Do not point an A record at the laptop's private IP.

A remotely managed Windows connector loads its routes from Cloudflare, not from
cloudflared/config.example.yml. Editing the example cannot configure your account.
The example is for an alternative locally managed tunnel, and requires your UUID,
private credentials path, Access team name, and Access application audience IDs.
Its Access placeholders intentionally do not authorize visitors.

To inspect the installed connector:

```powershell
Get-Service cloudflared
.\scripts\check-dev.ps1 -TunnelOnly
```

If the service is missing, install cloudflared from [official downloads](https://developers.cloudflare.com/tunnel/downloads/)
and use the Windows connector command from your tunnel dashboard in an elevated
terminal. Do not install another service if one is already running. Keep the
connector token out of source, screenshots, and chat; rotate previously shared
tokens using [Cloudflare's token guidance](https://developers.cloudflare.com/tunnel/reference/tunnel-tokens/).
A connector token is not an account-management API token.

## 4. Verify each layer

1. Local dashboard and /app-config.js respond on 5173.
2. Local API /health responds on 18080.
3. cloudflared service is running and connector readiness succeeds.
4. Your actual public names resolve in DNS.
5. Each published hostname points to its correct local origin.
6. An authenticated browser opens the dashboard and API.
7. Reporting and Remote Console use wss:// at the public API origin.
8. If MQTT is published, test a real board against the WSS hostname separately.

```powershell
.\scripts\check-dev.ps1
```

For a different domain, supply -PublicFrontendUrl and -PublicApiUrl. Protected
public HTTP checks may return login/authorization responses until authenticated;
the diagnostic does not bypass Access. A local WebSocket MQTT probe is not proof
of end-to-end public WSS or physical board connectivity.

Flask serves /app-config.js with a browser-safe API URL. CSV stays on the dashboard
origin, while OTA/Monitoring use the configured API origin. No secrets belong in
that response. Plain HTTPS pages must never fetch http://localhost API URLs.

## OTA and database settings

- Leave DB_BACKEND=sqlite for local development. Azure PostgreSQL is selectable,
  not simultaneously synchronized. See [database setup](DATABASE_SETUP.md).
- Do not publish any database port through the tunnel.
- -PublicOta changes the server's advertised firmware URL to the public API only
  after a DNS check. It does NOT configure authentication or guarantee ESP access.
  Use it only after device authorization and HTTPS hardware checks are complete.
- Firmware now configures ESP-IDF's CA bundle for public HTTPS; the WSS path is
  implemented but still needs board testing. Never disable certificate checking.
- Cloudflare hostnames remove dependence on the PC's changing LAN address once
  configured. Raw LAN MQTT and old firmware URLs still depend on that address.

## Common failures

| Symptom | What to check |
| --- | --- |
| DNS name does not exist | Domain registration, active Cloudflare zone, published hostname/DNS record. |
| Connector ready but no website | Tunnel ingress routes may still be empty or catch-all only. |
| 502 | Origin service is stopped or the published service port is wrong. |
| Access login page instead of JSON | Authenticate to the API origin; check CORS/preflight policy. |
| Windows access denied rebuilding .exe | Stop the previous project launcher; it may still own the binary. |
| No devices/logs | Board must register in the active database AND have working MQTT firmware. |
| Download connects to old address | Restart Rust with the intended profile; inspect its advertised URL. |
| Broker TLS fails after moving Wi-Fi | Private certificate SAN and firmware broker URL must match the new address. |
| Tunnel fails on campus Wi-Fi | Check permitted outbound tunnel traffic with the network administrator. |

Stop the application with Ctrl+C. Stopping/removing Cloudflare must not delete
databases or firmware artifacts. Back up secrets privately and never commit .env,
private keys, tunnel credentials, or real database connection strings.
