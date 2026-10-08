# Public dashboard with a login

Use the username and password in `data/private/dashboard-login.txt`. This file is
created locally, ignored by Git and restricted to your Windows account and
administrators. There is no default password and no public registration page.
Copy the password into your password manager; you can then remove that handoff
file. The gateway stores only the Argon2id password hash.

The authentication design separates administrator sessions from
device credentials. The implementation uses the libraries' documented
[encrypted session cookies](https://aiohttp-session.readthedocs.io/en/stable/reference.html#cookie-storage)
and [Argon2 password verification](https://argon2-cffi.readthedocs.io/en/stable/howto.html).

## Setup and daily use (Windows)

One-time setup, from the repository root:

```powershell
.\scripts\setup-dashboard-login.ps1 -DeviceId ESP32-S3-001
```

This also creates the two-factor login secret (see below). Use your real
registered device ID. This creates the administrator account and a
separate HTTP OTA credential for that device. It puts `DEVICE_OTA_TOKEN` in the
ignored `.env`, and its SHA-256 digest in the private gateway configuration.
Do not run device provisioning every start: it rotates the selected token.
You can also pass `-PublicUrl https://YOUR_HOSTNAME` to save the gateway and
firmware URLs in `.env`. The public launcher sets its origin automatically
from this computer's Tailscale hostname.

Stop your previous project launcher with Ctrl+C, then start the authenticated stack:

```powershell
.\scripts\dev-public.ps1
```

In a second PowerShell terminal, configure publishing once:

```powershell
.\scripts\setup-public-access.ps1
```

Tailscale may show an account approval link to enable Funnel. After enabling it,
rerun that command. Use `-PrivateOnly` to keep access limited to your tailnet.
The setup verifies the gateway, checks that all three internal listeners bind to
loopback, removes the old direct API routes, and publishes only the gateway.
It refuses to overwrite unrecognized Tailscale routes.

The fixed HTTPS address now works on a phone without Tailscale when **Funnel** is
enabled. Keep the PC, Tailscale and the launcher running. For future starts use
`dev-public.ps1`; do not switch to `dev.ps1` or `dev-tailscale.ps1` while Funnel is
enabled, because those older launchers do not start the gateway.

## Two-factor login (authenticator app)

Logging in needs the password **and** a 6-digit code from an authenticator app
(Google Authenticator, Microsoft Authenticator, Authy, 2FAS, ...). The gateway
refuses to start until this is set up, so a public dashboard is never
password-only.

1. Run `.\scripts\setup-dashboard-login.ps1` (existing accounts keep their
   password; only the missing authenticator secret is created).
2. Open `data/private/dashboard-totp-qr.svg` on the PC and scan it with the app,
   or type the setup key from `data/private/dashboard-totp.txt`.
3. Delete both files after the app shows codes, then restart `dev-public.ps1`.

Each code works once: a code that was already accepted is rejected, even
within its 30-second window. The PC clock must be correct (Windows: Settings >
Time > Sync now). Wrong password and wrong code return the same message, and
both count toward the login limit.

Lost phone: on the PC, run `.\scripts\setup-dashboard-login.ps1 -ResetTotp`,
add the new QR code to an app and restart the gateway. The old app entry stops
working. Only someone with access to this PC can do this.

## Active sessions

The dashboard's **Sessions** button lists every logged-in browser: login time,
last activity and browser/system. The current device is marked. You can revoke
a single session or **Log out all other sessions**; revoked browsers are sent
to the login page and their monitoring sockets close immediately. The browser
name comes from the browser's own User-Agent and can be faked, so treat it as a
hint. If you see a session you don't recognize, revoke it, then reset the password
and the authenticator (`-ResetPassword -ResetTotp`).

## What the login protects

```text
HTTPS (Funnel or Serve)
        |
login gateway :8088 (127.0.0.1)
        |-- /api/ota, /api/tinyml, /health --> Axum :18080 (127.0.0.1)
        |-- dashboard and CSV routes ------> Flask :5173 (127.0.0.1)
```

The gateway uses `aiohttp`, `aiohttp-session` with Fernet-encrypted cookies and
`argon2-cffi`. Login cookies are Secure, HttpOnly and SameSite=Strict. Sessions
last eight hours and are also tracked in memory: logout and gateway restarts
invalidate them. Logout/expiry close active monitoring sockets. Login attempts
are limited, and browser write requests must carry the configured same-origin
Origin header. Private viewer keys never reach the browser; logged-in users
automatically get Reporting and Remote Console access.

The existing local Flask and Axum endpoints remain local development services.
The gateway is the authentication boundary: never publish ports 5173 or 18080
directly or forward a more-specific `/api/...` tunnel route around it.
Only the gateway's login assets and minimal `/auth/health` are anonymous.

The account is an administrator: it can upload and compile trusted Rust projects.
Do not share it with untrusted visitors. Build jobs execute project build scripts
on this PC; authentication does not sandbox those builds.

## Connect the ESP32

An ESP32 cannot log in through a browser. It uses its own token:

```env
DEVICE_ID=ESP32-S3-001
DEVICE_OTA_TOKEN='GENERATED_BY_THE_SETUP_SCRIPT'
OTA_SERVER_URL=https://csv-ota-dashboard.YOUR_TAILNET.ts.net
OTA_PUBLIC_BASE_URL=https://csv-ota-dashboard.YOUR_TAILNET.ts.net
```

Use the printed hostname and keep the generated token private. The updated
runtime sends `Authorization: Bearer ...` to register, heartbeat, update-check,
OTA-status and download endpoints. It can only report as that device and
download the image currently assigned to it; it cannot upload/build/deploy,
read CSV data, or act as another device. It verifies HTTPS certificates and
will not send its token to a different download URL.

Rebuild and initially flash this updated runtime to the board. Firmware already
flashed with an old LAN URL or without a token cannot use the protected endpoint.
Managed dashboard builds embed the token and public URL on subsequent updates.
No board is automatically flashed by setting up this login. Provision one
device at a time; the current `.env` selects which device a managed build targets.
Previously provisioned device hashes remain in the gateway configuration.

MQTT has separate broker credentials. This HTTP publishing setup does not expose
the Mosquitto broker to the internet. Devices on the laptop's LAN can still send
MQTT logs, and a remote browser can watch them through the authenticated gateway.

## Reset a password, authenticator or device token

```powershell
.\scripts\setup-dashboard-login.ps1 -ResetPassword
.\scripts\setup-dashboard-login.ps1 -ResetTotp
.\scripts\setup-dashboard-login.ps1 -DeviceId ESP32-S3-001
```

Restart `dev-public.ps1` afterwards. Password reset creates a new random password
in the private handoff file. Device-token rotation requires rebuilding the
selected device firmware. Never put real credentials in ZIPs, Git, screenshots
or URLs. Remove a device's entry from the private `devices` mapping and restart
the gateway to revoke it.

## Verification

```powershell
Push-Location python
try { .\.venv\Scripts\python.exe -m unittest gateway.test_gateway -v }
finally { Pop-Location }
```

Tests use isolated local services and test credentials. They cover anonymous
access, CSRF, password throttling, session expiry/logout, device boundaries,
WebSocket authentication/logout, CSV events and streaming uploads/downloads.
For the optional real-browser check, install Playwright in an ignored local
directory and use the private credential handoff file (Microsoft Edge is used):

```powershell
npm install --prefix data/browser-check --no-save --package-lock=false playwright
$env:GATEWAY_TEST_URL='https://YOUR_HOSTNAME'
node scripts/test-login-ui.cjs
```

The browser check logs in, checks CSV/OTA and shared monitoring access, then logs
out. It never prints credentials. It saves only an empty mobile login screenshot
under `data/`. To disable public access, run `tailscale funnel --https=443 off`.
