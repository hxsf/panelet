#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Package stable preview resources; firmware-owned HTML/JS/CSS stay embedded.

Format: N4A1, u32le manifest length, canonical JSON, then each stored file in
manifest order. Resources are independently gzip-compressed with a fixed mtime.
The compatibility version is not a firmware version or an OTA slot.
"""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[1]
MANIFEST_MAX = 16 * 1024
FILE_MAX = 1536 * 1024
BUNDLE_MAX = 2 * 1024 * 1024


def canonical(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def examples(root=ROOT / "examples/widgets"):
    result = []
    for directory, name in [("counter", "按键计数器"), ("calendar", "月历"),
                            ("todo", "待办事项"), ("photo-album", "相册")]:
        path = root / directory
        resources = path / "resources"
        package = {"manifest": json.loads((path / "manifest.json").read_text()),
                   "source": (path / "main.rn").read_text(), "resources": []}
        if resources.exists():
            package["resources"] = [{"path": file.relative_to(resources).as_posix(), "text": file.read_text()}
                                    for file in sorted(resources.rglob("*")) if file.is_file()]
        environment = {"unix_ms": None, "utc_offset_minutes": 480, "http": [], "shared_files": {}}
        config, permissions = {}, None
        if directory == "todo":
            url = "https://preview.example.test/todo.json"
            config = {"url": url, "title": "待办事项", "auth_type": "none"}
            permissions = [url]
            environment["http"] = [{"url": url, "status": 200, "body": (path / "feed.example.json").read_text()}]
        result.append({"id": directory, "name": name, "package": package, "config": config,
                       "host_permissions": permissions, "environment": environment})
    return result


def build(wasm: Path, output: Path, api_version: int):
    if not 0 < api_version <= 0xFFFFFFFF:
        raise ValueError("API version must be a positive u32")
    module = wasm.read_bytes()
    if not module.startswith(b"\0asm\x01\0\0\0"):
        raise ValueError("Expected a compiled WebAssembly v1 module")
    font_notices = ("Panelet browser preview includes a subset of GNU Unifont 16.0.04.\n"
                    "Glyph shapes are unchanged; the selected glyph set and binary table format\n"
                    "are documented in 3rd-res/SOURCES.md in the Panelet source tree.\n\n"
                    + (ROOT / "3rd-res/fonts/NOTICE-16.0.04.txt").read_bytes().decode("utf-8")
                    + "\n\n" + (ROOT / "3rd-res/fonts/COPYING-16.0.04.txt").read_bytes().decode("utf-8")
                    + "\n\n" + (ROOT / "3rd-res/fonts/LICENSE.txt").read_bytes().decode("utf-8")
                    + "\n\n" + (ROOT / "3rd-res/fonts/OFL-1.1.txt").read_bytes().decode("utf-8"))
    panelet_notice = ("Panelet original firmware, editor and preview code is licensed under MPL-2.0.\n"
                      "Source code: https://github.com/hxsf/panelet\n"
                      "Bundled widget examples are MIT-licensed; their source copyright notices\n"
                      "and resources/LICENSE.txt travel with each example package. Third-party\n"
                      "code and fonts retain their own licenses and notices.\n\n"
                      + (ROOT / "LICENSE").read_text(encoding="utf-8"))
    sources = {"/examples.json": ("application/json; charset=utf-8", canonical(examples())),
               "/font-licenses.txt": ("text/plain; charset=utf-8", font_notices.encode()),
               "/panelet-license.txt": ("text/plain; charset=utf-8", panelet_notice.encode()),
               "/preview.wasm": ("application/wasm", module)}
    files, blobs = [], []
    for path, (mime, raw) in sorted(sources.items()):
        stored = gzip.compress(raw, compresslevel=9, mtime=0)
        if len(stored) > FILE_MAX:
            raise ValueError(f"{path} exceeds resource file limit after gzip")
        files.append({"path": path, "content_type": mime, "encoding": "gzip", "size": len(stored),
                      "sha256": hashlib.sha256(stored).hexdigest()})
        blobs.append(stored)
    manifest = {"schema_version": 1, "api_version": api_version, "files": files}
    encoded = canonical(manifest)
    if len(encoded) > MANIFEST_MAX or sum(map(len, blobs)) > BUNDLE_MAX:
        raise ValueError("Resource manifest or bundle exceeds its limit")
    packed = b"N4A1" + struct.pack("<I", len(encoded)) + encoded + b"".join(blobs)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(packed)
    output.with_suffix(".manifest.json").write_bytes(encoded + b"\n")
    summary = {"api_version": api_version, "bundle_id": hashlib.sha256(encoded).hexdigest(),
               "bundle_bytes": len(packed), "stored_bytes": sum(map(len, blobs)), "files": files}
    output.with_suffix(".summary.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wasm", type=Path, default=ROOT / "target/web-assets/raw/preview.wasm")
    parser.add_argument("--output", type=Path, default=ROOT / "target/web-assets/bundle.pna")
    parser.add_argument("--api-version", type=int,
                        default=int((ROOT / "app/os/web-assets-api-version").read_text().strip()))
    args = parser.parse_args()
    summary = build(args.wasm, args.output, args.api_version)
    print(f"Resource API v{summary['api_version']}: {summary['bundle_bytes']:,} B bundle")
    for file in summary["files"]:
        print(f"  {file['path']}: {file['size']:,} B ({file['encoding']})")
    print(f"Bundle ID: {summary['bundle_id']}")
    print(f"Output: {args.output.resolve()}")


if __name__ == "__main__":
    main()
