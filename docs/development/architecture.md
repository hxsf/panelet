# Panelet 架构与目录规划

Panelet 把带屏幕的开发板变成可定制的看板。OS 负责设备连接、输入、存储、显示和小组件运行；Studio 负责编辑与预览。项目名不绑定某块板或某种脚本语言。

当前唯一有实现与实机记录的设备是 NOTE4C。以下先描述实际代码，再列出接入第二款设备时的拆分方向；规划中的目录、硬件抽象和独立编译器尚未实现。

## 当前目录与职责

| 路径 | 当前职责 |
|---|---|
| [`app/os/`](../../app/os/) | ESP-IDF 固件组合入口，生成 `panelet-os`；也包含通过 feature 隔离的 Studio 原生程序和浏览器 WASM 入口 |
| [`app/os/src/board.rs`](../../app/os/src/board.rs) | NOTE4C GPIO、供电、SPI/I2C/ADC、RTC 和屏幕操作；应用中的原始 GPIO 编号集中在这里 |
| [`app/os/src/input.rs`](../../app/os/src/input.rs) | 独立 GPIO 采样任务；屏幕刷新阻塞主循环时仍捕获输入 |
| [`app/os/src/server.rs`](../../app/os/src/server.rs) | ESP-IDF 网络、HTTP/串口管理、NVS、时间同步、Rune 编译与会话调度，以及宿主服务适配 |
| [`app/os/src/storage.rs`](../../app/os/src/storage.rs) | ESP-IDF SPIFFS 挂载、容量查询、受控初始化与存储锁 |
| [`crates/panelet-core/`](../../crates/panelet-core/) | 可主机验证的输入路由、菜单、时间与 RTC 数据解析、LED 等逻辑；仍包含 PCF8563 相关内容 |
| [`crates/panelet-management/`](../../crates/panelet-management/) | 不依赖 HTTP transport 的 RPC broker、job/event、Wi-Fi profile 和小组件设置逻辑 |
| [`crates/panelet-rune-abi-poc/`](../../crates/panelet-rune-abi-poc/) | 当前 Rune ABI、capability、生命周期、宿主接口、Canvas 与 DisplayBroker；`poc` 后缀保留，尚未抽成语言无关运行层 |
| [`crates/panelet-ui/`](../../crates/panelet-ui/) | 字体、绘图命令与状态页；目前直接使用 NOTE4C 四色 framebuffer |
| [`crates/panelet-widget-package/`](../../crates/panelet-widget-package/) | `.pnw` 包解析与生成、manifest 和路径校验 |
| [`crates/panelet-widget-store/`](../../crates/panelet-widget-store/) | 包、配置、KV/private/shared 与版本化网页资源的持久化规则；使用适合 flat SPIFFS 的物理文件布局 |
| [`crates/ssd2683/`](../../crates/ssd2683/) | 可主机测试的 `no_std`、`embedded-hal` 驱动，以及 NOTE4C 的 400 × 300 四色像素打包 |
| [`app/os/src/editor/`](../../app/os/src/editor/) | `panelet-studio` 原生执行、`panelet_preview` WASM、编辑器页面与 Worker；当前仍与固件共用 package |
| [`examples/widgets/`](../../examples/widgets/) | 小组件源码、manifest、资源和使用说明 |
| [`experiments/`](../../experiments/) 与 `*-poc` 工具 | Rune 容量、预编译 Unit 等实验；实验成功不等于生产发布链已经可用 |

固件目前在设备上编译 Rune 源码，再由 Rune VM 执行。浏览器预览把 Rune 编译器、VM、Canvas 和字体一起编译为 WASM，在浏览器 Worker 内执行；设备只提供这些资源文件。原生 Studio 复用同一套预览逻辑。

管理 broker 可以保持与传输方式解耦，但仓库当前没有已接入的 BLE/GATT 管理通道。Wi-Fi 已有实现，蓝牙属于后续可选适配，不能从芯片支持 BLE 推导成 Panelet 已支持。

## 第二款设备接入前需要解除的耦合

| 当前耦合 | 具体位置 | 下一步边界 |
|---|---|---|
| 全局 ESP32-S3 构建选择 | `.cargo/config.toml`、`app/os/sdkconfig.defaults`、NOTE4C 分区 CSV | 显式选择设备及其 MCU、Flash/PSRAM、分区和恢复参数；不能靠改全局常量区分设备 |
| 引脚与电源操作 | `app/os/src/board.rs` | 放入具体 board adapter，保留早期 power-hold、BOOT/USB 恢复和失败清理顺序 |
| 固定尺寸、颜色和帧格式 | `ssd2683/src/framebuffer.rs`、`panelet-ui/src/{lib,canvas}.rs` | 提取显示描述和绘图目标；尺寸、调色板、像素格式由设备选择 |
| Rune 与 NOTE4C 显示绑定 | `panelet-rune-abi-poc/src/{mvp,os,display}.rs` | 移除固定 `CANVAS_*`、四色 palette 和通用层中的 `ssd2683::PartialWindow`；由显示后端报告刷新能力 |
| 预览固定 400 × 300、30,000 B | `app/os/src/editor/{runtime,transport}.rs` 与 `editor.html` | 原生和 WASM 使用同一设备显示描述；界面按描述解码像素和呈现输入 |
| 平台服务与调度混在固件入口 | `app/os/src/server.rs`、`storage.rs` | 随真实新平台需求拆出网络、时钟、任务和存储适配，保留稳定宿主契约 |

NOTE4 黑白屏是验证这些边界的下一款候选设备。它的尺寸、控制器、位序、BUSY 极性与刷新协议都须独立取证；不复用 NOTE4C 的四色编码或 PTLW 时序作为默认值。

## 下一阶段目标目录

以下是规划，不是现有目录，也不是本轮已经完成的抽象：

```text
app/os/                     固件启动与组件组合
boards/
  note4c/                   NOTE4C 接线、供电、设备配置与分区
  note4/                    独立完成适配后加入
platforms/esp-idf/           网络、NVS、任务、文件系统等平台服务
crates/
  panelet-core/             通用逻辑
  panelet-display/          显示描述、绘图目标与呈现契约
  panelet-ui/               与具体控制器无关的绘图和字体
  panelet-management/      管理命令、状态和配置
  panelet-widget-package/  包格式及校验
  panelet-widget-store/    持久化语义
  panelet-rune-abi-poc/    当前 Rune 适配，逐步解除设备耦合
  ssd2683/                 独立控制器驱动；其他控制器另建 crate
tools/studio/               从 app/os 提取原生与浏览器开发工具
docs/
  devices/<device>/        设备安装、恢复、限制与实机记录
  os/                      对小组件和管理工具提供的契约
  development/             构建、架构和接入说明
  guides/                  日常使用与外部服务集成
```

按依赖顺序实施：先定义显示描述与绘图目标，再提取 NOTE4C adapter 并复验原行为，随后接入 NOTE4。只有出现真实平台差异时才提取相应平台模块；不要一次为未知设备建立大量空接口。

## 编译器与 VM 的演进

如果 Rune 后续不适合体积、资源控制或语言体验，可另行实现 `panelet-compiler` 的原生与 WASM 版本，以及板端 `panelet-vm`。在决定替换前，现有 [`tools/panelet-rune-unit-compiler-poc/`](../../tools/panelet-rune-unit-compiler-poc/) 只用于受限 Rune Unit 实验，不能视为独立语言或通用离线发布工具。

正式拆分需先约定编译产物格式、编译器/VM/宿主 ABI 兼容性、验证器、资源限额和升级回退规则。设备应拒绝不兼容产物；Studio 预览与设备执行应共享语义和回归样例。界面、管理、硬件适配和小组件持久化不应依赖编译器必须留在板端。

## 命名与兼容性

项目品牌和 Rust package 采用 Panelet / `panelet-*`。NOTE4C 保留为设备型号。

对外小组件、资源包与实验 Unit 后缀分别改为 `.pnw`、`.pna`、`.pnu`，旧 `.n4w`、`.n4a`、`.rnu` 输入仍接受。既有 `note4c.rune/3` ABI、包格式、存储标识与刷写脚本的 `NOTE4C_*` 安全确认变量保持不变；它们代表已使用的契约或设备安全流程。完整映射与内部存储边界见[名称与兼容性](naming-and-compatibility.md)。以后需要更改协议时单独设计迁移，不以品牌替换触发清空存储或破坏旧包。

接入的具体步骤和验收项见[添加新硬件](adding-device.md)，当前命令见[构建与验证](building.md)。
