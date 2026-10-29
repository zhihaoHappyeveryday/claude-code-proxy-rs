"""本地 TLS 验证夹具；只接受环回连接与固定的测试认证材料。"""

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import base64
import hashlib
import json
import os
import socket
import socketserver
import ssl
import subprocess
import tempfile
import threading
import time


ROOT = Path(__file__).resolve().parent.parent
AUTH = "Basic " + base64.b64encode(b"probe:fixture").decode()


def client_hello(data):
    """解析线上 ClientHello；省略随机值，保留可比较的握手结构。"""
    body = memoryview(data)[4:]
    pos = 34

    def vector(width):
        nonlocal pos
        size = int.from_bytes(body[pos:pos + width])
        pos += width
        result = bytes(body[pos:pos + size])
        pos += size
        return result

    def words(value):
        return [int.from_bytes(value[i:i + 2]) for i in range(0, len(value), 2)]

    session = vector(1)
    ciphers = words(vector(2))
    compression = vector(1).hex()
    extensions = vector(2)
    values = []
    offset = 0
    while offset < len(extensions):
        kind = int.from_bytes(extensions[offset:offset + 2])
        size = int.from_bytes(extensions[offset + 2:offset + 4])
        value = extensions[offset + 4:offset + 4 + size]
        offset += size + 4
        if kind == 51:
            shares = []
            at = 2
            while at < len(value):
                group = int.from_bytes(value[at:at + 2])
                key_len = int.from_bytes(value[at + 2:at + 4])
                shares.append({"group": group, "length": key_len})
                at += 4 + key_len
            value = shares
        elif kind == 65037:
            # ECH GREASE 的公钥和密文随机；仍比较结构和字段长度。
            at = 6
            enc_len = int.from_bytes(value[at:at + 2])
            at += 2 + enc_len
            payload_len = int.from_bytes(value[at:at + 2])
            value = {"type": value[0], "kdf_aead": value[1:5].hex(),
                     "enc_length": enc_len, "payload_length": payload_len}
        elif kind in (10, 13):
            value = words(value[2:])
        elif kind == 43:
            value = words(value[1:])
        elif kind == 41:
            value = {"length": len(value), "resumed": True}
        else:
            value = value.hex()
        values.append({"type": kind, "value": value})
    return {"legacy_version": int.from_bytes(body[:2]), "session_id_length": len(session),
            "ciphers": ciphers, "compression": compression, "extensions": values}


def fixture_ca(directory, hostname="localhost"):
    config = directory / "certificate.cnf"
    config.write_text("[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=ext\n"
                      f"[dn]\nCN={hostname}\n[ext]\nsubjectAltName=DNS:{hostname}\n"
                      "basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,digitalSignature,keyEncipherment\n")
    cert, key = directory / "ca.pem", directory / "key.pem"
    subprocess.run(["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-x509", "-days", "1",
                    "-config", str(config), "-keyout", str(key), "-out", str(cert)],
                   check=True, capture_output=True)
    key.chmod(0o600)
    return cert, key


def server_context(cert, key, observations):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    context.set_alpn_protocols(["http/1.1"])

    def observe(_socket, direction, _version, content_type, message_type, data):
        if direction == "read" and content_type == 22 and message_type == 1:
            observations.append(client_hello(data))

    context._msg_callback = observe
    return context


class HttpsServer(ThreadingHTTPServer):
    daemon_threads = True

    def get_request(self):
        connection, address = super().get_request()
        connection.settimeout(5)
        try:
            secured = self.context.wrap_socket(connection, server_side=True)
            self.connections.append(secured)
            return secured, address
        except Exception:
            connection.close()
            raise


class Messages(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_POST(self):
        body = self.rfile.read(min(int(self.headers.get("Content-Length", 0)), 1024 * 1024))
        payload = json.loads(body)
        # 仅保存公开身份字段；不保存正文、认证或任意客户端 Header。
        allowed = {"user-agent", "x-stainless-package-version", "x-stainless-os", "x-stainless-arch",
                   "x-stainless-runtime", "x-stainless-runtime-version"}
        self.server.requests.append({"path": self.path,
                                     "identity": {k.lower(): v for k, v in self.headers.items()
                                                  if k.lower() in allowed},
                                     "proxy_auth_leaked": "Proxy-Authorization" in self.headers})
        msg = {"id": "msg_tls_probe", "type": "message", "role": "assistant", "model": "claude-sonnet-4-5",
               "content": [], "usage": {"input_tokens": 1, "output_tokens": 0}}
        if payload.get("stream"):
            events = [("message_start", {"message": msg}),
                      ("content_block_start", {"index": 0, "content_block": {"type": "text", "text": ""}}),
                      ("content_block_delta", {"index": 0, "delta": {"type": "text_delta", "text": "probe"}}),
                      ("content_block_stop", {"index": 0}),
                      ("message_delta", {"delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 1}}),
                      ("message_stop", {})]
            frames = [(name, f'event: {name}\ndata: {json.dumps({"type": name, **fields})}\n\n'.encode()) for name, fields in events]
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            keep_alive = getattr(self.server, "keepalive_sse", False)
            if keep_alive:
                self.send_header("Content-Length", str(sum(len(frame) for _, frame in frames)))
            else:
                self.send_header("Connection", "close")
            self.end_headers()
            self.close_connection = not keep_alive
            for name, frame in frames:
                try:
                    self.wfile.write(frame)
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                    return  # Expected when a cancellation test closes the upstream stream.
                if name == "message_start" and hasattr(self.server, "sse_gate"):
                    self.server.sse_gate.wait(timeout=5)
                time.sleep(0.08)
        else:
            msg.update(content=[{"type": "text", "text": "probe"}], stop_reason="end_turn", stop_sequence=None)
            encoded = json.dumps(msg).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)


class Connect(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(5)
        try:
            with self.server.context.wrap_socket(self.request, server_side=True) as connection:
                header = b""
                while not header.endswith(b"\r\n\r\n") and len(header) < 16384:
                    byte = connection.recv(1)
                    if not byte:
                        return
                    header += byte
                lines = header.decode("ascii").split("\r\n")
                expected = f"CONNECT localhost:{self.server.upstream_port} HTTP/1.1"
                headers = {k.lower(): v.strip() for k, v in (line.split(":", 1) for line in lines[1:] if ":" in line)}
                self.server.connects.append({"target_matches": lines[0] == expected,
                                             "authorized": headers.get("proxy-authorization") == AUTH,
                                             "upstream_auth_leaked": "authorization" in headers or "x-api-key" in headers})
                if getattr(self.server, "reject", False) or lines[0] != expected or headers.get("proxy-authorization") != AUTH:
                    connection.sendall(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n")
                    return
                if getattr(self.server, "stall", False):
                    time.sleep(4)
                    return
                with socket.create_connection(("127.0.0.1", self.server.upstream_port), timeout=5) as upstream:
                    connection.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")

                    def relay(source, destination):
                        try:
                            while data := source.recv(16384):
                                destination.sendall(data)
                        except (OSError, ssl.SSLError):
                            pass
                        finally:
                            for stream in (source, destination):
                                try:
                                    stream.shutdown(socket.SHUT_RDWR)
                                except OSError:
                                    pass

                    outbound = threading.Thread(target=relay, args=(connection, upstream), daemon=True)
                    outbound.start()
                    relay(upstream, connection)
                    outbound.join(timeout=2)
        except (OSError, ssl.SSLError):
            pass


@contextmanager
def lab():
    with tempfile.TemporaryDirectory(prefix="claude-tls-lab-") as directory:
        directory = Path(directory)
        cert, key = fixture_ca(directory)
        upstream = HttpsServer(("127.0.0.1", 0), Messages)
        upstream.hellos, upstream.requests, upstream.connections = [], [], []
        upstream.context = server_context(cert, key, upstream.hellos)
        proxy = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Connect)
        proxy.daemon_threads = True
        proxy.hellos, proxy.connects = [], []
        proxy.context = server_context(cert, key, proxy.hellos)
        proxy.upstream_port = upstream.server_port
        threads = [threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
                   for server in (upstream, proxy)]
        for thread in threads:
            thread.start()
        try:
            yield directory, cert, upstream, proxy
        finally:
            for server in (upstream, proxy):
                server.shutdown()
                server.server_close()
            for thread in threads:
                thread.join(timeout=2)


def capture_claude(directory, cert, upstream, proxy):
    """在操作系统网络限制下运行真实 CLI；禁止读取其个人授权配置。"""
    if not Path("/usr/bin/sandbox-exec").exists():
        raise RuntimeError("真实 Claude 捕获需要 macOS 网络沙箱；未提供限制机制时不运行")
    executable = Path.home() / ".npm-global/bin/claude"
    sandbox = ('(version 1) (allow default) (deny network*) '
               '(allow network-outbound (remote ip "localhost:*"))')
    env = {"PATH": os.environ["PATH"], "TMPDIR": str(directory),
           "CLAUDE_CONFIG_DIR": str(directory / "claude"),
           "ANTHROPIC_BASE_URL": f"https://localhost:{upstream.server_port}",
           "ANTHROPIC_API_KEY": "sk-ant-local-fixture-only", "NODE_EXTRA_CA_CERTS": str(cert),
           "HTTPS_PROXY": f"https://probe:fixture@localhost:{proxy.server_address[1]}",
           "HTTP_PROXY": f"https://probe:fixture@localhost:{proxy.server_address[1]}",
           "NO_PROXY": "", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
           "DISABLE_AUTOUPDATER": "1", "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1"}
    version = subprocess.run(["/usr/bin/sandbox-exec", "-p", sandbox, str(executable), "--version"],
                             env=env, capture_output=True, text=True, check=True, timeout=10).stdout.strip()
    command = ["/usr/bin/sandbox-exec", "-p", sandbox, str(executable), "--bare", "--print",
               "--model", "claude-sonnet-4-5", "--tools", "", "--strict-mcp-config",
               "--mcp-config", '{"mcpServers":{}}', "--setting-sources", "", "--no-session-persistence",
               "--system-prompt", "This is a local TLS probe. Reply probe.", "Reply probe."]
    result = subprocess.run(command, cwd=directory, env=env, capture_output=True, text=True, timeout=25)
    if result.returncode != 0 or not upstream.hellos or not upstream.requests:
        raise RuntimeError(f"本地 CLI 捕获失败: exit={result.returncode}, handshakes={len(upstream.hellos)}, "
                           f"requests={len(upstream.requests)}, output={result.stdout[-600:]}, error={result.stderr[-600:]}")
    return {"client_version": version, "executable_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
            "mode": "bare, fake API key, local HTTPS CONNECT proxy, sandboxed loopback only",
            "identity": upstream.requests[-1]["identity"], "hello": upstream.hellos[0],
            "proxy_hello": proxy.hellos[0]}
