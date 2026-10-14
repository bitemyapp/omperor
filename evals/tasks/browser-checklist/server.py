"""Serve the checklist page on loopback and persist its state for the grader."""
import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--port', type=int, required=True)
parser.add_argument('--state', type=Path, required=True)
args = parser.parse_args()
page = (Path(__file__).parent / 'page.html').read_bytes()
args.state.write_text(json.dumps(['Review release notes']))


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != '/':
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(page)))
        self.end_headers()
        self.wfile.write(page)

    def do_POST(self):
        try:
            size = int(self.headers.get('Content-Length', '0'))
            if self.path != '/state' or not 0 < size <= 4096:
                raise ValueError('invalid state request')
            items = json.loads(self.rfile.read(size))
            if not isinstance(items, list) or any(not isinstance(item, str) for item in items):
                raise ValueError('expected task names')
        except ValueError:
            self.send_error(400)
            return
        args.state.write_text(json.dumps(items))
        self.send_response(204)
        self.end_headers()

    def log_message(self, *_args):
        return


ThreadingHTTPServer(('127.0.0.1', args.port), Handler).serve_forever()
