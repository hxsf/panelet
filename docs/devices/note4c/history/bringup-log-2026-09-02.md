# NOTE4C 首次 Rust bring-up 实测记录（2026-09-02）

## 适用范围与证据等级

本记录只描述 2026-09-02 这一次通过 USB 供电完成的 NOTE4C 实测。仓库目标为 PCB V1.0，但本次没有拆机或重新目视核对 PCB 丝印，因此板级版本仍按项目范围记录，而不是新增的目视证据。

未记录 MAC、USB 序列号、完整串口路径、原厂 Flash 内容或其他设备标识。完整备份保存在仓库外，并已复制到 NAS；两份 SHA-256 经操作者确认一致。

## 环境

- 主机：Apple Silicon macOS 26.6.2；
- 供电：USB；电池是否同时参与供电未测量；
- 芯片：ESP32-S3 QFN56 revision v0.2；
- Flash：16 MiB、quad、3.3 V；
- 芯片报告的 embedded PSRAM：8 MiB；首个 Rust 镜像未启用 PSRAM；
- Rust 应用：`note4c-test-runner 0.1.0`，release；
- 应用 ESP-IDF：v5.5.3；
- 二级 bootloader 日志版本：`v5.5.1-838-gd66ebb86d2e`；
- ELF SHA-256：`6f354fdd20f86393d22a994a3824dbb49d53b17e351d98137f3ce808c12de5a1`；
- esptool：v4.12.0；espflash：v4.5.0；espup：v0.17.1；
- 仓库当时没有可引用的固件 commit；以 ELF SHA-256 和本记录对应的工作树为证据。

## 恢复与备份

| 项目 | 结果 | 证据边界 |
|---|---|---|
| 刷写前 BOOT + RESET | 连续两次通过 | `no_reset + no-stub` ROM `flash_id` 均成功 |
| 原厂 Flash 备份 | 通过 | 精确读取 16,777,216 B，本地 SHA-256 校验通过 |
| 第二副本 | 通过 | 操作者确认 NAS 副本 SHA-256 与本地一致 |
| 安全状态 | 允许当前无签名 demo | Secure Boot disabled；Flash Encryption disabled；download/USB/JTAG 禁用位均未设置 |
| 刷写范围 | 已明确接受 | espflash 覆盖 bootloader、partition table 和 app；未写 eFuse |
| 刷写后 BOOT + RESET | 通过 | 再次以 `no_reset + no-stub` 读取 ROM `flash_id` 成功 |
| 恢复演练后正常启动 | 通过 | 松开 BOOT 后复位，`boot:0xb (SPI_FAST_FLASH_BOOT)`；应用安全状态与菜单恢复 |

完整原厂镜像及其实际 SHA-256 只保留在受控备份位置，不进入仓库。

## 启动与安全状态

首次正常启动为 `SPI_FAST_FLASH_BOOT`，应用从 `0x10000` factory partition 加载。启动日志显示：

- `hold_release_healthy=true`，失败 GPIO mask 为 0；
- GPIO17 power hold 写入成功；
- EPD CS high、DC low、RESET low、rail off；
- NFC rail、audio rail、audio PA 均 off；
- 普通启动 `safe=false`；
- safe-mode 启动 `safe=true`，EPD/NFC/audio 均保持 off。

safe mode 中选择 `EPD_QUICK` 并长按确认后，日志明确返回 `board_safety_gate`、`Skipped`；屏幕没有变化。该结果证明本次运行的软件门禁生效，但没有用万用表验证每条 rail 的实际电压。

## 菜单与 GPIO

- `SYS`：通过；reset reason 为 11，free heap 与 minimum free heap 均为 365,080 B；
- BOOT/确认键：按下、释放和长按事件正常；
- 下键/电源键：按下、释放和菜单换项正常；
- 上键：按下、释放、菜单回绕和启动 safe mode 正常；
- 绿色 LED：`KEYS_LED` 中目视快速闪烁两次，结果记录为 `Observed`；
- GPIO19/20 未被应用作为普通 GPIO 占用，USB Serial/JTAG 正常。

## SSD2683 四色全刷

配置：400 x 300、30,000 B、2 bpp、MSB-first、SPI mode 0、10 MHz。只执行一次 full refresh，未发送 partial-window、LUT 或 MTP 编程命令。

| 阶段 | 实测耗时 |
|---|---:|
| initialize | 62 ms |
| reference-only `E9 01` | 0 ms（日志毫秒精度） |
| framebuffer load | 24 ms |
| PON + DRF refresh | 24,313 ms |
| POF + deep sleep | 169 ms |
| 总耗时 | 24,573 ms |

驱动结果为 `PoweredOff`、`driver_success=true`、`driver_cleanup_clean=true`、`forced_safe=true`。最终强制安全报告为 CS high、DC low、RESET low、EPD rail off。

目视结果：

- 从左到右出现黑、白、黄、红四色带；
- 左上白、右上黑、左下红、右下黄四个不等大的小矩形；
- 出现从左上到右下的黑色斜线；
- 刷新期间屏幕经历多次闪变，最终画面完整；
- 未观察到缺行或错误颜色，但未进行显微、灰阶或长期 ghosting 检查。

这次成功结果确认当前板与当前驱动的颜色码、packing、方向和全刷序列可工作。24.3 s 的 DRF 等待也与 active-low BUSY 软件解释相容；由于没有逻辑分析仪直接测量 GPIO8 电平，不能把它提升为板级电气波形测量结论。

## 尚未验证

- 局部刷新：本次历史运行没有发送 PTLW，不能证明 panel waveform 支持 partial refresh；`EPD_PARTIAL_EXP` 后来完成的一次单板实验另见 [Phase 1 实测记录](phase1-hardware-log-2026-09-02.md)，其结果仍为 `Inconclusive`；
- EPD、GPIO17 和其他 rail 的电压、电流及瞬态；
- 电池冷启动、充电状态、硬关机和低功耗；
- 后续新增的 `I2C_SCAN`、`RTC_READ`、`BATTERY` 与 `STATUS_SCREEN` 菜单项均未在本次历史运行中执行；它们的编译或主机测试结果不能改写成本次实机通过；
- 后续新增的 RAM `SessionResults` 状态码与非阻塞 LED 菜单/结果 pattern 也未在本次历史运行中目视验证；上文 LED 结果只对应当时 `KEYS_LED` 的两次快速闪烁；
- RTC、I2C、ADC 分压/校准、电池电压与状态、Wi-Fi、音频和 NFC；不得从新增代码推导电池百分比；
- 不同温度、重复全刷、ghosting、长期稳定性和异常断电恢复。
