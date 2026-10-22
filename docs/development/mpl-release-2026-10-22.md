# MPL-2.0 首次公开许可验证（2026-10-22）

维护者选择 Panelet 自有代码使用 MPL-2.0，小组件示例使用 MIT。当前范围见
[许可证与分发范围](licensing.md)；更名阶段的旧构建证据见
[迁移记录](panelet-migration-2026-10-22.md)。本轮只进行源码、主机和目标编译验证，
没有连接、刷写或重新验收真实设备。

## 实现范围

- 根 `LICENSE` 使用 Mozilla 发布的标准 MPL-2.0 全文；根 `NOTICE` 和可注释自有源码明确范围，12 个 workspace package 的 Cargo license 为 `MPL-2.0`。
- 四个示例目录的源码、资源和说明使用 MIT，完整通知在 `examples/widgets/LICENSE`。源码保留版权声明，包内 `resources/LICENSE.txt` 保留 1,023 字节完整许可条款，符合现有单资源 1 KiB 限额。
- 管理页两份默认 Rune 模板内嵌完整 MIT 通知。宿主页面、编辑器、运行时及通用打包工具仍为 MPL-2.0。
- 浏览器资源包新增 `panelet-license.txt`，含 MPL 全文、源码获取地址及示例／第三方例外说明；四份字体通知原始字节继续随包分发。
- 第三方源码、字体数据及硬件研究资料未重新许可、未删除；当前参考文件与原字节一致。
- 公开 `main` 从最终源码快照开始；原始 `develop` 历史与原子提交保留在本地。旧版本已有效授予他人的权利不会因此撤销。

## 已执行检查

| 检查 | 结果 |
|---|---|
| 显式 `aarch64-apple-darwin` 的 workspace 测试（排除固件包） | 190 项通过 |
| Studio Rust 测试，含四例真实渲染和 MIT 通知包往返 | 12 项通过 |
| Studio HTTP／隔离工作进程／包往返 | 5 项通过 |
| 原生与最新 WASM 的像素、事件、资源和包对照 | 19 组通过 |
| 浏览器页面、Worker、内置模板与真实 WASM | 15 项通过 |
| 最新资源包 HTTP／版本／容量／恢复检查 | 9 项通过 |
| 资源构建、CLI ZIP 导出及许可证原字节保留 | 6 项通过 |
| 备份、刷写和配网脚本 mock | 三套通过，未接触设备 |
| `scripts/build-mvp.sh` ESP32-S3 release 构建 | 通过，编译器无 warning |
| Cargo 许可元数据、脚本解析、gzip 产物 SPDX | 通过 |

`cargo fmt --all -- --check` 能解析 Rust 源码，但全仓检查仍有七个文件的既有格式差异，
已与变更前版本对照确认；本轮未顺带格式化无关实现。

当前待发布文本经过有限凭据模式扫描，未发现常见密钥、私钥、JWT 或带凭据 URL 候选。
此检查不代表完整安全审计或全部第三方权利审查。

## 本轮构建制品

| 产物 | 大小 | SHA-256 |
|---|---:|---|
| `panelet-os` ELF | 5,663,576 B | `f1fb0c2dab63af4493944917138d049b23d994ed5de3151a18a81a095f53e1cf` |
| 非 merge app image | 3,827,904 B | `3bd6f2ef87f1ab225c38105407c6399ed9d1da37b012e0549c5220a60e94cd84` |
| `preview.wasm` | 3,016,662 B | `9346e9a7e477031a9b567052beed3f9282e4f61bf30cd52e438d47659258bd22` |
| `bundle.n4a` | 951,143 B | `0f9585514acad39f3df9bca6c64ca7bdad0b44baf4b455f4d388aa09fda65091` |

app image 通过 `espflash save-image` 使用当前 OTA 分区表离线生成；app 槽为
4,128,768 B，剩余 **300,864 B**。这只是 Flash 可容纳性检查，不代表运行时内存、
实际显示或 OTA 恢复验证。制品与日志保存在忽略的 `target/panelet-mpl/` 和
`target/web-assets/`；本次 GitHub 发布仅包含源码，不发布预编译制品。

随后对外文件后缀统一为 `.pnw`、`.pna` 和实验用 `.pnu`，旧后缀输入继续接受；
完整对应关系见[名称与兼容性](naming-and-compatibility.md#项目文件后缀)。上表保留后缀
调整前真实构建的 `bundle.n4a` 名称、大小和哈希，不代表后续构建的 `.pna` 制品。

后缀统一后重新验证：受影响的包编解码、存储和实验 Unit 主机测试 46 项，Studio 12 项、
HTTP 5 项、原生/WASM 对照 19 组、浏览器 17 项、资源 HTTP 11 项、打包工具 7 项均通过。
浏览器检查直接执行管理页和两个 Studio 引擎的导出处理，确认下载 `.pnw`；新旧
`.pna`／`.n4a` 均能完成资源上传。实验 CLI 用 `.pnu` 和 `.rnu` 输出的内容均与原
1,467 B fixture 一致。完整 workspace 与硬件脚本的先前检查结果见上表，后缀修改没有
改变这些脚本或设备存储命名。

最新 ESP32-S3 release 编译器无 warning；再次离线生成非 merge app image，容量仍为
3,827,904 / 4,128,768 B。当前最终制品为：

| 产物 | 大小 | SHA-256 |
|---|---:|---|
| `panelet-os` ELF | 5,663,576 B | `ea6d0d510b353292230847cbeb285a4cc3d3838ae33bf8ce79166a6762477f7c` |
| 非 merge app image | 3,827,904 B | `a33eb167074d41d50b1e4cc249bff90c5a153554d6a6cbbf07fc7ec05f9a2aaa` |
| `preview.wasm` | 3,016,662 B | `7ac591e12bdfb31c12c441460c46371c81b7370bfc8a47b790279614d3955af7` |
| `bundle.pna` | 951,144 B | `b0d7b4a35a5fd88eb341ad8e80c46166e33c1d64a396775803a0c9f2f540aca7` |

这次构建的 WASM 内容哈希随编解码入口名称调整而变化，因此最新资源包哈希也变化；
这不是后缀改变导致容器格式变化。最新日志和 app image 位于忽略的
`target/panelet-extensions/`，资源文件位于 `target/web-assets/`。本轮仍未操作真实设备。
