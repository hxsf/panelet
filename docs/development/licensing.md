# 许可证与分发范围

Panelet 自有代码采用 **MPL-2.0**，完整条款见根目录 [LICENSE](../../LICENSE)。
[小组件示例](../../examples/widgets/README.md)单独采用 [MIT](../../examples/widgets/LICENSE)。
第三方代码、字体和硬件参考资料保留各自许可，不因根许可证改变而重新授权。

MPL-2.0 允许商业使用、修改和分发。分发包含受覆盖代码的可执行版本时，应向接收者提供
受 MPL 覆盖的源码及其修改，并说明取得源码的方式；不含 MPL 代码的独立文件可使用其他
许可。复制 MPL 代码到新文件不会使它脱离覆盖范围。保留源码中的版权和许可声明；
项目未增加强制界面署名、禁止商用或要求向上游提交修改的附加条件。

本文记录分发边界和当前已知材料，不代表全部依赖、历史内容或法律权利已经审查完成。完整条件以各材料随附的许可证为准。

## 自有代码、小组件和模板

- 没有另行标注的 Panelet 自有代码、构建配置和原创文档适用 MPL-2.0；第三方文件及明确另行标注的材料除外。可注释源码使用 `SPDX-License-Identifier: MPL-2.0`，其他格式依本页和根 [NOTICE](../../NOTICE) 确定范围。
- `examples/widgets/` 中四个示例子目录（`counter`、`calendar`、`todo`、`photo-album`）及其配套说明适用 MIT。目录中的通用打包工具 `build-upload.mjs` 明确标注为 MPL-2.0。管理页中明确标注 MIT 的内置 Rune 示例模板同样适用 MIT；这个例外不扩展到包围它们的 HTML/JavaScript、编辑器、编译器或固件实现。
- 用户独立编写的小组件由其作者决定许可。仅使用 Panelet 编辑器、编译器或 VM，不使独立的小组件自动成为 Panelet 代码；编译结果的许可还取决于其实际包含的材料。
- 复制或修改仓库示例、模板、运行库代码时，需要保留对应代码的版权与许可通知；独立编写和复制现有实现应区分处理。
- 小组件包含字体、图片、数据或第三方库时，由发布者处理这些内容各自的分发条件。生成的 PNG 等画面与可再次使用的字体数据是不同的分发对象。

## 第三方材料

具体来源、版本和修改记录见 [第三方来源清单](../../3rd-res/SOURCES.md)。下表说明容易在打包时遗漏的边界。

| 材料 | 许可与通知 | 分发时的处理 |
|---|---|---|
| 实验性 `rune-core 0.14.2` 快照 | 上游 `MIT OR Apache-2.0`；保留 [LICENSE-MIT](../../3rd-res/vendor/rune-core-0.14.2/LICENSE-MIT)、[LICENSE-APACHE](../../3rd-res/vendor/rune-core-0.14.2/LICENSE-APACHE) 和 [本地补丁说明](../../3rd-res/vendor/rune-core-0.14.2/LOCAL-PATCH.md) | 上游版权归原作者。本地补丁不能冒充未修改的官方版本；按 Apache-2.0 分发时，修改过的源文件也须有醒目的变更标记。 |
| GNU Unifont 16.0.04 及字形子集 | `OFL-1.1 OR (GPL-2.0-or-later WITH Font-exception-2.0)`；[精确版本通知](../../3rd-res/fonts/NOTICE-16.0.04.txt)、[精确版本 COPYING](../../3rd-res/fonts/COPYING-16.0.04.txt)、[原有许可证](../../3rd-res/fonts/LICENSE.txt)、[OFL 1.1](../../3rd-res/fonts/OFL-1.1.txt) | 字体数据保留自己的许可，不能用包的 Rust 代码许可证覆盖。裁剪字符集或转换存储格式也应保留这些通知；字体嵌入例外继续保留。 |
| `youn-ink-fourcolor-firmware` 参考源码 | 顶层 MIT，嵌套组件各有通知 | 保留原目录内的许可证与文件头。`esp_video` 使用带 Espressif 产品使用范围的 `ESPRESSIF MIT`，不能当作标准 MIT 向任意硬件移植；部分 Linux 头文件另有 GPL/Linux-syscall-note 或 BSD 选项。 |
| 厂商 Wiki、图片、原理图、datasheet 和 Product Proposal | 原作者保留权利；公开可下载不等于取得开源或再分发许可 | 作为研究来源保存，不自动成为可随 Panelet 公开再分发的材料；具体边界见下一节。 |

Unifont 的版权和作者材料来自 GNU 官方发布的精确 `16.0.04` 源码包，获取日期及 SHA-256 记录在字体通知中。源码包的 `COPYING` 明确记载字体双许可；其 `README` 的旧许可段落只描述 GPL 选项，因此不能仅凭该段落遗漏 OFL。通知保留了版权元数据和作者说明，未更改字形。

## 发布制品需要携带什么

发布仓库源码、Rust crate、固件、桌面编辑器或浏览器 WASM 时，应根据制品实际包含的内容准备通知：

1. Panelet 自有代码的 MPL-2.0 许可证及版权声明。分发可执行版本时提供对应源码及其获取说明，不能只声明使用 MPL。源码仓库为 <https://github.com/hxsf/panelet>；正式发布制品应指向对应版本或提交，并确保其源码实际可获得。
2. 实际包含的第三方代码所需的许可证、版权、变更说明，以及上游存在时的 `NOTICE`。根许可证不能替代它们；仅有 Cargo 的 `license` 字段也不能替代条款和通知。
3. 含 Unifont 原始数据、`cjk-glyphs.dat`、`ascii16-glyphs.dat` 或其编译嵌入副本的制品，应携带上表四份字体通知。通知必须随下载包可读地提供，不能只链接到本机仓库的 `3rd-res/`。
4. 分发模板或小组件包时，携带 MIT 版权声明和完整许可条款，以及字体和其他资源适用的通知。示例导出路径保留源码内的版权声明与包内许可资源；用户可自行决定独立小组件的许可。仅由字体渲染的普通画面，不等同于分发可复用的字体软件。

浏览器预览资源包随附 `panelet-license.txt`、各示例的 MIT 通知和 `font-licenses.txt`。
项目源码包保留根 `LICENSE`、`NOTICE` 和示例许可证。本轮公开的是源码仓库，未发布
预编译固件或桌面程序；未来二进制发行仍需按其实际依赖准备完整的第三方通知。

独立发布 crate 前检查实际打包清单：仓库其他目录内的通知不一定会被 Cargo 包含。浏览器资源压缩包、WASM 与原生程序的发布包也需要同样检查。本页没有宣称这些产品的全部依赖通知已自动收集完成。

## 硬件参考资料的保留与权利范围

硬件研究依赖准确版本的 Wiki、图片、原理图和 datasheet。仓库保留这些参考副本，并在
[`3rd-res/SOURCES.md`](../../3rd-res/SOURCES.md) 中记录来源、版本、修改与哈希；资料不适用
Panelet 自有代码的 MPL-2.0 或示例的 MIT 许可，文件内的原版权和许可声明继续保留。

权利范围须逐项判断：

- 已有明确许可的材料，按其条件保留通知、引用和再分发。
- 厂商公开下载的资料，记录原址和原声明；公开可下载本身不证明存在任意镜像权限。
- Wiki 的 `alr` 声明以及没有附独立许可的文档，标为“再分发权限未确认”，不能据此给整个目录重新授权，也不自动视为必须删除。
- `SSD2683_0.20_proposal.pdf` 来自用户提供的本地文件，目前没有公开 URL 或单独的再分发授权记录；其共享范围单独确认，不扩展为所有硬件资料的共同结论。

本次调整已恢复八份参考文件的 Git 跟踪，原始字节与备份一致。保留版本化参考资料和
标注版权范围是当前整理方案；先前统一过滤这些目录的发布副本已经撤回，不作为当前
待推送版本。公开分发具体材料时，按已核对的上游条款、许可或
其他适用依据处理；署名与项目许可证的排除说明不能代替缺少的授权。

公开前仍须排除实际设备凭据、个人信息和备份镜像。有限的文件名或密钥格式扫描不能
证明全部历史已无敏感内容。

## 首次公开与旧版本

2026-10-22 起，本版本按 MPL-2.0／示例 MIT 的范围准备首次 GitHub 发布。
公开 `main` 从该版本的源码快照开始，本地 `develop` 保留原始开发历史及原子提交；
历史资料和第三方参考副本仍在公开源码树中。这不是对旧授权的撤销：任何已经有效授予
他人的旧版本许可均不因新许可证、分支或 Git 历史组织方式而失效。

## 官方条款与说明

- [MIT 许可证](https://opensource.org/license/mit)
- [Mozilla Public License 2.0](https://www.mozilla.org/en-US/MPL/2.0/)
- [MPL 2.0 FAQ](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)
- [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0)
- [SIL Open Font License FAQ](https://openfontlicense.org/ofl-faq/)
- [GNU Unifont 16.0.04 官方发行目录](https://ftp.gnu.org/gnu/unifont/unifont-16.0.04/)
