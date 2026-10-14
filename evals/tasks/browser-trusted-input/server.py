"""Serve a page that accepts only real input, and persist what it submits for the grader."""
import argparse
import json
import secrets
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--port', type=int, required=True)
parser.add_argument('--state', type=Path, required=True)
args = parser.parse_args()
code = ''.join(secrets.choice('ABCDEFGHJKMNPQRSTUVWXYZ23456789') for _ in range(6))
page = (Path(__file__).parent / 'page.html').read_text().replace('{{CODE}}', code).encode()
state = {'expected': code, 'session': None, 'submitted': None}


def save():
    args.state.write_text(json.dumps(state))


save()


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
            if self.path not in ('/session', '/state') or not 0 < size <= 4096:
                raise ValueError('invalid request')
            body = json.loads(self.rfile.read(size))
            if not isinstance(body, dict) or not isinstance(body.get('token'), str):
                raise ValueError('expected a token')
        except ValueError:
            self.send_error(400)
            return
        if self.path == '/session':
            # Each page load registers a fresh token; only its own submissions count.
            state['session'] = body['token']
        elif body['token'] != state['session']:
            self.send_error(403)
            return
        else:
            state['submitted'] = {key: body.get(key) for key in ('approved', 'trusted', 'code')}
        save()
        self.send_response(204)
        self.end_headers()

    def log_message(self, *_args):
        return


ThreadingHTTPServer(('127.0.0.1', args.port), Handler).serve_forever()
