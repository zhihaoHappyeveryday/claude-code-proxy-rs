#!/usr/bin/env python3
"""Claude Code 账号 HTTP 验收；所有凭据、代理和上游均为本地模拟。"""
from pathlib import Path
import json
import runpy
import unittest
import urllib.request
import urllib.error
import datetime
import time
import urllib.parse
import hashlib
import base64

FIXTURE = runpy.run_path(str(Path(__file__).with_name("verify-claude-accounts.py")))


MESSAGES = runpy.run_path(str(Path(__file__).with_name("verify-claude-messages.py")))


class OAuthStub(MESSAGES["MessagesStub"]):
    def do_POST(self):
        if getattr(self.server,"fail_proxy",False) and self.path.startswith("http://"):
            self.rfile.read(int(self.headers.get("Content-Length",0)))
            self.server.failed_proxy_requests = getattr(self.server,"failed_proxy_requests",0) + 1
            self.send_response(502)
            self.send_header("Content-Length","0")
            self.end_headers()
            return
        if urllib.parse.urlsplit(self.path).path != "/v1/oauth/token":
            return super().do_POST()
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.observed.append((self.path, dict(self.headers), body))
        scope = "user:inference" if body.get("code", "").startswith("setup") else "user:profile user:inference"
        result = {"access_token":"sk-ant-oat01-exchanged-fixture", "token_type":"Bearer", "expires_in":3600,
            "scope":scope, "account":{"uuid":"account-fixture", "email_address":"fixture@example.test"}, "organization":{"uuid":"org-fixture"}}
        if scope != "user:inference":
            result["refresh_token"] = "sk-ant-ort01-refresh-fixture"
        if body.get("code") == "auto-code":
            result["expires_in"] = 1
            result["refresh_token"] = "sk-ant-ort01-auto-fixture"
        if body.get("code", "").startswith("pool-"):
            result["expires_in"] = 1
            result["account"]["uuid"] = body["code"]
            result["refresh_token"] = "sk-ant-ort01-" + body["code"]
        if body.get("refresh_token", "").startswith("sk-ant-ort01-pool-"):
            result["account"]["uuid"] = body["refresh_token"].removeprefix("sk-ant-ort01-")
            result["refresh_token"] = body["refresh_token"]
        payload = json.dumps(result).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


class OAuthHTTPFixture(FIXTURE["ClaudeHTTPFixture"]):
    flow_ttl = 1800
    upstream_handler = OAuthStub

    @classmethod
    def upstream_environment(cls):
        return {"CPR_CLAUDE_TEST_UPSTREAM": cls.upstream_url, "CPR_CLAUDE_REQUIRED_PROXY_ENDPOINT":cls.upstream_url,"CPR_CLAUDE_TEST_FLOW_TTL_SECONDS":str(cls.flow_ttl)}

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        status, result = cls.request("/api/admin/proxies/create", [{
            "name": "Isolated Claude proxy", "proxyUrl": cls.upstream_url,
        }])
        if status != 200:
            raise AssertionError((status, result))
        status, result = cls.request("/api/admin/proxies")
        cls.proxy_id = result["data"]["items"][0]["id"]

class ClaudeOAuthTests(OAuthHTTPFixture):
    def test_setup_token_import_requires_proxy_and_hides_credentials(self):
        token = "sk-ant-oat01-local-setup-fixture"
        data = {"name": "Setup fixture", "authentication_kind": "setup_token", "access_token": token}
        status, result = self.request("/api/admin/accounts/import", {
            "provider": "claude", "outboundProxyId": self.proxy_id, "data": data,
        })
        self.assertEqual(status, 201, result)
        account_id = result["data"]["accountIds"][0]
        status, detail = self.request("/api/admin/accounts/detail?accountId=" + account_id)
        self.assertEqual(status, 200, detail)
        self.assertEqual(detail["data"]["account"]["authenticationKind"], "setup_token")
        self.assertNotIn(token, json.dumps(detail))
        self.assertEqual(detail["data"]["credentialConfiguration"]["authentication_kind"], "setup_token")
        status, result = self.request("/api/admin/accounts/import", {"provider": "claude", "data": data})
        self.assertEqual(status, 400, result)

    def test_setup_token_messages_use_bearer_and_bound_proxy(self):
        status, result = self.request("/api/admin/accounts/import", {
            "provider": "claude", "outboundProxyId": self.proxy_id,
            "data": {"name": "Wire setup", "authentication_kind": "setup_token", "access_token": "sk-ant-oat01-wire-fixture"},
        })
        self.assertEqual(status, 201, result)
        status, result = self.request("/api/admin/client-keys/create", {"name": "OAuth wire client", "groupIds": [], "maxConcurrency": 1, "requestsPerMinute": 0})
        self.assertEqual(status, 201, result)
        key = result["data"]["plaintextKey"]
        payload = {"model":"claude-test-model", "max_tokens":1, "messages":[{"role":"user","content":"hello"}]}
        req = urllib.request.Request(self.base + "/v1/messages", data=json.dumps(payload).encode(), headers={"Content-Type":"application/json", "x-api-key":key, "anthropic-beta":"future-capability-2099", "anthropic-version":"2023-06-01"})
        try:
            response = urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=15)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            self.assertEqual(response.status, 200, response.read())
        path, headers, body = self.upstream.observed[-1]
        self.assertTrue(path.startswith(self.upstream_url + "/v1/messages"), path)
        headers = {k.lower():v for k,v in headers.items()}
        self.assertTrue(headers["authorization"].startswith("Bearer sk-ant-oat01-"))
        self.assertNotIn("x-api-key", headers)
        self.assertIn("future-capability-2099", headers["anthropic-beta"])
        self.assertIn("oauth-2025-04-20", headers["anthropic-beta"])
        self.assertEqual(body, payload)

    def test_oauth_and_setup_authorization_exchange_bind_state_and_are_idempotent(self):
        for method, expected_scope in (("oauth", "user:profile"), ("setup_token", "user:inference")):
            with self.subTest(method=method):
                status, result = self.request("/api/admin/accounts/oauth/start", {
                    "provider":"claude", "name":"Authorized " + method, "authorizationMethod":method, "outboundProxyId":self.proxy_id,
                })
                self.assertEqual(status, 201, result)
                flow = result["data"]
                query = urllib.parse.parse_qs(urllib.parse.urlsplit(flow["authorizationUrl"]).query)
                self.assertIn(expected_scope, query["scope"][0])
                if method == "setup_token":
                    self.assertEqual(query["scope"], ["user:inference"])
                self.assertEqual(query["code_challenge_method"], ["S256"])
                self.assertNotIn("code_verifier", query)
                callback = {"provider":"claude", "flowId":flow["flowId"], "callbackUrl":method + "-code#wrong-state"}
                count = len(self.upstream.observed)
                status, result = self.request("/api/admin/accounts/oauth/complete", callback)
                self.assertEqual(status, 400, result)
                self.assertEqual(len(self.upstream.observed), count)
                callback["callbackUrl"] = method + "-code#" + query["state"][0]
                status, result = self.request("/api/admin/accounts/oauth/complete", callback)
                self.assertEqual(status, 201, result)
                account_id = result["data"]["accountId"]
                token_path, _, exchange = self.upstream.observed[-1]
                self.assertTrue(token_path.startswith(self.upstream_url + "/"))
                challenge = base64.urlsafe_b64encode(hashlib.sha256(exchange["code_verifier"].encode()).digest()).decode().rstrip("=")
                self.assertEqual(challenge, query["code_challenge"][0])
                self.assertEqual(exchange["state"], query["state"][0])
                status, detail = self.request("/api/admin/accounts/detail?accountId=" + account_id)
                self.assertEqual(status, 200, detail)
                self.assertEqual(detail["data"]["account"]["authenticationKind"], method)
                self.assertNotIn("sk-ant-oat01", json.dumps(detail))
                count = len(self.upstream.observed)
                status, replay = self.request("/api/admin/accounts/oauth/complete", callback)
                self.assertEqual(status, 201, replay)
                self.assertEqual(replay["data"]["accountId"], account_id)
                self.assertEqual(len(self.upstream.observed), count)

    def test_oauth_refresh_and_reauthorize_preserve_account(self):
        status, flow = self.request("/api/admin/accounts/oauth/start", {"provider":"claude", "name":"Refresh fixture", "outboundProxyId":self.proxy_id})
        self.assertEqual(status, 201, flow)
        flow = flow["data"]
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(flow["authorizationUrl"]).query)["state"][0]
        status, result = self.request("/api/admin/accounts/oauth/complete", {"provider":"claude","flowId":flow["flowId"],"callbackUrl":"oauth-code#"+state})
        self.assertEqual(status, 201, result)
        account_id = result["data"]["accountId"]
        config_path = "/api/admin/accounts/client-profile?accountId=" + account_id
        revision = self.request(config_path)[1]["data"]["revision"]
        configuration = self.request("/api/admin/settings/client-profiles/claude")[1]["data"]["defaults"]
        configuration["httpIdentity"]["cliVersion"] = "2.1.250"
        status, saved = self.request("/api/admin/accounts/client-profile", {
            "accountId": account_id, "expectedRevision": revision, "configuration": configuration})
        self.assertEqual(status, 200, saved)
        status, result = self.request("/api/admin/accounts/refresh", {"accountId":account_id})
        self.assertEqual(status, 200, result)
        _, _, body = self.upstream.observed[-1]
        self.assertEqual(body["grant_type"], "refresh_token")
        self.assertEqual(body["refresh_token"], "sk-ant-ort01-refresh-fixture")
        self.assertEqual(self.request(config_path)[1]["data"], saved["data"])
        status, flow = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":"Reauthorize fixture","accountId":account_id})
        self.assertEqual(status, 201, flow)
        flow = flow["data"]
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(flow["authorizationUrl"]).query)["state"][0]
        status, result = self.request("/api/admin/accounts/oauth/complete", {"provider":"claude","flowId":flow["flowId"],"callbackUrl":"oauth-code#"+state})
        self.assertEqual(status, 201, result)
        self.assertEqual(result["data"]["accountId"], account_id)
        self.assertEqual(self.request(config_path)[1]["data"], saved["data"])

    def test_oauth_expiring_token_is_refreshed_by_worker(self):
        status, result = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":"Automatic refresh","outboundProxyId":self.proxy_id})
        self.assertEqual(status, 201, result)
        flow = result["data"]
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(flow["authorizationUrl"]).query)["state"][0]
        status, result = self.request("/api/admin/accounts/oauth/complete", {"provider":"claude","flowId":flow["flowId"],"callbackUrl":"auto-code#"+state})
        self.assertEqual(status, 201, result)
        account_id = result["data"]["accountId"]
        deadline = time.monotonic() + 40
        while time.monotonic() < deadline:
            status, detail = self.request("/api/admin/accounts/detail?accountId=" + account_id)
            self.assertEqual(status, 200, detail)
            expiry = datetime.datetime.fromisoformat(detail["data"]["credentialConfiguration"]["expires_at"].replace("Z", "+00:00"))
            if expiry > datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(minutes=30):
                break
            time.sleep(0.5)
        else:
            self.fail("Automatic OAuth refresh did not publish the renewed expiry")

    def test_setup_token_replacement_keeps_account_and_is_not_refreshable(self):
        status, result = self.request("/api/admin/accounts/import", {"provider":"claude", "outboundProxyId":self.proxy_id,
            "data":{"name":"Replace setup", "authentication_kind":"setup_token", "access_token":"sk-ant-oat01-before"}})
        self.assertEqual(status, 201, result)
        account_id = result["data"]["accountIds"][0]
        status, result = self.request("/api/admin/accounts/update", {"accountId":account_id, "enabled":True, "concurrencyLimit":None,"weight":1,"groupIds":[],
            "credential":{"access_token":"sk-ant-oat01-after","expires_at":"2030-01-01T00:00:00Z"}})
        self.assertEqual(status, 200, result)
        status, result = self.request("/api/admin/accounts/detail?accountId=" + account_id)
        self.assertEqual(status, 200, result)
        self.assertEqual(result["data"]["account"]["id"], account_id)
        self.assertEqual(result["data"]["credentialConfiguration"]["expires_at"], "2030-01-01T00:00:00Z")
        self.assertNotIn("sk-ant-oat01-after", json.dumps(result))
        count = len(self.upstream.observed)
        status, result = self.request("/api/admin/accounts/refresh", {"accountId":account_id})
        self.assertEqual(status, 400, result)
        self.assertEqual(len(self.upstream.observed), count)

    def test_manually_replaced_setup_token_requires_verification_again(self):
        status,result = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":"Verified setup","authorizationMethod":"setup_token","outboundProxyId":self.proxy_id})
        self.assertEqual(status,201,result)
        flow = result["data"]
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(flow["authorizationUrl"]).query)["state"][0]
        status,result = self.request("/api/admin/accounts/oauth/complete", {"provider":"claude","flowId":flow["flowId"],"callbackUrl":"setup-code#"+state})
        self.assertEqual(status,201,result)
        account_id = result["data"]["accountId"]
        status,result = self.request("/api/admin/accounts/update", {"accountId":account_id,"enabled":True,"concurrencyLimit":None,"weight":1,"groupIds":[],"credential":{"access_token":"sk-ant-oat01-unverified-replacement"}})
        self.assertEqual(status,200,result)
        status,result = self.request("/api/admin/accounts/detail?accountId="+account_id)
        self.assertEqual(status,200,result)
        self.assertEqual(result["data"]["account"]["status"],"error",result)
        self.assertEqual(result["data"]["account"]["errorReason"],"account_unverified",result)

    def test_proxy_failure_does_not_retry_directly_or_leak_credentials(self):
        status, result = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":"No direct fallback","outboundProxyId":self.proxy_id})
        self.assertEqual(status,201,result)
        flow = result["data"]
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(flow["authorizationUrl"]).query)["state"][0]
        before = len(self.upstream.observed)
        self.upstream.fail_proxy = True
        try:
            status,result = self.request("/api/admin/accounts/oauth/complete", {"provider":"claude","flowId":flow["flowId"],"callbackUrl":"oauth-code#"+state})
            self.assertEqual(status,502,result)
            self.assertEqual(len(self.upstream.observed),before)
            self.assertGreater(self.upstream.failed_proxy_requests,0)
            self.assertNotIn("oauth-code",json.dumps(result))
        finally:
            self.upstream.fail_proxy = False

    def test_missing_or_wrong_proxy_and_expired_setup_are_rejected(self):
        status,result = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":"Missing proxy"})
        self.assertEqual(status,400,result)
        status,result = self.request("/api/admin/accounts/import", {"provider":"claude","outboundProxyId":self.proxy_id,"data":{
            "name":"Expired setup","authentication_kind":"setup_token","access_token":"sk-ant-oat01-expired","expires_at":"2020-01-01T00:00:00Z"}})
        self.assertEqual(status,400,result)
        status,result = self.request("/api/admin/proxies/create", [{"name":"Wrong exit","proxyUrl":"http://127.0.0.1:9"}])
        self.assertEqual(status,200,result)
        status,result = self.request("/api/admin/proxies")
        wrong = next(v["id"] for v in result["data"]["items"] if v["name"]=="Wrong exit")
        status,result = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":"Wrong exit","outboundProxyId":wrong})
        self.assertEqual(status,400,result)


class ExpiredAuthorizationTests(OAuthHTTPFixture):
    flow_ttl = 1

    def test_expired_authorization_cannot_exchange_tokens(self):
        status,result = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":"Expiry fixture","outboundProxyId":self.proxy_id})
        self.assertEqual(status,201,result)
        flow = result["data"]
        state = urllib.parse.parse_qs(urllib.parse.urlsplit(flow["authorizationUrl"]).query)["state"][0]
        time.sleep(1.2)
        before = len(self.upstream.observed)
        status,result = self.request("/api/admin/accounts/oauth/complete", {"provider":"claude","flowId":flow["flowId"],"callbackUrl":"oauth-code#"+state})
        self.assertIn(status,(400,404),result)
        self.assertEqual(len(self.upstream.observed),before)


class MissingDeploymentProxyTests(OAuthHTTPFixture):
    @classmethod
    def upstream_environment(cls):
        cls.env.pop("CPR_CLAUDE_REQUIRED_PROXY_ENDPOINT", None)
        return {"CPR_CLAUDE_TEST_UPSTREAM": cls.upstream_url}

    def test_missing_deployment_proxy_cannot_start_authorization(self):
        status,result = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":"Missing deployment policy","outboundProxyId":self.proxy_id})
        self.assertEqual(status,400,result)
        self.assertEqual(self.upstream.observed,[])


class RefreshPoolTests(OAuthHTTPFixture):
    def test_disabled_first_batch_does_not_starve_later_active_account(self):
        for index in range(51):
            status,result = self.request("/api/admin/accounts/oauth/start", {"provider":"claude","name":f"Pool {index}","outboundProxyId":self.proxy_id})
            self.assertEqual(status,201,result)
            flow = result["data"]
            state = urllib.parse.parse_qs(urllib.parse.urlsplit(flow["authorizationUrl"]).query)["state"][0]
            status,result = self.request("/api/admin/accounts/oauth/complete", {"provider":"claude","flowId":flow["flowId"],"callbackUrl":f"pool-{index}#"+state,"settings":{"enabled":index==50,"concurrencyLimit":None,"weight":1,"groupIds":[]}})
            self.assertEqual(status,201,result)
            account_id = result["data"]["accountId"]
        deadline = time.monotonic()+40
        while time.monotonic()<deadline:
            status,result = self.request("/api/admin/accounts/detail?accountId="+account_id)
            self.assertEqual(status,200,result)
            expiry = datetime.datetime.fromisoformat(result["data"]["credentialConfiguration"]["expires_at"].replace("Z","+00:00"))
            if expiry>datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(minutes=30):
                return
            time.sleep(0.5)
        self.fail("Disabled first batch starved the active OAuth account")


if __name__ == "__main__":
    unittest.main(verbosity=2)
