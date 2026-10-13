"""Security boundaries and streaming regressions, using isolated HTTP services."""
import asyncio
import hashlib
import json
import types
import unittest
from unittest import mock

from aiohttp import web, WSMsgType
from aiohttp.test_utils import TestClient, TestServer
from argon2 import PasswordHasher
from cryptography.fernet import Fernet

from . import totp
from .config import Config, totp_required
from .server import create_app

ORIGIN = "https://dashboard.test"
PASSWORD = "test-only-long-password"
DEVICE_TOKEN = "test-device-credential-" + "a" * 32
VIEWER_TOKEN = "test-viewer-credential-" + "b" * 32
PASSWORD_HASH = PasswordHasher().hash(PASSWORD)
TOTP_SECRET = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP"
TOTP_KEY = totp.decode_secret(TOTP_SECRET)


class GatewayTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.upstream_calls = []
        self.ws_auth = []
        self.image = b"firmware-data" * 16000

        async def upstream(request):
            self.upstream_calls.append((request.method, request.path, dict(request.headers)))
            if request.path.endswith("/ws"):
                socket = web.WebSocketResponse()
                await socket.prepare(request)
                self.ws_auth.append(await socket.receive_json())
                await socket.send_json({"type": "connected"})
                async for _ in socket:
                    pass
                return socket
            if request.path.endswith("/update"):
                return web.json_response({"update_available": True, "firmware_id": "assigned-image"})
            if request.path.endswith("/download"):
                result = web.StreamResponse(headers={"Content-Type": "application/octet-stream",
                                                     "Content-Length": str(len(self.image))})
                await result.prepare(request)
                for i in range(0, len(self.image), 1024):
                    await result.write(self.image[i:i + 1024])
                await result.write_eof()
                return result
            if request.path == "/sse":
                result = web.StreamResponse(headers={"Content-Type": "text/event-stream"})
                await result.prepare(request)
                await result.write(b"data: hello\n\n")
                await result.write_eof()
                return result
            body = await request.read()
            return web.json_response({"ok": True, "bytes": len(body),
                                      "authorization": request.headers.get("Authorization", ""),
                                      "cookie": request.headers.get("Cookie", "")})

        local = web.Application(client_max_size=2 * 1024 * 1024)
        local.router.add_route("*", "/{path:.*}", upstream)
        self.upstream = TestServer(local)
        await self.upstream.start_server()
        address = str(self.upstream.make_url(""))
        self.config = Config(origin=ORIGIN, username="admin", password_hash=PASSWORD_HASH,
                             cookie_key=Fernet.generate_key().decode(),
                             device_hashes={"node-1": hashlib.sha256(DEVICE_TOKEN.encode()).hexdigest()},
                             viewer_token=VIEWER_TOKEN, totp_secret=TOTP_SECRET,
                             csv_origin=address, ota_origin=address,
                             max_body=2 * 1024 * 1024)
        self.client = TestClient(TestServer(create_app(self.config)))
        await self.client.start_server()
        self.cookie = ""
        # Fake clock for the TOTP module only, so each login can use a fresh code.
        self.now = 1_700_000_000.0
        clock = mock.patch.object(totp, "time", types.SimpleNamespace(time=lambda: self.now))
        clock.start()
        self.addCleanup(clock.stop)

    def next_code(self):
        self.now += totp.PERIOD
        return totp.code_at(TOTP_KEY, int(self.now // totp.PERIOD))

    async def asyncTearDown(self):
        await self.client.close()
        await self.upstream.close()

    def headers(self, **extra):
        result = {"Host": "dashboard.test"}
        if self.cookie:
            result["Cookie"] = self.cookie
        result.update(extra)
        return result

    async def login(self, code=None, user_agent="test-browser"):
        credentials = {"username": "admin", "password": PASSWORD}
        if self.config.require_totp:
            credentials["code"] = code or self.next_code()
        response = await self.client.post("/auth/login", headers=self.headers(Origin=ORIGIN, **{"User-Agent": user_agent}),
                                          json=credentials)
        self.assertEqual(response.status, 200, await response.text())
        cookie = response.cookies["__Host-dashboard"]
        self.assertTrue(cookie["secure"] and cookie["httponly"])
        self.assertEqual(cookie["samesite"], "Strict")
        self.cookie = "__Host-dashboard=" + cookie.value

    def test_totp_matches_rfc6238_vectors(self):
        key = b"12345678901234567890"
        self.assertEqual(totp.code_at(key, 59 // 30), "287082")
        self.assertEqual(totp.code_at(key, 1111111109 // 30), "081804")
        self.assertEqual(totp.match(key, "081804", now=1111111109 + 30), 1111111109 // 30)
        self.assertIsNone(totp.match(key, "081804", now=1111111109 + 90))
        self.assertIsNone(totp.match(key, "８１８０４", now=1111111109))

    def test_two_factor_is_opt_in_and_setting_is_validated(self):
        self.assertFalse(totp_required({}))
        self.assertFalse(totp_required({"GATEWAY_REQUIRE_TOTP": "false"}))
        self.assertTrue(totp_required({"GATEWAY_REQUIRE_TOTP": "true"}))
        with self.assertRaises(ValueError):
            totp_required({"GATEWAY_REQUIRE_TOTP": "typo"})
        for secret in ("", "not base32!", TOTP_SECRET):
            config = Config(origin=ORIGIN, username="admin", password_hash=PASSWORD_HASH,
                            cookie_key=Fernet.generate_key().decode(), viewer_token=VIEWER_TOKEN,
                            totp_secret=secret)
            self.assertFalse(config.require_totp)

    def test_enabled_two_factor_requires_valid_secret(self):
        for secret in ("", "not base32!", "JBSWY3DP"):
            with self.assertRaises(ValueError):
                Config(origin=ORIGIN, username="admin", password_hash=PASSWORD_HASH,
                       cookie_key=Fernet.generate_key().decode(), viewer_token=VIEWER_TOKEN,
                       totp_secret=secret, require_totp=True)

    async def test_password_only_login_and_error_messages(self):
        page = await self.client.get("/login", headers=self.headers())
        html = await page.text()
        self.assertNotIn('name="code"', html)
        self.assertNotIn("Authenticator code", html)
        for body, expected in [
                ({"username": "admin", "password": "wrong"}, 401),
                ({"username": "wrong", "password": PASSWORD}, 401),
                ({"username": "admin", "password": None}, 400),
                ({}, 400), ([], 400)]:
            response = await self.client.post("/auth/login", headers=self.headers(Origin=ORIGIN), json=body)
            self.assertEqual(response.status, expected)
            self.assertNotIn("authenticator", await response.text())
        # A retained secret must not silently re-enable the authenticator step.
        self.assertEqual(self.config.totp_secret, TOTP_SECRET)
        await self.login()

    async def test_password_only_gateway_starts_without_authenticator_setup(self):
        await self.client.close()
        self.config.totp_secret = ""
        self.client = TestClient(TestServer(create_app(self.config)))
        await self.client.start_server()
        await self.login()

    async def test_login_requires_fresh_authenticator_code(self):
        await self.client.close()
        self.config.require_totp = True
        self.client = TestClient(TestServer(create_app(self.config)))
        await self.client.start_server()
        page = await self.client.get("/login", headers=self.headers())
        self.assertIn('name="code"', await page.text())
        async def attempt(**body):
            return await self.client.post("/auth/login", headers=self.headers(Origin=ORIGIN),
                                          json={"username": "admin", "password": PASSWORD, **body})
        self.assertEqual((await attempt()).status, 401)
        self.assertEqual((await attempt(code="000000")).status, 401)
        self.assertEqual((await attempt(code=12345)).status, 400)
        code = self.next_code()
        response = await attempt(username="admin", password="wrong", code=code)
        self.assertEqual(response.status, 401)
        # A wrong password must not burn the code, but a used code cannot be replayed.
        await self.login(code=code)
        self.cookie = ""
        self.assertEqual((await attempt(code=code)).status, 401)
        self.now += 10
        self.assertEqual((await attempt(code=code)).status, 401)

    async def test_session_list_and_revocation(self):
        await self.login(user_agent="phone-browser")
        phone = self.cookie
        self.cookie = ""
        await self.login(user_agent="laptop-browser")
        laptop = self.cookie
        result = await self.client.get("/auth/sessions", headers=self.headers())
        sessions = (await result.json())["sessions"]
        self.assertEqual(len(sessions), 2)
        current = [s for s in sessions if s["current"]]
        self.assertEqual([s["user_agent"] for s in current], ["laptop-browser"])
        other = next(s for s in sessions if not s["current"])
        response = await self.client.post("/auth/sessions/revoke", json={"id": other["id"]},
                                          headers=self.headers(Origin="https://evil.test"))
        self.assertEqual(response.status, 403)
        anonymous = await self.client.post("/auth/sessions/revoke", json={"id": "others"},
                                           headers={"Host": "dashboard.test", "Origin": ORIGIN})
        self.assertEqual(anonymous.status, 401)
        response = await self.client.post("/auth/sessions/revoke", json={"id": other["id"]},
                                          headers=self.headers(Origin=ORIGIN))
        self.assertEqual(response.status, 200)
        self.cookie = phone
        self.assertEqual((await self.client.get("/api/ota/projects", headers=self.headers())).status, 401)
        self.cookie = laptop
        self.assertEqual((await self.client.get("/api/ota/projects", headers=self.headers())).status, 200)
        self.cookie = ""
        await self.login(user_agent="tablet-browser")
        tablet = self.cookie
        self.cookie = laptop
        response = await self.client.post("/auth/sessions/revoke", json={"id": "others"},
                                          headers=self.headers(Origin=ORIGIN))
        self.assertEqual((await response.json())["revoked"], 1)
        self.cookie = tablet
        self.assertEqual((await self.client.get("/auth/session", headers=self.headers())).status, 401)
        self.cookie = laptop
        result = await self.client.get("/auth/sessions", headers=self.headers())
        own = (await result.json())["sessions"]
        self.assertEqual(len(own), 1)
        response = await self.client.post("/auth/sessions/revoke", json={"id": own[0]["id"]},
                                          headers=self.headers(Origin=ORIGIN))
        self.assertEqual(response.status, 200)
        self.assertEqual((await self.client.get("/auth/session", headers=self.headers())).status, 401)

    async def test_anonymous_cannot_reach_either_backend(self):
        for method, path in [("GET", "/"), ("GET", "/sessions"), ("POST", "/upload"),
                             ("DELETE", "/reset-all"), ("GET", "/api/ota/devices"),
                             ("POST", "/api/ota/projects"), ("POST", "/api/ota/projects/id/build"),
                             ("POST", "/api/ota/devices/node-1/deploy"), ("GET", "/api/tinyml/status"),
                             ("GET", "/api/ota/firmware/x/download"), ("GET", "/api/ota/console/ws")]:
            response = await self.client.request(method, path, headers=self.headers())
            self.assertEqual(response.status, 401, path)
        response = await self.client.get("/", headers=self.headers(Accept="text/html"), allow_redirects=False)
        self.assertEqual(response.status, 302)
        self.assertEqual(response.headers["Location"], "/login")
        self.assertEqual(self.upstream_calls, [])

    async def test_viewer_key_cannot_bypass_login(self):
        result = await self.client.get("/api/tinyml/status", headers=self.headers(Authorization=f"Bearer {VIEWER_TOKEN}"))
        self.assertEqual(result.status, 401)

    async def test_login_csrf_and_throttling(self):
        response = await self.client.post("/auth/login", headers=self.headers(Origin="https://evil.test"), json={})
        self.assertEqual(response.status, 403)
        for _ in range(10):
            response = await self.client.post("/auth/login", headers=self.headers(Origin=ORIGIN),
                                              json={"username": "admin", "password": "wrong"})
            self.assertEqual(response.status, 401)
        response = await self.client.post("/auth/login", headers=self.headers(Origin=ORIGIN), json={})
        self.assertEqual(response.status, 429)
        self.assertEqual(response.headers["Retry-After"], "60")

    async def test_authenticated_proxy_and_server_only_viewer_token(self):
        await self.login()
        result = await self.client.get("/app-config.js", headers=self.headers())
        config = await result.text()
        self.assertIn(ORIGIN, config)
        self.assertNotIn(VIEWER_TOKEN, config)
        result = await self.client.get("/api/tinyml/status", headers=self.headers())
        body = await result.json()
        self.assertEqual(body["authorization"], f"Bearer {VIEWER_TOKEN}")
        self.assertEqual(body["cookie"], "")
        response = await self.client.post("/upload", headers=self.headers(Origin="https://evil.test"))
        self.assertEqual(response.status, 403)
        response = await self.client.post("/upload", headers=self.headers())
        self.assertEqual(response.status, 403)
        response = await self.client.post("/upload", headers=self.headers(Origin=ORIGIN), data=b"csv")
        self.assertEqual(response.status, 200)

    async def test_logout_revokes_replayed_cookie(self):
        await self.login()
        result = await self.client.post("/auth/logout", headers=self.headers(Origin=ORIGIN))
        self.assertEqual(result.status, 200)
        result = await self.client.get("/api/ota/projects", headers=self.headers())
        self.assertEqual(result.status, 401)

    async def test_session_expiration(self):
        self.config.session_seconds = 0.05
        await self.login()
        await asyncio.sleep(0.08)
        result = await self.client.get("/auth/session", headers=self.headers())
        self.assertEqual(result.status, 401)

    async def test_device_permissions(self):
        headers = self.headers(Authorization=f"Bearer {DEVICE_TOKEN}")
        for method, path in [("GET", "/api/ota/devices/node-1/update"),
                             ("POST", "/api/ota/devices/node-1/heartbeat"),
                             ("POST", "/api/ota/devices/node-1/ota-status")]:
            response = await self.client.request(method, path, headers=headers, json={})
            self.assertEqual(response.status, 200, path)
        for method, path in [("GET", "/api/ota/devices/node-2/update"),
                             ("DELETE", "/api/ota/devices/node-1/update"),
                             ("POST", "/api/ota/devices/node-1/deploy"),
                             ("POST", "/api/ota/projects"), ("GET", "/sessions"),
                             ("GET", "/api/tinyml/status"),
                             ("GET", "/api/ota/firmware/other-image/download")]:
            response = await self.client.request(method, path, headers=headers)
            self.assertEqual(response.status, 403, path)
        for device_id, expected in [("node-1", 200), ("node-2", 403)]:
            response = await self.client.post("/api/ota/devices/register", headers=headers, json={"device_id": device_id})
            self.assertEqual(response.status, expected)
        response = await self.client.get("/api/ota/firmware/assigned-image/download", headers=headers)
        self.assertEqual(await response.read(), self.image)

    async def test_streaming_upload_download_and_csv_events(self):
        await self.login()
        async def chunks():
            for _ in range(40):
                yield b"x" * 32768
        result = await self.client.post("/api/ota/projects", headers=self.headers(Origin=ORIGIN), data=chunks())
        self.assertEqual(result.status, 200)
        self.assertEqual((await result.json())["bytes"], 40 * 32768)
        result = await self.client.get("/api/ota/firmware/id/download", headers=self.headers())
        self.assertEqual(await result.read(), self.image)
        result = await self.client.get("/sse", headers=self.headers())
        self.assertEqual(await result.read(), b"data: hello\n\n")

    async def test_websockets_use_session_and_close_on_logout(self):
        await self.login()
        for path in ("/api/ota/console/ws", "/api/tinyml/ws"):
            socket = await self.client.ws_connect(path, headers=self.headers(Origin=ORIGIN))
            await socket.send_json({"token": "browser-session", "device_id": "node-1"})
            self.assertEqual((await socket.receive_json())["type"], "connected")
            self.assertEqual(self.ws_auth[-1]["token"], VIEWER_TOKEN)
            await self.client.post("/auth/logout", headers=self.headers(Origin=ORIGIN))
            self.assertEqual((await socket.receive(timeout=2)).type, WSMsgType.CLOSE)
            await socket.close()
            self.cookie = ""
            await self.login()

    async def test_encoded_path_and_host_cannot_bypass_boundary(self):
        result = await self.client.get("/api/ota/%252e%252e/projects", headers=self.headers())
        self.assertEqual(result.status, 400)
        result = await self.client.get("/auth/health", headers={"Host": "evil.test"})
        self.assertEqual(result.status, 400)


if __name__ == "__main__":
    unittest.main()
