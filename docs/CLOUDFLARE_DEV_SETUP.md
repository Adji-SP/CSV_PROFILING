# Cloudflare Tunnel development setup (Windows)

This is an **opt-in development profile**. The original `run-dev.bat` and
`run-dev.sh` still use Flask `:5000` and Rust `:7000`. You do not need Cloudflare
to work locally.

## What this repository actually runs

There is no Vite, npm frontend, Docker Compose, or Cargo workspace at the
repository root. Flask serves the plain HTML/CSS/JavaScript frontend **and**
the Pandas CSV API. Axum serves OTA, Reporting, Remote Console and WebSockets.
The Axum service stores records in SQLite by default or PostgreSQL when
`DATABASE_URL` is set. Its MQTT subscriber uses TLS; it does not currently
connect to a plaintext Mosquitto listener on `1883`.

For this profile, `scripts/dev.ps1` changes **process-local** listener settings:

| Public hostname | Local origin | Actual service |
| --- | --- | --- |
| `https://dev.iot-prototype.net` | `http://localhost:5173` | Flask dashboard **and CSV API** |
| `https://api-dev.iot-prototype.net` | `http://localhost:8080` | Rust Axum API and WebSockets |

This uses the existing Flask frontend server on `5173`, **not** Vite. Cloudflare
connects outbound from the laptop; no static public IP, A record to the laptop,
router forwarding, or inbound public firewall rule is needed. Cloudflare
terminates public HTTPS; both local origins remain HTTP. [Cloudflare's published
application routing guide](https://developers.cloudflare.com/tunnel/concepts/routing/)
describes the hostname-to-local-service mapping.

## Before you publish: important access warning

The current Flask CSV upload/reset routes and Rust OTA upload/build/deploy/delete
routes do **not** have general administrator authentication. The Monitoring
viewer token protects only Reporting and Remote Console. A Tunnel is a transport,
not an access policy: without protection, publishing either hostname exposes
these application routes to Internet visitors. **Do not make the routes public
to arbitrary visitors.** Put [Cloudflare Access](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/self-hosted-public-app/)
in front of the development dashboard and API for human-only use, or add
application-level admin/device authentication before allowing unattended
Internet clients. Protecting `api-dev` with interactive Access may block an
ESP32 or other non-browser client; design a separate device policy before
deploying remote OTA. This guide does not silently enable Access or pretend the
current API is hardened for public production use.

## 1. Start the application locally

Prerequisites: Python with `python/.venv` and `python/requirements.txt`, a host
Rust toolchain and `cargo`, and the existing ESP Rust toolchain only if you plan
to compile firmware. PostgreSQL is optional; without `DATABASE_URL`, Rust uses
`data/ota.db`. Keep PostgreSQL bound privately, for example `127.0.0.1:5432`.
Do not add a database Tunnel route.

From the repository root in PowerShell:

```powershell
python -m venv python\.venv
python\.venv\Scripts\python.exe -m pip install -r python\requirements.txt
.\scripts\dev.ps1 -CheckOnly
.\scripts\dev.ps1
```

If the virtual environment already exists, only install any missing dependency.
`dev.ps1` runs Flask and Rust as jobs in one
PowerShell terminal, forwards their output, and stops them on Ctrl+C. Keep that
terminal open. It does **not** install or start `cloudflared`. The two local
listeners bind to `127.0.0.1` by default. If a trusted LAN ESP32 must call Rust
directly, use `-AllowLanDevices`; this binds Rust to `0.0.0.0:8080`, so restrict
Windows Firewall to your trusted network. Set `OTA_PUBLIC_BASE_URL` in your
ignored `.env` to a reachable LAN URL for that case. Do not use localhost in
URLs sent to an ESP32.

If `-CheckOnly` reports that `8080` is occupied, inspect the owner with
`netstat -ano -p tcp | Select-String ':8080'` and `Get-Process -Id <PID>`.
Do not terminate an unrelated application just to use this profile. You can
instead run `.\scripts\dev.ps1 -ApiPort 18080` and
`.\scripts\check-dev.ps1 -LocalOnly -ApiPort 18080`, then change the Tunnel's
`api-dev` Service URL to `http://localhost:18080`. The public API hostname stays
the same. Choose a port actually available on your machine.

The launcher sets these environment variables only in its own process and jobs:
`CSV_BIND_ADDRESS=127.0.0.1`, `CSV_PORT=5173`, `OTA_BIND_ADDRESS=127.0.0.1`,
`OTA_PORT=8080` (or `-ApiPort`), and explicit `OTA_ALLOWED_ORIGINS` for local `5173`, public
`dev.iot-prototype.net`, and legacy local `5000`. Override the latter in your
PowerShell environment *before* starting the script if more origins are needed.
The original scripts keep their old ports and behavior.

Test local services before doing anything in Cloudflare:

```powershell
Invoke-WebRequest http://localhost:5173/ -UseBasicParsing
Invoke-WebRequest http://localhost:5173/app-config.js -UseBasicParsing
Invoke-RestMethod http://localhost:8080/health
.\scripts\check-dev.ps1 -LocalOnly
```

The CSV API stays on the dashboard's own origin, so CSV requests do not need
cross-origin permission. Flask's `/app-config.js` emits **only** a browser-safe
OTA API URL, never secrets. Locally it selects `http://localhost:8080`; on the
configured public frontend hostname it selects
`https://api-dev.iot-prototype.net`. The OTA and Reporting WebSockets derive
`ws://` or `wss://` from that same URL, so HTTPS pages use `wss://`.

If you launch Flask/Rust manually instead of using `dev.ps1`, set the equivalent
environment variables and include both allowed origins. Put private Rust values
in the ignored `.env`; Rust reads it with `dotenvy`. Flask reads its browser URL
settings from its process environment (which `dev.ps1` supplies), not `.env`.
The Rust-specific Windows
`OTA_CARGO_TARGET_DIR` short-path requirement remains in effect for firmware
builds. `RUST_LOG=info` is supported. Do not log or paste tokens, passwords,
database URLs or MQTT credentials.

## 2. Install `cloudflared` on Windows

First check the package ID and version on your machine:

```powershell
winget show --id Cloudflare.cloudflared -e
winget install --id Cloudflare.cloudflared -e
cloudflared --version
```

The [Microsoft WinGet manifest](https://github.com/microsoft/winget-pkgs/tree/master/manifests/c/Cloudflare/cloudflared)
uses this package ID. If `winget` offers an old release or is unavailable,
download the current Windows MSI from [Cloudflare's official downloads
page](https://developers.cloudflare.com/tunnel/downloads/) instead. Open a new
terminal after installing so PATH refreshes. `cloudflared` is not installed by
this repository; Windows versions require you to maintain updates.

## 3. Recommended: Dashboard-managed named Tunnel

Use a Cloudflare account with `iot-prototype.net` active in its DNS zone. In
Cloudflare Dashboard, go to **Networking → Tunnels → Create Tunnel**. Choose a
remotely managed Cloudflared tunnel named `csv-profiling-dev`. Add two
**Published application** routes under that tunnel:

1. Hostname `dev.iot-prototype.net` → Service URL `http://localhost:5173`.
2. Hostname `api-dev.iot-prototype.net` → Service URL `http://localhost:8080`.

Use the dashboard's **Windows connector** command on this laptop. A
remotely-managed tunnel uses its account-specific token; `cloudflared tunnel
login` and a repository `config.yml` are **not** needed. Cloudflare normally
creates the matching CNAME association when you add the published hostname in
a full Cloudflare DNS setup. If the name already has an A/AAAA/CNAME record,
resolve that conflict in the dashboard; never point an A record at your laptop.
For partial DNS setups, Cloudflare documents additional CNAME work at the
authoritative DNS provider. [Official named Tunnel setup](https://developers.cloudflare.com/tunnel/get-started/)
and [DNS behavior](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/self-hosted-public-app/).

To run the connector as a Windows service, use an **elevated Command Prompt**
and the exact installation command supplied by your tunnel's dashboard, in this
shape:

```text
cloudflared.exe service install <YOUR_TUNNEL_TOKEN>
```

Replace the placeholder privately; never save the real token in this repo,
`.env`, screenshots, tickets, or documentation. Anyone with the token can run
the tunnel. If leaked, rotate it in Cloudflare. The token is a connector secret,
**not** the Monitoring viewer token. [Cloudflare's Windows service command](https://developers.cloudflare.com/tunnel/get-started/)
and [token guidance](https://developers.cloudflare.com/tunnel/reference/tunnel-tokens/).

In an elevated PowerShell, manage the installed service:

```powershell
Get-Service cloudflared
Start-Service cloudflared
Stop-Service cloudflared
Restart-Service cloudflared
sc.exe query cloudflared
```

Use `cloudflared.exe service uninstall` from an elevated terminal to remove the
service when no longer needed. Uninstalling the Windows service does not remove
the Tunnel or public hostname routes from your Cloudflare account; delete those
separately in the dashboard when finished. A service reconnects when Windows
restarts or the network/public IP changes **while the laptop has Internet**;
the Flask/Rust `dev.ps1` jobs do not auto-start after reboot. Do not start a
second `cloudflared` service for the same machine; add routes to the existing
named tunnel instead.

## 4. Alternative: locally managed named Tunnel

Choose this only if you need to own the route configuration file. In a regular
terminal, use `cloudflared tunnel login`, select your Cloudflare zone, then:

```powershell
cloudflared tunnel create csv-profiling-dev
cloudflared tunnel list
```

Cloudflare prints a UUID and writes a **secret** UUID `.json` file to your
user's `.cloudflared` directory. Copy `cloudflared/config.example.yml` to a
`config.yml` **outside this repo**, typically under your user profile's
`.cloudflared` directory. Replace `YOUR_TUNNEL_UUID` and `YOUR_USERNAME` with
your own values, keeping `credentials-file` pointed at the real private JSON.
The example includes the required final `http_status:404` rule. Validate and
route both hostnames:

```powershell
cloudflared tunnel ingress validate
cloudflared tunnel route dns csv-profiling-dev dev.iot-prototype.net
cloudflared tunnel route dns csv-profiling-dev api-dev.iot-prototype.net
cloudflared tunnel run csv-profiling-dev
```

Only run `tunnel route dns` after verifying that the hostnames are unused; it
creates the CNAME association, not an A record to your PC. For the locally
managed **Windows service**, run `cloudflared.exe service install` elevated,
then follow [Cloudflare's Windows service guide](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/as-a-service/windows/)
to put the config and UUID JSON where the service account can read them and
point the service ImagePath at that config. A config left only in your personal
profile may work interactively but fail as a service. Validate the service
config with `cloudflared tunnel ingress validate` before restarting. The
service commands in section 3 also apply. Do not commit the real config,
certificate or credentials JSON.

## 5. Check public access and WebSockets

After the service reports connected and both application processes are running:

```powershell
.\scripts\check-dev.ps1
Invoke-RestMethod https://api-dev.iot-prototype.net/health
Invoke-WebRequest https://dev.iot-prototype.net/app-config.js -UseBasicParsing
Resolve-DnsName dev.iot-prototype.net
Resolve-DnsName api-dev.iot-prototype.net
```

Then open `https://dev.iot-prototype.net`. The browser should request the Rust
API at `https://api-dev.iot-prototype.net`, never `http://localhost:8080` from
the public page. Reporting and Remote Console use Rust WebSockets at that API
origin and automatically select `wss://` when HTTPS is used. [Cloudflare Tunnel
supports WebSockets](https://developers.cloudflare.com/cloudflare-one/faq/cloudflare-tunnels-faq/);
check the browser Network/WS panel and allowed origins if a stream fails.

Rust CORS is an explicit `OTA_ALLOWED_ORIGINS` list, not `*`. The tunnel profile
includes `http://localhost:5173` and `https://dev.iot-prototype.net`. The
Rust WebSocket handlers also check the browser `Origin` against that list.
Neither service trusts `X-Forwarded-For`, `X-Forwarded-Proto`, or
`CF-Connecting-IP` for authorization. Do not add proxy-header trust unless you
also enforce a trusted proxy boundary.

## MQTT and ESP32 limits

No browser code connects directly to MQTT; the Rust backend subscribes and
publishes Monitoring updates to the browser over its own WebSocket. This Tunnel
maps **HTTP only**. It does not publish PostgreSQL `5432`, raw MQTT `1883`,
or MQTT WebSockets. A local Mosquitto broker on `1883` would require a private
LAN connection, but this Rust subscriber currently configures TLS even when
no CA file is specified and defaults to `8883`; plaintext `1883` will not work
as-is. Use a TLS-configured broker or plan a separate, explicit local MQTT
transport change. Never enable anonymous public MQTT.

For an ESP32 on the same Wi-Fi, find the laptop LAN address with
`Get-NetIPAddress -AddressFamily IPv4`; it changes across networks. A home
DHCP reservation or mDNS may help locally, but is not needed for Cloudflare's
public HTTP hostnames. If an ESP32 is to use the public API over HTTPS, verify
its ESP-IDF TLS trust configuration first: the current example creates an HTTP
client with default settings and public HTTPS OTA has **not** been validated.
Do not disable certificate verification to make it connect. Device binaries
also embed `OTA_SERVER_URL` when built; changing networks or URL later may
require a new firmware build. For local LAN OTA, use `-AllowLanDevices`, a
reachable LAN `OTA_PUBLIC_BASE_URL`, and appropriately scoped Windows Firewall
rules. A hosted MQTT service is a separate future option.

## Troubleshooting

| Symptom | First check |
| --- | --- |
| Frontend public URL returns 502 | `http://localhost:5173/` must respond; then inspect the `dev` route Service URL and connector status. |
| Public API returns 502 | `http://localhost:8080/health` must respond; inspect the `api-dev` route. |
| Tunnel disconnected | `Get-Service cloudflared`, Dashboard Tunnel status, service logs, and outbound connectivity. |
| Works at home but not campus/hotspot | The network may block outbound Cloudflare Tunnel traffic on port `7844` (QUIC/UDP or HTTP2/TCP). Ask the network administrator; do not open inbound ports. |
| CORS or WebSocket Origin denied | Check `OTA_ALLOWED_ORIGINS` contains the **exact** frontend origin, including `https://`. Restart Rust after changing it. |
| Mixed-content warning | Inspect `/app-config.js`; the public page must select `https://api-dev.iot-prototype.net`, so WS becomes `wss://`. |
| Public URL works but localhost fails | Check the local Flask/Rust process and the port; this is not a DNS problem. |
| Localhost works but public fails | Check `cloudflared`, the named Tunnel's hostname route, DNS CNAME association, and origin Service URL. |
| Laptop changes Wi-Fi | The connector should reconnect automatically if the new network allows outbound traffic. DNS need not change. Restart the app jobs only if they stopped. |
| Monitoring says no devices | Device registration/MQTT may be absent. The viewer token only grants viewing; it does not create device data. |

Cloudflare's [firewall guide](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/tunnel-with-firewall/)
documents outbound port `7844`. The [Tunnel troubleshooting guide](https://developers.cloudflare.com/tunnel/troubleshooting/)
covers 502s, DNS conflicts and blocked connections.

## Shut down / remove

Press Ctrl+C in `dev.ps1` to stop Flask and Rust. The `cloudflared` service
keeps trying to reconnect; stop it with `Stop-Service cloudflared` (elevated)
if you want the public hostnames offline. To remove the setup, uninstall the
service with `cloudflared.exe service uninstall`, remove the Tunnel and public
routes in Cloudflare Dashboard, then remove any local credential copies. Do not
delete your database or firmware files as part of Tunnel cleanup.

Never commit `.env`, `.env.*` except `.env.example`, Tunnel tokens, private
`cloudflared/config.yml`, UUID JSON, PEM or key files. The example config is
intentionally credential-free.
