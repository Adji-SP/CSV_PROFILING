"""Real TLS + MQTT 3.1.1 checks, using stdlib only. Config arrives privately on stdin."""
import json
import os
import socket
import ssl
import struct
import sys
import uuid
from urllib.parse import urlsplit


def field(value):
    data = value.encode("utf-8")
    if len(data) > 65535:
        raise ValueError("MQTT setting is too long")
    return struct.pack("!H", len(data)) + data


def packet(kind, data):
    remaining = len(data)
    length = bytearray()
    while True:
        part = remaining % 128
        remaining //= 128
        length.append(part | (128 if remaining else 0))
        if not remaining:
            break
    return bytes([kind]) + length + data


def read_exact(connection, size):
    result = bytearray()
    while len(result) < size:
        part = connection.recv(size - len(result))
        if not part:
            raise ValueError("Broker disconnected before acknowledging the request")
        result.extend(part)
    return bytes(result)


def read_packet(connection):
    kind = read_exact(connection, 1)[0]
    size = 0
    for index in range(4):
        value = read_exact(connection, 1)[0]
        size += (value & 127) * (128 ** index)
        if not value & 128:
            if size > 65536:
                raise ValueError("Unexpectedly large broker diagnostic response")
            return kind, read_exact(connection, size)
    raise ValueError("Malformed MQTT response")


def connect(env, root, device=False):
    prefix = "DEVICE_MQTT_" if device else "MQTT_"
    if device:
        url = urlsplit(env.get("DEVICE_MQTT_URL") or "")
        if url.scheme != "mqtts" or not url.hostname or url.username or url.password:
            raise ValueError("DEVICE_MQTT_URL must be mqtts://broker-host:8883 without embedded credentials")
        host, port = url.hostname, url.port or 8883
        if host in ("localhost", "127.0.0.1", "::1", "0.0.0.0"):
            raise ValueError("DEVICE_MQTT_URL must be reachable from the ESP32, not localhost")
    else:
        host, port = env.get("MQTT_HOST"), int(env.get("MQTT_PORT") or 8883)
        if not host or "://" in host:
            raise ValueError("MQTT_HOST must contain a broker hostname, without a scheme")
    username = env.get(prefix + "USERNAME")
    if not username:
        raise ValueError(prefix + "USERNAME is missing")

    def path(name):
        value = env.get(prefix + name)
        if not value:
            return None
        full = value if os.path.isabs(value) else os.path.join(root, value)
        if not os.path.isfile(full):
            raise ValueError(prefix + name + " points to a missing file")
        return full

    ca = path("CA_FILE")
    ca_pem = env.get(prefix + "CA_PEM") or None
    context = ssl.create_default_context(cafile=ca, cadata=ca_pem)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    cert, key = path("CERT_FILE"), path("KEY_FILE")
    if bool(cert) != bool(key):
        raise ValueError(prefix + "CERT_FILE and KEY_FILE must be provided together")
    if cert:
        context.load_cert_chain(cert, key)
    raw = socket.create_connection((host, port), timeout=8)
    try:
        connection = context.wrap_socket(raw, server_hostname=host)
    except Exception:
        raw.close()
        raise
    try:
        password = env.get(prefix + "PASSWORD") or ""
        flags = 0x82 | (0x40 if password else 0)
        data = field("MQTT") + bytes([4, flags]) + struct.pack("!H", 20)
        data += field("setup-check-" + uuid.uuid4().hex[:16]) + field(username)
        if password:
            data += field(password)
        connection.sendall(packet(0x10, data))
        kind, response = read_packet(connection)
        if kind != 0x20 or len(response) != 2:
            raise ValueError("Broker returned an unexpected CONNECT response")
        if response[1] != 0:
            reasons = {1: "unsupported protocol", 2: "client ID rejected", 3: "broker unavailable", 4: "bad username/password", 5: "not authorized"}
            raise ValueError(prefix + " authentication failed: " + reasons.get(response[1], "unknown reason"))
        print(("Device" if device else "Backend") + " TLS certificate and MQTT authentication: PASS")
        return connection
    except Exception:
        connection.close()
        raise


def check(config):
    env, root = config["env"], config["root"]
    backend = connect(env, root)
    try:
        topics = ["devices/+/logs", "tinyml/v1/#"]
        data = struct.pack("!H", 1) + b"".join(field(topic) + bytes([1]) for topic in topics)
        backend.sendall(packet(0x82, data))
        kind, reply = read_packet(backend)
        if kind != 0x90 or reply[:2] != b"\0\1" or len(reply) != 4 or any(code > 2 for code in reply[2:]):
            raise ValueError("Backend subscriptions were rejected; check console-backend ACL")
        print("Backend subscriptions to device logs and TinyML: PASS")
        if config.get("probe_device"):
            device_id = env.get("DEVICE_ID") or ""
            if not device_id or len(device_id) > 128 or any(not (char.isascii() and (char.isalnum() or char in "-_")) for char in device_id):
                raise ValueError("DEVICE_ID must be a valid per-device identifier")
            device = connect(env, root, device=True)
            try:
                marker = uuid.uuid4().hex
                payload = json.dumps({"device_id": device_id, "level": "INFO", "target": "mqtt_setup_check", "message": "MQTT setup diagnostic, not ESP firmware: " + marker}).encode()
                device.sendall(packet(0x30, field("devices/" + device_id + "/logs") + payload))
                for _ in range(100):
                    kind, reply = read_packet(backend)
                    if kind >> 4 != 3:
                        continue
                    topic_size = struct.unpack("!H", reply[:2])[0]
                    offset = 2 + topic_size
                    if (kind >> 1) & 3:
                        backend.sendall(packet(0x40, reply[offset:offset + 2]))
                        offset += 2
                    if marker.encode() in reply[offset:]:
                        print("Device publish -> backend receive: PASS (one labelled diagnostic log sent)")
                        break
                else:
                    raise ValueError("Diagnostic log was not delivered; check device publish ACL")
            finally:
                device.close()
        else:
            print("Device publishing not tested. Use -ProbeDevice for one labelled diagnostic log.")
    finally:
        backend.close()


if __name__ == "__main__":
    try:
        check(json.load(sys.stdin))
    except ssl.SSLCertVerificationError:
        print("FAIL: TLS verification failed. Check CA trust, certificate expiry and hostname/IP SAN. Never disable verification.")
        sys.exit(1)
    except (TimeoutError, ConnectionError, socket.gaierror, OSError):
        print("FAIL: Broker connection/file access failed. Check broker listener, DNS, port, TLS files and firewall.")
        sys.exit(1)
    except (ValueError, KeyError) as error:
        print("FAIL:", error)
        sys.exit(1)
