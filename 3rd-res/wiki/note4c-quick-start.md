<div id="root">

<div>

# <a href="#zectrix-note4c-开发套件开发指南" class="toc-anchor">¶</a> ZECTRIX NOTE4C 开发套件开发指南

> **硬件适用范围：** 本指南仅适用于四色屏 **ZECTRIX NOTE4C 开发套件**（PCB 版本 **1.0**）：ESP32-S3 N16R8，配备 4.2 英寸 400 × 300 黑、白、红、黄四色电子墨水屏。除显示面板及其刷新行为外，NOTE4C 与黑白屏 NOTE4 开发套件使用相同的控制板、按键、音频、电源系统、USB 接口和外壳。两者的显示固件镜像不可混用。

<img src="/note4c-devkit-controls-ports-en.png" style="display:block;width:100%;max-width:760px;height:auto;aspect-ratio:38/37;" width="760" height="740" alt="ZECTRIX NOTE4C 开发套件按键与接口" />

## <a href="#产品说明" class="toc-anchor">¶</a> 产品说明

ZECTRIX NOTE4C 开发套件是一款组装完成的 ESP32-S3 四色电子墨水屏开发平台，适用于低功耗信息显示、电子标签、照片卡片、状态面板和自定义嵌入式项目。设备在一个成品中集成了显示屏、控制板、可充电电池、麦克风、扬声器、RTC、NFC、实体按键、USB Type-C、磁吸背板和外壳。

NOTE4C 是开放式开发设备，不包含开箱即用的消费版 NOTE4 固件、官方 NOTE4 AI 语音任务体验或 NOTE4 云服务。公开参考固件提供基础 Wi-Fi 配置、图片传输、四色屏测试和二次开发能力。

## <a href="#从这里开始" class="toc-anchor">¶</a> 从这里开始

- <a href="https://zectrix.com/en/note4c.html" class="is-external-link">NOTE4C 开发套件官方产品页</a>
- <a href="https://github.com/LazyYoun/youn-ink-fourcolor-firmware" class="is-external-link">NOTE4C 参考固件与源码</a>
- <a href="https://github.com/LazyYoun/youn-ink-fourcolor-firmware/blob/2bp/README.md" class="is-external-link">固件仓库 README</a>
- <a href="/notellm-2bp-v6.5.9-2bp-3fbbabb-20260705-mergebin.bin" class="is-asset-link">下载可直接刷写的 NOTE4C 合并固件</a>
- <a href="https://zectrix.com/en/firmware-updater.html" class="is-external-link">ZECTRIX 浏览器固件更新工具与下载模式说明</a>
- <a href="/zh/software/note4-development-guide" class="is-internal-link is-valid-page">黑白屏 NOTE4 开发套件开发指南</a>

## <a href="#重要说明" class="toc-anchor">¶</a> 重要说明

- 刷写前请确认设备是四色屏 NOTE4C，绝不要向 NOTE4C 安装黑白屏 NOTE4 固件镜像。
- PCB 版本 1.0 包含 NFC。未来的国际消费量产版可能取消 NFC，购买前请向 ZECTRIX 客服确认 NFC 配置。
- 安装第三方固件或更改分区表前，请完整备份 16 MB 原厂 Flash。
- 请使用支持数据传输的 USB 线。仅支持充电的线缆可以为设备供电，但不会显示串口。
- 四色屏全屏刷新通常需要 10 秒以上。刷新过程中的闪烁、短暂颜色反转或可见色粉移动均属正常现象。
- 电子墨水屏无需持续供电也会保留最后一幅画面。屏幕上有画面并不表示设备仍在运行。
- 本指南后文描述的是预编译参考固件的按键行为。其他固件项目可以为相同硬件输入定义不同的应用行为。

## <a href="#主要特性" class="toc-anchor">¶</a> 主要特性

- ESP32-S3 N16R8，16 MB Flash、8 MB PSRAM
- 4.2 英寸 400 × 300 黑、白、红、黄四色电子墨水屏
- SSD2683 显示控制器和 2 bpp BWRY 输出
- 2.4 GHz Wi-Fi 与 Bluetooth LE 5.0
- 集成麦克风、扬声器、ES8311 音频编解码器、PCF8563 RTC、NFC 和状态 LED
- 三个实体用户按键及一个内凹 RESET 针孔
- USB Type-C 供电、原生 USB 数据、固件刷写与串口监视
- 2,000+ mAh 可充电电池

## <a href="#包装内容" class="toc-anchor">¶</a> 包装内容

- 1 × ZECTRIX NOTE4C 开发套件
- 1 × 桌面支架
- 已组装的四色显示屏、控制板、电池、外壳、按键和磁吸背板
- 在线参考固件、源码和开发资料

## <a href="#典型应用" class="toc-anchor">¶</a> 典型应用

- 四色天气、日历、任务和状态看板
- 电子标签和低频更新信息面板
- 电子墨水照片卡片和画廊显示
- 支持语音的 ESP32-S3 交互界面
- NFC 触发工作流和智能标签
- 四色屏驱动、电源管理和嵌入式 UI 开发
- 家庭自动化面板和自定义 IoT 终端

## <a href="#规格参数" class="toc-anchor">¶</a> 规格参数

<div class="table-container">

| 项目          | 规格                                           |
|---------------|------------------------------------------------|
| 产品          | ZECTRIX NOTE4C Devkit                          |
| PCB 版本      | 1.0                                            |
| 主控          | ESP32-S3 N16R8                                 |
| Flash / PSRAM | 16 MB Flash / 8 MB PSRAM                       |
| 无线连接      | 2.4 GHz Wi-Fi、Bluetooth LE 5.0                |
| 显示屏        | 4.2 英寸四色电子墨水屏                         |
| 显示颜色      | 黑、白、红、黄                                 |
| 分辨率        | 400 × 300 像素                                 |
| 显示控制器    | SSD2683                                        |
| 显示格式      | 2 bpp BWRY                                     |
| 刷新          | 全屏刷新；每幅画面通常需要 10 秒以上           |
| 音频          | ES8311 编解码器、麦克风、扬声器                |
| RTC           | PCF8563                                        |
| NFC           | PCB 版本 1.0 包含                              |
| 操作键        | 上键、下键 / 电源键、BOOT / 确认键、内凹 RESET |
| USB           | USB Type-C 供电、原生 USB 数据、刷写和串口     |
| 电池          | 2,000+ mAh 可充电电池                          |
| 尺寸          | 97 × 97 × 8.75 mm（含磁吸后盖）                |
| 外壳          | ABS                                            |

</div>

## <a href="#按键与接口" class="toc-anchor">¶</a> 按键与接口

本指南顶部给出了按键与接口示意图。屏幕朝向使用者时：

<div class="table-container">

| 实体按键或接口 | 硬件信号 | 说明 |
|----|----|----|
| 右侧上键 | GPIO39 | 上键，低电平有效 |
| 右侧下键 | GPIO18 | 下键 / 电源相关输入，低电平有效 |
| 正面圆形按键 | GPIO0 | BOOT / 确认，低电平有效；启动配置引脚 |
| 圆形按键旁的正面小针孔 | 麦克风输入 | 连接到 ES8311 音频通路 |
| 扬声器孔列最右端的独立小孔 | GPIO3 | 状态 LED |
| 左侧 RESET 针孔，与下键等高 | ESP32-S3 EN / RST | 短按可复位主控 |
| 底部中央 USB Type-C 接口 | USB D- GPIO19、USB D+ GPIO20 | 供电、原生 USB 数据、刷写和串口 |

</div>

## <a href="#电源与复位" class="toc-anchor">¶</a> 电源与复位

GPIO17 控制主板的电源保持电路。使用电池运行的固件必须在启动后维持所需的电源锁存状态。GPIO18 同时连接到下键 / 电源键，因此开机、关机和长按行为取决于所安装的固件。

左侧内凹针孔是硬件 RESET 输入，连接 ESP32-S3 的 EN / RST 信号，不会清除固件或用户数据。

## <a href="#进入-esp32-s3-下载模式" class="toc-anchor">¶</a> 进入 ESP32-S3 下载模式

大多数电脑无需按键即可连接 NOTE4C 开发套件。如果浏览器更新工具或 `esptool` 无法识别设备：

1.  保持 USB Type-C 数据线连接。
2.  按住正面的圆形 **BOOT / 确认** 按键（GPIO0）。
3.  保持 BOOT 按下，同时短按一次左侧内凹 **RESET** 针孔。
4.  松开 BOOT。
5.  重新连接，并选择新出现的 ESP32-S3 串口设备。

以上操作会让 ESP32-S3 手动进入 ROM 下载模式。正常重启后设备会恢复常规运行。

## <a href="#引脚映射" class="toc-anchor">¶</a> 引脚映射

### <a href="#电子墨水屏接口" class="toc-anchor">¶</a> 电子墨水屏接口

控制器侧的电子墨水屏信号与 NOTE4 共用，但安装的面板和固件配置不同。

<div class="table-container">

| 功能               | ESP32-S3 GPIO |
|--------------------|--------------:|
| 电子墨水屏电源使能 |             6 |
| BUSY               |             8 |
| RESET              |             9 |
| DC                 |            10 |
| CS                 |            11 |
| SCK                |            12 |
| MOSI / SDA         |            13 |

</div>

### <a href="#按键-led-电池与充电" class="toc-anchor">¶</a> 按键、LED、电池与充电

<div class="table-container">

| 功能          | ESP32-S3 GPIO | 说明                     |
|---------------|--------------:|--------------------------|
| BOOT / 确认   |             0 | 低电平有效；启动配置引脚 |
| 充满 / 待机   |             1 | `STDBY_H`                |
| 充电状态      |             2 | `CHRG_L`                 |
| 状态 LED      |             3 | 绿色 LED                 |
| 电池电压 ADC  |             4 | 通过电阻分压读取电池电压 |
| RTC 中断      |             5 | PCF8563 中断             |
| NFC 场检测    |             7 | NFC 场检测信号           |
| 电源保持      |            17 | 主板电源锁存控制         |
| 下键 / 电源键 |            18 | 低电平有效               |
| NFC 电源控制  |            21 | NFC 掉电 / 使能控制      |
| 上键          |            39 | 低电平有效               |

</div>

### <a href="#音频接口" class="toc-anchor">¶</a> 音频接口

<div class="table-container">

| 功能                     | ESP32-S3 GPIO |
|--------------------------|--------------:|
| I2S MCLK                 |            14 |
| I2S BCLK / SCLK          |            15 |
| 编解码器 DOUT / ESP32 RX |            16 |
| I2S LRCK / WS            |            38 |
| 音频电源使能             |            42 |
| 编解码器 DIN / ESP32 TX  |            45 |
| 扬声器功放控制           |            46 |

</div>

### <a href="#i2c-外设" class="toc-anchor">¶</a> I2C 外设

<div class="table-container">

| 信号 / 外设      | 值     |
|------------------|--------|
| I2C SDA          | GPIO47 |
| I2C SCL          | GPIO48 |
| PCF8563 RTC 地址 | `0x51` |
| NFC 接口地址     | `0x55` |

</div>

### <a href="#usb-与-uart" class="toc-anchor">¶</a> USB 与 UART

<div class="table-container">

| 功能     | ESP32-S3 GPIO |
|----------|--------------:|
| USB D-   |            19 |
| USB D+   |            20 |
| UART0 TX |            43 |
| UART0 RX |            44 |

</div>

所有 GPIO 信号均使用 3.3 V 逻辑电平，请勿向 GPIO 直接输入 5 V。

## <a href="#原理图" class="toc-anchor">¶</a> 原理图

- <a href="/sch_zectrix_note4_developer_kit_v1.0_en.pdf" class="is-asset-link">下载 V1.0 共用控制板英文标注原理图 PDF</a>

NOTE4C PCB 版本 1.0 与该原理图中的 NOTE4 开发套件使用相同的控制板和外设映射。安装的电子墨水面板及其所需的固件配置不同，开发时请使用下文所述的 NOTE4C 四色面板配置。

## <a href="#刷写参考固件" class="toc-anchor">¶</a> 刷写参考固件

### <a href="#可直接刷写的合并镜像" class="toc-anchor">¶</a> 可直接刷写的合并镜像

以下合并镜像来自此前公开的 NOTE4C 开源参考固件版本。下一节的简易操作说明也以这个镜像为准。

<div class="table-container">

| 项目 | 值 |
|----|----|
| 下载 | <a href="/notellm-2bp-v6.5.9-2bp-3fbbabb-20260705-mergebin.bin" class="is-asset-link">NOTE4C 合并固件</a> |
| 文件名 | `notellm-2bp-v6.5.9-2bp-3fbbabb-20260705-mergebin.bin` |
| 目标硬件 | ESP32-S3 NOTE4C 四色屏硬件 |
| 文件类型 | 合并固件镜像 |
| 写入地址 | `0x0000` |
| 文件大小 | 3,054,752 字节 |
| SHA-256 | `26f209f7cdea8f62bff73f2e877e3ab421917c17497a2a23340129c6ef43fc36` |

</div>

刷写前请核对文件名、文件大小和校验值。不要将此镜像写入黑白屏 NOTE4。

### <a href="#推荐zectrix-浏览器固件更新工具" class="toc-anchor">¶</a> 推荐：ZECTRIX 浏览器固件更新工具

<a href="https://zectrix.com/en/firmware-updater.html" class="is-external-link">ZECTRIX 浏览器固件更新工具</a>通过 USB 直连安装本地 `.bin` 文件。浏览器只在本地读取固件文件，不会将其上传到 ZECTRIX。

1.  在桌面版 Chrome 或 Edge 中打开更新工具。Safari 和 Firefox 不提供所需的 Web Serial 连接能力。
2.  使用 USB 数据线将 NOTE4C 开发套件直接连接到电脑。
3.  点击 **连接设备**，选择 ZECTRIX / ESP32-S3 USB 串口设备。
4.  选择已经下载的合并 `.bin` 固件文件。
5.  选择是否保留设备设置。推荐选项会备份设置分区，并在安装后恢复；需要全新配置时再取消该选项。
6.  核对固件文件名，并确认写入地址为 `0x0000`。
7.  点击 **安装固件**，在安装完成前保持数据线连接。
8.  允许更新工具重启设备。如有需要，可在安装结果出现后使用 **再次重启**。

如果设备无法连接，请按上文的 BOOT / RESET 顺序进入下载模式，然后再次点击 **连接设备**。

### <a href="#备选esptool" class="toc-anchor">¶</a> 备选：esptool

安装或更新 `esptool`：

```
python3 -m pip install --upgrade esptool
```

在固件文件所在目录中执行：

```
python3 -m esptool \
  --chip esp32s3 \
  --port /dev/cu.usbmodemXXXX \
  --baud 460800 \
  write-flash 0x0 notellm-2bp-v6.5.9-2bp-3fbbabb-20260705-mergebin.bin
```

请将串口名称替换为电脑实际识别到的设备端口。Windows 使用对应的 `COM` 端口，并在需要时将 `python3` 改为 `python`。合并镜像必须写入 `0x0`。

## <a href="#使用预编译参考固件" class="toc-anchor">¶</a> 使用预编译参考固件

本节行为适用于上文列出的可直接刷写镜像。自行编译的固件或仓库后续版本可能会有不同表现。

### <a href="#按键操作" class="toc-anchor">¶</a> 按键操作

- 短按上键：上一项或上一页。
- 短按下键：下一项或下一页。
- 短按 BOOT：确认。
- 双击上键：打开快速页面切换。
- 长按下键约 1 秒：打开“设置”。
- 同时长按上键和下键约 1 秒：进入 Wi-Fi 配网。
- 在“相册”页长按 BOOT：进入热点传图模式。
- 在热点传图页长按 BOOT：退出传图模式。
- 深度休眠后按 BOOT：唤醒设备。

每次四色屏刷新完成后再按下一个按键。

### <a href="#首次配置-wi-fi" class="toc-anchor">¶</a> 首次配置 Wi-Fi

设备没有保存过网络时，固件通常会自动进入配网模式。如果没有自动进入：

1.  同时长按上键和下键约 1 秒。
2.  等待屏幕显示 Wi-Fi 配网。
3.  用手机连接名称以 `ZecTrix` 开头的开放热点。
4.  如果配网页面没有自动打开，请访问 `http://192.168.4.1`。
5.  选择 2.4 GHz Wi-Fi，输入密码并保存。

ESP32-S3 无法连接仅提供 5 GHz Wi-Fi 的网络。如果配网成功后“相册”页仍显示旧的连接状态，请短按一次 RESET，或先切换到其他页面再返回。

### <a href="#使用手机上传四色图片" class="toc-anchor">¶</a> 使用手机上传四色图片

1.  使用上键和下键进入“相册”。
2.  长按 BOOT，进入热点传图模式。
3.  手机连接：

```
热点：InkScreen-AP
密码：12345678
```

4.  在手机浏览器中打开 `http://192.168.4.1`。
5.  选择 **2 BP 四色** 转换模式。
6.  选择 JPG、PNG 或其他受支持的图片，并检查 400 × 300 预览。
7.  发送图片；如需立即显示，再选择“展示”。

浏览器会缩放图片并将其转换为黑、白、红、黄四色，无需另行转换为 BIN。

请勿混淆两个热点：

- `ZecTrix...`：首次配置家庭 Wi-Fi；默认无密码。
- `InkScreen-AP`：“相册”热点传图；密码为 `12345678`。

### <a href="#通过局域网上传图片" class="toc-anchor">¶</a> 通过局域网上传图片

NOTE4C 连接家庭网络后：

1.  在设备上打开“设置”。
2.  开启局域网服务。
3.  记下屏幕上显示的 IP 地址。
4.  在同一网络中的手机或电脑上打开 `http://设备IP/`，将“设备IP”替换为屏幕显示的地址。

局域网页面支持上传图片、预览四色转换、显示和管理图片，以及设置轮播。DHCP 地址可能发生变化，请以设备当前显示的地址为准。

## <a href="#软件开发" class="toc-anchor">¶</a> 软件开发

### <a href="#开源项目" class="toc-anchor">¶</a> 开源项目

公开仓库 <a href="https://github.com/LazyYoun/youn-ink-fourcolor-firmware" class="is-external-link">LazyYoun/youn-ink-fourcolor-firmware</a> 使用 `2bp` 作为默认分支，采用 MIT 许可证。仓库包含 ESP-IDF 固件，以及可选的服务器和管理组件。

```
git clone https://github.com/LazyYoun/youn-ink-fourcolor-firmware.git
cd youn-ink-fourcolor-firmware/firmware
```

当前仓库文档使用 ESP-IDF 6.0 环境。启用 ESP-IDF 环境后编译固件：

```
idf.py set-target esp32s3
idf.py menuconfig
idf.py build
idf.py -p /dev/cu.usbmodemXXXX flash monitor
```

请将 `/dev/cu.usbmodemXXXX` 替换为电脑实际识别到的串口。Windows 使用对应的 `COM` 端口。按 `Ctrl + ]` 退出串口监视器。

### <a href="#选择-note4c-面板" class="toc-anchor">¶</a> 选择 NOTE4C 面板

四色面板是仓库默认的 ZECTRIX 面板配置。在 `idf.py menuconfig` 中确认：

```
ZecTrix EPD panel type
└── 4-color BWRY SSD2683 EPD
```

对应配置：

```
CONFIG_ZECTRIX_EPD_PANEL_4COLOR_SSD2683=y
```

为 NOTE4C 编译时，请勿选择 1 bpp 黑白面板。

### <a href="#仓库范围" class="toc-anchor">¶</a> 仓库范围

该仓库是持续演进的参考项目，不是消费版 NOTE4 固件。目前包含固件 UI 和显示代码、AP 热点传图、可选 Python 服务器组件，以及图片 / 设备管理功能。启用外部服务、API、OTA 地址或自定义分区前，请先阅读仓库当前的 README 和源码。不要提交 `.env` 文件、API Key、Wi-Fi 凭据、编译产物、日志或本地数据库。

## <a href="#官方器件资料" class="toc-anchor">¶</a> 官方器件资料

- <a href="https://www.espressif.com/sites/default/files/documentation/esp32-s3_datasheet_en.pdf" class="is-external-link">ESP32-S3 数据手册 - Espressif</a>
- <a href="https://www.nxp.com/docs/en/data-sheet/PCF8563.pdf" class="is-external-link">PCF8563 RTC 数据手册 - NXP</a>
- <a href="https://www.everest-semi.com/pdf/ES8311%20PB.pdf" class="is-external-link">ES8311 产品简介 - Everest Semiconductor</a>
- <a href="https://components.espressif.com/components/espressif/es8311/versions/1.0.0?language=en" class="is-external-link">ES8311 ESP-IDF 组件 - Espressif Component Registry</a>

NOTE4C 使用 SSD2683 显示控制器。目前本文未提供由厂商官网公开托管的 SSD2683 数据手册，请以 NOTE4C 原理图映射、面板配置和公开驱动源码作为开发参考。

## <a href="#支持边界" class="toc-anchor">¶</a> 支持边界

NOTE4C 是开发设备。第三方固件、自定义分区表、服务器部署、外部 API 服务和硬件改装由开发者自行负责。安装前请保留经过验证的 16 MB Flash 备份，并记录准确的固件文件名、写入地址和校验值。如果怀疑硬件故障，请联系 ZECTRIX 支持，并提供设备型号、PCB 版本、固件文件名、复现步骤和现象照片或视频。

</div>

<div>

</div>

</div>
