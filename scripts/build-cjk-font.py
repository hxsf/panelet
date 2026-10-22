#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Rebuild the bounded Panelet Unifont subset without network access."""
import gzip
import hashlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / '3rd-res/fonts/unifont_all-16.0.04.hex.gz'
SHA256 = '20e8b505f602488697979eefc69857f7f6106bceab702f5ac559f4f84e0e7494'
raw = SOURCE.read_bytes()
if hashlib.sha256(raw).hexdigest() != SHA256:
    raise SystemExit('Unifont snapshot checksum mismatch')
characters = (ROOT / 'crates/panelet-ui/src/cjk-subset.txt').read_text().strip()
if list(characters) != sorted(set(characters)):
    raise SystemExit('Font subset must contain sorted, unique characters')
glyphs = {}
for line in gzip.decompress(raw).decode('ascii').splitlines():
    code, data = line.split(':')
    glyphs[int(code, 16)] = bytes.fromhex(data)
output = bytearray()
for character in characters:
    bitmap = glyphs[ord(character)]
    if len(bitmap) != 32:
        raise SystemExit(f'Expected a 16x16 glyph: U+{ord(character):04X}')
    output += ord(character).to_bytes(2, 'big') + bitmap
path = ROOT / 'crates/panelet-ui/src/cjk-glyphs.dat'
path.write_bytes(output)
print(f'{len(characters)} glyphs, {len(output)} bytes')

ascii_bitmap = b''.join(glyphs[code] for code in range(32,127))
if len(ascii_bitmap) != 95 * 16:
    raise SystemExit('Expected 8x16 printable ASCII glyphs')
(ROOT / 'crates/panelet-ui/src/ascii16-glyphs.dat').write_bytes(ascii_bitmap)
print(f'95 ASCII glyphs, {len(ascii_bitmap)} bytes')
