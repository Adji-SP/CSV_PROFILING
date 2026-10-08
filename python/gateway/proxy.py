"""Streaming HTTP and authenticated WebSocket forwarding to loopback services."""
import asyncio
import json
import re
import time

import aiohttp
from aiohttp import web, WSMsgType
from yarl import URL

from .auth import Auth, LoginSession, error
from .config import Config

CLIENT = web.AppKey("upstream-client", aiohttp.ClientSession)
VALIDATED_BODY = web.RequestKey("validated-body", bytes)
DEVICE_ROUTE = re.compile(r"/api/ota/devices/([A-Za-z0-9_-]{1,128})/(heartbeat|update|ota-status)\Z")
DOWNLOAD = re.compile(r"/api/ota/firmware/([A-Za-z0-9_-]{1,128})/download\Z")
WS_PATHS = {"/api/ota/console/ws", "/api/tinyml/ws"}
REQUEST_HEADERS = {"accept", "content-type", "content-length", "range", "if-range", "last-event-id"}
RESPONSE_HEADERS = {"content-type", "content-length", "content-disposition", "content-encoding",
                    "content-range", "accept-ranges", "etag", "last-modified"}


class Proxy:
    def __init__(self, config: Config, auth: Auth):
        self.config, self.auth = config, auth

    async def device_allowed(self, request: web.Request, device_id: str) -> bool:
        match = DEVICE_ROUTE.fullmatch(request.path)
        if match:
            expected = "GET" if match[2] == "update" else "POST"
            return match[1] == device_id and request.method == expected
        if request.path == "/api/ota/devices/register" and request.method == "POST":
            try:
                body = bytearray()
                async for chunk in request.content.iter_chunked(1024):
                    body.extend(chunk)
                    if len(body) > 4096:
                        return False
                request[VALIDATED_BODY] = bytes(body)
                value = json.loads(body)
                return isinstance(value, dict) and value.get("device_id") == device_id
            except (ValueError, web.HTTPException):
                return False
        download = DOWNLOAD.fullmatch(request.path)
        if download and request.method in {"GET", "HEAD"}:
            # A device may download only its currently assigned image.
            url = f"{self.config.ota_origin}/api/ota/devices/{device_id}/update"
            async with request.app[CLIENT].get(url, allow_redirects=False) as response:
                if response.status != 200:
                    return False
                manifest = await response.json()
                return (manifest.get("update_available") is True
                        and manifest.get("firmware_id") == download[1])
        return False

    async def dispatch(self, request: web.Request) -> web.StreamResponse:
        browser = await self.auth.browser(request)
        device_id = None if browser else self.auth.device(request)
        if not browser and not device_id:
            if request.method == "GET" and "text/html" in request.headers.get("Accept", ""):
                raise web.HTTPFound("/login")
            return error(401, "Log in to continue.")
        if request.headers.get("Content-Encoding", "identity").lower() != "identity":
            return error(415, "Compressed request bodies are not supported.")
        if request.content_length and request.content_length > self.config.max_body:
            return error(413, "Upload exceeds the configured size limit.")
        if device_id:
            if not await self.device_allowed(request, device_id):
                return error(403, "This device credential cannot access that resource.")
        elif (request.method not in {"GET", "HEAD", "OPTIONS"}
              or request.path in WS_PATHS) and not self.auth.same_origin(request):
            return error(403, "Invalid request origin.")
        if request.path == "/app-config.js":
            payload = {"otaApiBase": self.config.origin, "loginSession": True}
            return web.Response(text=f"window.APP_CONFIG = Object.freeze({json.dumps(payload)});\n",
                                content_type="text/javascript")
        if request.headers.get("Upgrade", "").lower() == "websocket":
            if not browser or request.path not in WS_PATHS:
                return error(403, "WebSocket access is not allowed here.")
            return await self.websocket(request, browser)
        return await self.http(request)

    async def http(self, request: web.Request) -> web.StreamResponse:
        is_ota = request.path == "/health" or request.path.startswith(("/api/ota/", "/api/tinyml/"))
        upstream = self.config.ota_origin if is_ota else self.config.csv_origin
        # Only an explicitly configured loopback host can be contacted, never a client URL.
        url = URL(upstream + request.raw_path, encoded=True)
        headers = {k: v for k, v in request.headers.items() if k.lower() in REQUEST_HEADERS}
        headers["Host"] = URL(self.config.origin).authority
        if request.path.startswith(("/api/ota/console/", "/api/tinyml/")):
            headers["Authorization"] = f"Bearer {self.config.viewer_token}"
        if self.auth.same_origin(request):
            headers["Origin"] = self.config.origin

        async def body():
            total = 0
            if VALIDATED_BODY in request:
                yield request[VALIDATED_BODY]
                return
            async for chunk in request.content.iter_chunked(65536):
                total += len(chunk)
                if total > self.config.max_body:
                    raise web.HTTPRequestEntityTooLarge(max_size=self.config.max_body, actual_size=total)
                yield chunk

        async with request.app[CLIENT].request(
            request.method, url, headers=headers, data=body() if request.can_read_body or VALIDATED_BODY in request else None,
            allow_redirects=False,
        ) as source:
            # Do not relay an upstream redirect to a loopback address or a third-party host.
            if 300 <= source.status < 400 and "Location" in source.headers:
                return error(502, "Unexpected redirect from the local service.")
            response = web.StreamResponse(status=source.status, headers={
                k: v for k, v in source.headers.items() if k.lower() in RESPONSE_HEADERS})
            await response.prepare(request)
            async for chunk in source.content.iter_chunked(65536):
                await response.write(chunk)
            await response.write_eof()
            return response

    async def websocket(self, request: web.Request, session: LoginSession):
        if len(session.sockets) >= 8:
            return error(429, "Too many monitoring connections in this session.")
        source = await request.app[CLIENT].ws_connect(
            self.config.ota_origin + request.path, origin=self.config.origin,
            max_msg_size=1024 * 1024, heartbeat=30,
        )
        socket = web.WebSocketResponse(max_msg_size=8192, heartbeat=30)
        await socket.prepare(request)
        session.sockets.add(socket)
        tasks = []
        try:
            # Existing clients send a first auth frame. Replace the viewer credential
            # on the server; it is never returned to the browser.
            first = await asyncio.wait_for(socket.receive(), timeout=8)
            if first.type != WSMsgType.TEXT:
                return socket
            data = json.loads(first.data)
            if not isinstance(data, dict):
                return socket
            auth_frame = {"token": self.config.viewer_token}
            if request.path == "/api/ota/console/ws":
                device_id = data.get("device_id", "")
                if not isinstance(device_id, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", device_id):
                    return socket
                auth_frame["device_id"] = device_id
            await source.send_json(auth_frame)

            async def relay_from_upstream():
                async for message in source:
                    if message.type == WSMsgType.TEXT:
                        await socket.send_str(message.data)
                    elif message.type == WSMsgType.BINARY:
                        await socket.send_bytes(message.data)

            async def wait_for_browser_close():
                # Both monitoring protocols are read-only after the initial frame.
                async for _ in socket:
                    pass

            tasks = [asyncio.create_task(relay_from_upstream()), asyncio.create_task(wait_for_browser_close())]
            await asyncio.wait(tasks, timeout=max(0, session.expires - time.monotonic()),
                               return_when=asyncio.FIRST_COMPLETED)
        except (ValueError, asyncio.TimeoutError, ConnectionError):
            pass
        finally:
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
            session.sockets.discard(socket)
            await source.close()
            await socket.close()
        return socket


async def client_context(app):
    async with aiohttp.ClientSession(
        timeout=aiohttp.ClientTimeout(total=None, connect=10, sock_read=180),
        auto_decompress=False, cookie_jar=aiohttp.DummyCookieJar(),
        connector=aiohttp.TCPConnector(limit=64), trust_env=False,
    ) as client:
        app[CLIENT] = client
        yield
