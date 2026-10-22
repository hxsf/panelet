#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Exercise the real desktop HTTP server/worker; never contact a device."""
import argparse
import http.client
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest


class EditorTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = subprocess.Popen([str(BINARY), "--port", "0"], stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True)
        line = cls.server.stdout.readline()
        match = re.search(r"127\.0\.0\.1:(\d+)", line)
        if not match:
            cls.server.kill()
            cls.server.wait()
            raise RuntimeError("Editor did not print a loopback URL: " + line)
        cls.port = int(match.group(1))
        cls.examples = cls.get("/examples")[1]

    @classmethod
    def tearDownClass(cls):
        cls.server.terminate()
        cls.server.wait(timeout=5)
        cls.server.stdout.close()
        cls.server.stderr.close()

    @classmethod
    def exchange(cls, method, path, body=None, content_type="application/json", extra=None):
        connection = http.client.HTTPConnection("127.0.0.1", cls.port, timeout=15)
        headers = {"Content-Type": content_type}
        headers.update(extra or {})
        connection.request(method, path, body=body, headers=headers)
        response = connection.getresponse()
        data = response.read()
        connection.close()
        if response.getheader("Content-Type", "").startswith("application/json"):
            data = json.loads(data)
        return response.status, data

    @classmethod
    def get(cls, path):
        return cls.exchange("GET", path)

    def preview(self, example, events=()):
        request = {key: example[key] for key in ("package", "config", "environment", "host_permissions")}
        request["events"] = events
        return self.exchange("POST", "/preview/v1", json.dumps(request).encode())

    def test_examples_real_worker_and_inputs(self):
        for example in self.examples:
            status, result = self.preview(example)
            self.assertEqual(status, 200)
            self.assertTrue(result["ok"], result["error"])
            self.assertEqual(len(result["pixels"]), 30000)
            self.assertGreater(len(set(result["pixels"])), 1)
        counter = self.examples[0]
        baseline = self.preview(counter)[1]["pixels"]
        changed = self.preview(counter, [{"kind": "click", "key": "boot"}])[1]
        self.assertTrue(changed["ok"])
        self.assertNotEqual(baseline, changed["pixels"])
        self.assertEqual(self.preview(counter)[1]["pixels"], baseline)

    def test_package_roundtrip_and_invalid_crc(self):
        package = self.examples[0]["package"]
        status, encoded = self.exchange("POST", "/package/encode", json.dumps(package).encode())
        self.assertEqual(status, 200)
        status, decoded = self.exchange("POST", "/package/decode", encoded, "application/octet-stream")
        self.assertEqual(status, 200)
        self.assertEqual(decoded, package)
        damaged = bytearray(encoded)
        damaged[50] ^= 1
        self.assertEqual(self.exchange("POST", "/package/decode", damaged, "application/octet-stream")[0], 400)

    def test_error_then_worker_recovery(self):
        example = json.loads(json.dumps(self.examples[0]))
        example["package"]["source"] = "struct { broken"
        status, result = self.preview(example)
        self.assertEqual(status, 200)
        self.assertFalse(result["ok"])
        self.assertEqual(result["error"]["code"], "rune_compile")
        self.assertIn("main.rn:", result["error"]["message"])
        self.assertIsNone(result["pixels"])
        example["package"]["source"] = "struct Widget {} #[widget::component] impl Widget { pub fn new() { Widget {} } #[widget::mounted] pub async fn mounted(self) { loop {} os::runtime::ok(()) } }"
        self.assertFalse(self.preview(example)[1]["ok"])
        self.assertTrue(self.preview(self.examples[0])[1]["ok"])

    def test_http_boundaries_and_served_ui(self):
        self.assertEqual(self.get("/")[0], 200)
        self.assertIn(b"400", self.get("/")[1])
        self.assertIn(b"font", self.get("/editor.css")[1])
        self.assertEqual(self.get("/missing")[0], 404)
        self.assertEqual(self.exchange("POST", "/preview/v1", b"{}", extra={"Origin": "https://unrelated.example"})[0], 403)
        self.assertEqual(self.exchange("POST", "/preview/v1", b"{}", extra={"Host": "unrelated.example"})[0], 403)
        self.assertEqual(self.exchange("POST", "/preview/v1", b"x" * 65537)[0], 413)
        self.assertEqual(self.exchange("POST", "/preview/v1", b"{}", "text/plain")[0], 415)

    def test_launcher_explicit_host_target_and_toolchain_fallback(self):
        launcher = Path(__file__).resolve().parents[1] / "run-widget-editor.sh"
        for fallback in (False, True):
            with self.subTest(fallback=fallback), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                shim = root / "bin"
                shim.mkdir()
                native = root / "toolchains" / "stable-test" / "bin"
                native.mkdir(parents=True)
                host_bin = native if fallback else shim
                log = root / "cargo-args"
                (host_bin / "rustc").write_text('#!/bin/sh\nprintf "rustc 1.90.0\\nhost: aarch64-apple-darwin\\n"\n')
                (host_bin / "cargo").write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$EDITOR_TEST_ARGS"\n')
                if fallback:
                    (shim / "rustc").write_text('#!/bin/sh\nexit 1\n')
                    (shim / "rustc").chmod(0o755)
                for name in ("rustc", "cargo"):
                    (host_bin / name).chmod(0o755)
                env = dict(os.environ, PATH=str(shim) + os.pathsep + os.environ["PATH"],
                           RUSTUP_HOME=str(root), EDITOR_TEST_ARGS=str(log))
                subprocess.run([str(launcher), "--port", "18746"], env=env, check=True, capture_output=True)
                arguments = log.read_text().splitlines()
                expected = ["run", "-p", "panelet-os", "--no-default-features", "--features", "desktop-editor",
                            "--bin", "panelet-studio", "--target", "aarch64-apple-darwin", "--", "--port", "18746"]
                self.assertEqual(arguments, ([] if fallback else ["+stable"]) + expected)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path, required=True)
    args, remaining = parser.parse_known_args()
    BINARY = args.bin.resolve()
    unittest.main(argv=[__file__, *remaining])
