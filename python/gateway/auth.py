"""Browser sessions and separately scoped device credentials."""
import asyncio
import hashlib
import hmac
import json
import secrets
import time
from collections import deque
from dataclasses import dataclass, field

from aiohttp import web
from aiohttp_session import get_session, new_session
from argon2 import PasswordHasher
from argon2.exceptions import VerificationError, InvalidHashError

from . import totp
from .config import Config


def error(status: int, message: str) -> web.Response:
    return web.json_response({"error": {"message": message}}, status=status)


async def read_json(request: web.Request, limit: int = 4096):
    """Read a bounded JSON body even if Transfer-Encoding: chunked omits Content-Length."""
    body = bytearray()
    async for chunk in request.content.iter_chunked(1024):
        body.extend(chunk)
        if len(body) > limit:
            raise ValueError("Request is too large.")
    return json.loads(body)


@dataclass
class LoginSession:
    expires: float
    # Public handle for the session list; the cookie's sid is never sent to the page.
    handle: str = field(default_factory=lambda: secrets.token_urlsafe(12))
    created: float = field(default_factory=time.time)
    last_seen: float = field(default_factory=time.time)
    user_agent: str = ""
    sockets: set = field(default_factory=set)


class Auth:
    def __init__(self, config: Config):
        self.config = config
        self.hasher = PasswordHasher()
        self.totp_key = totp.decode_secret(config.totp_secret) if config.require_totp else None
        # A code is accepted once: any later login must use a newer time step.
        self.last_totp_counter = -1
        self.sessions: dict[str, LoginSession] = {}
        # Account-wide throttle cannot be bypassed using forged forwarding headers.
        self.attempts: deque[float] = deque()
        self.password_lock = asyncio.Lock()

    def same_origin(self, request: web.Request) -> bool:
        return request.headers.get("Origin") == self.config.origin

    async def browser(self, request: web.Request) -> LoginSession | None:
        cookie = await get_session(request)
        session = self.sessions.get(cookie.get("sid", ""))
        if session and session.expires > time.monotonic():
            session.last_seen = time.time()
            return session
        return None

    def device(self, request: web.Request) -> str | None:
        scheme, _, token = request.headers.get("Authorization", "").partition(" ")
        if scheme.lower() != "bearer" or not 32 <= len(token) <= 256:
            return None
        digest = hashlib.sha256(token.encode()).hexdigest()
        for device_id, expected in self.config.device_hashes.items():
            if hmac.compare_digest(digest, expected):
                return device_id
        return None

    async def login(self, request: web.Request) -> web.Response:
        if not self.same_origin(request):
            return error(403, "Open the login page at the configured HTTPS address.")
        if request.content_type != "application/json":
            return error(415, "Send login credentials as JSON.")
        now = time.monotonic()
        while self.attempts and self.attempts[0] < now - 60:
            self.attempts.popleft()
        if len(self.attempts) >= 10 or self.password_lock.locked():
            response = error(429, "Too many attempts. Wait a minute and try again.")
            response.headers["Retry-After"] = "60"
            return response
        self.attempts.append(now)
        try:
            data = await read_json(request)
            username, password = data["username"], data["password"]
            code = data.get("code", "") if self.config.require_totp else ""
            if not all(isinstance(value, str) for value in (username, password, code)):
                raise ValueError()
            if len(username) > 128 or len(password) > 1024 or len(code) > 16:
                raise ValueError()
        except (ValueError, KeyError, TypeError):
            return error(400, "Enter your username, password and authenticator code."
                         if self.config.require_totp else "Enter your username and password.")
        async with self.password_lock:
            try:
                valid = await asyncio.to_thread(self.hasher.verify, self.config.password_hash, password)
            except (VerificationError, InvalidHashError):
                valid = False
            valid = valid and hmac.compare_digest(username.encode(), self.config.username.encode())
            if self.config.require_totp:
                # Checked under the lock so two requests cannot both spend the same code.
                counter = totp.match(self.totp_key, code.replace(" ", ""))
                if valid and counter is not None and counter > self.last_totp_counter:
                    self.last_totp_counter = counter
                else:
                    valid = False
        if not valid:
            # One message for every factor: never reveal which part was wrong.
            return error(401, "Username, password or authenticator code is incorrect."
                         if self.config.require_totp else "Username or password is incorrect.")
        await self.revoke(request)
        if len(self.sessions) >= 32:
            return error(429, "Too many active sessions. Log out on another device first.")
        session = await new_session(request)
        sid = secrets.token_urlsafe(32)
        session["sid"] = sid
        self.sessions[sid] = LoginSession(time.monotonic() + self.config.session_seconds,
                                          user_agent=request.headers.get("User-Agent", "")[:200])
        return web.json_response({"username": self.config.username})

    async def revoke(self, request: web.Request):
        cookie = await get_session(request)
        session = self.sessions.pop(cookie.get("sid", ""), None)
        cookie.invalidate()
        if session:
            await self.close_sockets(session)

    async def logout(self, request: web.Request) -> web.Response:
        if not self.same_origin(request):
            return error(403, "Invalid request origin.")
        await self.revoke(request)
        return web.json_response({"logged_out": True})

    async def list_sessions(self, request: web.Request) -> web.Response:
        current = await self.browser(request)
        if not current:
            return error(401, "Log in to continue.")
        now = time.monotonic()
        items = [{"id": session.handle, "created": session.created, "last_seen": session.last_seen,
                  "user_agent": session.user_agent, "current": session is current}
                 for session in self.sessions.values() if session.expires > now]
        items.sort(key=lambda item: item["last_seen"], reverse=True)
        return web.json_response({"sessions": items})

    async def revoke_session(self, request: web.Request) -> web.Response:
        """Revoke one session by handle, or every other session with {"id": "others"}."""
        current = await self.browser(request)
        if not current:
            return error(401, "Log in to continue.")
        if not self.same_origin(request):
            return error(403, "Invalid request origin.")
        try:
            target = (await read_json(request))["id"]
            if not isinstance(target, str):
                raise ValueError()
        except (ValueError, KeyError, TypeError):
            return error(400, "Choose a session to revoke.")
        if target == "others":
            victims = [sid for sid, session in self.sessions.items() if session is not current]
        else:
            victims = [sid for sid, session in self.sessions.items() if session.handle == target]
            if not victims:
                return error(404, "That session has already ended.")
        for sid in victims:
            session = self.sessions.pop(sid)
            await self.close_sockets(session)
            if session is current:
                (await get_session(request)).invalidate()
        return web.json_response({"revoked": len(victims)})

    @staticmethod
    async def close_sockets(session: LoginSession):
        await asyncio.gather(*(ws.close(code=1008, message=b"Login expired")
                               for ws in tuple(session.sockets)), return_exceptions=True)

    async def cleanup(self):
        while True:
            await asyncio.sleep(5)
            for sid, session in tuple(self.sessions.items()):
                if session.expires <= time.monotonic():
                    self.sessions.pop(sid, None)
                    await self.close_sockets(session)
