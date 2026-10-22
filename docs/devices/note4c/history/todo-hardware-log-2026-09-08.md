# 待办小组件上板记录（2026-09-08）

## 条件与证据

- 目标板：ZECTRIX NOTE4C、PCB V1.0、四色 SSD2683，沿用项目单板记录，本轮未重新拍摄丝印。
- 供电：USB 数据连接；电池是否同时接入未确认，未测量电流、电压或电气波形。
- 主机：Apple Silicon macOS；ESP-IDF v5.5.3、Espressif Rust、esptool v4.12.0。
- 固件基线 commit：`52a816854c6045fa8fdabaf57ba71158b675de9d`，含当前未提交工作树的 server、配置/轮询、BOOT/LED、编译栈与字体修改。
- 最终字号版 ELF SHA-256：`010b3b3ad73f1b5005688adfa8a0aa40efeb148b8fecb376cf51ca7bd38c77f1`。
- app image：4,076,592 / 4,128,768 B（98.74%，余 52,176 B）。这是 Flash 容量，不是运行内存或续航证据。
- 本记录不包含实际网络地址、SSID、token、MAC、USB serial 或备份内容。

## 刷写与恢复边界

复用 [首次 bring-up 记录](bringup-log-2026-09-02.md) 中操作者确认的完整 16 MiB 原厂备份与第二副本。此次只读检查确认 Secure Boot disabled、Flash Encryption disabled，USB ROM 通信成功。设备分区表与当前双 OTA CSV 一致；OTA copy 0 的 sequence=1、state=VALID，CRC 校验通过，映射到 `ota_0`，另一副本为空。

刷写前另在仓库外私有目录保存当前完整 app 槽供回退。通过 `scripts/flash-server.sh` 的 app-only 模式，只写 `ota_0 @ 0x20000`。没有重写 bootloader、分区表或 otadata，也没有格式化 assets、擦除 NVS 或执行 eFuse 写操作。后续启动保留了配网、日历包和持久化 storage Ready 状态。

脚本新增了独立的已有布局核验门槛，自动从 ELF 生成非 merge 镜像，并检查 ESP magic 与 0x3f0000 槽容量。mock 覆盖缺少 gate、错误镜像、超限镜像和仅 app 写入命令。

本机打开被动串口时观测到 `USB_UART_CHIP_RESET`。因此等待网络、存储和启动显示空闲后再上传，不将该 USB 复位与下述编译栈故障混为一谈。

## 故障与修复

1. TODO 上传编译时，原 32 KiB widget pthread 出现 stack overflow 并软件复位，包未发布。改为单个 64 KiB worker 后上传成功；编译后最低剩余栈 25,208 B，随后运行记录为 23,880 B。VM 64 KiB 堆配额、编译 1 MiB 堆配额和指令/墙钟限制没有放宽。该水位只代表本轮示例，不代表所有源码的栈上限。
2. 用默认路由推导服务地址时选到了 VPN 虚拟接口，设备请求返回 `ESP_ERR_HTTP_CONNECT`。改为物理 Wi-Fi 接口后，HTTP GET、JSON 解析与显示任务成功。测试服务只暴露隔离的示例 JSON 目录。
3. 用户确认旧版画面正常，但英数比中文小，且备注像自动换行。新版改用两行排版，标题 18px、备注 16px 并缩进，最多 6 项；英数基础可见字高扩为 14px，大写显示。旧 `canvas.text` 和日历字体保持兼容，新增 `text_sized` 为按需选择。

## 已完成的闭环

以下首轮运行使用修复编译栈、加入 BOOT/LED 的版本；最后再加载分字号版。

| 测试 | 观察 | 结论 |
|---|---|---|
| 上传与配置 | TODO 包编译/持久化成功，配置 job 成功，激活为当前组件 | Pass（本轮操作） |
| 本地 HTTP 拉取 | 设备 `widget.poll` 成功，HTTP 服务收到对应 GET，界面切到 widget | Pass |
| 首次全刷 | PON/DRF 阶段 19,815 ms，POF/deep-sleep 167 ms，含初始化/传输约 20.1 s；job 成功 | Pass（软件 transport/清理条件） |
| 不变数据 | 后续自动检查成功，日志 `canvas unchanged; full refresh skipped`，没有对应 EPD 阶段 | Pass |
| 修改 JSON | 原子替换一条备注，不手动运行；下一周期自动全刷。PON/DRF 19,816 ms，POF/deep-sleep 167 ms | Pass |
| 修改后再次检查 | 多轮回到 unchanged/skip，无重复物理刷新 | Pass |
| BOOT 短按 | pressed→released 后生成手动 job，约 2.35 s 后 success；内容相同所以跳过 EPD | Pass |
| LED | 用户两次确认“常亮，然后闪两下灭掉”；不再有空闲 heartbeat | Pass（用户目视 + 日志） |
| 旧版画面 | 用户确认“画面正常”，并提出字号/备注排版改进 | Observed（用户目视） |

PON/DRF 是串口阶段耗时，包含上电/稳定等待和 BUSY 等待，不是逻辑分析仪测得的纯 BUSY_N 电平时长。job 成功要求现有实现中的 driver 与 forced-safe 清理成功；未以仪表独立测量 rail 电压。

## 最终配置与剩余边界

最终 TODO `0.2.0` 已上传并编译成功，激活保持为 `com.note4c.todo`，检查间隔恢复为 300 秒。最终自动拉取 job 成功，`main_screen=widget`、`display_busy=false`；PON/DRF 19,815 ms，POF/deep-sleep 167 ms，最低剩余栈 23,880 B。用户随后确认“新界面不错”。本地 HTTP JSON 服务保持运行，实际连接信息只在本机私有配置中。

最终启动恢复多个已存小组件时记录到一次非致命 task watchdog 告警，随后启动与拉取正常。地址解析定位在 `restore_widgets` → 文件 metadata / SPIFFS 查找路径；尚未增加恢复扫描的协作式让出 CPU。这项启动资源问题仍待修复，不能将本轮描述为“无告警启动”。用户已转入 WebDAV/快捷指令配置，本轮未继续改动或刷写设备。

124 个 host workspace 测试通过；新字体测试除了统一行高，还校验了数字的实际 14 行可见墨迹。预览使用真实 Rune + Canvas 渲染，不能代替显示面板逐字目视核对。

本轮未接入 WebDAV、验证 HTTPS/账号密码认证、测试受控断电一致性或测量功耗。当前 TODO 支持无认证 HTTP 与可选 HTTPS Bearer；若 WebDAV 使用 Basic 认证，需要另行接入。BOOT + RESET 的硬件路径不变，但没有把本轮软件更新当作新的电气安全验证。
