#!/usr/bin/env python3
"""A static server that honours Range requests, for the browser demo.

Python's built-in `http.server` does not implement Range, and without it a Mokuji
file has to be downloaded whole, which is the one thing the format exists to
avoid. Twenty lines of handler are cheaper than explaining why the demo appears
to work but transfers four megabytes.

Usage: python3 serve.py [--index PATH] [--reverse PATH] [--stac PATH] [--port N]
"""

import argparse
import http.server
import mimetypes
import pathlib
import re

HERE = pathlib.Path(__file__).parent


class Handler(http.server.SimpleHTTPRequestHandler):
    index_path: pathlib.Path
    reverse_path: pathlib.Path | None = None
    stac_path: pathlib.Path | None = None

    def translate_path(self, path):
        name = path.split("?")[0]
        if name == "/index.mokuji":
            return str(self.index_path)
        if name == "/reverse.mokuji" and self.reverse_path:
            return str(self.reverse_path)
        if name == "/stac.mokuji" and self.stac_path:
            return str(self.stac_path)
        return str(HERE / name.lstrip("/"))

    def do_GET(self):
        path = pathlib.Path(self.translate_path(self.path))
        if path.is_dir():
            path = path / "index.html"
        if not path.is_file():
            self.send_error(404)
            return

        data = path.read_bytes()
        content_type = mimetypes.guess_type(str(path))[0] or "application/octet-stream"
        match = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers.get("Range", ""))

        if match:
            start = int(match.group(1))
            end = int(match.group(2)) if match.group(2) else len(data) - 1
            end = min(end, len(data) - 1)
            if start > end:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{len(data)}")
                self.end_headers()
                return
            body = data[start : end + 1]
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
        else:
            body = data
            self.send_response(200)

        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Accept-Ranges", "bytes")
        # The demo counts requests, so a cached response would make it lie.
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--index", default="addresses.mokuji")
    parser.add_argument("--reverse", help="the index reverse.html reads")
    parser.add_argument("--stac", help="the index stac.html reads")
    parser.add_argument("--port", type=int, default=8787)
    args = parser.parse_args()

    Handler.index_path = pathlib.Path(args.index).resolve()
    if not Handler.index_path.is_file():
        raise SystemExit(f"no index at {Handler.index_path}")
    served = [Handler.index_path.name]
    if args.reverse:
        Handler.reverse_path = pathlib.Path(args.reverse).resolve()
        if not Handler.reverse_path.is_file():
            raise SystemExit(f"no index at {Handler.reverse_path}")
        served.append(f"{Handler.reverse_path.name} at /reverse.html")
    if args.stac:
        Handler.stac_path = pathlib.Path(args.stac).resolve()
        if not Handler.stac_path.is_file():
            raise SystemExit(f"no index at {Handler.stac_path}")
        served.append(f"{Handler.stac_path.name} at /stac.html")

    print(f"serving {', '.join(served)} at http://localhost:{args.port}/")
    http.server.ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
