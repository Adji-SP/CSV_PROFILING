"""Local-only account/device provisioning. No public registration or default password."""
import argparse
import base64
import csv
import hashlib
import json
import os
import secrets
import subprocess
from pathlib import Path
from urllib.parse import urlsplit

from argon2 import PasswordHasher
from cryptography.fernet import Fernet
from dotenv import dotenv_values, set_key

from . import totp
from .config import ROOT, DEVICE_ID


def private_directory(path: Path):
    path.mkdir(parents=True, exist_ok=True)
    if os.name == "nt":
        account = subprocess.check_output(["whoami", "/user", "/fo", "csv", "/nh"], text=True)
        sid = next(csv.reader([account.strip()]))[1]
        subprocess.run(["icacls", str(path), "/inheritance:r", "/grant:r",
                        f"*{sid}:(OI)(CI)F", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"],
                       check=True, stdout=subprocess.DEVNULL)
    else:
        path.chmod(0o700)


def provision_totp(directory: Path, auth: dict):
    secret = base64.b32encode(secrets.token_bytes(totp.MIN_KEY_BYTES)).decode()
    auth["totp_secret"] = secret
    uri = totp.provisioning_uri(secret, auth["username"], "CSV OTA Dashboard")
    grouped = " ".join(secret[i:i + 4] for i in range(0, len(secret), 4))
    # Same private handoff pattern as the password: never printed to the terminal.
    (directory / "dashboard-totp.txt").write_text(
        "Add this account to an authenticator app (Google Authenticator, Authy, 2FAS...).\n"
        "Scan dashboard-totp-qr.svg, or choose 'Enter a setup key' and type:\n\n"
        f"Setup key: {grouped}\n\n{uri}\n\n"
        "Delete this file and the QR image after setup. New secret:\n"
        "scripts/setup-dashboard-login.ps1 -ResetTotp\n", encoding="utf-8")
    qr = directory / "dashboard-totp-qr.svg"
    qr.unlink(missing_ok=True)
    try:
        import segno
        segno.make(uri, error="m").save(str(qr), scale=8, border=4)
        print("Authenticator setup saved in data/private/dashboard-totp.txt and dashboard-totp-qr.svg")
    except ImportError:
        print("Authenticator setup key saved in data/private/dashboard-totp.txt")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--username", default="admin")
    parser.add_argument("--reset-password", action="store_true")
    parser.add_argument("--reset-totp", action="store_true",
                        help="Replace the authenticator app secret (the old app entry stops working).")
    parser.add_argument("--device-id", help="Provision/rotate this device's HTTP OTA credential.")
    parser.add_argument("--public-url", help="Use this HTTPS origin for the gateway and new firmware builds.")
    args = parser.parse_args()
    if not 1 <= len(args.username) <= 128:
        parser.error("Username must contain between 1 and 128 characters.")
    if args.device_id and not DEVICE_ID.fullmatch(args.device_id):
        parser.error("Device ID must contain only letters, digits, underscores or hyphens.")
    if args.public_url:
        args.public_url = args.public_url.rstrip("/")
        url = urlsplit(args.public_url)
        if url.scheme != "https" or not url.hostname or url.username or url.path or url.query or url.fragment:
            parser.error("Public URL must be an HTTPS origin.")
    directory = ROOT / "data/private"
    private_directory(directory)
    auth_file = directory / "dashboard-auth.json"
    auth = json.loads(auth_file.read_text()) if auth_file.exists() else {}
    if not auth or args.reset_password:
        password = secrets.token_urlsafe(24)
        auth.update(username=args.username, password_hash=PasswordHasher().hash(password),
                    cookie_key=Fernet.generate_key().decode())
        # Private local handoff file; the password never appears in the terminal or chat.
        (directory / "dashboard-login.txt").write_text(
            f"Username: {args.username}\nPassword: {password}\n\n"
            "Keep this file private. Password reset: scripts/setup-dashboard-login.ps1 -ResetPassword\n",
            encoding="utf-8")
        print("Administrator credentials saved in data/private/dashboard-login.txt")
    else:
        print("Administrator already provisioned; credentials unchanged.")
    if not auth.get("totp_secret") or args.reset_totp:
        provision_totp(directory, auth)
    if args.device_id:
        token = secrets.token_urlsafe(32)
        auth.setdefault("devices", {})[args.device_id] = hashlib.sha256(token.encode()).hexdigest()
        set_key(str(ROOT / ".env"), "DEVICE_ID", args.device_id)
        set_key(str(ROOT / ".env"), "DEVICE_OTA_TOKEN", token)
        print(f"OTA credential provisioned for {args.device_id}. Rebuild that device firmware to use it.")
    auth_file.write_text(json.dumps(auth, indent=2) + "\n", encoding="utf-8")
    if args.public_url:
        for name in ("GATEWAY_PUBLIC_URL", "OTA_SERVER_URL", "OTA_PUBLIC_BASE_URL"):
            set_key(str(ROOT / ".env"), name, args.public_url)
        print("Gateway and new firmware URLs saved in .env.")
    if os.name != "nt":
        for item in directory.iterdir():
            if item.is_file():
                item.chmod(0o600)
    print("Restart the authenticated dashboard after provisioning or resetting credentials.")


if __name__ == "__main__":
    main()
