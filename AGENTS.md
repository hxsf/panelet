# Panelet 项目约定

本文件只保存本仓库的长期约束。通用协作偏好放在用户级 `~/.codex/AGENTS.md`；当前功能、实测结果和开发进度放在 README 与 `docs/`，不要在这里复制状态清单。按本次任务选择下列入口，无需每次通读。

## 范围与证据

- Panelet 是面向显示设备的可编程看板平台。当前已接入目标是 ZECTRIX NOTE4C Devkit、PCB V1.0、4.2 英寸 400 × 300 四色 SSD2683 屏；下文 GPIO、供电与显示参数约束适用于此设备。NOTE4 黑白固件及显示时序不能视为兼容；新设备必须建立独立证据与验证记录。
- 板级事实优先查官方 Wiki 与共享 V1.0 原理图，来源入口见 [3rd-res/SOURCES.md](3rd-res/SOURCES.md)。归档参考固件仅是实现证据，不是控制器 datasheet，也不代表该固件已在本项目实机验证。
- 区分文档事实、源码推断、主机测试、目标编译与实机观测。归档源码和官方资料冲突时保留双方依据，硬件实测后再作取舍。
- 不臆造 SSD2683 未公开命令、NFC 型号、供电时序、PSRAM 总线模式或局刷能力。已有实测仅适用于记录中的板卡、固件与条件；编译成功不能证明显示、电池百分比、睡眠电流、音频、NFC、Wi-Fi 或唤醒行为。

## 硬件与数据安全

- 电池供电时尽早将 GPIO17 配置为 power-hold 输出并断言保持状态；依赖它之前须在硬件上验证极性及关机行为。
- GPIO18 同时参与下键/电源键输入和物理开机。GPIO0 是 BOOT/确认键及 strapping pin；GPIO45/46 也有 ESP32-S3 strapping 约束。保留 BOOT + RESET 与原生 USB GPIO19/20 恢复路径。
- GPIO 均为 3.3 V 逻辑，禁止施加 5 V。GPIO47/48 共用一个 I2C 总线，串行化 ES8311、PCF8563 和 NFC 事务。
- 普通 `screen` 使用有成功基线的自动差异窗口策略，基线失效时回退全刷；规则见 [显示策略](docs/devices/note4c/screen-partial-refresh.md)。全刷和 PTLW 窗口刷新都可能超过 10 秒。BUSY 等待必须有界；每次刷新尝试，无论成功、错误或超时，都必须尝试安全清理并关闭屏幕供电，报告清理失败。
- PTLW 使用已有完整 DTM + 窗口指令路径，保留诊断实验入口；不由窗口限制有效推断高速局刷、长期 waveform 或寿命保证。初始实验依据见 [Phase 1 记录](docs/devices/note4c/history/phase1-hardware-log-2026-09-02.md)，后续策略与验证边界见显示策略文档。
- 首次改变设备固件或分区前，完成同一设备完整 16 MiB 原厂 Flash 备份、SHA-256 核对、第二份受控副本与恢复演练。已有核验记录可复用；不要把后续开发固件备份冒充原厂镜像。备份、凭据、设备标识及个人数据不得提交或上传。
- 实际刷写前遵循 [恢复与备份](docs/devices/note4c/bringup-and-recovery.md) 和 [分区策略](docs/devices/note4c/firmware-partition-layout.md)，使用 `scripts/flash-server.sh`，显式识别串口并核实备份、安全状态兼容性和分区覆盖授权。不得为使脚本通过而自行伪造确认变量。
- `.cargo/config.toml` 的 runner 会刷写设备：`cargo run` 不是编译检查。编译、主机测试可按任务推进；实际写 Flash、格式化存储、eFuse 操作或改变设备安全状态，需要对应操作的明确授权及前置条件。缺少条件时先完成可独立进行的代码与主机验证，只暂停依赖该条件的硬件步骤。

## 实现边界与入口

- 固件采用 ESP-IDF `std`、`esp-idf-hal` / `esp-idf-svc` 路线，服务于 Wi-Fi、USB/串口日志、NVS 与外设支持；不要因通用模板建议另起技术栈。
- `app/os/` 是当前固件入口；板级 pin map 和供电时序集中在 [board.rs](app/os/src/board.rs)，应用业务代码不得散布原始 GPIO 编号。
- `crates/ssd2683/` 保持 `embedded-hal` trait 边界和可主机测试的 `no_std` 驱动。framebuffer 固定为 30,000 字节、MSB-first 2 bpp：`00` 黑、`01` 白、`10` 黄、`11` 红。
- 大帧数据优先用固定缓冲区或显式放置的 PSRAM 缓冲区；原子变量与同步原语不得放入 PSRAM。PSRAM 配置与容量判断须有对应源码/实测依据。
- `crates/panelet-core/` 与 `crates/panelet-ui/` 放主机可验证逻辑和绘图；RPC/网络行为查 [管理服务](docs/os/management-server-mvp.md)，Rune ABI、包格式、capability 与存储行为查 [小组件契约](docs/os/widget-runtime-mvp.md) 及对应 `panelet-*` crates。
- 新设备接入与源码提取遵循[架构与目录规划](docs/development/architecture.md)和[添加设备](docs/development/adding-device.md)。规划中的目录、能力和编译器拆分不能写成已实现；重命名不得顺带迁移设备已有协议、包格式、凭据或存储身份。
- 新板 bring-up 或重建硬件依赖证据时按电源/按键 → RTC/电池 → 显示测试图 → 睡眠/唤醒 → 网络 → 音频 → NFC 推进。已有证据可复用，普通软件改动不必重跑整条路线。
- 第三方快照放在 `3rd-res/`，并在 `3rd-res/SOURCES.md` 记录 URL、branch/tag/commit、获取日期、许可证与本地修改。

## 验证与完成条件

按改动选择相关检查。修复本次改动导致的失败并重跑受影响检查；通过后，没有新变化、失败或未解决风险就停止扩大验证。文档修改检查内容、链接和 diff 即可。

主机测试必须显式指定 host target，避免继承仓库的 Xtensa 默认值：

```sh
NOTE4C_HOST_TARGET="$(rustc +stable -vV | sed -n 's/^host: //p')"
cargo +stable test -p ssd2683 --all-features --target "$NOTE4C_HOST_TARGET"
```

上例适用于显示驱动；其他局部改动替换为受影响 crate。修改像素打包、framebuffer 或显示驱动时，必须验证像素打包、缓冲区长度、颜色映射与 SSD2683 命令顺序，覆盖相关错误/超时清理路径。跨 crate 集成改动可运行完整主机套件：

```sh
cargo +stable test --workspace --exclude panelet-os --all-features \
  --target "$NOTE4C_HOST_TARGET"
```

改动备份、刷写或配网脚本时，运行对应 mock 测试；不要用连接真实设备代替脚本测试：

```sh
scripts/tests/test-backup-factory-flash.sh
scripts/tests/test-flash-server.sh
scripts/tests/test-provision-wifi.py
```

固件、ESP-IDF 配置或固件依赖变更用 `scripts/build-mvp.sh` 检查目标构建；工具链准备见 [README](README.md)。涉及镜像大小或分区布局时按分区文档核对可容纳性，不能将其当作运行时内存或 OTA 恢复验证。

实机测试记录板卡版本、固件 commit（工作树有改动时补充差异标识）、电源来源、观测到的 BUSY 时序、相关电流/电压与 pass/fail；无法判定时明确标为未确认，不能写 pass。记录放在对应 `docs/devices/<device>/history/` 下；历史制品名、提交标识与测量值不因品牌更名而改写。

交付应包含实现、受影响检查结果及必要的契约/文档更新。缺少设备或测量条件时准确区分已完成的软件验证与待执行的实机项目，不把待测行为写成已验证。

本次指令分层依据见 [编写说明](docs/development/agent-guidance-design.md)，仅在维护指令时查阅。
