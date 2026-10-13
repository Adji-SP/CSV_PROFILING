"""Public entry point. Flask and Axum MUST both bind only to loopback."""
import asyncio
import contextlib
import re
from pathlib import Path
from urllib.parse import urlsplit

import aiohttp
from aiohttp import web
from aiohttp_session import setup
from aiohttp_session.cookie_storage import EncryptedCookieStorage

from .auth import Auth, error
from .config import Config
from .proxy import Proxy, client_context

ASSETS = Path(__file__).parent / "static"


def create_app(config: Config) -> web.Application:
    auth = Auth(config)
    proxy = Proxy(config, auth)
    authenticator_field = (ASSETS / "authenticator.html").read_text(encoding="utf-8") if config.require_totp else ""
    login_html = (ASSETS / "login.html").read_text(encoding="utf-8").replace(
        "<!-- AUTHENTICATOR_FIELD -->", authenticator_field)

    @web.middleware
    async def boundary(request, handler):
        # Never trust client-controlled X-Forwarded-* or identity headers.
        allowed_hosts = {urlsplit(config.origin).netloc, "127.0.0.1:8088", "localhost:8088"}
        if request.host not in allowed_hosts:
            return error(400, "Unexpected hostname.")
        raw_path = request.raw_path.split("?", 1)[0]
        if (re.search(r"%(?:2f|5c|2e|25|00)", raw_path, re.I)
                or "\\" in raw_path or "//" in raw_path
                or any(part in {".", ".."} for part in raw_path.split("/"))):
            return error(400, "Invalid URL path.")
        try:
            return await handler(request)
        except web.HTTPException as exc:
            if 300 <= exc.status < 400:
                raise
            return error(exc.status, exc.reason)
        except (aiohttp.ClientError, asyncio.TimeoutError):
            return error(502, "The local service is unavailable. Try again shortly.")

    app = web.Application(middlewares=[boundary], client_max_size=config.max_body,
                          handler_args={"auto_decompress": False})
    setup(app, EncryptedCookieStorage(
        config.cookie_key, cookie_name="__Host-dashboard", max_age=config.session_seconds,
        secure=True, httponly=True, samesite="Strict", path="/",
    ))

    async def security_headers(request, response):
        response.headers["Cache-Control"] = "no-store"
        response.headers["X-Content-Type-Options"] = "nosniff"
        response.headers["Referrer-Policy"] = "no-referrer"
        response.headers["X-Frame-Options"] = "SAMEORIGIN"
        response.headers["Content-Security-Policy"] = "frame-ancestors 'self'; base-uri 'self'; object-src 'none'"
        response.headers["Strict-Transport-Security"] = "max-age=31536000"
        response.headers["Server"] = "dashboard"

    async def auth_context(app):
        task = asyncio.create_task(auth.cleanup())
        yield
        task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await task
        for session in tuple(auth.sessions.values()):
            await auth.close_sockets(session)

    async def login_page(request):
        if await auth.browser(request):
            raise web.HTTPFound("/")
        return web.Response(text=login_html, content_type="text/html")

    async def session_info(request):
        if not await auth.browser(request):
            return error(401, "Log in to continue.")
        return web.json_response({"username": config.username})

    async def health(request):
        return web.json_response({"service": "dashboard-gateway", "authentication": "required"})

    async def login_css(request):
        return web.FileResponse(ASSETS / "login.css")

    async def login_js(request):
        return web.FileResponse(ASSETS / "login.js")

    app.on_response_prepare.append(security_headers)
    app.cleanup_ctx.extend([client_context, auth_context])
    app.router.add_get("/login", login_page)
    app.router.add_get("/gateway-login.css", login_css)
    app.router.add_get("/gateway-login.js", login_js)
    app.router.add_post("/auth/login", auth.login)
    app.router.add_post("/auth/logout", auth.logout)
    app.router.add_get("/auth/session", session_info)
    app.router.add_get("/auth/sessions", auth.list_sessions)
    app.router.add_post("/auth/sessions/revoke", auth.revoke_session)
    app.router.add_get("/auth/health", health)
    app.router.add_route("*", "/{path:.*}", proxy.dispatch)
    return app
