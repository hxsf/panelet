# NOTE4C 硬件、驱动与 esp32-rs 实现路线

调研日期：2026-09-01

适用范围：ZECTRIX NOTE4C Devkit，PCB V1.0，4.2 英寸四色面板

## 1. 结论先行

NOTE4C 是一块 ESP32-S3 N16R8 开发设备，配备 16 MB Flash、8 MB PSRAM、400 x 300 的黑/白/红/黄四色电子纸、ES8311 音频、PCF8563 RTC、NFC、电池和三个实体按键。

建议首版 Rust 固件采用 ESP-IDF `std` 路线：

- 工程：官方 [`esp-idf-template`](https://github.com/esp-rs/esp-idf-template)
- target：`xtensa-esp32s3-espidf`
- HAL/服务：`esp-idf-hal`、`esp-idf-svc`，必要时通过 `esp-idf-sys` 使用 ESP-IDF API
- 显示：自行实现基于 `embedded-hal 1.0` traits 的 SSD2683 驱动
- 图形：先用固定 30,000 字节 2 bpp framebuffer；测试通过后再接 `embedded-graphics`

采用 `std` 不是因为 NOTE4C 无法运行 `no_std`，而是现有参考固件本身基于 ESP-IDF，首版还需要可靠的 Wi-Fi、NVS、ADC 校准、FreeRTOS 和诊断能力。显示驱动保持 `embedded-hal` 边界后，将来仍可迁移到 `esp-hal` 的 `no_std` target `xtensa-esp32s3-none-elf`。

当前 Rust 生态没有可直接用于 NOTE4C SSD2683 BWRY 面板的驱动。`epd-waveshare::Epd4in2` 面向其他 4.2 英寸黑白面板，控制器、颜色格式和刷新序列都不相同，不能只换尺寸或引脚后使用。

## 2. 证据等级与边界

本文将结论分为五类：

| 标记 | 含义 |
|---|---|
| **官方** | ZECTRIX Wiki、产品页、V1.0 原理图或芯片厂商数据手册明确给出 |
| **控制器 proposal** | Solomon Systech SSD2683 Product Proposal Rev 0.20（May 2024）；不是 final datasheet，内容仍可能变化 |
| **参考源码** | 示例固件 commit `51812e4...` 的实现行为；除非另有实测记录，否则不等同于板级验证 |
| **生态现状** | 2026-09-01 查询到的 esp-rs 官方文档或上游 crate 状态 |
| **待实测** | 文档/代码没有形成闭环，或会随硬件批次、供电和软件版本变化 |

核心本地资料：

- [官方 Wiki 快照](../../../3rd-res/wiki/note4c-quick-start.md)（[原址](https://wiki.zectrix.com/zh/hardware/note4c/quick-start)）
- [V1.0 共用控制板原理图](../../../3rd-res/datasheets/sch_zectrix_note4_developer_kit_v1.0_en.pdf)（[厂商原址](https://wiki.zectrix.com/sch_zectrix_note4_developer_kit_v1.0_en.pdf)）
- [SSD2683 Product Proposal Rev 0.20](../../../3rd-res/datasheets/SSD2683_0.20_proposal.pdf)；[来源与权利范围](../../../3rd-res/SOURCES.md#component-datasheets)单独记录。
- [固定版本的参考固件](../../../3rd-res/youn-ink-fourcolor-firmware/README.md)
- [第三方资料来源、版本和许可证](../../../3rd-res/SOURCES.md)

重要边界：

- NOTE4C 与黑白 NOTE4 共用控制板，但显示面板和固件镜像不兼容。
- 本地 SSD2683 文档由用户提供，没有公开来源 URL，且封面明确标为 Product Proposal Rev 0.20，而不是 final datasheet。本文用它约束控制器协议和电气时序，但不把 proposal 当成 NOTE4C 面板的完整规格或量产保证。
- 官方只承诺/描述四色全屏刷新；参考源码也会强制四色路径每次实际刷新都走 full refresh。
- 没有观察到触摸屏或 SD 卡硬件。示例 manifest 中的通用触摸/FATFS 依赖不能当作板载硬件证据。
- 示例配置中写有 NFC 型号 `GT23SC6699`，但官方 Wiki和公开原理图未标器件型号。本文只把该型号视为参考源码的声明。

## 3. 硬件规格

| 项目 | 规格 | 证据 |
|---|---|---|
| 主控 | ESP32-S3，双核 Xtensa LX7，最高 240 MHz | 官方 |
| 模组配置 | N16R8 | 官方 |
| 存储 | 16 MB Flash、8 MB PSRAM | 官方 |
| 无线 | 2.4 GHz 802.11 b/g/n Wi-Fi、Bluetooth LE 5 | 官方 |
| 显示 | 4.2 英寸电子纸，400 x 300 | 官方 |
| 颜色/格式 | 黑、白、红、黄；SSD2683；2 bpp BWRY | 官方 |
| 刷新 | 全屏刷新通常超过 10 秒 | 官方 |
| 音频 | ES8311 codec、模拟麦克风、扬声器/功放 | 官方、原理图 |
| RTC | PCF8563，带 INT | 官方、原理图 |
| NFC | PCB V1.0 包含；I2C + 场检测 | 官方、原理图 |
| 用户输入 | 上、下/电源、BOOT/确认三键；RESET 针孔 | 官方 |
| USB | Type-C，原生 USB 数据、下载与串口 | 官方 |
| 电池 | 2,000+ mAh 可充电电池 | 官方；具体批次待确认 |
| 尺寸/外壳 | 97 x 97 x 8.75 mm（含磁吸后盖），ABS | 官方 |
| 逻辑电平 | 3.3 V；GPIO 不可直接输入 5 V | 官方 |

`N16R8` 和官方页面确认了容量，但没有在页面中确认 Flash/PSRAM 的总线模式。参考固件选择 16 MB QIO Flash 和 80 MHz Octal PSRAM；新工程应通过实际模组、启动日志和内存测试确认，不能仅凭 `N16R8` 字样推断。

官方资料没有列出触摸、IMU、环境温湿度、光照或气压传感器。ESP32-S3 的片内温度传感器也不能当作环境温度传感器使用。

SSD2683 proposal 的每页页脚写明该控制器产品只能与获许可的 E Ink `E5 Spectra 3100 FPL` 配合使用。这能说明 **SSD2683 的许可适用范围**，却不能单独确认焊在这台 NOTE4C 上的 FPL 批次或商品名；现有 Wiki、共享控制板原理图和参考源码也没有给出面板料号。要把“NOTE4C 显示屏就是 Spectra 3100”升级为板级事实，仍需面板 FPC 丝印、BOM/采购料号或厂商书面确认。

## 4. 完整引脚映射

### 4.1 电子墨水屏

| 功能 | GPIO | 方向/有效电平 | 备注 |
|---|---:|---|---|
| EPD power enable | 6 | 输出，高开启 | 参考源码使用 GPIO hold |
| BUSY | 8 | 输入；暂按低为 busy | proposal 文字、p27-p28 波形和参考源码支持 active-low；GPIO8 电平仍需实测 |
| RESET | 9 | 输出，低复位 | 独立于整机 RESET |
| DC | 10 | 输出，低命令/高数据 | 手工 GPIO |
| CS | 11 | 输出，低选中 | 参考源码不使用 SPI 外设自动 CS |
| SCK | 12 | 输出 | SPI3；mode 0 仅由 proposal 时序图推断 |
| MOSI/SDA | 13 | 输出 | 四色路径只写；官方未列 MISO |

### 4.2 电源、按键、电池、LED

| 功能 | GPIO | 方向/有效电平 | 备注 |
|---|---:|---|---|
| BOOT / 确认键 | 0 | 输入，低有效 | strapping pin；按住并复位可进 ROM download |
| charge full / standby | 1 | 输入，高有效 | 原理图 `STDBY_H` |
| charging detect | 2 | 输入，低有效 | 原理图 `CHRG_L` |
| 绿色状态 LED | 3 | 输出，低点亮 | LED 接 3V3，由 GPIO 灌电流 |
| battery ADC | 4 | ADC1 channel 3 | 经电阻分压；源码按 1:1 分压换算，待实测 |
| RTC INT | 5 | 输入，通常低有效 | PCF8563 开漏中断，需上拉 |
| NFC field detect | 7 | 输入，低有效 | 参考源码双边沿中断、20 ms debounce |
| main power hold | 17 | 输出，高保持 | 电池启动后应尽早拉高 |
| 下键 / 电源键 | 18 | 输入，低有效 | 参考源码启动时等待按键释放 |
| NFC power | 21 | 输出，高开启 | 参考源码在传输前 power-cycle |
| 上键 | 39 | 输入，低有效 | 参考源码长按阈值 1 s |

硬件 RESET 针孔连接 ESP32-S3 EN/RST，不会擦除 Flash。

参考源码的 [`config.h`](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix-s3-epaper-4.2/config.h) 使用两个非常相似、但并不重名的宏：

- GPIO17：主电源保持输出；
- GPIO18：下键/物理电源键输入。

`VBAT_PWR_PIN` 是 GPIO17，传给 `BoardPowerBsp` 并被拉高；`VBAT_PWR_GPIO` 是 GPIO18，随后由 `InitializePower()` 轮询，直到按键松开。Rust BSP 应改用不易混淆的名字，例如 `POWER_HOLD = GPIO17` 和 `POWER_BUTTON = GPIO18`。

### 4.3 音频

| 功能 | GPIO | 方向 |
|---|---:|---|
| I2S MCLK | 14 | MCU -> ES8311 |
| I2S BCLK/SCLK | 15 | MCU -> ES8311 |
| codec DOUT | 16 | ES8311 -> MCU |
| I2S LRCK/WS | 38 | MCU -> ES8311 |
| audio rail / I2C pull-up supply enable | 42 | 输出，高开启 |
| codec DIN | 45 | MCU -> ES8311 |
| speaker PA enable | 46 | 输出，高开启 |

参考固件配置 I2S0、16 kHz、16-bit、MCLK=256fs、MCU master、全双工。GPIO42 开启原理图中的 `AVDD_3V3` 音频 rail；I2C 上拉也接到该 rail，但 RTC/NFC 芯片本体并不是由 GPIO42 直接供电。GPIO46 在代码中同时叫 `AUDIO_CODEC_PA_PIN` 和 `Audio_AMP_PIN`，实际是同一个功放控制信号。

GPIO45、GPIO46 也是 ESP32-S3 strapping 相关引脚。板子已经把它们用于音频，但固件不得在复位采样阶段假定可随意施加外部电平。

### 4.4 I2C

| 信号/器件 | 值 | 证据 |
|---|---|---|
| SDA | GPIO47 | 官方、源码 |
| SCL | GPIO48 | 官方、源码 |
| ES8311 | 7-bit `0x18` | 原理图；启动时仍应 scan |
| PCF8563 | 7-bit `0x51` | 官方、数据手册、源码 |
| NFC | 7-bit `0x55` | 官方、原理图、源码 |

参考固件以 400 kHz、100 ms timeout、ACK check 访问共享 I2C0，并用递归互斥锁串行化访问。容易遗漏的板级行为是：I2C 上拉接在 GPIO42 控制的 `AVDD_3V3` rail；每次 RTC/NFC I2C 访问前参考源码也会强制拉高 GPIO42。因此 bring-up 访问 I2C 时必须临时使能 GPIO42，但不要把它描述成 RTC/NFC 芯片本体的电源控制。

当前 Rust BSP 采用 `esp-idf-hal 0.46.2` 的 `I2cDriver` inherent `write`/`read`/`write_read` 方法，不走 `embedded-hal` 的无限等待 `BLOCK`。配置为 I2C0、400 kHz、关闭内部 pull-up、hardware timeout 约 100 ms，并给每笔事务传入 100 ms FreeRTOS tick timeout。driver 可以在 GPIO42 仍被 held low 时完成构造；真正测试时由 `&mut Board` 串行执行：先确认 GPIO21 NFC low，再严格执行 GPIO46 PA low -> GPIO42 high -> 3 ms settle -> transaction -> GPIO42 low。正常与错误返回都尝试 rail off；一旦安全置低失败，本次启动后续 I2C 测试会被禁止。

`I2C_SCAN` 只用空 write probe 扫 `0x08..=0x77`，没有寄存器写入；每笔和整轮都受限，并分别统计 `ESP_FAIL`/NACK、timeout 和其它错误。NACK 继续扫描；首个 timeout 或其它错误会被记录并立即中止，避免在总线疑似卡死或非法状态下重复事务，所以只有无总线错误时才会探测完整 112 个地址。因 GPIO21 始终 low，本轮只期待 ES8311 `0x18` 与 PCF8563 `0x51`，不要求 NFC `0x55` 出现。`RTC_READ` 只以 register pointer `0x02` 连读 7 B，两次读取相隔约 1100 ms；host-testable parser 校验 BCD、字段范围、实际日历与分钟/日期跨界。VL 置位表示读数不可信；century bit 只报告原值，不武断映射年代。2026-09-02 的单板实测完整扫描 112 个地址并发现 `0x18`、`0x51`，RTC 在约 1.1 s 间隔内推进 1 s；这只确认地址 ACK、读取和短时走时，不证明 codec 功能、RTC 精度或 GPIO42 电气时序。

### 4.5 USB、UART 和扩展排针

| 功能 | GPIO |
|---|---:|
| USB D- | 19 |
| USB D+ | 20 |
| UART0 TX | 43 |
| UART0 RX | 44 |

原理图 6 针 J3 排针：

| J3 pin | 信号 |
|---:|---|
| 1 | GPIO41 |
| 2 | GPIO40 |
| 3 | UART0 RX / GPIO44 |
| 4 | UART0 TX / GPIO43 |
| 5 | GND |
| 6 | 3V3 |

ESP32-S3 的 USB-Serial/JTAG 和 USB OTG 会共享内部 PHY。下载/日志与自定义 CDC/HID/MSC 不能被当作互不相关的两套 USB 使用；实现 USB 应用前先确定所需模式和调试退路。

## 5. 电源、启动和关机

### 5.1 建议启动顺序

根据原理图和参考固件，Rust 首版按以下顺序 bring-up：

1. 尽早把 GPIO17 配成输出并拉高，保持主电源锁存。
2. 预装 GPIO3/6/9/11/17/21/42/46 的安全 latch 与方向，再释放可能从原厂固件继承的 GPIO hold；GPIO42 默认保持 low。
3. 将 GPIO18 配成上拉输入；如仍为低，按参考源码行为等待物理电源键释放，同时保留 watchdog/超时诊断。
4. 初始化串口或 USB-Serial/JTAG 日志，并用 GPIO3 低有效 LED 做最小存活指示。
5. 在 GPIO42 low 时构造 I2C47/48 driver；只有执行 I2C 测试事务时，才按 PA low -> GPIO42 high -> settle -> transaction -> GPIO42 low 的顺序临时使能共享 pull-up rail。本轮 GPIO21 始终 low，不访问 NFC。
6. 需要刷屏时才拉高 GPIO6，完成刷新和 panel deep-sleep 后再拉低。
7. 音频使用前开启 GPIO42 和 GPIO46；不用时关闭功放，避免静态耗电/爆音。

GPIO17 的“应尽早”具体时间窗口尚未测量。首个固件应在入口处第一批 GPIO 操作中完成，而不是等网络、日志或堆初始化之后再设置。

### 5.2 关机与 deep sleep 是两件事

- panel sleep：SSD2683 进入 deep sleep，随后 GPIO6 断 EPD rail；电子纸仍保留画面。
- MCU deep sleep：ESP32-S3 低功耗运行状态；唤醒源需要单独配置。
- hard power-off：把 GPIO17 拉低，释放主板电源锁存。

参考固件的 deep sleep 只把 GPIO0 低电平配置为 EXT0 唤醒源。虽然 RTC INT 接到 GPIO5，参考固件并没有证明 RTC 已接入深睡唤醒。实现 RTC 定时唤醒时必须单独验证 RTC flag 清除、GPIO5 电平、ESP32 唤醒配置和电源锁存是否能组成闭环。

## 6. SSD2683 四色屏驱动

### 6.1 Rust 驱动接口

建议建立独立 `ssd2683` 模块，泛型依赖：

- `embedded_hal::spi::SpiDevice<u8>`，或 `SpiBus` + 独占设备包装；
- `embedded_hal::digital::OutputPin`：power、reset、dc、cs；
- `embedded_hal::digital::InputPin`：busy；
- `embedded_hal::delay::DelayNs`；
- 一个单调时钟/截止时间，用于真正有上限的 BUSY 等待。

驱动层不依赖 FreeRTOS、Wi-Fi、UI 或 `std`，让 command-order 和 framebuffer 测试能在主机运行。

proposal 给出的 SPI 写周期下限为 50 ns，即写时钟不得超过 20 MHz；读周期下限为 400 ns，即读时钟不得超过 2.5 MHz。NOTE4C 公开引脚只给出 SDA/MOSI、没有 MISO，当前驱动因此只实现写入，并把两种频率上限分别编码，不能因为参考固件曾配置 40 MHz 就突破 proposal 上限。

4-wire 时序表说明数据在 SCL 上升沿移入，时序图的空闲电平看起来为低；本文据此把总线配置为 SPI mode 0（CPOL=0、CPHA=0）。proposal 没有直接写出 CPOL/CPHA 编号，所以 **mode 0 是由图推断**，仍需用 NOTE4C 实机波形确认。

### 6.2 帧格式

- 分辨率：400 x 300。
- 2 bits/pixel，总大小：`400 * 300 * 2 / 8 = 30,000` 字节。
- row-major，每行 100 字节。
- 一个字节容纳四个水平像素，最左像素在 bits 7..6。
- 编码：`00=black`、`01=white`、`10=yellow`、`11=red`。
- 纯色 fill byte：黑 `0x00`、白 `0x55`、黄 `0xAA`、红 `0xFF`。

proposal 的 DTM 表只把这四个码称为 `Gray 0..3`，没有把二进制值绑定到黑、白、黄、红颜料。上述颜色名称和编码来自 NOTE4C 参考固件及其目标面板映射，**不是 SSD2683 proposal 的通用颜色定义**；必须通过四色实测图确认。

参考源码使用 current、previous 和 TX snapshot 三个 30,000 字节 PSRAM buffer，共 90,000 字节，不含 UI 内存。首个同步全刷实现只需一个 framebuffer；需要异步绘制/发送时再增加 snapshot。不要把锁、原子变量或同步对象放到 PSRAM。

### 6.3 参考源码中的全刷序列

结合 proposal 限制后的当前 Rust 全刷序列如下。命令含义和上限来自 proposal，`0xE9 0x01` 及面板颜色映射则仅来自参考源码。该全刷序列已在 2026-09-02 的一块 NOTE4C 上成功运行一次；这份单板结果不能外推为所有批次、电气时序或长期可靠性结论：

1. SPI 4-wire 写模式、手工 CS、MSB first；Rust demo 固定 10 MHz。mode 0 只是时序图推断，写速率绝不超过 20 MHz。
2. RESET 保持 low，再拉高 GPIO6 EPD rail。
3. RESET low 至少 20 ms，随后释放为 high。proposal 的 display-on 图给出默认 `LUT_EN=0` 时约 50 ms MTP/LUT 自动加载期，因此 Rust 驱动先静默等待 50 ms，再读取 BUSY。
4. 按 proposal 引脚文字和参考源码暂定 BUSY_N low 表示忙、high 表示可接受命令；所有等待都有 120 s 上限。
5. 命令 `0xE9`，数据 `0x01`。这个命令没有出现在 proposal 的用户命令表中，只能称为参考固件兼容命令，不能扩展成任意 raw-command API。
6. 命令 `0x10`（DTM），等 idle。
7. 逐行发送 `100 B x 300` 的 2 bpp 帧；示例每 16 行 yield 一次。
8. 命令 `0x04`（power on），等 idle；随后按参考固件策略再等待 10 ms 稳定窗口。
9. 命令 `0x12`（DRF）后必须完整写参数 `0x00`，等待 10 ms 后再开始首次 BUSY 采样，并继续等到 idle。
10. 命令 `0x02`（POF）后必须完整写参数 `0x00`，再等 idle。proposal 说明 POF 关闭高压模拟块，但在 VDD 仍存在时寄存器和 SRAM 才保持。
11. 等待 20 ms，命令 `0x07`、数据 `0xA5`（deep sleep）。
12. GPIO6 low，关闭 EPD rail。

进入 DSLP 后只能通过硬件 reset 回到 standby；Rust demo 随后还会切断 VDD，因此不得依赖控制器 RAM 保留，下一次显示必须重新发送完整 30,000 B framebuffer。参考源码对四色 BUSY 使用 120 s 上限，每 50 ms 轮询。Rust 驱动返回明确 timeout/error：若 BUSY 已经超时为低，不再发送控制器不会响应的 POF/DSLP，而是独立尝试 bus idle、RESET low 和 GPIO6 rail off；其他 SPI/I/O 不确定错误会按“高压可能已开启”做 best-effort POF，再执行独立硬件安全操作。板级调用随后还会再强制一次安全输出。所有异常关断路径仍需实机确认。

proposal 的引脚说明和命令文字规定 `L=busy`、`H=non-busy`，参考源码也采用 active-low。p27-p28 的波形图中，`BUSY_N` 在 SPI 发送 PON、DRF 或 POF 命令时为 high，命令发送完成、内部 operation 开始后转为 low，完成后再回到 high，与文字规范一致。当前实现据此使用 active-low；单板全刷与约 24.3 s 的有界等待已经成功，但尚未用逻辑分析仪直接测量 GPIO8，因此不能把软件解释提升为板级电气波形结论。

### 6.4 刷新策略与局刷实验边界

- 正常应用策略仍只使用全屏刷新；不复用归档固件的黑白 partial path。归档 NOTE4C 四色路径明确强制 full refresh，因此它不是四色局刷的实现证据。
- proposal 的 `0x83 PTLW` 证明 SSD2683 控制器定义了 inclusive partial window，但没有给出 NOTE4C 四色面板的局刷 waveform、LUT、颜料限制、刷新寿命或参考序列。因此它**不能证明 NOTE4C 四色屏支持安全局刷**。
- `ssd2683` 只有启用 `experimental-partial-refresh` feature 才暴露 `PartialWindow` 和 PTLW 实验 API。窗口固定按 NOTE4C 的 400 x 300 边界校验；坐标 inclusive，`x_start` 必须四像素对齐，`x_end % 4 == 3`，编码为 proposal 定义的 8 B 坐标加 1 B PMODE。
- `EPD_PARTIAL_EXP` 是协议探针，不是功能支持声明。它先完成一次 baseline 全刷并关电；新周期重新初始化并发送完整 30,000 B DTM，再以 `PTLW enable -> PON/DRF -> PTLW disable -> POF/DSLP` 刷新中心 `x=160..239, y=110..189`；观察后只做一次完整全刷恢复。它不实现短窗口 DTM，也不新增 raw、MTP 或 LUT 接口。2026-09-02 已在一块单板上运行一次：可见变化局限于窗口且恢复后无可见残影，但 PON/DRF 仍约 24.3 s，与全刷相同，结果保持 `Inconclusive`，不构成速度、寿命或生产支持证据。
- 实验项需要两次独立的约 1 秒 BOOT 长按，中间必须释放；导航会取消 armed，每次启动最多尝试一次。safe mode 或板级 EPD safety gate 不健康时硬拒绝。
- 合并短时间内的多次 UI 更新，再进行一次全刷。
- 当前同步 demo 在刷新期间不扫描或排队按键；未来若改为异步 UI，输入应与刷新状态解耦，只能在安全阶段处理排队事件，且不得并发写 panel。
- 刷新结束后断 EPD rail，不以屏幕仍有画面判断设备是否运行。

## 7. 其他外设的驱动方式

### 7.1 PCF8563 RTC

- I2C `0x51`，最大 400 kbit/s。
- 时间/日期是 BCD；寄存器 `0x02..0x08`。
- 闹钟寄存器 `0x09..0x0C`。
- countdown control/value 为 `0x0E/0x0F`；参考代码使用 1 Hz。
- GPIO5 接开漏 INT，输入需上拉；处理后要清 alarm/timer flag。

参考源码注释把器件写成 `PCF8563T/5`，但官方 Wiki 和公开原理图只确认到 PCF8563 系列；具体封装/后缀应以实物 BOM 或芯片丝印为准。

可以评估现有 `pcf8563` 类 `embedded-hal` crate，但应先核对它使用的 `embedded-hal` 主版本、alarm/timer 覆盖范围和 century/year 语义。最小寄存器驱动很小，自行实现也可保持行为可审计。

### 7.2 电池与充电状态

板上没有观察到 fuel-gauge IC：

- GPIO4 / ADC1_CH3 读取电阻分压；参考源码使用 12-bit、12 dB attenuation 和 curve-fitting calibration，再把校准 mV 乘 2，因此软件假定约 1:1 分压，但仍需从阻值或万用表实测确认。
- 参考源码取 10 次无延时平均，并用 `(-V^2 + 9016V - 19189000) / 10000` 映射百分比。
- 这个多项式只是示例固件经验曲线，不是通用 Li-ion SOC 模型，不应直接当作准确剩余电量。
- GPIO2 低代表充电活动，GPIO1 高代表充满；二者是输入状态，不是充电控制脚。

当前 `BATTERY` 正是按这个保守边界实现：GPIO1/2 设为浮空输入，约 2 s、100 点统计 first/last、high duty 和 stable/mixed；GPIO4 以 ADC1_CH3、12-bit、DB12、Curve calibration 读取 10 组 raw 与 calibrated mV。日志中的 `estimated_battery_mv_assuming_2_to_1` 明确只是未验证假设，测试只记 `Observed`，不输出 SOC，也不据此关机。2026-09-02 的单板实测得到约 2083 mV pin 电压和按 2:1 假设估算的约 4166 mV；用万用表在 USB/电池、空载/刷屏/音频等工况对照后，才能确认分压比并建立本机电量曲线和滤波策略。

### 7.3 NFC

官方可确认：I2C `0x55`、GPIO21 power、GPIO7 field detect。参考源码另行声明：

- 型号 `GT23SC6699`；
- GPIO21 high enable，传输会先 low 5 ms、再 high 1 ms 并 probe；
- GPIO7 active-low，双边沿 + 20 ms debounce；
- 16-byte block，UID block `0x00`，user block `0x01..0x37`，即 880 B；
- 读：先写一字节 block address，等 10 ms，再收 16 B；
- 写：发送 block address + 16 B，之后等 5 ms；
- 上层使用 Type 2 NDEF TLV。

在没有芯片官方资料和实机验证前，不要把上述 block layout 或 NDEF 能力暴露为稳定公共 API。先做地址 probe、只读 UID/固定测试块，再验证写入与手机读取。

### 7.4 ES8311 音频

推荐 std 首版使用 ESP-IDF/`esp_codec_dev` 的成熟配置方式，或通过小型 C wrapper 暴露给 Rust；I2S 数据通路仍由应用配置。最小验证：

1. GPIO42 high，I2C scan 确认 `0x18`。
2. 配置 I2S0：16 kHz、16-bit、MCLK 256fs、MCU master。
3. 先保持 GPIO46 PA off，初始化 codec 和静音状态。
4. 播放低幅度测试音前再打开 PA；停止时先 mute、关 PA，再关 rail。
5. 分别验证麦克风 RX 和扬声器 TX，记录通道布局和增益。

### 7.5 按键与 LED

- 三键都按 active-low、带 pull-up 处理。
- GPIO18 还有物理开机含义，按键抽象中应保留 `power` 语义。
- GPIO0 是 strapping/BOOT，不要在复位阶段外部强拉低。
- GPIO3 LED active-low。参考固件的指示方式为：充满常亮、充电 200 ms 亮/2800 ms 灭、活动 120 ms 脉冲；应用可重新定义，但底层极性应固定在 BSP。

### 7.6 存储、USB、触摸

- 示例设备存储使用内置 Flash 的 SPIFFS，不是 SD 卡。
- 未观察到 NOTE4C 触摸控制器或触摸初始化。
- 示例没有实现应用层 USB device/host 功能；官方只确认 Type-C 可供电、原生 USB 数据、下载和串口。
- 如未来做 CDC/HID/MSC，std 路线通常需要 ESP-IDF TinyUSB managed component + `esp-idf-sys`/C wrapper；no_std `esp-hal::otg_fs` 目前仍是不稳定 API。

## 8. esp32-rs 路线比较

| 维度 | ESP-IDF `std` | `esp-hal` `no_std` |
|---|---|---|
| target | `xtensa-esp32s3-espidf` | `xtensa-esp32s3-none-elf` |
| 模板 | `esp-idf-template` | `esp-generate` |
| GPIO/SPI/I2C | 成熟的 `esp-idf-hal` 封装 | 稳定层已足够驱动本板基础外设 |
| Wi-Fi/BLE | `esp-idf-svc`/ESP-IDF，迁移参考固件更直接 | `esp-radio` + `esp-rtos`，由生成器组合 |
| ADC 校准 | ESP-IDF 能力成熟 | 当前 analog/ADC API 仍涉及 unstable |
| USB/SDMMC/FATFS | ESP-IDF 组件更完整 | OTG、SDHOST/SDMMC 等仍在演进 |
| 构建/体积 | 工具链和运行时更重 | 更小、更直接，但组合网络/服务成本更高 |
| SSD2683 | 都需要自写驱动 | 都需要自写驱动 |

推荐阶段性方案：

- M0-M3 使用 std，快速获得日志、ADC 校准、I2C/SPI、Wi-Fi 和 FreeRTOS 诊断。
- SSD2683、PCF8563 和 board-support 模块使用 `embedded-hal` 抽象和纯 Rust 状态机。
- 硬件能力稳定后，如小工具不需要网络、复杂 USB/FS，再评估 no_std 分支；不要一开始维护两套固件。

官方工具链资料：

- [Rust on ESP Book](https://docs.espressif.com/projects/rust/book/)
- [`espup`](https://github.com/esp-rs/espup)
- [`espflash`](https://github.com/esp-rs/espflash)
- [`esp-idf-template`](https://github.com/esp-rs/esp-idf-template)
- [`esp-idf-hal`](https://github.com/esp-rs/esp-idf-hal)
- [`esp-hal` ESP32-S3 docs](https://docs.espressif.com/projects/rust/esp-hal/1.1.1/esp32s3/esp_hal/)

版本会变化。开始实现时应固定工具/crate 版本和 `Cargo.lock`，并重新核对官方文档；标记为 unstable 的 API 不受 SemVer 稳定性承诺。

## 9. 当前仓库结构与实现状态

当前目录和模块职责统一见[架构与目录规划](../../development/architecture.md)。固件入口为 `app/os/`，NOTE4C 的原始 GPIO 与供电策略集中在 `app/os/src/board.rs`；默认配置启用 8 MiB Octal PSRAM。设备专属文档与实测记录放在 `docs/devices/note4c/`，OS 契约放在 `docs/os/`。规划中的通用显示和 board adapter 尚未完成，目录更名不代表已经支持 NOTE4。

当前 `panelet-os` 默认启动 Wi-Fi、HTTP 和小组件运行时，不自动运行总线或显示诊断测试；正常启动仍可能呈现配网、连接信息或前台画面。它先尽早保持 GPIO17，再把 EPD、NFC、音频/I2C rail 和 PA 置为关闭。正常模式的三键仅交给前台小组件；通过管理页显式进入诊断模式后，才操作 `SYS`、`KEYS_LED`、`EPD_QUICK`、`EPD_PARTIAL_EXP`、`I2C_SCAN`、`RTC_READ`、`BATTERY`、`STATUS_SCREEN` 八项串口菜单。状态页和全刷都需单次长按；局刷实验需要两次独立长按且中间释放。显示测试固定使用 10 MHz SPI，并在正常或异常路径结束时再次尝试 RESET low 和 rail off。启动时按住上键进入 safe mode 后，三项显示测试与两个 GPIO42 I2C 测试都会被拒绝。操作步骤见[诊断说明](diagnostics.md)。

会话结果只存在 RAM，状态码为 `- P O ? F S A`，复位即清空；选择变化不会触发显示刷新。独立输入任务在显示期间继续采样，诊断 runner 仍没有运行中按键取消能力；菜单、armed 和执行结果以 JSON 日志为准。历史 LED pattern 实测保留在对应日志，不作为当前版本全部导航提示已验收的依据。`BATTERY` 约 2 s 统计 GPIO1 `STDBY_H` 与 GPIO2 `CHRG_L`，并读取 GPIO4/ADC1_CH3 的 10 组 12-bit DB12 Curve-calibrated 样本；它只报告 pin mV 和未经万用表确认的 2:1 估算，不报告 SOC 或执行低电量关机。

主机侧 framebuffer、RTC/calendar、UI、菜单/LED、命令顺序、BUSY timeout 和清理测试不能替代 ESP32-S3 target 编译，更不能替代实机波形、画面、功耗与恢复测试。截至 2026-09-02，全刷、PTLW 实验、I2C scan、RTC 只读、电池观测、状态页和 LED 菜单编码已在一块单板上运行，详见 [Phase 1 实测记录](history/phase1-hardware-log-2026-09-02.md)。PTLW 只在单次实验中限制了可见内容变化范围，其耗时与全刷相同；I2C ACK、RTC 秒推进和 ADC API 成功也不能替代长期精度、分压、电气波形或生产支持验证。

## 10. 实施里程碑与验收

### M0：备份和最小启动

- 备份完整 16 MB 原厂 Flash，离线保存并记录 SHA-256；不要提交到 Git。
- 已建立 std 工程；GPIO17 early high，GPIO42 在当前最小镜像中保持 low，等待并记录 GPIO18 的启动/松键行为。
- 串口日志与 GPIO3 active-low blink。
- 验收：USB 和电池供电分别冷启动 20 次，无意外掉电；记录板版本和固件 commit。

### M1：输入、电池和 I2C

- 三键 debounce/press/long-press。
- ADC 记录 GPIO4 raw、calibrated pin mV 和假设 2:1 的 battery mV，并用万用表确认分压比；不计算 SOC。
- GPIO1/2 连续约 2 s 统计 raw、占空与稳定性，不从单点武断判断充电状态。
- GPIO21 NFC rail 保持关闭时，I2C scan 只期望 `0x18`、`0x51`，不要求 `0x55`；RTC 只读并确认约 1 s 推进，不设置时间。
- 验收：不以 compile success 代替实测；保存脱敏串口日志和电压误差。

### M2：四色屏最小驱动

- 先以当前 10 MHz SPI bring-up；proposal 上限为写 20 MHz、读 2.5 MHz，不测试 40 MHz。
- 四个纯色、四色竖条、棋盘格和像素位置测试。
- 完整 BUSY timeout、refresh、deep-sleep、GPIO6 power-off。
- 验收：颜色编码、方向、30,000 B 长度、BUSY 极性与时间均有实测记录；异常路径可安全关屏。

### M3：低功耗

- 合并刷新、关闭 EPD/PA/NFC rail。
- GPIO0 唤醒；再单独研究 PCF8563 GPIO5 RTC 唤醒。
- 验收：测量而不是估计 deep sleep 和 hard-off 电流；连续 50 次睡眠/唤醒。

### M4：网络和应用

- 2.4 GHz Wi-Fi、NVS 配置、时间同步。
- 小工具 UI 和更新调度；避免秒级无意义刷新。
- 最后再加 OTA、音频、NFC 写操作和 USB class。

## 11. 待实机确认清单

- 实际 Flash/PSRAM 总线模式与 Rust/ESP-IDF 配置。
- GPIO17 最晚拉高时间、关机时序和 USB 供电下的行为。
- GPIO42 实际供电范围，以及关闭后 I2C pull-up/bus、RTC/NFC 通信和 codec 的状态。
- GPIO4 分压阻值、ADC 校准误差和适合本电池的 SOC 曲线。
- 10 MHz SPI 的信号完整性；若需提高速率，仅在不超过 20 MHz 的范围内比较刷新可靠性。
- SSD2683 完整 init 含义、BUSY 时间分布、异常恢复和是否存在安全 partial refresh。
- NFC 实际器件型号、block layout、写周期和 NDEF 兼容性。
- ES8311 地址、通道映射、PA 时序、增益和底噪。
- GPIO5 是否可在目标整机电源状态下实现 RTC 唤醒。
- 不同生产批次的电池、NFC、panel 与 PCB 差异。

完成这些测试之前，本文只是一份“官方资料 + 固定示例源码 + 当前 Rust 生态”的设计基线，不是量产硬件验证报告。

## 12. 关键源码索引

以下位置是本文“参考源码”结论的主要落点，revision 均为 `51812e4ab3fa80ba7a5a5a274635ca2cf3901a25`：

- 板级 GPIO、I2S、I2C、RTC、NFC、EPD 定义：[config.h](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix-s3-epaper-4.2/config.h#L6)
- 电源 -> I2C -> RTC -> NFC -> charge -> display -> buttons 初始化：[zectrix-s3-epaper-4.2.cc](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix-s3-epaper-4.2/zectrix-s3-epaper-4.2.cc#L62)
- early power、共享 I2C、display 参数、电池 ADC/SOC：[zectrix-s3-epaper-4.2.cc](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix-s3-epaper-4.2/zectrix-s3-epaper-4.2.cc#L265)
- GPIO hold 与 LED active-low 行为：[board_power_bsp.cc](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix-s3-epaper-4.2/board_power_bsp.cc#L8)
- SPI 配置、BUSY 极性、reset/init/full-refresh/deep-sleep：[custom_lcd_display.cc](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix-s3-epaper-4.2/custom_lcd_display.cc#L668)
- 四色 2 bpp 值与 MSB-first packing：[rawdraw.h](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/rawdraw/rawdraw.h#L1)
- 四色强制 full refresh：[custom_lcd_display.cc](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix-s3-epaper-4.2/custom_lcd_display.cc#L613)
- PCF8563 寄存器、alarm 与 countdown：[rtc_pcf8563.cc](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix-s3-epaper-4.2/rtc_pcf8563.cc#L7)
- NFC block layout 与能力：[zectrix_nfc.h](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix/zectrix_nfc.h#L23)
- NFC power-cycle 和 I2C block transfer：[zectrix_nfc.cc](../../../3rd-res/youn-ink-fourcolor-firmware/firmware/main/boards/zectrix/zectrix_nfc.cc#L615)
