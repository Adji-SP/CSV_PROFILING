"""Provision two local MQTT identities; never print or pass passwords in argv.

Called by setup-mqtt.ps1 after that script restricts the broker data directory ACL.
Changes only the two selected broker accounts and allowlisted .env settings.
"""
import argparse
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import tempfile
import uuid


def update_env(text, settings):
    kept = []
    for entry in text.splitlines(keepends=True):
        match = re.match(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)", entry)
        if match and match[1] in settings:
            value = match[2].strip()
            if value.startswith(('"', "'")):
                pattern = r'(?:"(?:\\.|[^"\\])*"|\x27[^\x27]*\x27)\s*(?:#.*)?'
                if not re.fullmatch(pattern, value):
                    raise ValueError("Move multiline MQTT settings to *_FILE before provisioning")
            continue
        kept.append(entry)
    text = "".join(kept)
    for key, value in settings.items():
        line = "" if value is None else key + "=" + ('"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"')
        if line:
            text = (text.rstrip() + "\n" if text.strip() else "") + line + "\n"
    return text


def provision(args):
    root, folder = Path(args.root).resolve(), Path(args.data_dir).resolve()
    env_path, password_path = root / ".env", folder / "passwords"
    if not env_path.is_file() or not folder.is_dir():
        raise ValueError("Create the project .env and protected MQTT data folder first")
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", args.device_id) or args.device_id == "console-backend":
        raise ValueError("Invalid device identity")
    backend_password, device_password = secrets.token_urlsafe(36), secrets.token_urlsafe(36)
    identities = {"console-backend": backend_password, args.device_id: device_password}
    backup = folder / ("credentials-backup-" + uuid.uuid4().hex)
    backup.mkdir()
    shutil.copy2(env_path, backup / "project.env")
    if password_path.exists():
        shutil.copy2(password_path, backup / "passwords")
    # -U hashes an isolated plaintext file, not existing password hashes.
    # The broker folder already has restricted ACLs; plaintext is removed on exit.
    stage = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=folder, prefix="provision-", delete=False) as file:
            stage = Path(file.name)
            for name, password in identities.items():
                file.write(name + ":" + password + "\n")
        result = subprocess.run([args.passwd_tool, "-U", str(stage)], capture_output=True)
        if result.returncode:
            raise ValueError("mosquitto_passwd could not hash the new identities; existing accounts unchanged")
        hashes = stage.read_text(encoding="utf-8").splitlines()
        if len(hashes) != 2 or any(":$" not in line for line in hashes):
            raise ValueError("Unexpected password tool output; refusing to install it")
        previous = password_path.read_text(encoding="utf-8").splitlines() if password_path.exists() else []
        kept = [line for line in previous if line.split(":", 1)[0] not in identities]
        stage.write_text("\n".join(kept + hashes) + "\n", encoding="utf-8")
        settings = {
            "MQTT_HOST": "localhost", "MQTT_PORT": str(args.port),
            "MQTT_USERNAME": "console-backend", "MQTT_PASSWORD": backend_password,
            "MQTT_CA_FILE": (folder / "ca.crt").as_posix(),
            "MQTT_CERT_FILE": None, "MQTT_KEY_FILE": None,
            "DEVICE_ID": args.device_id,
            "DEVICE_MQTT_URL": f"mqtts://{args.address}:{args.port}",
            "DEVICE_MQTT_USERNAME": args.device_id, "DEVICE_MQTT_PASSWORD": device_password,
            "DEVICE_MQTT_CA_FILE": (folder / "ca.crt").as_posix(),
            "DEVICE_MQTT_CA_PEM": None, "DEVICE_MQTT_CERT_PEM": None, "DEVICE_MQTT_KEY_PEM": None,
            "DEVICE_MQTT_CERT_FILE": None, "DEVICE_MQTT_KEY_FILE": None,
        }
        original = env_path.read_text(encoding="utf-8-sig")
        replacement = update_env(original, settings)
        os.replace(stage, password_path)
        stage = None
        try:
            env_path.write_text(replacement, encoding="utf-8")
        except OSError:
            if (backup / "passwords").exists():
                shutil.copy2(backup / "passwords", password_path)
            else:
                password_path.unlink(missing_ok=True)
            shutil.copy2(backup / "project.env", env_path)
            raise
        print("MQTT accounts provisioned; strong passwords saved only in the ignored project .env.")
        print("Existing firmware with earlier credentials must be rebuilt. No board was flashed.")
        print("Previous settings are recoverable in the protected MQTT credentials-backup folder.")
    finally:
        if stage is not None:
            stage.unlink(missing_ok=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    for key in ("root", "data-dir", "passwd-tool", "device-id", "address"):
        parser.add_argument("--" + key, required=True)
    parser.add_argument("--port", type=int, default=8883)
    try:
        provision(parser.parse_args())
    except (OSError, ValueError):
        print("MQTT provisioning failed; check paths, account tool and multiline PEM settings. No credentials were printed.")
        raise SystemExit(1)
