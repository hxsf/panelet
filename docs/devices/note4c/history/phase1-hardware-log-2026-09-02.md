# NOTE4C Phase 1 实机测试记录（2026-09-02）

## 适用范围与证据边界

本记录描述 2026-09-02 在同一块 NOTE4C 上完成的 Phase 1 固件实测。设备通过 USB 连接；电池是否同时参与供电、PCB 丝印版本、环境温度、rail 电压和电流均未测量。记录不包含 MAC、USB 序列号、完整串口路径、原厂 Flash 内容或其他设备标识。

- 应用：`note4c-test-runner 0.1.0`，release；
- 应用 ESP-IDF：v5.5.3；
- 应用 ELF SHA-256：`480c7466f634b313aefbc711e81866d0ec0329de97289c86a76ef2ac6d2bcaf2`；
- 芯片：ESP32-S3 QFN56 revision v0.2；
- Flash：16 MiB，应用从 factory partition `0x10000` 启动；
- 主机：Apple Silicon macOS；
- 工具：espflash v4.5.0、esptool.py v4.12.0。

本记录中的 `Pass` 或 `Observed` 只适用于本次单板、单次运行和明确列出的观察，不能外推到其他批次、温度、寿命或生产支持。

## 启动与安全状态

刷写后应用以 `SPI_FAST_FLASH_BOOT` 正常启动。启动日志确认：

- `hold_release_healthy=true`，失败 GPIO mask 为 0；
- EPD CS high、DC low、RESET low、rail off；
- NFC rail、GPIO42 audio/I2C rail 和 PA 均 off；
- I2C、ADC1_CH3、GPIO1 `STDBY_H` 和 GPIO2 `CHRG_L` 初始化成功；
- 普通启动 `safe=false`，菜单默认选中 `SYS`。

本次没有重新执行 Phase 1 safe-mode 的全部拒绝矩阵，也没有用万用表或逻辑分析仪测量 GPIO6/21/42/46 的实际电压。因此这里只确认软件日志和 API 返回，不把 rail 状态提升为电气测量结论。

## 三键与 LED

- 下键的 pressed/released 与菜单换项正常；
- BOOT 的 pressed/released/long-pressed 正常；
- `KEYS_LED` 中绿色 LED 目视闪烁两次，结果为 `Observed`；
- 菜单选择后 LED 序号闪烁、测试运行时常亮、完成后结果闪烁并回到 heartbeat，均有目视响应；
- 没有逐一计时测量全部 safe/armed/failure LED pattern。

## I2C scan

`I2C_SCAN` 保持 GPIO21 NFC rail off，以 400 kHz、100 ms 单事务上限扫描 `0x08..=0x77`。结果：

| 项目 | 实测 |
|---|---:|
| 尝试地址 | 112 |
| 总线事务阶段耗时 | 11 ms |
| ACK 地址 | `0x18`、`0x51` |
| `ESP_FAIL`/NACK | 110 |
| timeout | 0 |
| 其他错误 | 0 |
| `0x55` NFC | 未出现；本测试不要求 |

结果为 `Pass`。这确认了本次运行中 ES8311 地址 `0x18` 和 PCF8563 地址 `0x51` 可 ACK，但地址响应不能替代 codec、RTC 精度或 NFC 功能验证。GPIO42 的 3 ms settle 和 rail-off 只由软件路径确认，未测量电气波形。

## PCF8563 RTC 只读

从 `0x02` 连续读取 7 字节，间隔请求值 1100 ms；实际间隔 1099 ms。两次解析结果：

- 第一次：`YY=24, 01-06, weekday=6, 11:40:23`；
- 第二次：`YY=24, 01-06, weekday=6, 11:40:24`；
- `century_bit=false`，未映射具体世纪；
- 两次均 `VL=false`；
- BCD、字段范围和日历校验通过，进度为 `AdvancedBy(1)`。

结果为 `Pass`，含义仅为 I2C 读取和本次 1 秒推进正常。RTC 内容明显不是当前日期时间，固件没有设置或校准 RTC，也没有验证长期频率误差、闹钟、INT 或深睡唤醒。

## 电池与充电状态观测

GPIO1/2 以约 20 ms 周期采样 100 次，状态阶段耗时 1979 ms：

- GPIO1 `STDBY_H`：100/100 high，`stable_high`；
- GPIO2 `CHRG_L`：100/100 high，`stable_high`；
- GPIO4 ADC1_CH3：12-bit、DB12、Curve calibration，10 个 raw 样本为 `2515..2517`；
- 平均 raw 约 `2516`；
- 平均 ADC pin 电压约 `2083 mV`；
- 按参考固件但尚未用万用表确认的 2:1 假设，估算约 `4166 mV`。

结果为 `Observed`。GPIO 组合与“未处于低有效充电、standby/full 信号为高”相容，但不能仅凭本次观测确认电池充满。`4166 mV` 也不能在分压比和万用表读数确认前作为准确电池电压，更不能换算为 SOC 百分比或关机阈值。

## 状态页

`STATUS_SCREEN` 使用普通全刷，不使用 PTLW。操作者目视确认：

- 页面方向正确；
- 标题 `NOTE4C TEST` 和 `MODE NORMAL` 正确；
- 八项菜单及状态码可见；
- 底部按钮说明可见。

| 阶段 | 实测耗时 |
|---|---:|
| initialize | 63 ms |
| framebuffer load | 24 ms |
| PON + DRF | 24,313 ms |
| POF + deep sleep | 169 ms |
| 总耗时 | 24,574 ms |

驱动、清理和 forced-safe 均成功，结果为 `Observed`。页面是 RAM session snapshot；选择变化不会刷新光标，复位后会话状态清空。

## SSD2683 PTLW 局刷实验

窗口为 inclusive `x=160..239, y=110..189`，黑白棋盘格理论打包区域为 1600 B。实验仍在每个周期发送完整 30,000 B framebuffer；没有短窗口 DTM、LUT、MTP 或 raw command。

| 阶段 | 总耗时 | PON + DRF |
|---|---:|---:|
| baseline full | 24,572 ms | 24,313 ms |
| partial experimental | 24,577 ms | 24,312 ms |
| recovery full | 24,574 ms | 24,313 ms |

三个阶段的 driver、cleanup 和 forced-safe 均成功。目视观察：

- baseline 是正常完整四色全刷；
- 中央黑白棋盘格只在目标窗口内出现；
- 窗口外没有可见闪烁或内容变化；
- 窗口边缘整齐；
- 最后的 full recovery 正常恢复基线；
- 恢复后未见残影或颜色污染。

结论：在这块单板、这一次运行中，SSD2683 `PTLW` 对“可见内容变化范围”的限制有效，但局刷的 PON/DRF 耗时与全刷相同，没有速度收益。单次无可见残影不能证明四色 FPL 的长期 waveform 安全、刷新寿命、温度范围或高频局刷能力；固件仍把结果记为 `Inconclusive`，不得据此宣称 NOTE4C 已获得生产级局刷支持。

## ROM 恢复与正常启动

完成测试后按 BOOT + RESET 进入 ROM download，esptool.py v4.12.0 的只读 `chip_id` 成功识别 ESP32-S3 revision v0.2。随后释放串口工具的 DTR/RTS 控制并在不按 BOOT 的情况下手工短按 RESET，操作者确认绿色 heartbeat 恢复。

`espflash monitor` 的 `Ctrl+R` 在本机 USB Serial/JTAG 路径上会再次把设备带入 download mode，因此后续被动监视不应使用该快捷键；先关闭监视器释放控制线，再手工 RESET 可正常启动。这个现象属于主机串口控制线行为，不是 Flash 损坏或固件失去 ROM 恢复能力。

## 尚未验证

- Wi-Fi、BLE、ES8311 音频播放/录音、NFC field/read/write；
- RTC 校时、长期精度、alarm/timer、GPIO5 INT 和深睡唤醒；
- ADC 分压比、电池端万用表对照、充电状态各阶段和 SOC 曲线；
- GPIO42、EPD 和其他 rail 的电压、电流、settle 与关断波形；
- Phase 1 safe-mode 的完整实机拒绝矩阵；
- 局刷的重复次数、温度、寿命、彩色窗口和窗口尺寸边界。
