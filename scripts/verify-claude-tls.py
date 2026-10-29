#!/usr/bin/env python3
"""通过实际 TLS/HTTPS 代理接口验证候选传输；不会访问真实 Claude。"""
import json
from pathlib import Path
import subprocess
import threading
import unittest

from tls_lab import ROOT, capture_claude, fixture_ca, lab


class ClaudeTlsTests(unittest.TestCase):
    maxDiff = None

    def test_template_matches_actual_claude_client_through_https_proxy(self):
        with lab() as (directory, cert, upstream, proxy):
            reference = capture_claude(directory, cert, upstream, proxy)
            report = ROOT / ".runtime/proofs/tls"
            report.mkdir(parents=True, exist_ok=True)
            (report / "claude-local-capture.json").write_text(json.dumps(reference, indent=2) + "\n")
            upstream.hellos.clear()
            proxy.hellos.clear()
            result = subprocess.run([str(ROOT / "tools/tls-probe/target/debug/claude-tls-probe"),
                                     str(upstream.server_port), str(proxy.server_address[1]), str(cert)],
                                    capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(upstream.hellos, "候选实现必须完成代理隧道内的 TLS 握手")
            (report / "rust-local-capture.json").write_text(json.dumps({"hello": upstream.hellos[0],
                                                                       "proxy_hello": proxy.hellos[0]}, indent=2) + "\n")
            self.assertEqual(upstream.hellos[0], reference["hello"])
            # 模拟范围为隧道内上游；外层只要求认证与 CONNECT 协议正确。
            # wreq 在代理外层省略 ALPN，实际客户端携带 http/1.1，报告保留差异。
            self.assertTrue(proxy.hellos)
            self.assertTrue(all(item["authorized"] and item["target_matches"] for item in proxy.connects))
            self.assertFalse(any(item["upstream_auth_leaked"] for item in proxy.connects))
            self.assertFalse(any(item["proxy_auth_leaked"] for item in upstream.requests))

    def test_sse_arrives_before_upstream_completes(self):
        with lab() as (_, cert, upstream, proxy):
            upstream.sse_gate = threading.Event()
            process = subprocess.Popen([str(ROOT / "tools/tls-probe/target/debug/claude-tls-probe"),
                                        str(upstream.server_port), str(proxy.server_address[1]), str(cert)],
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            observed, arrived = [], threading.Event()

            def first_line():
                observed.append(process.stdout.readline())
                arrived.set()

            reader = threading.Thread(target=first_line, daemon=True)
            reader.start()
            try:
                self.assertTrue(arrived.wait(timeout=2), "SSE 不应等待上游结束才输出")
                self.assertEqual(observed, ["event: message_start\n"])
            finally:
                upstream.sse_gate.set()
                reader.join(timeout=2)
                output, error = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 0, error)
            self.assertIn("event: message_stop", output)

    def test_untrusted_https_proxy_stops_before_connect_or_upstream(self):
        with lab() as (directory, _, upstream, proxy):
            other = directory / "unrelated"
            other.mkdir()
            ca, _ = fixture_ca(other)
            result = subprocess.run([str(ROOT / "tools/tls-probe/target/debug/claude-tls-probe"),
                                     str(upstream.server_port), str(proxy.server_address[1]), str(ca)],
                                    capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(proxy.connects, "证书校验失败后不得向代理发送认证材料")
            self.assertFalse(upstream.hellos, "不得在代理证书错误后直连")


if __name__ == "__main__":
    unittest.main(verbosity=2)
