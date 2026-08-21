"""Plain-HTTP shim on localhost that forwards to the real API through the
sandbox's egress proxy. Exists only because rm-providers cannot use a proxy."""
import http.server, os, urllib.request, urllib.error, threading, sys

UPSTREAM = "https://api.openai.com"
opener = urllib.request.build_opener(
    urllib.request.ProxyHandler({"https": os.environ["HTTPS_PROXY"]})
)

class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        req = urllib.request.Request(
            UPSTREAM + self.path, data=body, method="POST",
            headers={"Content-Type": "application/json",
                     "Authorization": self.headers.get("Authorization", "")},
        )
        try:
            with opener.open(req, timeout=120) as r:
                data, code = r.read(), r.status
        except urllib.error.HTTPError as e:
            data, code = e.read(), e.code
        except Exception as e:
            data, code = str(e).encode(), 502
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

srv = http.server.ThreadingHTTPServer(("127.0.0.1", 8731), H)
print("shim on 8731", flush=True)
srv.serve_forever()
