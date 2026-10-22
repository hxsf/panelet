#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Embed firmware-coupled HTML/CSS/JS only; never build/embed the WASM engine."""
import gzip
from pathlib import Path
import re
import sys


def build(manifest: Path, output: Path, api: int):
    assert 0 < api <= 0xFFFFFFFF
    output.mkdir(parents=True, exist_ok=True)
    admin = (manifest / "src/server.html").read_text()
    editor = (manifest / "src/editor/editor.html").read_text()
    editor, count = re.subn(r'<html\b([^>]*)>',
                            lambda m: f'<html{m[1]} data-preview-engine="wasm" data-assets-api="{api}">',
                            editor, count=1)
    assert count == 1
    css = ("/* SPDX-License-Identifier: MPL-2.0 */\n"
           + re.search(r"<style>([\s\S]*?)</style>", admin).group(1))
    resources = {
        "server.html": admin,
        "editor.html": editor,
        "editor.css": css,
        "editor-worker.js": (manifest / "src/editor/preview-worker.js").read_text(),
    }
    for name, text in resources.items():
        raw = text.encode()
        data = gzip.compress(raw, compresslevel=9, mtime=0)
        assert gzip.decompress(data) == raw
        (output / (name + ".gz")).write_bytes(data)
        print(f"Embedded {name}: {len(raw)} -> {len(data)} gzip bytes")


if __name__ == "__main__":
    build(Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3]))
