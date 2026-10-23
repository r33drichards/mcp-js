#!/usr/bin/env python3
"""Local PUT/GET fixture for the demo's real AWS SDK adapter.
Not a general S3 server: no auth enforcement, listing, deletion or replication.
Run only on the private Docker network with synthetic data.
"""
import hashlib
import os
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit, unquote
ROOT = Path('/data'); ROOT.mkdir(exist_ok=True)
class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def key(self):
        p = unquote(urlsplit(self.path).path)
        if not p.startswith('/demo/') or not p[6:] or '/' in p[6:]:
            return None
        return ROOT / hashlib.sha256(p.encode()).hexdigest()
    def reply(self, code, body=b''):
        self.send_response(code); self.send_header('Content-Length', str(len(body)))
        self.send_header('Content-Type', 'application/octet-stream'); self.end_headers()
        self.wfile.write(body)
    def do_PUT(self):
        target = self.key()
        if target is None: return self.reply(400)
        n = int(self.headers.get('Content-Length', '0'))
        if n > 16 * 1024 * 1024: return self.reply(413)
        data = self.rfile.read(n)
        # SDK configured to avoid aws-chunked optional checksum framing.
        if self.headers.get('Content-Encoding', '') == 'aws-chunked':
            return self.reply(501, b'aws-chunked unsupported by demo fixture')
        tmp = target.with_suffix('.'+str(os.getpid())+'.tmp')
        tmp.write_bytes(data); tmp.replace(target)
        self.reply(200)
    def do_GET(self):
        target = self.key()
        if target is None or not target.exists(): return self.reply(404)
        self.reply(200, target.read_bytes())
ThreadingHTTPServer(('0.0.0.0', 9000), Handler).serve_forever()
