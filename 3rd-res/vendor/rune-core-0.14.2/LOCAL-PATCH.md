# 实验性 Rune 本地补丁

此快照最初用于 `rune-footprint-poc` 的 runtime-only 尺寸可行性测量。当前根
`Cargo.toml` 的 `[patch.crates-io]` 在整个 workspace 生效，普通固件、Studio 和 WASM
也会解析到这个 `rune-core`；不能将它描述成只在实验构建中使用。
**补丁不构成可用于生产的外部字节码格式，也不是上游 Rune 0.14.2 的兼容性声明**。

## 快照来源

- crate：`rune-core` `0.14.2`
- 上游：<https://github.com/rune-rs/rune>
- 上游 commit：`971350fde49f3ee8ee05f0e8f4416919f298738c`（来自快照的 `.cargo_vcs_info.json`）。
- 许可证：`MIT OR Apache-2.0`（与随附上游 `Cargo.toml` 一致）
- 来源：本机 Cargo registry 已缓存的精确 `0.14.2` crate；未联网下载，2026-09-03 复制到本实验目录。

## 本地代码修改

`src/item/serde.rs` 的 `ItemBuf` 解码路径原先把 `Item::serialize()` 输出的
`ComponentRef` 外部标签表示读成内部标签的 `Component`，造成 round-trip 失败。
补丁仅在该文件中增加一个拥有字符串的、同外部标签的中间枚举，再转换为
`Component`。它让非借用式 CBOR decoder 能读回 Rune 写出的 `ItemBuf`。

在无补丁的官方 `rune-core 0.14.2` 上，实际测到：

- `bincode 1.3.3` 在 `Unit` 的未知长度 HashMap 上拒绝编码；
- CBOR/JSON 在 ItemBuf 处解码失败（例如 `missing field type`）；
- 初始 `ComponentRef<'de>` 读取法在 Ciborium 上失败：`invalid type: string "simple", expected a borrowed string`。

该补丁的实验结论仅能确认“若修复该对称性缺陷，runtime-only 链接体积可能是多少”。
生产实施必须等待上游可审计的修复或维护自己的完整、安全审计过的序列化协议；
同时只接受签名、带 key-rotation 和 anti-rollback 的可信编译器输出。

## 许可证补齐与分发

2026-10-22 从上述精确上游 commit 补入此前 crate 快照中缺失的两份许可证全文，
文件逐字保留上游内容，没有把上游版权改为 Panelet：

| 本地文件 | 上游原文 | SHA-256 |
|---|---|---|
| [LICENSE-MIT](LICENSE-MIT) | [固定 commit 的 MIT 文本](https://raw.githubusercontent.com/rune-rs/rune/971350fde49f3ee8ee05f0e8f4416919f298738c/LICENSE-MIT) | `69f4c49c53b3798ea665d464784ede97b4826e59d15a1bac74cbb268789c8b14` |
| [LICENSE-APACHE](LICENSE-APACHE) | [固定 commit 的 Apache-2.0 文本](https://raw.githubusercontent.com/rune-rs/rune/971350fde49f3ee8ee05f0e8f4416919f298738c/LICENSE-APACHE) | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |

MIT 文本中的 `Copyright (c) 2020 John-John Tedro` 属于原作者，必须按所选许可保留。
本次补全文本没有改变原有双许可选项或补丁逻辑；2026-10-22 在 `serde.rs` 文件头补充了明确的本地修改标记。分发此快照时，应保留本说明与上游通知；
若按 Apache-2.0 选项分发修改版，还须在修改过的源文件上保留醒目的变更标记。
项目层面的字体、模板和参考资料边界见 [许可证与分发范围](../../../docs/development/licensing.md)。
