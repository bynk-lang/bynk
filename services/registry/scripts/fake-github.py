#!/usr/bin/env python3
"""A stand-in for GitHub's OAuth device flow and user API (R3, #2363), for
scripts/check-accounts.sh. Each device authorisation signs in the next user of
a fixed list: the first poll of a code is `authorization_pending`, the next
answers an access token for that user.

    fake-github.py <port-file>

It listens on a free localhost port and writes it to <port-file>.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs

USERS = [
    {"id": 101, "login": "ann"},
    {"id": 102, "login": "bob"},
    {"id": 103, "login": "cy"},
]
codes = {}  # device_code -> {"user": ..., "polled": int}
REPOSITORIES = {
    "conformance-fixture/widgets": 777,
    "conformance-fixture/gadgets": 888,
}


class Fake(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def answer(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def form(self):
        length = int(self.headers.get("content-length") or 0)
        return {k: v[0] for k, v in parse_qs(self.rfile.read(length).decode()).items()}

    def do_POST(self):
        form = self.form()
        if self.path == "/login/device/code":
            if not form.get("client_id"):
                return self.answer(400, {"error": "unauthorized_client"})
            n = len(codes)
            code = f"dc{n}"
            codes[code] = {"user": USERS[n % len(USERS)], "polled": 0}
            return self.answer(200, {
                "device_code": code,
                "user_code": f"FAKE-{n:04d}",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900,
                "interval": 1,
            })
        if self.path == "/login/oauth/access_token":
            entry = codes.get(form.get("device_code", ""))
            if entry is None:
                return self.answer(200, {"error": "expired_token"})
            entry["polled"] += 1
            if entry["polled"] == 1:
                return self.answer(200, {"error": "authorization_pending"})
            return self.answer(200, {"access_token": "gho_" + form["device_code"], "token_type": "bearer"})
        self.answer(404, {"message": "Not Found"})

    def do_GET(self):
        if self.path == "/user":
            token = (self.headers.get("authorization") or "").removeprefix("Bearer ")
            entry = codes.get(token.removeprefix("gho_"))
            if entry is None:
                return self.answer(401, {"message": "Bad credentials"})
            return self.answer(200, entry["user"])
        if self.path.startswith("/repos/"):
            name = self.path.removeprefix("/repos/")
            if name in REPOSITORIES:
                return self.answer(200, {"id": REPOSITORIES[name], "full_name": name})
            return self.answer(404, {"message": "Not Found"})
        if self.path.startswith("/users/"):
            login = self.path.removeprefix("/users/")
            for user in USERS:
                if user["login"] == login:
                    return self.answer(200, user)
        self.answer(404, {"message": "Not Found"})


server = HTTPServer(("127.0.0.1", 0), Fake)
with open(sys.argv[1], "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
