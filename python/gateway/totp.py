"""RFC 6238 time-based one-time passwords (HMAC-SHA1, 30-second steps, 6 digits)."""
import base64
import hashlib
import hmac
import struct
import time
from urllib.parse import quote, urlencode

PERIOD = 30
DIGITS = 6
MIN_KEY_BYTES = 20


def decode_secret(secret: str) -> bytes:
    cleaned = secret.replace(" ", "").upper()
    return base64.b32decode(cleaned + "=" * (-len(cleaned) % 8))


def code_at(key: bytes, counter: int) -> str:
    digest = hmac.new(key, struct.pack(">Q", counter), hashlib.sha1).digest()
    offset = digest[-1] & 0x0F
    value = struct.unpack(">I", digest[offset:offset + 4])[0] & 0x7FFFFFFF
    return str(value % 10 ** DIGITS).zfill(DIGITS)


def match(key: bytes, code: str, now: float | None = None, window: int = 1) -> int | None:
    """Return the matching time-step counter, or None. Allows +/- one step of clock drift."""
    if len(code) != DIGITS or not code.isascii() or not code.isdigit():
        return None
    current = int((time.time() if now is None else now) // PERIOD)
    found = None
    # Check every candidate so timing does not reveal which step matched.
    for counter in range(current - window, current + window + 1):
        if hmac.compare_digest(code_at(key, counter), code):
            found = counter
    return found


def provisioning_uri(secret: str, username: str, issuer: str) -> str:
    label = quote(f"{issuer}:{username}")
    query = urlencode({"secret": secret, "issuer": issuer, "algorithm": "SHA1",
                       "digits": DIGITS, "period": PERIOD})
    return f"otpauth://totp/{label}?{query}"
