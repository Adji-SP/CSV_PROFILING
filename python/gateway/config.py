"""Fail-closed public gateway configuration; secrets never enter browser config."""
import json
import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import urlsplit

from dotenv import dotenv_values

from . import totp

ROOT = Path(__file__).resolve().parents[2]
DEVICE_ID = re.compile(r"[A-Za-z0-9_-]{1,128}\Z")


@dataclass
class Config:
    origin: str
    username: str
    password_hash: str = field(repr=False)
    cookie_key: str = field(repr=False)
    device_hashes: dict[str, str] = field(default_factory=dict, repr=False)
    viewer_token: str = field(default="", repr=False)
    totp_secret: str = field(default="", repr=False)
    csv_origin: str = "http://127.0.0.1:5173"
    ota_origin: str = "http://127.0.0.1:18080"
    max_body: int = 52 * 1024 * 1024
    session_seconds: int = 8 * 60 * 60

    def __post_init__(self):
        origin = urlsplit(self.origin)
        if (origin.scheme != "https" or not origin.hostname or origin.username
                or origin.password or origin.path or origin.query or origin.fragment):
            raise ValueError("GATEWAY_PUBLIC_URL must be an HTTPS origin without a trailing slash.")
        if not self.username or not self.password_hash.startswith("$argon2id$"):
            raise ValueError("Run setup-dashboard-login.ps1 to provision an administrator.")
        # Two-factor login is mandatory: the gateway refuses to start without it.
        try:
            key = totp.decode_secret(self.totp_secret)
        except ValueError:
            key = b""
        if len(key) < totp.MIN_KEY_BYTES:
            raise ValueError("Two-factor login is not set up. Run setup-dashboard-login.ps1.")
        for upstream in (self.csv_origin, self.ota_origin):
            parsed = urlsplit(upstream)
            if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1"
                    or parsed.path or parsed.query or parsed.fragment or parsed.username):
                raise ValueError("Gateway upstreams must be loopback HTTP origins.")
        for device_id, digest in self.device_hashes.items():
            if not DEVICE_ID.fullmatch(device_id) or not re.fullmatch(r"[a-f0-9]{64}", digest):
                raise ValueError("Invalid device credential record.")
        if len(self.viewer_token) < 32:
            raise ValueError("CONSOLE_VIEWER_TOKEN must be configured for the login gateway.")
        if not 1024 <= self.max_body <= 1024 * 1024 * 1024:
            raise ValueError("Gateway upload limit must be between 1 KiB and 1 GiB.")


def load_config() -> Config:
    values = {**dotenv_values(ROOT / ".env", interpolate=False), **os.environ}
    auth_path = Path(values.get("GATEWAY_AUTH_FILE") or ROOT / "data/private/dashboard-auth.json")
    if not auth_path.is_absolute():
        auth_path = ROOT / auth_path
    try:
        auth = json.loads(auth_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ValueError("Dashboard credentials are missing or invalid. Run setup-dashboard-login.ps1.") from error
    return Config(
        origin=(values.get("GATEWAY_PUBLIC_URL") or "").rstrip("/"),
        username=auth["username"], password_hash=auth["password_hash"],
        cookie_key=auth["cookie_key"], device_hashes=auth.get("devices", {}),
        totp_secret=auth.get("totp_secret", ""),
        viewer_token=values.get("CONSOLE_VIEWER_TOKEN") or "",
        csv_origin=values.get("GATEWAY_CSV_ORIGIN") or "http://127.0.0.1:5173",
        ota_origin=values.get("GATEWAY_OTA_ORIGIN") or "http://127.0.0.1:18080",
        max_body=(int(values.get("OTA_MAX_UPLOAD_MB") or 50) + 2) * 1024 * 1024,
    )
