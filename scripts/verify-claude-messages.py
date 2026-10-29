#!/usr/bin/env python3
"""在真实 HTTP 接口验收 Messages；凭据和上游均为隔离测试数据。"""

from pathlib import Path
import json
import http.client
import runpy
import time
import threading
import datetime
import urllib.parse
import unittest
import urllib.error
import urllib.request

FIXTURE = runpy.run_path(str(Path(__file__).with_name("verify-claude-accounts.py")))


class MessagesStub(FIXTURE["AnthropicStub"]):
    protocol_version = "HTTP/1.1"
    def do_POST(self):
        self.server.last_client_port = self.client_address[1]
        body = self.rfile.read(int(self.headers["Content-Length"]))
        self.server.observed.append((self.path, dict(self.headers), json.loads(body)))
        content = json.loads(body).get("messages", [{}])[0].get("content", "")
        if content == "timeout":
            time.sleep(5)
            return
        if isinstance(content, str) and content.startswith("error-"):
            status = int(content.split("-")[1])
            response = json.dumps({"type": "error", "error": {"type": "rate_limit_error", "message": "fixture rejected"}}).encode()
            self.server.error_bytes = response
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(response)))
            self.send_header("retry-after", "7")
            self.send_header("anthropic-ratelimit-unified-status", "rejected")
            self.send_header("Connection", "anthropic-hop")
            self.send_header("anthropic-hop", "do-not-forward")
            self.end_headers()
            self.wfile.write(response)
            return
        if json.loads(body).get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.close_connection = True
            self.send_header("request-id", "req_stream")
            self.end_headers()
            frames = [
                b': keep-alive\n\n',
                b'event: message_start\ndata: {"type":"message_start","message":{"id":"msg_stream","type":"message","role":"assistant","model":"claude-test-model","content":[],"usage":{"input_tokens":11,"output_tokens":0,"cache_read_input_tokens":5}}}\n\n',
                b'event: content_block_delta\ndata: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}\n\n',
                b'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}\n\n',
                b'event: message_stop\ndata: {"type":"message_stop"}\n\n',
            ]
            self.server.stream_bytes = b''.join(frames)
            if content == "idle":
                self.wfile.write(frames[1])
                self.wfile.flush()
                time.sleep(5)
                return
            if content == "fragmented":
                delta = b'event: content_block_delta\r\ndata: ' + json.dumps({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "input_json_delta", "partial_json": "大段代码" * 12000}}, ensure_ascii=False).encode() + b'\r\n\r\n'
                first_usage = b'event: message_delta\ndata: {"type":"message_delta","usage":{"output_tokens":1}}\n\n'
                wire = frames[0] + frames[1] + delta + first_usage + frames[3] + frames[4]
                self.server.stream_bytes = wire
                for offset in range(0, len(wire), 1021):
                    self.wfile.write(wire[offset:offset + 1021])
                    self.wfile.flush()
                return
            if content == "missing-start":
                self.wfile.write(frames[-1])
                self.wfile.flush()
                return
            if content in ("cancel", "truncated", "stream-error"):
                self.wfile.write(frames[1])
                self.wfile.flush()
                if content == "truncated":
                    return
                if content == "stream-error":
                    self.wfile.write(b'event: error\ndata: {"type":"error","error":{"type":"overloaded_error","message":"fixture overloaded"}}\n\n')
                    self.wfile.flush()
                    return
                self.server.cancelled = threading.Event()
                for _ in range(100):
                    try:
                        self.wfile.write(b': ping\n\n')
                        self.wfile.flush()
                    except (BrokenPipeError, ConnectionResetError):
                        self.server.cancelled.set()
                        return
                    time.sleep(0.05)
                return
            for frame in frames:
                self.wfile.write(frame)
                self.wfile.flush()
                time.sleep(0.3)
            return
        result = {"id": "msg_fixture", "type": "message", "role": "assistant",
                  "model": "claude-test-model", "content": [{"type": "text", "text": "hello"}],
                  "stop_reason": "end_turn", "stop_sequence": None,
                  "usage": {"input_tokens": 11, "output_tokens": 3,
                            "cache_creation_input_tokens": 7, "cache_read_input_tokens": 5}}
        response = json.dumps(result).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(response)))
        self.send_header("request-id", "req_mock_anthropic")
        self.end_headers()
        self.wfile.write(response)


class ClaudeMessagesFixture(FIXTURE["ClaudeHTTPFixture"]):
    upstream_handler = MessagesStub
    request_timeout_seconds = 3

    @classmethod
    def upstream_environment(cls):
        return {"CPR_CLAUDE_TEST_UPSTREAM":cls.upstream_url, "CPR_CLAUDE_REQUIRED_PROXY_ENDPOINT":cls.upstream_url}

    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        status, result = cls.request("/api/admin/proxies/create", [{"name":"Messages proxy","proxyUrl":cls.upstream_url}])
        if status != 200 or result["data"][0]["status"] != "succeeded":
            raise AssertionError((status,result))
        status, result = cls.request("/api/admin/proxies")
        proxy_id = result["data"]["items"][0]["id"]
        status, result = cls.request("/api/admin/accounts/import", {
            "provider": "claude", "outboundProxyId":proxy_id, "data": {"name": "Messages fixture", "authentication_kind":"setup_token", "access_token": "sk-ant-upstream-fixture"},
        })
        if status != 201:
            raise AssertionError((status, result))
        cls.account_id = result["data"]["accountIds"][0]
        status, result = cls.request("/api/admin/accounts/update", {"accountId": cls.account_id, "enabled": True,
            "concurrencyLimit": 1, "weight": 1, "groupIds": []})
        if status != 200:
            raise AssertionError((status, result))
        status, result = cls.request("/api/admin/client-keys/create", {
            "name": "Messages client fixture", "groupIds": [], "maxConcurrency": 1,
            "requestsPerMinute": 0,
        })
        if status != 201:
            raise AssertionError((status, result))
        cls.key = result["data"]["plaintextKey"]
        cls.key_id = result["data"]["id"]
        cls.wire = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def messages(self, body, headers):
        self.last_request_at = datetime.datetime.now(datetime.timezone.utc)
        if "Connection" in headers:
            target = urllib.parse.urlsplit(self.base)
            conn = http.client.HTTPConnection(target.hostname, target.port, timeout=15)
            conn.request("POST", "/v1/messages?beta=true", json.dumps(body).encode(), {"Content-Type": "application/json", **headers})
            return conn.getresponse()
        request = urllib.request.Request(self.base + "/v1/messages?beta=true", data=json.dumps(body).encode(),
                                         headers={"Content-Type": "application/json", **headers})
        try:
            return self.wire.open(request, timeout=15)
        except urllib.error.HTTPError as error:
            return error

    def latest_record(self, expected_transport=None):
        query = urllib.parse.urlencode({"currentPage": 1, "pageSize": 100, "status": "all",
            "startTime": (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=1)).isoformat(), "endTime": (datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(minutes=1)).isoformat()})
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            status, result = self.request("/api/admin/usage/records?" + query)
            self.assertEqual(status, 200, result)
            for row in result["data"]["items"]:
                if datetime.datetime.fromisoformat(row["createdAt"].replace("Z", "+00:00")) < self.last_request_at:
                    continue
                if expected_transport and row["clientTransport"] != expected_transport:
                    continue
                status, detail = self.request("/api/admin/usage/records/detail?" + urllib.parse.urlencode({"id": row["id"]}))
                self.assertEqual(status, 200, detail)
                if detail["data"]["logicalOutcome"] not in ("in_progress", "pending", "running"):
                    return detail["data"]
            time.sleep(0.05)
        self.fail("请求记录未收敛")

class ClaudeMessagesTests(ClaudeMessagesFixture):
    def test_stream_deadline_applies_after_headers(self):
        before = time.monotonic()
        with self.messages({"model": "claude-test-model", "max_tokens": 32, "stream": True,
                            "messages": [{"role": "user", "content": "idle"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200)
            result = response.read()
            self.assertIn(b"message_start", result)
            self.assertIn(b"event: error", result)
        self.assertLess(time.monotonic() - before, 4.5)
        detail = self.latest_record("http_sse")
        self.assertNotIn(detail["logicalOutcome"], ("succeeded", "cancelled"))
        self.assertEqual(detail["clientStatusCode"], 200)
        self.assertEqual(detail["tokenDetails"]["inputTokens"], 11)

    def test_fragmented_large_unicode_tool_content_and_cumulative_usage(self):
        with self.messages({"model": "claude-test-model", "max_tokens": 32, "stream": True,
                            "messages": [{"role": "user", "content": "fragmented"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200)
            self.assertEqual(response.read(), self.upstream.stream_bytes)
        detail = self.latest_record("http_sse")
        self.assertEqual(detail["logicalOutcome"], "succeeded")
        self.assertEqual(detail["tokenDetails"]["outputTokens"], 3, "累计 output usage 不能相加为4")

    def test_disabled_key_and_empty_account_group_cannot_reach_upstream(self):
        body = {"model": "claude-test-model", "max_tokens": 32, "messages": [{"role": "user", "content": "scope"}]}
        before = len(self.upstream.observed)
        for headers in ({"x-api-key": "sk_invalid"}, {"x-api-key": self.key, "Authorization": "Bearer sk_other"}):
            with self.messages(body, headers) as response:
                self.assertEqual(response.status, 401)
        self.assertEqual(self.request("/api/admin/client-keys/disable", {"id": self.key_id})[0], 200)
        try:
            with self.messages(body, {"x-api-key": self.key}) as response:
                self.assertEqual(response.status, 401)
        finally:
            self.assertEqual(self.request("/api/admin/client-keys/enable", {"id": self.key_id})[0], 200)
        status, result = self.request("/api/admin/account-groups/create", {"name": "Empty scope", "color": "#123456FF"})
        self.assertEqual(status, 201, result)
        group_id = result["data"]["id"]
        status, result = self.request("/api/admin/client-keys/create", {"name": "Scoped key", "groupIds": [group_id], "maxConcurrency": 1, "requestsPerMinute": 0})
        self.assertEqual(status, 201, result)
        with self.messages(body, {"x-api-key": result["data"]["plaintextKey"]}) as response:
            self.assertIn(response.status, (403, 404, 503))
        self.assertEqual(len(self.upstream.observed), before)

    def test_message_stop_without_start_is_not_a_success(self):
        with self.messages({"model": "claude-test-model", "max_tokens": 32, "stream": True,
                            "messages": [{"role": "user", "content": "missing-start"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200)
            self.assertIn(b"event: error", response.read())
        detail = self.latest_record("http_sse")
        self.assertNotEqual(detail["logicalOutcome"], "succeeded")
        self.assertIsNone(detail["tokenDetails"]["inputTokens"])

    def test_sequential_requests_reuse_upstream_connection(self):
        ports = []
        for _ in range(3):
            with self.messages({"model": "claude-test-model", "max_tokens": 32,
                                "messages": [{"role": "user", "content": "reuse"}]}, {"x-api-key": self.key}) as response:
                self.assertEqual(response.status, 200)
                response.read()
            ports.append(self.upstream.last_client_port)
        self.assertEqual(len(set(ports)), 1, "同一出口应复用上游连接池")

    def test_client_cancel_closes_upstream_and_retains_partial_usage(self):
        response = self.messages({"model": "claude-test-model", "max_tokens": 32, "stream": True,
                                  "messages": [{"role": "user", "content": "cancel"}]}, {"x-api-key": self.key})
        self.assertEqual(response.status, 200)
        self.assertEqual(response.readline(), b"event: message_start\n")
        response.close()
        self.assertTrue(self.upstream.cancelled.wait(2), "客户端取消后上游必须及时关闭")
        detail = self.latest_record("http_sse")
        self.assertEqual(detail["logicalOutcome"], "cancelled", detail)
        self.assertEqual(detail["tokenDetails"]["inputTokens"], 11)
        self.assertIsNone(detail["tokenDetails"]["cacheWriteInputTokens"])
        with self.messages({"model": "claude-test-model", "max_tokens": 32,
                            "messages": [{"role": "user", "content": "after cancel"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200)
            response.read()

    def test_truncated_stream_records_failure_and_keeps_observed_usage(self):
        with self.messages({"model": "claude-test-model", "max_tokens": 32, "stream": True,
                            "messages": [{"role": "user", "content": "truncated"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200)
            result = response.read()
            self.assertIn(b"message_start", result)
            self.assertIn(b"event: error", result)
            self.assertNotIn(b"message_stop", result)
        detail = self.latest_record("http_sse")
        self.assertNotEqual(detail["logicalOutcome"], "succeeded")
        self.assertEqual(detail["tokenDetails"]["inputTokens"], 11)

    def test_sse_error_is_forwarded_once(self):
        with self.messages({"model": "claude-test-model", "max_tokens": 32, "stream": True,
                            "messages": [{"role": "user", "content": "stream-error"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200)
            result = response.read()
            self.assertIn(b"fixture overloaded", result)
            self.assertEqual(result.count(b"event: error"), 1)
        self.assertNotEqual(self.latest_record("http_sse")["logicalOutcome"], "succeeded")

    def test_timeout_has_anthropic_error_envelope_and_releases_capacity(self):
        before = time.monotonic()
        with self.messages({"model": "claude-test-model", "max_tokens": 32,
                            "messages": [{"role": "user", "content": "timeout"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 504)
            self.assertEqual(json.loads(response.read())["type"], "error")
        self.assertLess(time.monotonic() - before, 4.5)
        detail = self.latest_record("http_json")
        self.assertEqual(detail.get("clientStatusCode"), 504, detail)
        self.assertNotEqual(detail["logicalOutcome"], "cancelled")
        self.assertNotEqual(detail["logicalOutcome"], "succeeded")
        self.assertIsNone(detail["tokenDetails"]["outputTokens"])
        with self.messages({"model": "claude-test-model", "max_tokens": 32,
                            "messages": [{"role": "user", "content": "after timeout"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200)
            response.read()

    def test_upstream_errors_preserve_status_body_and_retry_headers_but_not_hop_headers(self):
        for code in (401, 429, 529):
            with self.messages({"model": "claude-test-model", "max_tokens": 32,
                                "messages": [{"role": "user", "content": f"error-{code}"}]},
                               {"x-api-key": self.key, "Connection": "close, anthropic-client-hop", "anthropic-client-hop": "private"}) as response:
                self.assertEqual(response.status, code)
                self.assertEqual(response.read(), self.upstream.error_bytes)
                self.assertEqual(response.headers.get("retry-after"), "7")
                self.assertEqual(response.headers.get("anthropic-ratelimit-unified-status"), "rejected")
                self.assertIsNone(response.headers.get("anthropic-hop"))
            upstream_headers = {k.lower(): v for k, v in self.upstream.observed[-1][1].items()}
            self.assertNotIn("anthropic-client-hop", upstream_headers)
            detail = self.latest_record("http_json")
            self.assertEqual(detail["clientStatusCode"], code)
            self.assertEqual(detail["upstreamStatusCode"], code)
            self.assertEqual(detail["metadata"]["claudeClient"]["phase"], "response")
            self.assertIsNone(detail["tokenDetails"]["inputTokens"])
            self.assertIsNone(detail["tokenDetails"]["outputTokens"])

    def test_management_records_usage_and_upstream_request_id(self):
        with self.messages({"model": "claude-test-model", "max_tokens": 32, "stream": True,
                            "messages": [{"role": "user", "content": "usage"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200)
            response.read()
        detail = self.latest_record("http_sse")
        self.assertEqual(detail["logicalOutcome"], "succeeded", detail)
        self.assertEqual(detail["upstreamRequestId"], "req_stream")
        self.assertEqual(detail["clientStatusCode"], 200)
        self.assertEqual(detail["upstreamStatusCode"], 200)
        self.assertEqual(detail["tokenDetails"]["inputTokens"], 11)
        self.assertEqual(detail["tokenDetails"]["outputTokens"], 3)
        self.assertEqual(detail["tokenDetails"]["cachedInputTokens"], 5)
        self.assertIsNone(detail["tokenDetails"]["cacheWriteInputTokens"])
        self.assertIsNone(detail["billing"], "未配置价格不能伪造金额")

    def test_sse_is_delivered_incrementally_without_rewriting(self):
        before = time.monotonic()
        with self.messages({"model": "claude-test-model", "max_tokens": 32, "stream": True,
                            "messages": [{"role": "user", "content": "hello"}]}, {"x-api-key": self.key}) as response:
            self.assertEqual(response.status, 200, response.read() if response.status != 200 else "")
            self.assertIn("text/event-stream", response.headers.get("Content-Type", ""))
            self.assertEqual(response.headers.get("request-id"), "req_stream")
            first = response.readline()
            self.assertEqual(first, b": keep-alive\n")
            self.assertLess(time.monotonic() - before, 0.9, "首段不能等待整个生成完成")
            received = first + response.read()
            self.assertEqual(received, self.upstream.stream_bytes)
            self.assertNotIn(b"[DONE]", received)

    def test_plain_messages_preserve_anthropic_protocol_and_replace_authentication(self):
        body = {"model": "claude-test-model", "max_tokens": 32, "stream": False,
                "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
                "metadata": {"user_id": "fixture"}, "future_field": {"preserve": True}}
        with self.messages(body, {}) as response:
            self.assertEqual(response.status, 401)
        for auth in ({"x-api-key": self.key}, {"Authorization": "Bearer " + self.key}):
            with self.messages(body, {**auth, "anthropic-version": "2023-06-01",
                    "anthropic-beta": "future-capability-2026-10-28", "anthropic-future": "keep"}) as response:
                result = json.loads(response.read())
                self.assertEqual(response.status, 200, result)
                self.assertEqual(result["content"], [{"type": "text", "text": "hello"}])
                self.assertEqual(result["usage"]["input_tokens"], 11)
                self.assertEqual(response.headers.get("request-id"), "req_mock_anthropic")
            path, headers, forwarded = self.upstream.observed[-1]
            headers = {key.lower(): value for key, value in headers.items()}
            self.assertEqual(path, self.upstream_url + "/v1/messages?beta=true")
            self.assertEqual(headers["authorization"], "Bearer sk-ant-upstream-fixture")
            self.assertNotIn("x-api-key", headers)
            self.assertNotIn(self.key, json.dumps(headers))
            self.assertEqual(headers["anthropic-beta"], "future-capability-2026-10-28,oauth-2025-04-20,claude-code-20250219")
            self.assertEqual(headers["anthropic-future"], "keep")
            self.assertEqual(headers["anthropic-version"], "2023-06-01")
            self.assertEqual(forwarded, body)


if __name__ == "__main__":
    unittest.main(verbosity=2)
