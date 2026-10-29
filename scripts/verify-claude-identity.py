#!/usr/bin/env python3
"""通过管理与推理 HTTP 接口验收 Claude Code 客户端身份；仅使用本地上游。"""
from pathlib import Path
import runpy
import json
import time
import unittest
import urllib.request
import urllib.error
import subprocess

MESSAGES = runpy.run_path(str(Path(__file__).with_name("verify-claude-messages.py")))


class ClaudeIdentityTests(MESSAGES["ClaudeMessagesFixture"]):
    def test_invalid_configuration_is_rejected_without_changing_saved_profile(self):
        status, options = self.request("/api/admin/settings/client-profiles/claude")
        self.assertEqual(status, 200)
        good = options["data"]["defaults"]
        self.assertEqual(self.save_identity(good)[0], 200)
        for change in ({"cliVersion": "2.1.312\r\nx-api-key: injected"}, {"cliVersion": "latest"},
                       {"nodeVersion": "v24.3.0"}, {"sdkVersion": "00.94.0"}, {"os": "Plan9"},
                       {"arch": "unknown"}, {"authorization": "Bearer not-allowed"}, {"cliVersion": None}):
            with self.subTest(fields=list(change)):
                invalid = {**good, "httpIdentity": {**good["httpIdentity"], **change}}
                status, _ = self.request("/api/admin/settings/client-profiles/claude/preview", {"configuration": invalid})
                self.assertEqual(status, 400)
                status, _ = self.save_identity(invalid)
                self.assertEqual(status, 400)
        status, options = self.request("/api/admin/settings/client-profiles/claude")
        self.assertEqual(status, 200)
        self.assertEqual(options["data"]["globalConfiguration"], good)

    def test_identity_endpoints_require_admin_session(self):
        client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        for path, payload in (("/api/admin/settings/client-profiles/claude", None),
                              ("/api/admin/settings/client-profiles/claude/preview", b'{"configuration":null}')):
            req = urllib.request.Request(self.base + path, data=payload,
                                         headers={"Content-Type": "application/json", "Origin": self.base})
            with self.assertRaises(urllib.error.HTTPError) as error:
                client.open(req, timeout=5)
            self.assertEqual(error.exception.code, 401)
            error.exception.close()

    def test_saved_identity_survives_restart_and_reset_restores_default_wire_profile(self):
        configuration = {"cliVersion": "2.1.250", "sdkVersion": "0.93.0", "os": "MacOS", "arch": "x64", "nodeVersion": "22.18.0"}
        self.assertEqual(self.save_identity(configuration)[0], 200)
        cls = type(self)
        cls.stop_server()
        cls.process = subprocess.Popen([str(MESSAGES["FIXTURE"]["ROOT"] / "backend/target/debug/claude-code-proxy-rs")],
                                       cwd=cls.temp.name, env=cls.env, stdout=cls.log, stderr=subprocess.STDOUT, start_new_session=True)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            try:
                if self.request("/healthz")[0] == 204:
                    break
            except (urllib.error.URLError, ConnectionError):
                pass
            time.sleep(0.1)
        else:
            self.fail("重启的测试服务未就绪")
        status, _ = self.request("/api/auth/login", {"mode": "admin", "username": self.connections["ADMIN_USERNAME"], "password": self.connections["ADMIN_PASSWORD"]})
        self.assertEqual(status, 200)
        status, options = self.request("/api/admin/settings/client-profiles/claude")
        self.assertEqual(status, 200)
        self.assertEqual(options["data"]["globalConfiguration"]["httpIdentity"], configuration)
        status, result = self.request(f"/api/admin/dashboard/summary?period=today&kind=usage&asOf={int(time.time()*1000)}")
        self.assertEqual(status, 200, result)
        profile = next(p for p in result["data"]["wireProfiles"] if p["provider"] == "claude")
        self.assertEqual(profile["userAgent"], "claude-cli/2.1.250 (external, sdk-cli)")
        self.assertEqual(self.save_identity(None)[0], 200)
        with self.messages({"model": "claude-test-model", "max_tokens": 8, "messages": [{"role": "user", "content": "reset"}]},
                           {"x-api-key": self.key, "User-Agent": "claude-cli/bad (external, cli)"}) as response:
            self.assertEqual(response.status, 200, response.read())
        wire = {k.lower(): v for k, v in self.upstream.observed[-1][1].items()}
        self.assertEqual(wire["user-agent"], "claude-cli/2.1.312 (external, sdk-cli)")
        self.assertEqual(wire["x-stainless-os"], "MacOS")
        self.assertNotIn("x-stainless-helper-method", wire)
        self.assertTrue(wire["x-client-request-id"])

    def test_settings_changes_during_sse_apply_only_to_later_requests(self):
        body = {"model": "claude-test-model", "max_tokens": 8, "stream": True, "messages": [{"role": "user", "content": "hello"}]}
        with self.messages(body, {"x-api-key": self.key, "User-Agent": "other-client"}) as response:
            first = response.readline()
            self.assertEqual(first, b": keep-alive\n")
            configuration = {"cliVersion": "2.1.250", "sdkVersion": "0.93.0", "os": "MacOS", "arch": "x64", "nodeVersion": "22.18.0"}
            self.assertEqual(self.save_identity(configuration)[0], 200)
            self.assertEqual(first + response.read(), self.upstream.stream_bytes)
        earlier = {k.lower(): v for k, v in self.upstream.observed[-1][1].items()}
        self.assertEqual(earlier["user-agent"], "claude-cli/2.1.312 (external, sdk-cli)")
        body["stream"] = False
        with self.messages(body, {"x-api-key": self.key, "User-Agent": "other-client"}) as response:
            self.assertEqual(response.status, 200, response.read())
        later = {k.lower(): v for k, v in self.upstream.observed[-1][1].items()}
        self.assertEqual(later["user-agent"], "claude-cli/2.1.250 (external, sdk-cli)")

    def test_native_sse_keeps_client_identity_body_and_capabilities(self):
        body = {"model": "claude-test-model", "max_tokens": 8, "stream": True,
                "system": [{"type": "text", "text": "x-anthropic-billing-header: cc_version=2.1.200.abc; cch=opaque-signature;"}],
                "metadata": {"user_id": "original-device-and-session"},
                "messages": [{"role": "user", "content": "hello"}]}
        headers = {"x-api-key": self.key, "User-Agent": "claude-cli/2.1.200 (external, cli)",
                   "x-stainless-package-version": "0.80.0", "x-stainless-os": "Windows",
                   "x-stainless-runtime-version": "v20.10.0", "x-app": "cli",
                   "x-client-request-id": "native-request-fixture", "anthropic-beta": "future-test-2026-10-29,oauth-2025-04-20"}
        with self.messages(body, headers) as response:
            self.assertEqual(response.status, 200)
            self.assertEqual(response.read(), self.upstream.stream_bytes)
        _, forwarded_headers, forwarded_body = self.upstream.observed[-1]
        wire = {k.lower(): v for k, v in forwarded_headers.items()}
        self.assertEqual(wire["user-agent"], headers["User-Agent"])
        self.assertEqual(wire["x-stainless-os"], "Windows")
        self.assertEqual(wire["x-stainless-package-version"], "0.80.0")
        self.assertEqual(wire["x-stainless-runtime-version"], "v20.10.0")
        self.assertNotIn("x-stainless-arch", wire, "原生客户端缺失的身份字段不能混入默认环境")
        self.assertEqual(wire["x-app"], "cli")
        self.assertEqual(wire["x-client-request-id"], "native-request-fixture")
        self.assertEqual(wire["x-stainless-helper-method"], "stream")
        self.assertEqual(wire["accept"], "application/json")
        self.assertEqual(set(wire["anthropic-beta"].split(',')), {"future-test-2026-10-29", "oauth-2025-04-20", "claude-code-20250219"})
        self.assertEqual(forwarded_body, body)

    def save_identity(self, configuration):
        status, result = self.request("/api/admin/settings")
        self.assertEqual(status, 200, result)
        data = result["data"]
        for field in ("smartSchedulingDefaults", "updatedAt", "updatedAtDisplay", "openaiClientProfile", "xaiClientProfile"):
            data.pop(field, None)
        if configuration is not None and "schemaVersion" not in configuration:
            configuration = {"schemaVersion": 1, "httpIdentity": configuration, "tls": {"mode": "standard"}}
        data["providerRequestProfiles"] = {"claude": configuration}
        return self.request("/api/admin/settings/update", data)

    def tearDown(self):
        self.assertEqual(self.save_identity(None)[0], 200)

    def test_saved_identity_controls_non_native_messages_headers(self):
        configuration = {"cliVersion": "2.1.250", "sdkVersion": "0.93.0", "os": "MacOS", "arch": "x64", "nodeVersion": "22.18.0"}
        status, result = self.save_identity(configuration)
        self.assertEqual(status, 200, result)
        status, result = self.request("/api/admin/settings/client-profiles/claude/preview", {"configuration": None})
        self.assertEqual(status, 200, result)
        self.assertEqual(result["data"]["userAgent"], "claude-cli/2.1.250 (external, sdk-cli)")
        with self.messages({"model": "claude-test-model", "max_tokens": 8,
                            "messages": [{"role": "user", "content": "identity"}]},
                           {"x-api-key": self.key, "User-Agent": "custom-client/1", "X-Stainless-OS": "Windows"}) as response:
            self.assertEqual(response.status, 200, response.read())
        headers = {k.lower(): v for k, v in self.upstream.observed[-1][1].items()}
        self.assertEqual(headers["user-agent"], "claude-cli/2.1.250 (external, sdk-cli)")
        self.assertEqual(headers["x-stainless-os"], "MacOS")
        self.assertEqual(headers["x-stainless-arch"], "x64")
        self.assertEqual(headers["x-stainless-package-version"], "0.93.0")
        self.assertEqual(headers["x-stainless-runtime-version"], "v22.18.0")
        self.assertEqual(headers["x-app"], "cli")
        self.assertEqual(headers["authorization"], "Bearer sk-ant-upstream-fixture")

    def test_default_identity_is_available_in_preview_and_dashboard(self):
        status, result = self.request("/api/admin/settings/client-profiles/claude")
        self.assertEqual(status, 200, result)
        self.assertEqual(result["data"]["defaults"]["httpIdentity"]["cliVersion"], "2.1.312")
        status, result = self.request("/api/admin/settings/client-profiles/claude/preview", {"configuration": None})
        self.assertEqual(status, 200, result)
        preview = result["data"]
        self.assertEqual(preview["userAgent"], "claude-cli/2.1.312 (external, sdk-cli)")
        self.assertEqual(preview["policy"], "native-first")
        self.assertIsNone(preview["verifiedAt"])
        status, result = self.request(f"/api/admin/dashboard/summary?period=today&kind=usage&asOf={int(time.time()*1000)}")
        self.assertEqual(status, 200, result)
        profile = next(p for p in result["data"]["wireProfiles"] if p["provider"] == "claude")
        self.assertEqual(profile["userAgent"], preview["userAgent"])
        self.assertEqual(profile["product"], "Claude Code")
        self.assertIsNone(profile.get("verifiedAt"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
