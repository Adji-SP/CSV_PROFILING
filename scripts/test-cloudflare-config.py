"""Loopback-free tests for the browser URL selection in the Flask frontend."""

import json
import os
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "python"))
from profiler_server import app  # noqa: E402


def config(response):
    prefix = "window.APP_CONFIG = Object.freeze("
    body = response.get_data(as_text=True)
    assert body.startswith(prefix) and body.endswith(");\n")
    return json.loads(body[len(prefix):-3])


class BrowserConfigTests(unittest.TestCase):
    def test_default_ports_match_launchers(self):
        for port, api in (("5173", "http://localhost:18080"), ("5000", "http://localhost:7000")):
            with patch.dict(os.environ, {"CSV_PORT": port}, clear=True):
                response = app.test_client().get("/app-config.js", base_url=f"http://localhost:{port}")
                self.assertEqual(config(response)["otaApiBase"], api)

    def test_public_and_local_select_different_api_origins(self):
        settings = {
            "CSV_PORT": "5173",
            "APP_PUBLIC_FRONTEND_HOST": "dev.iot-prototype.net",
            "APP_LOCAL_OTA_API_BASE_URL": "http://localhost:8080",
            "APP_PUBLIC_OTA_API_BASE_URL": "https://api-dev.iot-prototype.net",
            "CONSOLE_VIEWER_TOKEN": "test-secret-must-not-be-sent",
        }
        with patch.dict(os.environ, settings):
            client = app.test_client()
            local = client.get("/app-config.js", base_url="http://localhost:5173")
            public = client.get("/app-config.js", base_url="https://dev.iot-prototype.net")
            self.assertEqual(config(local)["otaApiBase"], "http://localhost:8080")
            self.assertEqual(config(public)["otaApiBase"], "https://api-dev.iot-prototype.net")
            self.assertNotIn("test-secret", public.get_data(as_text=True))
            self.assertEqual(public.headers["Cache-Control"], "no-store")
            with client.get("/", base_url="http://localhost:5173") as index:
                self.assertEqual(index.status_code, 200)


if __name__ == "__main__":
    unittest.main()
