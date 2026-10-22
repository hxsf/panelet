#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Exercise desktop web-assets HTTP with a real temporary store, never a device.

Build panelet-studio with the desktop-editor feature, then pass --bin (as in
test-widget-editor.py) or set NOTE4C_EDITOR_BIN. An existing
target/web-assets/bundle.pna is tested automatically, falling back to legacy
bundle.n4a when absent; --bundle or NOTE4C_WEB_ASSETS_BUNDLE selects an explicit
archive of either name. The N4A1 contents are unchanged.
"""

import argparse
from dataclasses import dataclass
import gzip
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[2]
TOKEN = "preview-token"
CHUNK_SIZE = 32 * 1024
TOTAL_BYTES = 8 * 1024 * 1024
ASSET_CSP = "default-src 'none'; sandbox; frame-ancestors 'none'; base-uri 'none'"
BINARY = None
REAL_BUNDLE = None


def compact_json(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def sha256(value):
    return hashlib.sha256(value).hexdigest()


def unsigned_leb128(value):
    encoded = bytearray()
    while value >= 128:
        encoded.append((value & 127) | 128)
        value >>= 7
    encoded.append(value)
    return bytes(encoded)


@dataclass
class Bundle:
    manifest: dict
    files: list

    @property
    def bundle_id(self):
        # Matches WebAssetManifest's documented canonical field ordering.
        manifest = {
            "schema_version": self.manifest["schema_version"],
            "api_version": self.manifest["api_version"],
            "files": [{key: file[key] for key in
                       ("path", "content_type", "encoding", "size", "sha256")}
                      for file in self.manifest["files"]],
        }
        return sha256(compact_json(manifest))

    def index(self, path):
        return next(index for index, file in enumerate(self.manifest["files"])
                    if file["path"] == path)


def fixture_bundle(label="first", api=1):
    # A valid module containing only a custom section. Deterministic entropy
    # makes the gzip payload span three chunks, unlike a compressible zero pad.
    entropy = b"".join(hashlib.sha256(f"{label}:{index}".encode()).digest()
                       for index in range(2060))
    custom = b"\x07fixture" + entropy
    wasm = b"\x00asm\x01\x00\x00\x00\x00" + unsigned_leb128(len(custom)) + custom
    source = [
        ("/examples.json", "application/json", "gzip", compact_json([{"name": label}])),
        ("/license.txt", "text/plain; charset=utf-8", "identity", b"Fixture data only.\n"),
        ("/page.html", "text/html", "identity",
         b"<!doctype html><title>Untrusted asset</title><script>document.title='executed';</script>"),
        ("/preview.wasm", "application/wasm", "gzip", wasm),
    ]
    files, metadata = [], []
    for path, content_type, encoding, value in sorted(source):
        data = gzip.compress(value, mtime=0) if encoding == "gzip" else value
        files.append(data)
        metadata.append({"path": path, "content_type": content_type,
                         "encoding": encoding, "size": len(data), "sha256": sha256(data)})
    return Bundle({"schema_version": 1, "api_version": api, "files": metadata}, files)


def read_archive(path):
    data = path.read_bytes()
    if len(data) < 8 or data[:4] != b"N4A1":
        raise ValueError("Archive lacks the N4A1 header")
    length, = struct.unpack_from("<I", data, 4)
    if not 0 < length <= 16384 or length + 8 > len(data):
        raise ValueError("Invalid archive manifest length")
    manifest = json.loads(data[8:8 + length])
    if manifest["schema_version"] != 1:
        raise ValueError("Unsupported archive schema")
    paths = [file["path"] for file in manifest["files"]]
    if paths != sorted(set(paths)):
        raise ValueError("Archive files are not uniquely path-sorted")
    offset, files = 8 + length, []
    for file in manifest["files"]:
        value = data[offset:offset + file["size"]]
        if len(value) != file["size"] or sha256(value) != file["sha256"]:
            raise ValueError("Archive file size or SHA-256 is invalid: " + file["path"])
        files.append(value)
        offset += len(value)
    if offset != len(data):
        raise ValueError("Archive has trailing data")
    return Bundle(manifest, files)


def default_bundle(root=ROOT):
    for filename in ("bundle.pna", "bundle.n4a"):
        candidate = root / "target" / "web-assets" / filename
        if candidate.is_file():
            return candidate
    return None


class ArchiveSelectionTests(unittest.TestCase):
    def test_default_prefers_pna_and_falls_back_to_legacy_n4a(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            assets = root / "target" / "web-assets"
            assets.mkdir(parents=True)
            self.assertIsNone(default_bundle(root))
            legacy, current = assets / "bundle.n4a", assets / "bundle.pna"
            legacy.touch()
            self.assertEqual(default_bundle(root), legacy)
            current.touch()
            self.assertEqual(default_bundle(root), current)
            current.unlink()
            self.assertEqual(default_bundle(root), legacy)

    def test_new_and_legacy_archive_names_preserve_the_same_contents(self):
        bundle = fixture_bundle()
        manifest = compact_json(bundle.manifest)
        data = b"N4A1" + struct.pack("<I", len(manifest)) + manifest + b"".join(bundle.files)
        with tempfile.TemporaryDirectory() as directory:
            for extension in ("pna", "n4a"):
                with self.subTest(extension=extension):
                    path = Path(directory) / ("bundle." + extension)
                    path.write_bytes(data)
                    self.assertEqual(read_archive(path), bundle)


@dataclass
class Response:
    status: int
    headers: dict
    body: bytes

    def json(self):
        return json.loads(self.body)

    def decoded(self):
        return gzip.decompress(self.body) if self.headers.get("content-encoding") == "gzip" else self.body


class WebAssetsHttpTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="panelet-web-assets-http-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.assets = self.root / "assets"
        self.assets.mkdir()
        self.sentinel = self.assets / "user-data.keep"
        self.sentinel.write_bytes(b"Existing widget/user files must survive resource updates.\n")
        self.server = None
        self.server_logs = []
        self.addCleanup(self.stop_server)
        self.start_server()

    def start_server(self):
        serial = len(self.server_logs)
        stdout = (self.root / f"server-{serial}.stdout").open("w+")
        stderr = (self.root / f"server-{serial}.stderr").open("w+")
        self.server_logs.extend([stdout, stderr])
        self.server = subprocess.Popen(
            [str(BINARY), "--web-assets", str(self.assets), "--port", "0"],
            stdout=stdout, stderr=stderr, text=True, cwd=ROOT)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            stdout.seek(0)
            output = stdout.read()
            match = re.search(r"127\.0\.0\.1:(\d+)", output)
            if match:
                self.port = int(match.group(1))
                return
            if self.server.poll() is not None:
                break
            time.sleep(0.025)
        stderr.seek(0)
        self.fail("Editor did not start in --web-assets mode:\n" + output + stderr.read())

    def stop_server(self):
        if self.server is not None:
            if self.server.poll() is None:
                self.server.terminate()
                try:
                    self.server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.server.kill()
                    self.server.wait(timeout=5)
            self.server = None
        for stream in self.server_logs:
            if not stream.closed:
                stream.close()

    def restart_server(self):
        self.stop_server()
        self.start_server()

    def request(self, method, path, body=None, *, auth=False,
                content_type="application/json", headers=None):
        request_headers = dict(headers or {})
        if body is not None:
            request_headers.setdefault("Content-Type", content_type)
        if auth:
            request_headers.setdefault("X-Note4c-Token", TOKEN)
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=15)
        try:
            connection.request(method, path, body=body, headers=request_headers)
            response = connection.getresponse()
            return Response(response.status, dict((key.lower(), value)
                            for key, value in response.getheaders()), response.read())
        finally:
            connection.close()

    def post(self, operation, value, **kwargs):
        kwargs.setdefault("auth", True)
        return self.request("POST", "/web/v1/" + operation, compact_json(value), **kwargs)

    def require_json(self, response, status=200):
        self.assertEqual(response.status, status, response.body[:1000])
        self.assertTrue(response.headers.get("content-type", "").startswith("application/json"))
        return response.json()

    def status(self):
        result = self.require_json(self.request("GET", "/web/v1/status", auth=True))
        self.assertEqual(result["api_version"], 1)
        self.assertIs(result["simulated"], True)
        self.assertIs(result["storage"]["ready"], True)
        self.assertIs(result["storage"]["simulated"], True)
        self.assertEqual(result["storage"]["total_bytes"], TOTAL_BYTES)
        used = sum(path.stat().st_size for path in self.assets.iterdir() if path.is_file())
        self.assertEqual(result["storage"]["used_bytes"], used)
        self.assertEqual(result["storage"]["free_bytes"], TOTAL_BYTES - used)
        self.assertEqual(result["storage"]["safe_available_bytes"], max(0, TOTAL_BYTES * 3 // 4 - used))
        self.assertEqual(result["web"]["api_version"], 1)
        self.assertEqual(result["web"]["issues"], [])
        self.assertEqual(self.sentinel.read_bytes(), b"Existing widget/user files must survive resource updates.\n")
        return result["web"]

    def assert_progress(self, actual, bundle):
        self.assertEqual(actual["bundle_id"], bundle.bundle_id)
        self.assertEqual(len(actual["files"]), len(bundle.files))
        for progress, file in zip(actual["files"], bundle.manifest["files"]):
            self.assertEqual(progress["path"], file["path"])
            self.assertEqual(progress["size"], file["size"])
            self.assertGreaterEqual(progress["received"], 0)
            self.assertLessEqual(progress["received"], file["size"])
            self.assertEqual(progress["complete"], progress["received"] == file["size"])
        return actual

    def begin(self, bundle):
        return self.assert_progress(self.require_json(self.post("begin", bundle.manifest)), bundle)

    def chunk(self, bundle, index, offset, data, **kwargs):
        kwargs.setdefault("auth", True)
        return self.request("POST", f"/web/v1/chunk?bundle={bundle.bundle_id}&file={index}&offset={offset}",
                            data, content_type="application/octet-stream", **kwargs)

    def upload_remaining(self, bundle, progress):
        for index, data in enumerate(bundle.files):
            offset = progress["files"][index]["received"]
            while offset < len(data):
                part = data[offset:offset + CHUNK_SIZE]
                progress = self.assert_progress(self.require_json(self.chunk(bundle, index, offset, part)), bundle)
                self.assertEqual(progress["files"][index]["received"], offset + len(part))
                offset += len(part)
        return progress

    def install(self, bundle):
        self.upload_remaining(bundle, self.begin(bundle))
        committed = self.require_json(self.post("commit", {"bundle_id": bundle.bundle_id}))
        self.assertEqual(committed["bundle_id"], bundle.bundle_id)
        self.assertEqual(committed["manifest"], bundle.manifest)
        self.assertGreater(committed["generation"], 0)
        return committed

    def manifest(self, api=1):
        return self.require_json(self.request("GET", f"/web/v1/manifest?api={api}"))

    def asset_path(self, bundle, index):
        return "/web/" + bundle.bundle_id + bundle.manifest["files"][index]["path"]

    def assert_assets(self, bundle):
        for index, metadata in enumerate(bundle.manifest["files"]):
            response = self.request("GET", self.asset_path(bundle, index))
            self.assertEqual(response.status, 200, response.body[:500])
            self.assertEqual(response.body, bundle.files[index])
            self.assertEqual(response.headers["content-type"], metadata["content_type"])
            self.assertEqual(response.headers.get("content-encoding"),
                             "gzip" if metadata["encoding"] == "gzip" else None)
            self.assertEqual(response.headers["etag"], '"' + metadata["sha256"] + '"')
            self.assertIn("immutable", response.headers["cache-control"])
            self.assertIn("max-age=31536000", response.headers["cache-control"])
            self.assertEqual(response.headers["x-content-type-options"], "nosniff")
            self.assertEqual(response.headers.get("content-security-policy"), ASSET_CSP)
            if metadata["encoding"] == "gzip":
                self.assertGreater(len(response.decoded()), 0)
            cached = self.request("GET", self.asset_path(bundle, index),
                                  headers={"If-None-Match": response.headers["etag"]})
            self.assertEqual(cached.status, 304)
            self.assertEqual(cached.body, b"")
            self.assertEqual(cached.headers["etag"], response.headers["etag"])
            self.assertIn("immutable", cached.headers["cache-control"])
            self.assertEqual(cached.headers.get("content-security-policy"), ASSET_CSP)
            stale = self.request("GET", self.asset_path(bundle, index),
                                 headers={"If-None-Match": '"outdated"'})
            self.assertEqual(stale.status, 200)
            self.assertEqual(stale.body, bundle.files[index])

    def test_empty_store_and_browser_only_routes(self):
        web = self.status()
        self.assertIsNone(web["active"])
        self.assertIsNone(web["previous"])
        self.assertIsNone(web["pending"])
        self.assertEqual(web["installed"], [])
        self.assertEqual(self.request("GET", "/web/v1/manifest?api=1").status, 404)
        self.assertEqual(self.post("rollback", {}).status, 404)
        for path in ("/", "/editor"):
            with self.subTest(path=path):
                response = self.request("GET", path)
                self.assertEqual(response.status, 200)
                page = response.decoded().decode()
                self.assertRegex(page, r'data-preview-engine=[\"\']wasm[\"\']')
                self.assertRegex(page, r'data-assets-api=[\"\']1[\"\']')
                self.assertIn("no-store", response.headers.get("cache-control", ""))
                policy = response.headers.get("content-security-policy", "")
                self.assertIn("'wasm-unsafe-eval'", policy)
                self.assertNotRegex(policy, r"(?:^|;)\s*sandbox(?:\s|;|$)")
        for path, fragment in (("/editor-worker.js", b"WebAssembly"), ("/editor.css", b"font")):
            response = self.request("GET", path)
            self.assertEqual(response.status, 200)
            self.assertIn(fragment, response.decoded())
            if path == "/editor-worker.js":
                policy = response.headers.get("content-security-policy", "")
                self.assertIn("'wasm-unsafe-eval'", policy)
                self.assertNotRegex(policy, r"(?:^|;)\s*sandbox(?:\s|;|$)")
        for path in ("/preview/v1", "/package/encode", "/package/decode"):
            with self.subTest(native_path=path):
                content_type = "application/octet-stream" if path == "/package/decode" else "application/json"
                self.assertEqual(self.request("POST", path, b"{}", auth=True,
                                              content_type=content_type).status, 404)

    def test_html_asset_uses_sandbox_policy_on_initial_and_cached_responses(self):
        # The manifest permits arbitrary valid MIME types. Uploaded HTML must
        # never inherit the firmware page's script or same-origin permissions.
        bundle = fixture_bundle("html-resource")
        self.install(bundle)
        index = bundle.index("/page.html")
        response = self.request("GET", self.asset_path(bundle, index))
        self.assertEqual(response.status, 200)
        self.assertEqual(response.headers["content-type"], "text/html")
        self.assertEqual(response.body, bundle.files[index])
        self.assertEqual(response.headers.get("content-security-policy"), ASSET_CSP)
        cached = self.request("GET", self.asset_path(bundle, index),
                              headers={"If-None-Match": response.headers["etag"]})
        self.assertEqual(cached.status, 304)
        self.assertEqual(cached.headers.get("content-security-policy"), ASSET_CSP)
        self.assertEqual(cached.body, b"")

    def test_token_origin_and_content_type_boundaries(self):
        bundle = fixture_bundle()
        for headers in ({}, {"X-Note4c-Token": "incorrect"}):
            self.assertEqual(self.request("GET", "/web/v1/status", headers=headers).status, 401)
            for operation, value in (("begin", bundle.manifest), ("commit", {"bundle_id": bundle.bundle_id}),
                                     ("rollback", {})):
                self.assertEqual(self.post(operation, value, auth=False, headers=headers).status, 401)
            self.assertEqual(self.chunk(bundle, 0, 0, b"x", auth=False, headers=headers).status, 401)
        for headers in ({"Origin": "https://unrelated.example"}, {"Host": "unrelated.example"},
                        {"Origin": "null"}, {"Sec-Fetch-Site": "cross-site"}):
            with self.subTest(headers=headers):
                self.assertEqual(self.post("begin", bundle.manifest, headers=headers).status, 403)
        self.assertEqual(self.post("begin", bundle.manifest, content_type="text/plain").status, 415)
        self.assertEqual(self.post("begin", bundle.manifest,
                                   headers={"Origin": f"http://127.0.0.1:{self.port}"}).status, 200)
        self.assertIsNone(self.status()["active"])

    def test_resumable_upload_survives_restart_and_publishes_atomically(self):
        first = fixture_bundle("first")
        first_commit = self.install(first)
        second = fixture_bundle("second")
        progress = self.begin(second)
        index = second.index("/preview.wasm")
        self.assertGreater(len(second.files[index]), CHUNK_SIZE * 2)
        part = second.files[index][:CHUNK_SIZE]
        progress = self.require_json(self.chunk(second, index, 0, part))
        self.assertEqual(progress["files"][index]["received"], CHUNK_SIZE)
        # Replaying a delivered chunk is safe after a lost acknowledgement.
        self.assertEqual(self.require_json(self.chunk(second, index, 0, part)), progress)
        self.assertEqual(self.chunk(second, index, 0, b"different").status, 409)
        self.assertEqual(self.chunk(second, index, CHUNK_SIZE + 1, b"x").status, 409)
        self.assertEqual(self.post("commit", {"bundle_id": second.bundle_id}).status, 400)
        self.assertEqual(self.manifest()["bundle_id"], first.bundle_id)
        self.assertEqual(self.request("GET", self.asset_path(second, index)).status, 404)
        self.restart_server()
        web = self.status()
        self.assertEqual(web["active"]["bundle_id"], first.bundle_id)
        self.assertEqual(web["pending"]["bundle_id"], second.bundle_id)
        self.assertEqual(web["pending"]["files"][index]["received"], CHUNK_SIZE)
        progress = self.begin(second)
        self.assertEqual(progress["files"][index]["received"], CHUNK_SIZE)
        self.upload_remaining(second, progress)
        committed = self.require_json(self.post("commit", {"bundle_id": second.bundle_id}))
        self.assertGreater(committed["generation"], first_commit["generation"])
        self.assertEqual(self.require_json(self.post("commit", {"bundle_id": second.bundle_id})), committed)
        self.assertEqual(self.manifest()["bundle_id"], second.bundle_id)
        self.assertEqual(self.status()["previous"]["bundle_id"], first.bundle_id)
        self.assertIsNone(self.status()["pending"])
        self.assert_assets(first)
        self.assert_assets(second)

    def test_bad_hash_and_abandoned_upload_never_replace_active(self):
        active = fixture_bundle("good")
        self.install(active)
        candidate = fixture_bundle("candidate")
        self.begin(candidate)
        index = candidate.index("/examples.json")
        damaged = bytearray(candidate.files[index])
        damaged[-1] ^= 1
        self.assertEqual(self.chunk(candidate, index, 0, damaged).status, 400)
        self.assertEqual(self.post("commit", {"bundle_id": candidate.bundle_id}).status, 400)
        self.assertEqual(self.manifest()["bundle_id"], active.bundle_id)
        self.assertEqual(self.status()["pending"]["files"][index]["received"], 0)
        # Cancel means the client stops submitting; even a complete pending
        # upload must remain invisible until the explicit commit request.
        self.upload_remaining(candidate, self.begin(candidate))
        self.assertEqual(self.manifest()["bundle_id"], active.bundle_id)
        self.restart_server()
        self.assertEqual(self.manifest()["bundle_id"], active.bundle_id)
        self.assertTrue(all(file["complete"] for file in self.status()["pending"]["files"]))
        replacement = fixture_bundle("replacement")
        self.install(replacement)
        self.assertEqual(self.status()["previous"]["bundle_id"], active.bundle_id)
        self.assertEqual(self.request("GET", self.asset_path(candidate, index)).status, 404)
        self.assert_assets(active)

    def test_api_versions_coexist_and_rollback_is_compatible(self):
        old = fixture_bundle("old")
        new = fixture_bundle("new")
        future = fixture_bundle("future", api=2)
        self.install(old)
        self.install(new)
        self.install(future)
        self.assertEqual(self.manifest(1)["bundle_id"], new.bundle_id)
        self.assertEqual(self.manifest(2)["bundle_id"], future.bundle_id)
        self.assertEqual(self.request("GET", "/web/v1/manifest?api=3").status, 404)
        self.assertEqual(self.request("GET", "/web/v1/manifest").json()["bundle_id"], new.bundle_id)
        web = self.status()
        self.assertEqual(web["active"]["bundle_id"], new.bundle_id)
        self.assertEqual({version["api_version"] for version in web["installed"]}, {1, 2})
        rolled_back = self.require_json(self.post("rollback", {}))
        self.assertEqual(rolled_back["bundle_id"], old.bundle_id)
        self.assertEqual(self.manifest(1)["bundle_id"], old.bundle_id)
        self.assertEqual(self.manifest(2)["bundle_id"], future.bundle_id)
        self.restart_server()
        self.assertEqual(self.status()["active"]["bundle_id"], old.bundle_id)
        self.assertEqual(self.status()["previous"]["bundle_id"], new.bundle_id)
        for bundle in (old, new, future):
            self.assert_assets(bundle)

    def test_shared_storage_budget_rejects_upload_without_replacing_active(self):
        previous = fixture_bundle("previous")
        self.install(previous)
        active = fixture_bundle("installed")
        self.install(active)
        pending = fixture_bundle("pending")
        self.upload_remaining(pending, self.begin(pending))
        # Other assets users count toward the same conservative 75% budget.
        # A sparse temporary file tests accounting without a physical device.
        with (self.assets / "other-user-data.bin").open("wb") as file:
            file.truncate(TOTAL_BYTES * 3 // 4)
        result = self.require_json(self.request("GET", "/web/v1/status", auth=True))
        self.assertEqual(result["storage"]["safe_available_bytes"], 0)
        self.assertGreater(result["storage"]["free_bytes"], 0)
        self.assertEqual(self.post("begin", fixture_bundle("new").manifest).status, 507)
        self.assertEqual(self.post("commit", {"bundle_id": pending.bundle_id}).status, 507)
        self.assertEqual(self.post("rollback", {}).status, 507)
        self.assertEqual(self.manifest()["bundle_id"], active.bundle_id)
        web = self.status()
        self.assertEqual(web["active"]["bundle_id"], active.bundle_id)
        self.assertEqual(web["previous"]["bundle_id"], previous.bundle_id)
        self.assertEqual(web["pending"]["bundle_id"], pending.bundle_id)
        self.assertTrue(all(file["complete"] for file in web["pending"]["files"]))
        self.assert_assets(previous)
        self.assert_assets(active)

    def test_invalid_paths_queries_and_request_limits(self):
        bundle = fixture_bundle()
        for path in ("../escape", "/../escape", "/a/../escape", "/a//b", "/%2e%2e/escape"):
            with self.subTest(manifest_path=path):
                bad = json.loads(json.dumps(bundle.manifest))
                bad["files"][0]["path"] = path
                self.assertEqual(self.post("begin", bad).status, 400)
        self.assertIsNone(self.status()["pending"])
        for query in ("api=0", "api=no", "api=1&api=2", "api=1&unexpected=1", ""):
            self.assertEqual(self.request("GET", "/web/v1/manifest?" + query).status, 400)
        self.begin(bundle)
        for query in (f"bundle={bundle.bundle_id}&file=0", f"bundle={bundle.bundle_id}&file=0&file=1&offset=0",
                      f"bundle=invalid&file=0&offset=0", f"bundle={bundle.bundle_id}&file=-1&offset=0",
                      f"bundle={bundle.bundle_id}&file=0&offset=-1"):
            self.assertEqual(self.request("POST", "/web/v1/chunk?" + query, b"x", auth=True,
                                          content_type="application/octet-stream").status, 400)
        index = bundle.index("/preview.wasm")
        self.assertEqual(self.chunk(bundle, index, 0, b"x" * (CHUNK_SIZE + 1)).status, 413)
        self.assertEqual(self.request("POST", "/web/v1/begin", b"x" * 16385, auth=True).status, 413)
        self.assertEqual(self.post("begin", {}).status, 400)
        self.assertEqual(self.post("rollback", {"api": 2}).status, 400)
        self.assertEqual(self.post("commit", {"bundle_id": "invalid"}).status, 400)
        self.install(bundle)
        for path in ("/web/invalid/preview.wasm", f"/web/{bundle.bundle_id}/../user-data.keep",
                     f"/web/{bundle.bundle_id}/%2e%2e/user-data.keep", f"/web/{bundle.bundle_id}/missing",
                     f"/web/{bundle.bundle_id}//preview.wasm", "/missing"):
            with self.subTest(asset_path=path):
                self.assertEqual(self.request("GET", path).status, 404)
        self.status()

    def test_generated_resource_archive_if_available(self):
        if REAL_BUNDLE is None:
            self.skipTest("No generated .pna or legacy .n4a bundle; pass --bundle to test it")
        bundle = read_archive(REAL_BUNDLE)
        self.install(bundle)
        manifest = self.manifest(bundle.manifest["api_version"])
        self.assertEqual(manifest["bundle_id"], bundle.bundle_id)
        self.assertEqual(manifest["files"], bundle.manifest["files"])
        self.assert_assets(bundle)
        wasm_index = bundle.index("/preview.wasm")
        wasm = self.request("GET", self.asset_path(bundle, wasm_index)).decoded()
        self.assertEqual(wasm[:8], b"\x00asm\x01\x00\x00\x00")
        examples_index = bundle.index("/examples.json")
        examples = json.loads(self.request("GET", self.asset_path(bundle, examples_index)).decoded())
        self.assertIsInstance(examples, list)
        self.assertGreater(len(examples), 0)
        self.restart_server()
        self.assertEqual(self.manifest(bundle.manifest["api_version"])["bundle_id"], bundle.bundle_id)
        self.status()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path, default=os.environ.get("NOTE4C_EDITOR_BIN"),
                        help="panelet-studio binary; also NOTE4C_EDITOR_BIN")
    parser.add_argument("--bundle", type=Path, default=os.environ.get("NOTE4C_WEB_ASSETS_BUNDLE"),
                        help="optional generated .pna or legacy .n4a archive; also NOTE4C_WEB_ASSETS_BUNDLE")
    args, remaining = parser.parse_known_args()
    if args.bin is None:
        parser.error("pass --bin or set NOTE4C_EDITOR_BIN (build with desktop-editor)")
    BINARY = args.bin.resolve()
    if not BINARY.is_file() or not os.access(BINARY, os.X_OK):
        parser.error("--bin must name an executable panelet-studio binary")
    if args.bundle is not None:
        REAL_BUNDLE = args.bundle.resolve()
        if not REAL_BUNDLE.is_file():
            parser.error("--bundle must name an existing .pna or legacy .n4a archive")
    else:
        REAL_BUNDLE = default_bundle()
    unittest.main(argv=[__file__, *remaining])
