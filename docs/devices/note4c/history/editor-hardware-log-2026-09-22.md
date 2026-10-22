# 本机编辑器入口固件刷写记录（2026-09-22）

## 条件与固件

NOTE4C Devkit，项目目标 PCB V1.0；本轮通过 USB 连接供电和串口，未重新目视核对
PCB 丝印，未测量电压、电流或电池供电贡献。用户明确授权刷写当前固件。

- 基于 `c0747b6f32feca1a2e2c6c9a1ade68e633ac159e` 的未提交工作树。
- 源码差异指纹：`3e7fcf08f736bbbf62f089f223a2fc51f7afdc0e58cc1381cc9f530975e69b03`。
  按 [ABI 3 记录](abi3-hardware-log-2026-09-09.md) 的算法，对 `app/server`、`crates`、
  `examples/widgets` 下 65 个 `.rs/.toml/.html/.rn/.json` 文件按相对路径排序，
  依次哈希相对路径、NUL、内容、NUL。
- ELF SHA-256：`1342ab5d02b9765ffc84bd00e5194794f042f5320acc1f6b4de12da0cfddfb2f`。
- 非 merge app image SHA-256：`b264632fe920ea673f7c54f98cafeae825f002130530cdb0f55e0fe7b634f5de`。
- 镜像 **3,732,080 / 4,128,768 B（90.39%）**，余 **396,688 B**。
  镜像 checksum 和 validation hash 本地检查有效；目标构建成功。

本次设备端变化为管理页的小组件包导出和本机预览入口。真实预览执行位于独立的
macOS / Linux `note4c-editor` bin，未编入固件，见[编辑器说明](../../../development/widget-editor.md)。

## 刷写前核对与范围

复用 [首次 bring-up 记录](bringup-log-2026-09-02.md) 中同板完整 16 MiB 原厂备份、
NAS 第二副本和恢复演练核验；该记录已在后续 app-only 更新中复用。本轮另只读保存
`0x0..0x410000` 共 4,259,840 B 的当前内容，覆盖启动元数据和整个 `ota_0`，
并重新计算 SHA-256 核对。这是更新前的开发固件回退副本，不是原厂镜像。

本轮显式选定 USB 串口，USB vendor 匹配 Espressif，ROM 识别 ESP32-S3 rev 0.2；
只读安全查询确认 Secure Boot disabled、Flash Encryption disabled，ROM 下载通信成功。
设备分区表与当前双 OTA CSV 完全一致。OTA copy 0 的 sequence=1、state=VALID、
CRC 正确，映射至 `ota_0 @ 0x20000`，容量 `0x3f0000`；copy 1 为空。

满足以上条件后调用 `scripts/flash-server.sh` 的 app-only 模式，返回成功，仅写
`ota_0 @ 0x20000`。没有重写 bootloader、分区表、otadata 或 assets，没有擦除 NVS，
也没有执行 eFuse 写操作。设备标识、凭据、地址、回退副本、原始串口和 RPC 证据
保存在仓库外的本机私有目录，未写入本文。

## 启动与管理页观测

- 启动日志 ELF SHA 前缀 `1342ab5d0` 与目标制品一致。
- PSRAM 启动 memtest `OK`，存储 `Ready`；HTTP 管理服务启动。
- 设备返回的管理页 gzip 解压后为 69,078 B，与本次 `server.html` 逐字节相同。
  新增导出和本机预览入口已随固件更新。
- 使用原有 Admin credential 只读查询 `dashboard.get`：存储 ready/persistent、
  normal 模式、3 个组件、前台仍为 `com.note4c.todo`，输入 overflow=0。
- 启动状态画面刷新：PON/DRF **19,764 ms**，POF/deep-sleep **167 ms**，
  driver 总计 **20,040 ms**，结果 `ok`，终态 `PoweredOff`。
  这些是驱动阶段日志，并非仪表测得的纯 BUSY_N 脉宽或供电电压。
- 100 秒串口观察窗口未捕获 panic、Guru Meditation、watchdog 或栈溢出。
  Rune worker 最低剩余栈 8,376 / 65,536 B。

## 待办数据拉取异常与验证边界

启动恢复了原前台并完成 Rune 编译，但此次 `widget.mount` job #1 返回 failed /
`widget_host_error`；对应日志为 HTTP 连接 `Connection reset by peer`。
状态查询时 `presentation=idle`、`display_busy=false`，本次待办尚未提交新画面。
本轮没有修改待办数据源或凭据，没有据此判定数据源故障的具体原因，也没有把待办
内容刷新记为通过。

本记录确认固件写入、启动、存储恢复、管理页更新和启动刷新 transport；未重新验收
实体画面的字形、颜色或布局，未做按键压力、长期稳定性、功耗或数据源修复测试。
