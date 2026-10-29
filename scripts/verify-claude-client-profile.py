#!/usr/bin/env python3
"""统一客户端配置验收；仅通过管理及本地模拟 Messages HTTP 接口。"""
from pathlib import Path
import runpy
import unittest
import json
import threading
import time
import urllib.request
import urllib.error

from tls_lab import ROOT, lab, fixture_ca, server_context

FIXTURE = runpy.run_path(str(Path(__file__).with_name("verify-claude-messages.py")))


class ClaudeClientProfileTests(FIXTURE["ClaudeMessagesFixture"]):
    def test_account_configuration_requires_admin_and_explicit_configuration(self):
        path = f"/api/admin/accounts/client-profile?accountId={self.account_id}"
        client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with self.assertRaises(urllib.error.HTTPError) as error:
            client.open(self.base + path, timeout=5)
        self.assertEqual(error.exception.code, 401)
        error.exception.close()
        current = self.request(path)[1]["data"]
        status, _ = self.request("/api/admin/accounts/client-profile", {
            "accountId": self.account_id, "expectedRevision": current["revision"]})
        self.assertEqual(status, 422, "缺字段不能被当成清除配置")

    def test_invalid_template_and_identity_are_rejected(self):
        configuration = self.request("/api/admin/settings/client-profiles/claude")[1]["data"]["defaults"]
        for tls in ({"mode": "emulate"}, {"mode": "unknown"},
                    {"mode": "emulate", "template": {"id": "unknown", "revision": 1}},
                    {"mode": "emulate", "template": {"id": "claude-code-macos-arm64-2.1.312", "revision": 2}}):
            with self.subTest(tls=tls):
                configuration["tls"] = tls
                self.assertEqual(self.request("/api/admin/settings/client-profiles/claude/preview",
                                              {"configuration": configuration})[0], 400)
        configuration["tls"] = {"mode": "emulate", "template": {"id": "claude-code-macos-arm64-2.1.312", "revision": 1}}
        configuration["httpIdentity"]["nodeVersion"] = "22.0.0"
        self.assertEqual(self.request("/api/admin/settings/client-profiles/claude/preview",
                                      {"configuration": configuration})[0], 400)

    def test_account_override_is_separate_from_credentials_and_rejects_stale_save(self):
        path = f"/api/admin/accounts/client-profile?accountId={self.account_id}"
        status, result = self.request(path)
        self.assertEqual(status, 200, result)
        current = result["data"]
        self.assertIsNone(current["configuration"])
        status, options = self.request("/api/admin/settings/client-profiles/claude")
        self.assertEqual(status, 200)
        configuration = options["data"]["defaults"]
        configuration["httpIdentity"]["cliVersion"] = "2.1.250"
        command = {"accountId": self.account_id, "expectedRevision": current["revision"], "configuration": configuration}
        status, result = self.request("/api/admin/accounts/client-profile", command)
        self.assertEqual(status, 200, result)
        self.assertEqual(self.request("/api/admin/accounts/client-profile", command)[0], 409)
        status, result = self.request(path)
        self.assertEqual(status, 200)
        self.assertEqual(result["data"]["configuration"], configuration)
        self.assertGreater(result["data"]["revision"], current["revision"])
        self.assertNotIn("sk-ant-upstream-fixture", json.dumps(result))
        with self.messages({"model": "claude-test-model", "max_tokens": 8,
                            "messages": [{"role": "user", "content": "override"}]},
                           {"x-api-key": self.key, "User-Agent": "local-fixture"}) as response:
            self.assertEqual(response.status, 200, response.read())
        wire = {k.lower(): v for k, v in self.upstream.observed[-1][1].items()}
        self.assertIn("2.1.250", wire["user-agent"])
        self.assertEqual(self.latest_record()["metadata"]["claudeClient"]["source"], "account")
        status, result = self.request("/api/admin/accounts/client-profile", {
            "accountId": self.account_id, "expectedRevision": result["data"]["revision"], "configuration": None})
        self.assertEqual(status, 200, result)
        self.assertIsNone(self.request(path)[1]["data"]["configuration"])

    def test_preview_combines_identity_and_a_versioned_tls_template(self):
        status, result = self.request("/api/admin/settings/client-profiles/claude")
        self.assertEqual(status, 200)
        options = result["data"]
        configuration = options["defaults"]
        self.assertEqual(configuration["schemaVersion"], 1)
        self.assertEqual(configuration["tls"], {"mode": "standard"})
        template = options["tlsTemplates"][0]
        self.assertEqual(template["id"], "claude-code-macos-arm64-2.1.312")
        self.assertEqual(template["revision"], 1)
        self.assertEqual(template["evidence"], "local-client-capture")
        configuration["tls"] = {"mode": "emulate", "template": {"id": template["id"], "revision": 1}}
        status, result = self.request("/api/admin/settings/client-profiles/claude/preview", {"configuration": configuration})
        self.assertEqual(status, 200, result)
        preview = result["data"]
        self.assertEqual(preview["configuration"], configuration)
        self.assertEqual(preview["tls"]["mode"], "emulate")
        self.assertIsNone(preview["verifiedAt"], "本地握手观测不能冒充真实账号联调")


class ClaudeTlsGatewayTests(FIXTURE["ClaudeMessagesFixture"]):
    @classmethod
    def upstream_environment(cls):
        cls.tls_lab = lab()
        cls.directory, cls.ca, cls.tls_upstream, cls.tls_proxy = cls.tls_lab.__enter__()
        cls.addClassCleanup(lambda: cls.tls_lab.__exit__(None, None, None))
        cls.bad_certificates = {}
        for layer in ("proxy", "upstream"):
            for hostname in ("localhost", "wrong.test.invalid"):
                directory = cls.directory / f"{layer}-{hostname}"
                directory.mkdir()
                cert, key = fixture_ca(directory, hostname)
                cls.bad_certificates[(layer, hostname)] = (cert, key)
        # Trust the wrong-name certificates to isolate hostname verification from CA failure.
        bundle = cls.directory / "trust.pem"
        bundle.write_bytes(cls.ca.read_bytes() + b"".join(cert.read_bytes() for (layer, name), (cert, _) in cls.bad_certificates.items() if name != "localhost"))
        cls.ca = bundle
        cls.upstream_url = f"https://probe:fixture@localhost:{cls.tls_proxy.server_address[1]}"
        return {"CPR_CLAUDE_TEST_UPSTREAM": f"https://localhost:{cls.tls_upstream.server_port}",
                "CPR_CLAUDE_REQUIRED_PROXY_ENDPOINT": f"https://localhost:{cls.tls_proxy.server_address[1]}",
                "CLAUDE_PROXY_CA_CERTIFICATE": str(cls.ca)}

    def profile(self, emulate=True):
        configuration = self.request("/api/admin/settings/client-profiles/claude")[1]["data"]["defaults"]
        if emulate:
            configuration["tls"] = {"mode": "emulate", "template": {"id": "claude-code-macos-arm64-2.1.312", "revision": 1}}
        return configuration

    def save_global(self, configuration):
        settings = self.request("/api/admin/settings")[1]["data"]
        for field in ("smartSchedulingDefaults", "updatedAt", "updatedAtDisplay", "openaiClientProfile", "xaiClientProfile"):
            settings.pop(field, None)
        settings["providerRequestProfiles"] = {"claude": configuration}
        status, result = self.request("/api/admin/settings/update", settings)
        self.assertEqual(status, 200, result)

    def setUp(self):
        self.save_global(self.profile())
        self.account_profile(None)
        self.tls_upstream.hellos.clear()
        self.tls_proxy.connects.clear()
        self.tls_proxy.hellos.clear()
        self.tls_upstream.requests.clear()

    def account_profile(self, configuration):
        revision = self.request(f"/api/admin/accounts/client-profile?accountId={self.account_id}")[1]["data"]["revision"]
        status, result = self.request("/api/admin/accounts/client-profile", {
            "accountId": self.account_id, "expectedRevision": revision, "configuration": configuration})
        self.assertEqual(status, 200, result)

    def send(self, streaming=False, headers=None):
        return self.messages({"model": "claude-test-model", "max_tokens": 8, "stream": streaming,
                              "messages": [{"role": "user", "content": "probe"}]},
                             {"x-api-key": self.key, "User-Agent": "local-fixture", **(headers or {})})

    def test_messages_uses_selected_template_inside_https_proxy(self):
        status, result = self.request("/api/admin/settings/client-profiles/claude")
        self.assertEqual(status, 200)
        configuration = result["data"]["defaults"]
        configuration["tls"] = {"mode": "emulate", "template": {"id": "claude-code-macos-arm64-2.1.312", "revision": 1}}
        status, result = self.request("/api/admin/settings")
        self.assertEqual(status, 200)
        settings = result["data"]
        for field in ("smartSchedulingDefaults", "updatedAt", "updatedAtDisplay", "openaiClientProfile", "xaiClientProfile"):
            settings.pop(field, None)
        settings["providerRequestProfiles"] = {"claude": configuration}
        self.assertEqual(self.request("/api/admin/settings/update", settings)[0], 200)
        with self.messages({"model": "claude-test-model", "max_tokens": 8, "stream": True,
                            "messages": [{"role": "user", "content": "probe"}]},
                           {"x-api-key": self.key, "User-Agent": "local-fixture"}) as response:
            self.assertEqual(response.status, 200)
            self.assertIn(b"event: message_stop", response.read())
        reference = json.loads((ROOT / "scripts/fixtures/claude-2.1.312-macos-arm64-tls.json").read_text())
        self.assertEqual(self.tls_upstream.hellos[0], reference["hello"])
        self.assertTrue(self.tls_proxy.connects)
        self.assertFalse(any(row["proxy_auth_leaked"] for row in self.tls_upstream.requests))
        metadata = self.latest_record()["metadata"]["claudeClient"]
        self.assertTrue(metadata["applied"])
        self.assertEqual(metadata["configuration"]["tls"]["template"]["revision"], 1)

    def test_account_override_and_configuration_update_preserve_inflight_sse(self):
        self.account_profile(self.profile())
        self.save_global(self.profile(False))
        self.tls_upstream.sse_gate = threading.Event()
        try:
            with self.send(True) as response:
                self.assertEqual(response.readline(), b"event: message_start\n")
                self.account_profile(self.profile(False))
                self.tls_upstream.sse_gate.set()
                self.assertIn(b"message_stop", response.read())
        finally:
            del self.tls_upstream.sse_gate
        reference = json.loads((ROOT / "scripts/fixtures/claude-2.1.312-macos-arm64-tls.json").read_text())["hello"]
        self.assertEqual(self.tls_upstream.hellos[0], reference)
        with self.send() as response:
            self.assertEqual(response.status, 200, response.read())
        self.assertNotEqual(self.tls_upstream.hellos[-1], reference)

    def test_reuses_connection_but_new_configuration_opens_new_connection(self):
        for _ in range(2):
            with self.send() as response:
                self.assertEqual(response.status, 200, response.read())
        self.assertEqual(len(self.tls_upstream.hellos), 1)
        self.account_profile(self.profile())
        with self.send() as response:
            self.assertEqual(response.status, 200, response.read())
        self.assertEqual(len(self.tls_upstream.hellos), 2)
        self.assertEqual(self.tls_upstream.hellos[0], self.tls_upstream.hellos[1], "新连接没有未经验证的 PSK 恢复扩展")

    def test_platform_configuration_switch_back_does_not_reactivate_old_pool(self):
        for emulate in (True, False, True):
            self.save_global(self.profile(emulate))
            with self.send() as response:
                self.assertEqual(response.status, 200, response.read())
        self.assertEqual(len(self.tls_upstream.hellos), 3, "A→B→A 必须建立三条连接，不能重新启用旧 A 池")

    def test_independent_key_profiles_do_not_retire_each_other(self):
        keys = []
        for emulate in (True, False):
            status, created = self.request("/api/admin/client-keys/create", {
                "name": f"TLS key {emulate}", "groupIds": [], "maxConcurrency": 1,
                "requestsPerMinute": 0, "providerRequestProfileOverrides": {"claude": self.profile(emulate)}})
            self.assertEqual(status, 201, created)
            keys.append(created["data"]["plaintextKey"])
        for key in [*keys, *keys]:
            with self.send(headers={"x-api-key": key}) as response:
                self.assertEqual(response.status, 200, response.read())
        self.assertEqual(len(self.tls_upstream.hellos), 2, "不同密钥的稳定配置应各自复用")

    def test_retired_pool_drains_after_concurrent_final_responses(self):
        status, created = self.request("/api/admin/client-keys/create", {
            "name": "Concurrent TLS fixture", "groupIds": [], "maxConcurrency": 3, "requestsPerMinute": 0})
        self.assertEqual(status, 201, created)
        key = created["data"]["plaintextKey"]
        self.assertEqual(self.request("/api/admin/accounts/update", {
            "accountId": self.account_id, "enabled": True, "concurrencyLimit": 3, "weight": 1, "groupIds": []})[0], 200)
        self.tls_upstream.sse_gate = threading.Event()
        self.tls_upstream.keepalive_sse = True
        responses, readers, completed = [], [], []
        try:
            for _ in range(2):
                response = self.send(True, {"x-api-key": key})
                self.assertEqual(response.readline(), b"event: message_start\n")
                responses.append(response)
            old_connections = self.tls_upstream.connections[-2:]
            self.save_global(self.profile(False))
            with self.send(headers={"x-api-key": key}) as response:
                self.assertEqual(response.status, 200, response.read())
            def read(response):
                try:
                    completed.append(b"message_stop" in response.read())
                finally:
                    response.close()
            readers = [threading.Thread(target=read, args=(response,)) for response in responses]
            for reader in readers:
                reader.start()
            self.tls_upstream.sse_gate.set()
            for reader in readers:
                reader.join(timeout=3)
            self.assertEqual(completed, [True, True])
            deadline = time.monotonic() + 2
            while any(connection.fileno() >= 0 for connection in old_connections):
                self.assertLess(time.monotonic(), deadline, "排空后不应等待下一次请求才释放旧池")
                time.sleep(0.01)
        finally:
            for response in responses:
                response.close()
            self.tls_upstream.sse_gate.set()
            del self.tls_upstream.sse_gate
            self.tls_upstream.keepalive_sse = False
            for reader in readers:
                reader.join(timeout=3)
            self.request("/api/admin/accounts/update", {
                "accountId": self.account_id, "enabled": True, "concurrencyLimit": 1, "weight": 1, "groupIds": []})

    def test_incompatible_native_identity_stops_before_proxy(self):
        with self.send(headers={"User-Agent": "claude-cli/2.1.312 (external, cli)",
                                "x-stainless-runtime-version": "v22.0.0"}) as response:
            self.assertEqual(response.status, 400, response.read())
        self.assertFalse(self.tls_proxy.hellos)
        metadata = self.latest_record()["metadata"]["claudeClient"]
        self.assertFalse(metadata["applied"])
        self.assertEqual(metadata["phase"], "configuration")

    def test_compatible_native_identity_is_preserved(self):
        reference = json.loads((ROOT / "scripts/fixtures/claude-2.1.312-macos-arm64-tls.json").read_text())
        headers = {"User-Agent": reference["identity"]["user-agent"],
                   **{k: v for k, v in reference["identity"].items() if k != "user-agent"}}
        with self.send(headers=headers) as response:
            self.assertEqual(response.status, 200, response.read())
        self.assertEqual(self.tls_upstream.requests[-1]["identity"], reference["identity"])

    def test_accounts_do_not_share_connections_or_tls_sessions(self):
        proxy_id = self.request("/api/admin/proxies")[1]["data"]["items"][0]["id"]
        status, imported = self.request("/api/admin/accounts/import", {
            "provider": "claude", "outboundProxyId": proxy_id,
            "data": {"name": "Second TLS fixture", "authentication_kind": "setup_token", "access_token": "sk-ant-second-tls-fixture"}})
        self.assertEqual(status, 201, imported)
        second = imported["data"]["accountIds"][0]
        def enable(account_id, enabled):
            self.assertEqual(self.request("/api/admin/accounts/update", {
                "accountId": account_id, "enabled": enabled, "concurrencyLimit": 1, "weight": 1, "groupIds": []})[0], 200)
        try:
            enable(second, False)
            with self.send() as response:
                self.assertEqual(response.status, 200, response.read())
            enable(self.account_id, False)
            enable(second, True)
            with self.send() as response:
                self.assertEqual(response.status, 200, response.read())
            self.assertEqual(len(self.tls_upstream.hellos), 2)
            self.assertEqual(self.tls_upstream.hellos[0], self.tls_upstream.hellos[1])
        finally:
            enable(second, False)
            enable(self.account_id, True)

    def test_stream_cancel_releases_capacity_and_proxy_tunnel(self):
        self.tls_upstream.sse_gate = threading.Event()
        try:
            response = self.send(True)
            self.assertEqual(response.readline(), b"event: message_start\n")
            response.close()
            self.tls_upstream.sse_gate.set()
            deadline = time.monotonic() + 2
            while True:
                with self.send() as next_response:
                    next_response.read()
                    if next_response.status == 200:
                        break
                self.assertLess(time.monotonic(), deadline, "取消后账号容量必须释放")
                time.sleep(0.05)
            self.assertGreaterEqual(len(self.tls_upstream.hellos), 2)
        finally:
            del self.tls_upstream.sse_gate

    def test_proxy_407_and_timeout_never_fall_back_to_direct(self):
        for behavior in ("reject", "stall"):
            setattr(self.tls_proxy, behavior, True)
            try:
                self.account_profile(None)
                with self.send() as response:
                    self.assertGreaterEqual(response.status, 400)
                    response.read()
                self.assertFalse(self.tls_upstream.requests)
                self.assertFalse(self.tls_upstream.hellos)
            finally:
                setattr(self.tls_proxy, behavior, False)

    def test_certificate_checks_on_both_tls_layers(self):
        for layer, server in (("proxy", self.tls_proxy), ("upstream", self.tls_upstream)):
            for hostname in ("localhost", "wrong.test.invalid"):
                with self.subTest(layer=layer, hostname=hostname):
                    cert, key = self.bad_certificates[(layer, hostname)]
                    original = server.context
                    server.context = server_context(cert, key, server.hellos)
                    self.account_profile(None)
                    try:
                        with self.send() as response:
                            self.assertGreaterEqual(response.status, 400)
                            response.read()
                        self.assertFalse(self.tls_upstream.requests)
                    finally:
                        server.context = original


if __name__ == "__main__":
    unittest.main(verbosity=2)
