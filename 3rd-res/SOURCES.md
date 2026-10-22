# Third-party source manifest

The initial hardware-research snapshot was retrieved on 2026-09-01; later entries carry their own retrieval dates. These files are reference material, not original work of this repository.

Third-party code and font snapshots retain their upstream copyright and license terms. Panelet's own code uses MPL-2.0, with MIT-licensed widget examples; neither grant relicenses these third-party materials. The reference firmware has a top-level MIT license, but nested components have separate terms, including an ESPRESSIF MIT restriction on use with Espressif products in `components/espressif__esp_video/LICENSE` and per-file Linux header licenses. Consult each component before reuse, especially on another hardware platform.

Hardware documentation is retained in `wiki/` and `datasheets/` as versioned research references. These materials are outside Panelet's own code license and retain their original copyright notices and terms. The tables record provenance, revision, modifications and hashes; keeping a reference copy does not relicense it.

The Wiki advertises `alr` (all rights reserved). A separate redistribution grant has not been established in this manifest for the vendor documentation or the locally supplied SSD2683 proposal. This is an unresolved rights record, not a blanket determination that every reference must be removed. Any public distribution decision should be made for the specific material and its applicable terms; documenting attribution alone does not create permission. See [reference-material policy](README.md) and [distribution boundaries](../docs/development/licensing.md).

## ZECTRIX documentation

| Local path | Upstream | SHA-256 / snapshot note |
|---|---|---|
| `wiki/note4c-quick-start.html` | <https://wiki.zectrix.com/zh/hardware/note4c/quick-start> | `2cf9e3153856256c7b4c6988255cc2295c7291503f78e62f3ef3b0cfe70a1e1c`; raw HTML; page reported update time 2026-08-19 |
| `wiki/note4c-quick-start.md` | same as above | Mechanical HTML-to-GFM conversion with Pandoc; links remain upstream-relative |
| `wiki/note4c-devkit-controls-ports-en.png` | <https://wiki.zectrix.com/note4c-devkit-controls-ports-en.png> | `09bfeaf6f5ced25240962d717115bfff3a047d72289e3efade0973f5cefe161a`; Wiki control/port diagram |
| `datasheets/sch_zectrix_note4_developer_kit_v1.0_en.pdf` | <https://wiki.zectrix.com/sch_zectrix_note4_developer_kit_v1.0_en.pdf> | `ac81a4d44439c5ba1cd1cc47cf138ac6e57f954af1196fd39ae0b62ebf45d040`; two-page shared NOTE4/NOTE4C controller-board schematic, V1.0 |

The precompiled firmware linked by the Wiki was intentionally not archived. It is not needed to understand the hardware, and binary images should not be committed here.

## Reference firmware

| Local path | Upstream | Revision | License |
|---|---|---|---|
| `youn-ink-fourcolor-firmware/` | <https://github.com/LazyYoun/youn-ink-fourcolor-firmware> | default branch `2bp`, commit `51812e4ab3fa80ba7a5a5a274635ca2cf3901a25` | Top-level project: MIT; nested components retain their own licenses |

The Git metadata and build outputs are not included. Source files are otherwise an unmodified GitHub archive of the commit above.

## Experimental dependency snapshot

| Local path | Upstream | Revision | License | Local modifications |
|---|---|---|---|---|
| `vendor/rune-core-0.14.2/` | <https://crates.io/crates/rune-core/0.14.2> / <https://github.com/rune-rs/rune> | crate `0.14.2`, upstream commit `971350fde49f3ee8ee05f0e8f4416919f298738c`; retrieved from the local Cargo registry cache on 2026-09-03 | MIT OR Apache-2.0 | One experimental `ItemBuf` serde round-trip correction in `src/item/serde.rs`, with an explicit local-modification header added on 2026-10-22; patched file SHA-256 `a7c777d4119c59070fd7881adb1a32229e3a2654d4d5d8feca6284f69b79d403`. See `LOCAL-PATCH.md`. |

This snapshot originated in the patched precompiled-`Unit` experiment. The current workspace-wide `[patch.crates-io]` also selects it in normal firmware, Studio and WASM builds. The correction does not establish an upstream-compatible serialization contract or a production bytecode ABI.

## Pinned upstream dependency revision

| Dependency | Upstream | Revision | License | Reason |
|---|---|---|---|---|
| `esp-idf-svc` | <https://github.com/esp-rs/esp-idf-svc> | `d15fb4e7abc98d5e68bd83b6569105d91a90a5ca` | MIT OR Apache-2.0 | Includes upstream handling for ESP-IDF 5.5 Wi-Fi events and changes unknown event IDs from panic to warning; used directly as an exact Cargo git revision, with no local snapshot or modification. |

## Embedded font subset

| Source | Revision | License | Local modifications |
|---|---|---|---|
| [GNU Unifont HEX build](https://unifoundry.com/pub/unifont/unifont-16.0.04/font-builds/unifont_all-16.0.04.hex.gz) | 16.0.04; retrieved 2026-09-05 | SIL Open Font License 1.1 OR GPL-2.0-or-later with GNU Font Embedding Exception | `crates/panelet-ui/src/cjk.rs` retains only the 16×16 glyphs needed for Chinese weekday, lunar-date and sexagenary-year labels. The glyph bitmaps are otherwise unchanged. |

## TODO / common Chinese font snapshot

| Local path | Source / version | Retrieval and integrity | License / local modifications |
|---|---|---|---|
| `fonts/unifont_all-16.0.04.hex.gz` | [GNU Unifont 16.0.04 HEX](https://unifoundry.com/pub/unifont/unifont-16.0.04/font-builds/unifont_all-16.0.04.hex.gz) | 2026-09-07; SHA-256 `20e8b505f602488697979eefc69857f7f6106bceab702f5ac559f4f84e0e7494` | SIL OFL 1.1 OR GPL-2.0-or-later with GNU Font Embedding Exception; snapshot unmodified |
| `fonts/LICENSE.txt`, `fonts/OFL-1.1.txt` | [Upstream license](https://unifoundry.com/LICENSE.txt), [OFL](https://unifoundry.com/OFL-1.1.txt) | 2026-09-07 | Unmodified upstream notices |

`crates/panelet-ui/src/cjk-glyphs.dat` supersedes the calendar-only embedded subset with 3,767 glyphs: GB2312 level-1 Hanzi, all previously included calendar characters, and selected punctuation. `cjk-subset.txt` is the exact selection. `scripts/build-cjk-font.py` verifies the snapshot checksum and regenerates the 128,078-byte, big-endian codepoint/bitmap table; glyph shapes are unchanged. This does not imply coverage of all Chinese, other languages or emoji. Font data retains the upstream dual license, independently of the surrounding Rust code license. The same script also emits `crates/panelet-ui/src/ascii16-glyphs.dat` (95 printable ASCII glyphs, 1,520 bytes); the opt-in sized-text renderer uses it for supplementary punctuation. Latin letters/digits and existing punctuation are rendered from the pre-existing 5×7 glyphs enlarged to 10×14 pixels. The legacy calendar text renderer is unchanged.

## Component datasheets

| Local path | Upstream / provenance | Revision and integrity | Vendor / rights |
|---|---|---|---|
| `datasheets/esp32-s3_datasheet_en.pdf` | <https://www.espressif.com/sites/default/files/documentation/esp32-s3_datasheet_en.pdf> | Upstream current file retrieved 2026-09-01; SHA-256 `2d5a7cb7fd559d8d972bd88db32669c0196d23f22d7afaafb0f63d099b589a3f`; local modifications: none | Espressif |
| `datasheets/PCF8563.pdf` | <https://www.nxp.com/docs/en/data-sheet/PCF8563.pdf> | Upstream current file retrieved 2026-09-01; SHA-256 `2ed5d1b0a1051d0e81399417c4ff0ced0658fa36e13c0487670f9cb6dfb4f1d4`; local modifications: none | NXP |
| `datasheets/ES8311-PB.pdf` | <https://www.everest-semi.com/pdf/ES8311%20PB.pdf> | Upstream current file retrieved 2026-09-01; SHA-256 `2e569273aa625a35fdc733fd4b4095318a8ecdfd50742c4a284e97f8bf2bd49b`; local modifications: none; upstream URL timed out during 2026-09-01 recheck | Everest Semiconductor |
| `datasheets/SSD2683_0.20_proposal.pdf` | User-provided local file; no public URL supplied | Product Proposal Rev 0.20, May 2024; SHA-256 `d4c6d5a2ad630e4887011338f3363d38c22450db73fa1e36178fdbfd0011f8c5` | Solomon Systech Limited; no license or redistribution permission supplied |

The SSD2683 document labels itself a **Product Proposal**, not a final datasheet. It is useful controller evidence, but its revision, POR values and contents may change without notice. The reference copy remains under Solomon Systech copyright and is retained in the repository without changing that copyright. No public download URL or separate redistribution permission is recorded; its public-sharing scope remains an item-specific question.

## License text restoration (2026-10-22)

- `vendor/rune-core-0.14.2/LICENSE-MIT` and `LICENSE-APACHE` were restored verbatim from the fixed upstream commit above; hashes and URLs are in `LOCAL-PATCH.md`.
- `fonts/NOTICE-16.0.04.txt` records the complete font copyright and author notices from the exact GNU Unifont 16.0.04 source package. `fonts/COPYING-16.0.04.txt` is the unchanged upstream license description; its SHA-256 is `5a65797606332f2f63057fd81ea0ff35f74043583515bf900ee9b42efa8176a6`. Together with `fonts/LICENSE.txt` and `fonts/OFL-1.1.txt`, these notices accompany the browser resource bundle.
- Exact GNU source package: <https://ftp.gnu.org/gnu/unifont/unifont-16.0.04/unifont-16.0.04.tar.gz>, SHA-256 `2bd4e4679757126f48e1bf2c1be40b09aa92162bfedda4683ce5fbc70a2a5972`. Original whitespace is retained in the vendored `COPYING` text.
