#!/usr/bin/env python3
"""Private loopback regression for upload bodies on Nginx auth_request routes.

No production directories, login cookies, CLI sessions or models are used.
"""
from __future__ import annotations
import json
import shutil
import socket
import subprocess
import tempfile
import threading
import time
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

NGINX = shutil.which("nginx") or "/usr/sbin/nginx"
IMAGE_BYTES = b"x" * 1435606

class Backend(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path != "/__auth/check":
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.server.auth_paths.append(self.path)
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.server.auth_bodies.append(len(body))
        cookie = self.headers.get("Cookie", "")
        self.send_response(200 if cookie == "fixture=valid" else 401)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.server.upload_bodies.append(body)
        data = json.dumps({"ok": True, "bytes": len(body)}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

@contextmanager
def proxy(root, backend_port, auth_limit):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    config = root / "nginx.conf"
    config.write_text(f"""pid {root}/nginx.pid;
error_log {root}/error.log;
events {{ worker_connections 32; }}
http {{
    access_log off;
    client_body_temp_path {root}/body;
    proxy_temp_path {root}/proxy;
    server {{
        listen 127.0.0.1:{port};
        location = /__auth/sessiondock-check {{
            internal;
            {auth_limit}
            proxy_pass http://127.0.0.1:{backend_port}/__auth/check;
            proxy_pass_request_body off;
            proxy_set_header Content-Length '';
            proxy_set_header Cookie $http_cookie;
        }}
        location /sessiondock/ {{
            auth_request /__auth/sessiondock-check;
            client_max_body_size 512m;
            proxy_pass http://127.0.0.1:{backend_port}/;
        }}
    }}
}}
""")
    opener = build_opener(ProxyHandler({}))
    process = subprocess.Popen([NGINX, "-p", str(root) + "/", "-c", str(config), "-g", "daemon off;"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        deadline = time.monotonic() + 5
        while True:
            if process.poll() is not None:
                raise AssertionError(process.stderr.read().decode())
            try:
                opener.open(f"http://127.0.0.1:{port}/__auth/sessiondock-check", timeout=.2)
            except HTTPError:
                break
            except (URLError, TimeoutError):
                if time.monotonic() > deadline:
                    raise AssertionError("private nginx did not start")
                time.sleep(.02)
        yield opener, f"http://127.0.0.1:{port}"
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        process.stderr.close()

def request(opener, base, body, cookie="fixture=valid", declared_size=None):
    headers = {"Cookie": cookie, "Content-Type": "image/png"}
    if declared_size is not None:
        headers["Content-Length"] = str(declared_size)
    req = Request(base + "/sessiondock/api/session/conversation/attachment", data=body, headers=headers)
    try:
        with opener.open(req, timeout=5) as response:
            return response.status, response.read()
    except HTTPError as error:
        return error.code, error.read()

def main():
    if not Path(NGINX).is_file():
        print("SKIP nginx_upload_auth: nginx unavailable")
        return
    backend = ThreadingHTTPServer(("127.0.0.1", 0), Backend)
    backend.auth_bodies, backend.upload_bodies, backend.auth_paths = [], [], []
    thread = threading.Thread(target=backend.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="sessiondock-nginx-auth-") as directory:
            root = Path(directory)
            with proxy(root, backend.server_port, "") as (opener, base):
                assert request(opener, base, b"small")[0] == 200
                assert request(opener, base, IMAGE_BYTES)[0] == 500
                assert backend.upload_bodies == [b"small"]
                assert "auth request unexpected status: 413" in (root / "error.log").read_text()
            backend.auth_bodies.clear()
            backend.upload_bodies.clear()
            with proxy(root, backend.server_port, "client_max_body_size 0;") as (opener, base):
                status, data = request(opener, base, IMAGE_BYTES)
                assert status == 200 and json.loads(data)["bytes"] == len(IMAGE_BYTES)
                assert backend.upload_bodies == [IMAGE_BYTES]
                assert backend.auth_bodies == [0]
                assert request(opener, base, IMAGE_BYTES, cookie="fixture=expired")[0] == 401
                assert backend.upload_bodies == [IMAGE_BYTES]
                assert request(opener, base, b"", declared_size=512 * 1024 * 1024 + 1)[0] == 413
                assert backend.upload_bodies == [IMAGE_BYTES]
                assert all(size == 0 for size in backend.auth_bodies)
                assert backend.auth_paths and all(path == "/__auth/check" for path in backend.auth_paths)
        print("PASS nginx_upload_auth: reproduce 413-to-500, 1.37 MiB upload, bodyless auth, expired login refusal and upload limit")
    finally:
        backend.shutdown()
        backend.server_close()
        thread.join(timeout=5)

if __name__ == "__main__":
    main()
