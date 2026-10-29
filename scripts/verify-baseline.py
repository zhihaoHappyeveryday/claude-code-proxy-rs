#!/usr/bin/env python3
"""通过运行中的 HTTP 服务验收工程基线，不访问内部实现或数据库表。"""

import http.cookiejar
import json
import os
from pathlib import Path
import unittest
import urllib.error
import urllib.request


class BaselineTests(unittest.TestCase):
    def setUp(self):
        self.base = os.environ.get("CC_PROXY_BASE_URL", "http://127.0.0.1:8080").rstrip("/")
        self.cookies = http.cookiejar.CookieJar()
        self.client = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), urllib.request.HTTPCookieProcessor(self.cookies)
        )

    def request(self, path, payload=None, cookie=None):
        body = None if payload is None else json.dumps(payload).encode()
        headers = {"Origin": self.base, "Content-Type": "application/json"}
        client = self.client
        if cookie is not None:
            headers["Cookie"] = cookie
            # 独立客户端重放原始凭据，避免 CookieJar 清空造成注销验证假阳性。
            client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        request = urllib.request.Request(
            self.base + path, data=body,
            headers=headers,
        )
        try:
            response = client.open(request, timeout=10)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            data = response.read()
            return response.status, json.loads(data) if data else None

    def test_health(self):
        """启动完成的服务对外报告就绪。"""
        status, _ = self.request("/healthz")
        self.assertEqual(status, 204)

    def test_unsupported_provider_endpoints_are_absent(self):
        """Claude 产品不发布继承的 Codex、OpenAI 图像与实时协议入口。"""
        for path in ("/v1/responses", "/v1/chat/completions", "/v1/images/generations", "/v1/realtime"):
            with self.subTest(path=path):
                self.assertEqual(self.request(path, {})[0], 404)
        self.assertEqual(self.request("/v1/models")[0], 404)
        for path in ("/api/admin/settings/client-profiles/openai", "/api/admin/settings/client-profiles/xai",
                     "/api/admin/settings/client-downloads/codex-desktop/windows", "/api/admin/settings/pricing"):
            with self.subTest(path=path):
                self.assertEqual(self.request(path)[0], 404)
        for path in ("/api/admin/settings/pricing/update", "/api/admin/settings/pricing/sync/preview",
                     "/api/admin/settings/pricing/sync"):
            with self.subTest(path=path):
                self.assertEqual(self.request(path, {})[0], 404)

    def test_admin_login_accounts_and_logout(self):
        """账号列表要求有效管理员会话，退出后不能继续访问。"""
        path = Path(__file__).resolve().parent.parent / "connections.env"
        credentials = dict(line.split("=", 1) for line in path.read_text().splitlines() if "=" in line)
        username = os.environ.get("CC_PROXY_ADMIN_USERNAME", credentials["ADMIN_USERNAME"])
        password = os.environ.get("CC_PROXY_ADMIN_PASSWORD", credentials["ADMIN_PASSWORD"])
        accounts = "/api/admin/accounts"
        self.assertEqual(self.request(accounts)[0], 401)
        self.assertEqual(self.request("/api/auth/login", {
            "mode": "admin", "username": username, "password": "incorrect-baseline-password",
        })[0], 401)
        self.assertEqual(self.request(accounts)[0], 401)
        status, data = self.request("/api/auth/login", {
            "mode": "admin", "username": username, "password": password,
        })
        self.assertEqual(status, 200)
        self.assertEqual(data["data"]["role"], "admin")
        session = next(cookie for cookie in self.cookies if cookie.name == "cpr_session")
        self.assertTrue(session.has_nonstandard_attr("HttpOnly"))
        stale_cookies = "; ".join(
            f"{cookie.name}={cookie.value}" for cookie in self.cookies
            if cookie.name in ("cpr_session", "cpr_refresh")
        )
        status, data = self.request(accounts)
        self.assertEqual(status, 200)
        self.assertIsInstance(data["data"]["items"], list)
        status, data = self.request("/api/auth/status")
        self.assertEqual(status, 200)
        self.assertTrue(data["data"]["authenticated"])
        self.assertEqual(self.request("/api/auth/logout", {})[0], 200)
        self.assertEqual(self.request(accounts)[0], 401)
        self.assertFalse(self.request("/api/auth/status")[1]["data"]["authenticated"])
        self.assertEqual(self.request(accounts, cookie=stale_cookies)[0], 401)
        self.assertFalse(self.request("/api/auth/status", cookie=stale_cookies)[1]["data"]["authenticated"])
        self.assertFalse(self.request("/api/auth/refresh", {}, cookie=stale_cookies)[1]["data"]["authenticated"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
