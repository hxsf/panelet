#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Offline bundle/deployer checks; no device or external server is contacted."""
import contextlib
import gzip
import hashlib
import http.client
import http.server
import importlib.util
import io
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import threading
import unittest
import zipfile
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


builder = load("build_web_assets", ROOT / "scripts/build-web-assets.py")
deploy = load("deploy_web_assets", ROOT / "scripts/deploy-web-assets.py")


class ToolsTests(unittest.TestCase):
    def test_builder_cli_defaults_to_pna(self):
        summary = {"api_version": 1, "bundle_bytes": 8, "files": [], "bundle_id": "fixture"}
        with patch("sys.argv", ["build-web-assets.py", "--api-version", "1"]), \
                patch.object(builder, "build", return_value=summary) as build, \
                contextlib.redirect_stdout(io.StringIO()):
            builder.main()
        self.assertEqual(build.call_args.args[1], ROOT / "target/web-assets/bundle.pna")

    def test_deterministic_new_and_legacy_bundles_and_corruption_before_network(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            wasm = root / "preview.wasm"
            wasm.write_bytes(b"\0asm\x01\0\0\0")
            one, two = root / "one.pna", root / "two.n4a"
            first = builder.build(wasm, one, 1)
            builder.build(wasm, two, 1)
            self.assertEqual(one.read_bytes(), two.read_bytes())
            manifest, offsets, bundle_id = deploy.parse_bundle(one)
            self.assertEqual(deploy.parse_bundle(two), (manifest, offsets, bundle_id))
            self.assertEqual(bundle_id, first["bundle_id"])
            self.assertEqual([file["path"] for file in manifest["files"]],
                             ["/examples.json", "/font-licenses.txt", "/panelet-license.txt", "/preview.wasm"])
            for metadata, offset in zip(manifest["files"], offsets):
                blob = one.read_bytes()[offset:offset + metadata["size"]]
                self.assertEqual(hashlib.sha256(blob).hexdigest(), metadata["sha256"])
                if metadata["path"] == "/panelet-license.txt":
                    notice = gzip.decompress(blob)
                    self.assertIn((ROOT / "LICENSE").read_bytes(), notice)
                    self.assertIn(b"https://github.com/hxsf/panelet", notice)
                elif metadata["path"] == "/font-licenses.txt":
                    notice = gzip.decompress(blob)
                    for filename in ["NOTICE-16.0.04.txt", "COPYING-16.0.04.txt", "LICENSE.txt", "OFL-1.1.txt"]:
                        self.assertIn((ROOT / "3rd-res/fonts" / filename).read_bytes(), notice)
            class NoNetwork:
                def request(self, *args, **kwargs):
                    raise AssertionError("Invalid bundle reached the network")
            data = bytearray(one.read_bytes())
            data[-1] ^= 1
            two.write_bytes(data)
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                deploy.install(NoNetwork(), two)
            one.write_bytes(one.read_bytes() + b"extra")
            with self.assertRaisesRegex(ValueError, "trailing"):
                deploy.parse_bundle(one)

    def test_examples_keep_mit_notices_in_browser_resources_and_cli_archives(self):
        notice = (ROOT / "examples/widgets/LICENSE").read_text()
        terms = notice[notice.index("Permission"):]
        for example in builder.examples():
            package = example["package"]
            self.assertTrue(package["source"].startswith("// SPDX-License-Identifier: MIT\n"))
            self.assertIn("Copyright (c) 2026 Panelet contributors", package["source"])
            resources = {item["path"]: item["text"] for item in package["resources"]}
            self.assertEqual(resources["LICENSE.txt"], terms)
            archive = subprocess.check_output(["node", str(ROOT / "examples/widgets/build-upload.mjs"), example["id"]])
            with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
                self.assertEqual(zipped.read("main.rn").decode(), package["source"])
                self.assertEqual(zipped.read("resources/LICENSE.txt").decode(), terms)

    def test_bool_version_and_unordered_paths_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / "bad.pna"
            for manifest in [
                {"schema_version": True, "api_version": 1, "files": []},
                {"schema_version": 1, "api_version": True, "files": []},
                {"schema_version": 1, "api_version": 1, "files": [{"path": "/../escape", "content_type": "text/plain", "encoding": "identity", "size": 0, "sha256": hashlib.sha256(b"").hexdigest()}]},
            ]:
                data = json.dumps(manifest).encode()
                bundle.write_bytes(b"N4A1" + struct.pack("<I", len(data)) + data)
                with self.assertRaises(ValueError):
                    deploy.parse_bundle(bundle)

    def test_truncated_response_retries_exact_idempotent_request(self):
        seen = []
        class Response:
            def __init__(self, truncated):
                self.truncated = truncated
            def __enter__(self):
                return self
            def __exit__(self, *args):
                return False
            def read(self, limit):
                if self.truncated:
                    raise http.client.IncompleteRead(b'{"bundle_id":', 12)
                return b'{"bundle_id":"confirmed"}'
        class Opener:
            def open(self, request, timeout):
                seen.append((request.full_url, request.data, dict(request.header_items())))
                return Response(len(seen) == 1)
        client = deploy.Client("http://127.0.0.1:12345", "fake-test-token")
        client.opener = Opener()
        with patch.object(deploy.time, "sleep"):
            result = client.request("/web/v1/commit", {"bundle_id": "abc"})
        self.assertEqual(result, {"bundle_id": "confirmed"})
        self.assertEqual(len(seen), 2)
        self.assertEqual(seen[0], seen[1])

    def test_already_installed_bundle_uses_read_only_status(self):
        with tempfile.TemporaryDirectory() as directory:
            wasm = Path(directory) / "preview.wasm"
            wasm.write_bytes(b"\0asm\x01\0\0\0")
            bundle = Path(directory) / "bundle.pna"
            summary = builder.build(wasm, bundle, 1)
            calls = []
            class Installed:
                def request(self, route, *args):
                    calls.append(route)
                    return {"storage": {"ready": True}, "web": {"installed": [
                        {"api_version": 1, "active": {"bundle_id": summary["bundle_id"]}}]}}
            result = deploy.install(Installed(), bundle, progress=lambda _: None)
            self.assertEqual(result["bundle_id"], summary["bundle_id"])
            self.assertEqual(calls, ["/web/v1/status"])

    def test_redirect_does_not_send_token_to_another_origin(self):
        hits = []
        class Destination(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                hits.append(self.headers.get("X-Note4c-Token"))
                self.send_response(200); self.end_headers(); self.wfile.write(b"{}")
            def log_message(self, *args):
                pass
        with contextlib.ExitStack() as stack:
            destination = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Destination)
            stack.callback(destination.server_close)
            class Redirect(Destination):
                def do_GET(self):
                    self.send_response(302)
                    self.send_header("Location", f"http://127.0.0.1:{destination.server_port}/capture")
                    self.end_headers()
            source = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Redirect)
            stack.callback(source.server_close)
            threads = []
            for server in (source, destination):
                thread = threading.Thread(target=server.serve_forever, daemon=True)
                thread.start(); threads.append(thread)
                stack.callback(server.shutdown)
            client = deploy.Client(f"http://127.0.0.1:{source.server_port}", "fake-test-token")
            with self.assertRaisesRegex(ValueError, "HTTP 302"):
                client.request("/web/v1/status")
            self.assertEqual(hits, [])


if __name__ == "__main__":
    unittest.main()
