# NOTE4C 硬件诊断

仅适用于当前 NOTE4C PCB V1.0 适配。首次操作前阅读[设备入口](README.md)和
[恢复与备份](bringup-and-recovery.md)；诊断操作可能改变显示或外设状态。

正常模式的三键只交给前台小组件，未绑定事件忽略，不穿透到诊断菜单。在管理页“硬件诊断”点击“进入诊断模式”，或调用 `system.mode.set {mode:"diagnostic"}` 后，前台会话暂停，按键才用于下列菜单。仅打开诊断页面不会切换模式。完成后点击“返回正常模式”或调用 `{mode:"normal"}`，宿主重建所选前台会话；模式不持久化，普通重启回到正常模式。safe mode 不允许通过该接口解除其保护。

菜单输出在 USB Serial/JTAG 日志中，菜单选择不会自动执行显示测试。普通启动仍可能显示配网、连接信息或前台小组件画面。

- 上键：选择上一项；启动时持续按住上键可进入 safe mode。
- 下键 / 电源键：选择下一项。
- BOOT / 确认键：执行当前诊断项；默认 `SYS` 只检查系统信息，不运行前台组件。`EPD_QUICK` 与 `STATUS_SCREEN` 必须长按约 1 秒。
- `EPD_PARTIAL_EXP`：必须长按 BOOT 约 1 秒、完全释放、再长按约 1 秒；第一次长按后按上/下键会撤销 armed 状态，每次启动最多进入一次。
- safe mode：EPD、NFC 和 GPIO42 音频/I2C 上拉 rail 保持关闭，并拒绝 `EPD_QUICK`、`EPD_PARTIAL_EXP`、`STATUS_SCREEN`、`I2C_SCAN` 与 `RTC_READ`。

菜单共有 8 项，顺序为 `SYS`、`KEYS_LED`、`EPD_QUICK`、`EPD_PARTIAL_EXP`、`I2C_SCAN`、`RTC_READ`、`BATTERY`、`STATUS_SCREEN`；默认选中 `SYS`，所以按一次上键可回绕到末尾状态页。`KEYS_LED` 可用于检查 LED 输出；菜单选择、armed 状态和执行结果以 USB JSON 日志为准。历史版本的 LED 导航与同步提示记录不代表当前输入路由下已经复验这些行为。

`I2C_SCAN` 以 400 kHz 扫描 7-bit `0x08..=0x77`，每笔事务 timeout 100 ms、整轮另设 12 s 上限；它只发送空 probe，不写设备寄存器。`ESP_FAIL`/NACK 属于正常未响应地址并继续扫描；首个 timeout 或其它错误会被记录并立即终止，避免对疑似卡死或非法状态的总线继续发事务。因此只有无总线错误时才会完成全部 112 个地址。每次事务窗口严格保持 GPIO46 PA low，开启 GPIO42、等待 3 ms，访问后无论成功失败都再次关闭 GPIO42；GPIO21 NFC rail 始终 low，因此该诊断只要求发现 ES8311 `0x18` 与 PCF8563 `0x51`，`0x55` 缺席不算失败。`RTC_READ` 只从 `0x02` 连读 7 字节，两次相隔约 1100 ms；它校验 BCD、日期跨界、秒推进和 VL，但只原样报告 century bit，不映射具体世纪。VL 置位时结果为不可信的 `Inconclusive`。

`BATTERY` 用约 2 秒的多点采样统计 GPIO1 `STDBY_H` 与 GPIO2 `CHRG_L` 的 raw、占空和稳定性，再以 ADC1_CH3/GPIO4、12-bit、DB12、Curve calibration 读取 10 组 raw/mV。它只报告 `adc_pin_mv` 和尚待万用表确认的 `estimated_battery_mv_assuming_2_to_1`，不推导 SOC 百分比，也不触发低电量关机。

`STATUS_SCREEN` 在一次显式长按后做一次普通全刷，不使用 PTLW。页面显示 8 项及本次启动内 RAM `SessionResults` 快照：`- P O ? F S A` 分别表示未运行、Pass、Observed、Inconclusive、Fail、Skipped、Aborted；复位后快照消失，选择变化不会自动刷屏。只要全刷 transport 和安全清理完成，该项记为 `Observed`，画面正确性仍需人工确认。

`EPD_QUICK` 发送一个非对称的黑、白、黄、红四色测试图，使用 10 MHz SPI 和有上限的 BUSY 等待；无论成功或错误都会再次尝试复位并关闭 EPD rail。当前测试同步执行，刷屏开始后不能用按键中止，恢复手段是硬件 BOOT + RESET。由于四色刷新可能超过 10 秒，不要在 BUSY 期间断电，除非测试明显异常并准备重新进入 ROM download。

`EPD_PARTIAL_EXP` 仅用于在全刷和恢复路径已经实机通过后探测 proposal 的 `0x83 PTLW`。它先全刷 baseline 并关电，等待约 10 秒；随后重新上电并仍发送完整 30,000 B modified framebuffer，只把中心 `x=160..239, y=110..189` 的 PTLW window 设为黑白棋盘格；再观察约 10 秒，并自动用一次全刷恢复 baseline 后关电。局刷失败时，只有 forced-safe 成功且 EPD 仍可用才延时后尝试一次全刷恢复，绝不循环。测试结果即使命令传输成功也记为 `Inconclusive`，必须结合画面、BUSY/阶段时序和 rail 测量人工判断；代码不实现短窗口 DTM、raw command、MTP 或 LUT 编程。
