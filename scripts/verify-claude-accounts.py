#!/usr/bin/env python3
"""在独立 PostgreSQL schema 中通过真实 HTTP 验证 Claude 账号管理。"""

import http.cookiejar
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import runpy
import signal
import socket
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.parse
import urllib.request
import uuid


ROOT = Path(__file__).resolve().parent.parent
DEV = runpy.run_path(str(ROOT / "scripts/dev"))


class AnthropicStub(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.server.observed.append((self.path, self.headers.get("x-api-key"), self.headers.get("anthropic-version")))
        key = self.headers.get("x-api-key", "")
        if key == "sk-ant-timeout":
            time.sleep(12)
        if key == "sk-ant-redirect":
            self.send_response(302)
            self.send_header("Location", "/must-not-receive-key")
            self.end_headers()
            return
        payload = {"data": [{"type": "model", "id": "claude-test-model", "display_name": "Claude test model"}], "has_more": False}
        if key == "sk-ant-denied":
            payload = {"error": {"type": "authentication_error", "message": "Invalid key " + key}}
        body = json.dumps(payload).encode()
        self.send_response(401 if key == "sk-ant-denied" else 200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass


class ClaudeHTTPFixture(unittest.TestCase):
    upstream_handler = AnthropicStub
    request_timeout_seconds = None
    test_admin_password = None

    @classmethod
    def setUpClass(cls):
        cls.connections = DEV["read_env"](ROOT / "connections.env")
        cls.env = DEV["environment"]()
        cls.schema = "claude_accounts_" + uuid.uuid4().hex
        database = urllib.parse.urlsplit(cls.connections["TEST_DATABASE_URL"])
        cls.pg_env = {**cls.env, "PGPASSWORD": urllib.parse.unquote(database.password or "")}
        cls.pg_url = DEV["without_password"](cls.connections["TEST_DATABASE_URL"])
        cls.sql(f'CREATE SCHEMA "{cls.schema}"')
        cls.addClassCleanup(lambda: cls.sql(f'DROP SCHEMA "{cls.schema}" CASCADE'))
        cls.temp = tempfile.TemporaryDirectory(prefix="claude-account-http-")
        cls.addClassCleanup(cls.temp.cleanup)
        temp = Path(cls.temp.name)
        (temp / "deploy").mkdir()
        config = temp / "deploy/config.yaml"
        config_text = (ROOT / "deploy/config.yaml").read_text()
        if cls.test_admin_password:
            import re
            config_text = re.sub(r"default_password:.*", "default_password: " + json.dumps(cls.test_admin_password), config_text)
            cls.connections["ADMIN_PASSWORD"] = cls.test_admin_password
        if cls.request_timeout_seconds is not None:
            import re
            config_text = re.sub(r"request_timeout_seconds:.*", f"request_timeout_seconds: {cls.request_timeout_seconds}", config_text)
        DEV["private_write"](config, config_text)
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        cls.base = f"http://127.0.0.1:{port}"
        cls.env.update({
            "CPR_SERVER_HOST": "127.0.0.1", "CPR_SERVER_PORT": str(port),
            "CPR_DATABASE_URL": cls.pg_url + "?" + urllib.parse.urlencode({"options": f"-csearch_path={cls.schema}"}),
            "CPR_REDIS_URL": DEV["without_password"](cls.connections["TEST_REDIS_URL"]),
            "CPR_WEB_DIST_DIR": str(ROOT / "frontend/dist"),
        })
        # 显式分配独立逻辑库时，允许与使用 DB15 的 Rust Store 回归隔离。
        redis_database = os.environ.get("CPR_HTTP_TEST_REDIS_DATABASE")
        if redis_database is not None:
            if redis_database not in {"14", "15"}:
                raise ValueError("HTTP 验收只允许使用测试 Redis DB14 或 DB15")
            redis_url = urllib.parse.urlsplit(cls.env["CPR_REDIS_URL"])
            cls.env["CPR_REDIS_URL"] = urllib.parse.urlunsplit(redis_url._replace(path="/" + redis_database))
        cls.upstream = ThreadingHTTPServer(("127.0.0.1", 0), cls.upstream_handler)
        cls.upstream.observed = []
        cls.upstream_url = f"http://127.0.0.1:{cls.upstream.server_port}"
        threading.Thread(target=cls.upstream.serve_forever, daemon=True).start()
        cls.addClassCleanup(cls.upstream.server_close)
        cls.addClassCleanup(cls.upstream.shutdown)
        cls.env.update(cls.upstream_environment())
        cls.log = (temp / "server.log").open("wb")
        cls.addClassCleanup(cls.log.close)
        cls.process = subprocess.Popen(
            [str(ROOT / "backend/target/debug/claude-code-proxy-rs")],
            cwd=temp, env=cls.env, stdout=cls.log, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        cls.addClassCleanup(cls.stop_server)
        cls.client = urllib.request.build_opener(
            urllib.request.ProxyHandler({}),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
        )
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            try:
                if cls.request("/healthz")[0] == 204:
                    break
            except (urllib.error.URLError, TimeoutError):
                pass
            if cls.process.poll() is not None:
                raise RuntimeError("隔离测试服务启动失败；请检查本地应用配置")
            time.sleep(0.1)
        else:
            raise RuntimeError("隔离测试服务未就绪")
        status, _ = cls.request("/api/auth/login", {
            "mode": "admin", "username": cls.connections["ADMIN_USERNAME"],
            "password": cls.connections["ADMIN_PASSWORD"],
        })
        if status != 200:
            raise RuntimeError("隔离测试服务登录失败")

    @classmethod
    def upstream_environment(cls):
        return {}

    @classmethod
    def sql(cls, statement):
        subprocess.run(["psql", cls.pg_url, "-v", "ON_ERROR_STOP=1", "-c", statement],
                       env=cls.pg_env, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    @classmethod
    def stop_server(cls):
        if cls.process.poll() is None:
            os.killpg(cls.process.pid, signal.SIGINT)
            try:
                cls.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(cls.process.pid, signal.SIGKILL)
                cls.process.wait()

    @classmethod
    def request(cls, path, payload=None):
        body = None if payload is None else json.dumps(payload).encode()
        request = urllib.request.Request(cls.base + path, data=body, headers={
            "Origin": cls.base, "Content-Type": "application/json",
        })
        try:
            response = cls.client.open(request, timeout=40)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            data = response.read()
            return response.status, json.loads(data) if data else None

class ClaudeAccountTests(ClaudeHTTPFixture):
    def test_api_key_import_is_no_longer_supported(self):
        status, result = self.request("/api/admin/accounts/import", {
            "provider":"claude", "data":{"name":"Retired API Key", "api_key":"sk-ant-legacy-test", "base_url":"https://api.anthropic.com", "transport":"http"},
        })
        self.assertEqual(status,400,result)


if __name__ == "__main__":
    unittest.main(verbosity=2)
