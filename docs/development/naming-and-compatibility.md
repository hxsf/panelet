# Panelet 名称与兼容性

2026-10-22，项目由设备专属的 NOTE4C 实验仓库更名为 Panelet。品牌名、源码包名与
设备型号分开：Panelet 是平台，NOTE4C 是目前支持的硬件。项目描述固定为：

> Turn your display board into a personal dashboard with customizable widgets.

## 当前源码入口

| 原入口 | 当前入口 |
|---|---|
| `app/server/` / `note4c-server` | `app/os/` / `panelet-os` |
| `note4c-editor` | `panelet-studio` |
| `note4c_preview` | `panelet_preview` |
| `crates/note4c-*` | `crates/panelet-*` |
| `tools/note4c-rune-unit-compiler-poc` | `tools/panelet-rune-unit-compiler-poc` |

`ssd2683` 保留控制器名称。实验 crate 保留 `poc` 后缀；当前实际使用的 Rune 宿主仍在
`panelet-rune-abi-poc`，尚未拆成正式 `panelet-lang`、`panelet-vm`、`panelet-compiler`。
这些未来名称不代表已经存在可安装的语言或编译器产品。

`build-mvp.sh`、`run-widget-editor.sh`、`flash-server.sh` 等脚本名保留稳定入口，内部构建
目标已经更新。主机命令必须显式指定 host target，避免触发固件 runner。

## 项目文件后缀

对外文件名使用 Panelet 后缀。此次只改默认产物名、导出文件名和界面文案，包内数据结构与兼容版本不变：

| 类型 | 当前后缀 | 旧后缀 | 含义与处理 |
|---|---|---|---|
| 小组件安装包 | `.pnw` | `.n4w` | Panelet Widget；仍为 STORE-only ZIP，导入和设备上传继续接受旧包 |
| 浏览器预览资源包 | `.pna` | `.n4a` | Panelet Assets；默认产物为 `target/web-assets/bundle.pna`，安装器继续接受旧包 |
| 实验性预编译 Unit | `.pnu` | `.rnu` | Panelet Unit；仅用于当前 Rune POC，旧 Unit 字节仍可读取，不代表正式独立语言格式 |

标准或语言既有后缀保持原名：Rune 源码 `.rn`、WebAssembly `.wasm`、固件镜像 `.bin`、JSON 配置 `.json`。不为这些通用格式创造 Panelet 私有后缀。

扩展名不作为内容有效性的证明；各解析器仍按容器结构、校验和、版本及资源限制验证内容。已有 `.n4w`、`.n4a` 或 `.rnu` 文件无需重新编译或安装，仅为统一名称时可以改用新后缀；重命名本身不会升级 ABI 或修复无效包。历史测量记录保留当时文件名、大小与哈希。

设备内部 SPIFFS 小组件物理名继续使用 `p<hash>.n4w`、`t<hash>.n4w`，不批量改名或迁移。对外导出 `.pnw` 与内部存储名是不同层次；已有包、配置、私有数据和前台选择保持原身份。

## 本次保留的兼容身份

以下名字参与设备通信、包验收、数据恢复或权限边界，不能机械地替换成 Panelet：

- `note4c.rune/3` ABI、Rune 的 `note4c` 内部类型路径和生命周期函数名。
- `application/vnd.note4c.widget+zip` 和 `X-Note4C-Token` HTTP header；旧后缀输入及含 `n4w` 的既有错误码继续兼容。
- `NOTE4C-WIDGET-STORE` marker、`note4c_net` NVS namespace、`N4A1` 资源包 magic、`N4CRUNE\0` 实验 Unit magic，以及上述内部 `.n4w` 存储名。
- 设备凭据派生盐 `NOTE4C\0`；修改它会影响已有设备的确定性凭据。
- 现有 `com.note4c.*` 示例 widget ID、`note4c.auth.v2` / `note4c.page.v2` 浏览器存储键。
- `NOTE4C_*` 脚本环境变量，尤其备份、安全状态、分区覆盖、串口和 app-only 确认变量。
- `NOTE4C` 型号字符串、设备 SoftAP 名称和 NOTE4C 分区文件名。

资源 API 版本仍为 `1`。仅 Cargo 包名、品牌文案或对外文件后缀变化，不构成修改资源接口版本的理由。
本次不更改包的二进制格式、NVS、assets 索引、持久化 widget 身份，也不格式化设备存储。

## 以后需要改协议时

先单独设计版本协商、旧格式读取、失败恢复和数据迁移，再做相应变更。不要以消除旧品牌
字符串为由破坏已安装设备。历史日志中的旧路径、名称和制品哈希保留为当时证据，不能
把它们替换成新名字后宣称新固件已经实测。
