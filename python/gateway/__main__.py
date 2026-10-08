from aiohttp import web
from .config import load_config
from .server import create_app

if __name__ == "__main__":
    try:
        config = load_config()
        application = create_app(config)
    except (ValueError, KeyError) as error:
        raise SystemExit(f"Gateway configuration error: {error}") from None
    print(f"Authenticated dashboard: {config.origin}", flush=True)
    # Loopback only. Serve/Funnel is the sole public entry point; no LAN bypass.
    # Access logs are disabled so paths/query strings cannot leak credentials.
    web.run_app(application, host="127.0.0.1", port=8088, access_log=None,
                handler_cancellation=True)
