# NOTE4C 首次刷机前的恢复与备份

本文适用于 ZECTRIX NOTE4C Devkit PCB V1.0（ESP32-S3 N16R8）。目标是在改写任何 Flash 内容前，先证明 ROM 下载模式仍可进入，并生成一份可审计的完整 16 MiB 原厂 Flash 快照。

这些步骤不会证明自定义固件已经可以安全控制显示、电源或休眠。它们只建立刷写退路。

## 不可越过的边界

- 不执行 `erase-flash` 或 mass erase。
- 不写 eFuse，不启用 Secure Boot、Flash Encryption、anti-rollback 或安全下载模式。
- 不禁用 ROM download、USB Serial/JTAG、USB OTG download 或 JTAG。
- 不自动恢复镜像；恢复始终是独立、人工复核的操作。
- 不在完成原厂备份前更改分区表或写入自定义固件。
- 备份可能包含 Wi-Fi 凭据、NVS、设备标识、校准数据和个人内容。必须保存在仓库外，不得提交、上传或分享。

## 1. 准备

需要：

- 支持数据传输的 USB Type-C 线；
- macOS 或 Linux；
- Espressif `esptool` 工具集，其中包含 `esptool` 和 `espefuse`；
- 仓库外一个已经存在且仅当前用户可访问的输出目录；
- 一根可短按左侧 RESET 针孔的细针。

检查 `esptool` 和脚本，但不连接设备：

```sh
esptool version
scripts/backup-factory-flash.sh --help
```

不要用 `espefuse version` 做离线版本检查：部分 v4/v5 CLI 会把 `version` 当作无效操作，并先尝试打开默认串口。备份脚本因此不会运行该命令；实际的 `espefuse summary` 始终带用户明确指定的 `--port`。

脚本兼容当前用连字符的 esptool 命令和旧版使用下划线的命令。它固定指定 `--chip esp32s3`，不依赖自动芯片识别来决定读取范围。

## 2. BOOT + RESET 恢复演练

NOTE4C 正面的圆形 BOOT / 确认键连接 GPIO0，低电平有效；左侧与下键等高的内凹针孔是 ESP32-S3 EN / RST。硬件操作顺序来自本仓库归档的 ZECTRIX Wiki：

1. 保持 USB 数据线连接。
2. 按住正面圆形 **BOOT / 确认** 键。
3. BOOT 保持按下，同时短按一次左侧内凹 **RESET** 针孔。
4. 松开 BOOT。
5. 观察系统出现新的 ESP32-S3 串口设备。

macOS 常见端口类似 `/dev/cu.usbmodemXXXX`，Linux 常见端口类似 `/dev/ttyACM0` 或稳定的 `/dev/serial/by-id/...`。必须记录并人工选择本次新出现的那个端口，不能让备份脚本遍历或猜测设备。

演练通过标准：

- BOOT + RESET 后出现一个明确的 ESP32-S3 串口；
- 再短按 RESET 后，设备退出 ROM 下载模式并尝试启动原厂固件；
- 重复一次仍然成功；
- 没有执行任何 Flash 写入或擦除命令。

若串口没有出现，先更换已知支持数据的 USB 线和主机端口。不要以“原厂界面还能启动”代替 ROM 下载模式演练，因为自定义固件失效时只能依赖 ROM 路径恢复。

## 3. 完整备份

先创建仓库外的私有目录。例如：

```sh
mkdir -m 700 /Volumes/PRIVATE/note4c-backups
```

再次按 BOOT + RESET 进入 ROM 下载模式，然后显式传入串口和输出目录：

```sh
scripts/backup-factory-flash.sh \
  --port /dev/cu.usbmodemXXXX \
  --output-dir /Volumes/PRIVATE/note4c-backups
```

Linux 示例只需把串口替换为实际设备，例如 `/dev/ttyACM0`。脚本拒绝：

- 省略 `--port` 或使用非 `/dev/...` 的端口；
- 不存在或不是字符设备的串口；
- 位于本仓库内部的输出目录；
- 把 `/` 作为输出目录；
- Flash 未明确报告为 16 MB；
- 读取结果不是精确的 16,777,216 字节；
- 无法计算 SHA-256。

执行期间只调用以下设备侧只读操作：

- `flash-id` / `flash_id`；
- `get-security-info` / `get_security_info`；
- `espefuse summary`，而且只请求 Secure Boot、Flash Encryption、download、JTAG 和 USB download 相关的白名单字段；
- `read-flash 0x0 0x1000000` / `read_flash 0x0 0x1000000`。

脚本不会请求 MAC、custom MAC、optional unique ID、密钥块或用户 eFuse 块。esptool 连接横幅可能主动打印 MAC，所以原始输出只进入权限为 0700 的临时目录；终端错误信息和落盘摘要会脱敏，退出时删除临时目录。

## 4. 验收备份

成功后会在输出目录内新建一个时间戳目录：

```text
note4c-factory-backup-YYYYMMDDTHHMMSSZ/
├── factory-flash-16MiB.bin
├── factory-flash-16MiB.bin.sha256
├── manifest.txt
├── flash-summary.txt
├── security-summary.txt
├── efuse-security-summary.txt
└── restore-command.txt
```

验收必须同时满足：

1. `manifest.txt` 的 `Actual bytes` 是 `16777216`；
2. 重新计算 SHA-256 与记录值一致；
3. `flash-summary.txt` 明确报告 16 MB；
4. 阅读两个安全摘要；如果任一个为 `UNAVAILABLE`，把安全状态记为未知；
5. 把备份复制到第二个受控存储位置后，再比对两份 SHA-256；
6. 确认整个目录没有进入 Git 工作区、同步盘或公开备份系统。

macOS 校验：

```sh
cd /Volumes/PRIVATE/note4c-backups/note4c-factory-backup-YYYYMMDDTHHMMSSZ
shasum -a 256 -c factory-flash-16MiB.bin.sha256
```

Linux 校验：

```sh
cd /secure/note4c-backups/note4c-factory-backup-YYYYMMDDTHHMMSSZ
sha256sum -c factory-flash-16MiB.bin.sha256
```

## 5. 安全状态如何影响恢复

`restore-command.txt` 只是一份不会自动执行的候选命令模板，其中每一行都被注释；即使用 `bash restore-command.txt` 打开也不会执行写入。不要因为有了完整镜像就立即做“恢复测试”：恢复本身会覆盖设备 Flash，是破坏性操作。模板同时记录了 esptool v5 常用的 `write-flash` 和 v4 的 `write_flash` 拼写，必须先核对本机版本，再人工复制对应的注释行。

如果 Flash Encryption、Secure Boot 或下载模式限制处于启用或未知状态，不要直接写回完整镜像。下载模式中的读出内容可能是密文，写入路径还受芯片安全配置和 esptool 行为约束；错误处理可能得到无法启动的镜像。应先保存原始摘要并针对该设备的安全配置设计独立恢复流程。

仅当安全状态已经人工确认、镜像 SHA-256 验证成功、确实需要恢复且接受覆盖当前 Flash 时，才复核模板中的三个关键参数：

- 芯片固定为 `esp32s3`；
- 整体镜像写入地址固定为 `0x0`；
- 镜像必须是这台板对应的精确 16 MiB 备份。

不得为绕过工具保护添加 `--force`，不得先执行 mass erase，也不得把另一台设备的备份混用。

## 6. 首次自定义刷写的放行条件

满足以下条件后，才能开始 `panelet-os` 固件刷写：

- BOOT + RESET 演练连续成功两次；
- 16 MiB 备份长度和 SHA-256 已验证；
- 第二份备份副本的 SHA-256 一致；
- Flash / security / eFuse 摘要可用、已人工阅读并保存；Secure Boot、Flash Encryption 与 download 限制已明确确认允许刷入当前无签名 demo；
- 知道如何再次进入 ROM 下载模式；
- 已接受 `espflash flash <ELF>` 会覆盖 bootloader、partition table 和 `ota_0` app，并擦除 `otadata`；理解自定义双 OTA 分区与原厂完整内容不是同一件事；
- 已核对当前固件的内存与启动配置：默认启用 NOTE4C N16R8 的 8 MiB Octal PSRAM 和启动 memtest，不进入自动休眠，并保留 GPIO0 和原生 USB 恢复路径；本流程不操作 eFuse。

如果任一安全摘要为 `UNAVAILABLE`，或相关保护已启用但尚无针对本设备的签名、加密和恢复方案，必须停止；不能设置下面的 `NOTE4C_SECURITY_STATE_COMPATIBLE=YES` 来绕过未知状态。

实机执行时记录 PCB 版本、供电方式、主机系统、esptool 版本、操作时间和通过/失败。记录中不要包含 MAC、USB 序列号、完整串口路径、NVS 内容或备份哈希之外的私密数据。

## 7. 当前 Panelet 固件的首次刷写流程

早期固件于 2026-09-02 在 Apple Silicon macOS 上完成 ESP32-S3 release target 编译和一块 NOTE4C 的两轮 bring-up；第二轮覆盖 I2C scan、RTC 只读、电池观测、状态页和单次 PTLW 实验，详见 [Phase 1 实测记录](history/phase1-hardware-log-2026-09-02.md)。这些单板结果不是当前 Panelet 构建的实机验收，也不能替代电气波形、长期精度、分压校准或局刷寿命验证。完成本页前六节的放行条件后，按[构建说明](../../development/building.md)重新构建并核对镜像容量；只有再次确认当前串口属于目标 NOTE4C，才执行受控刷写：

```sh
scripts/build-mvp.sh

NOTE4C_FACTORY_BACKUP_VERIFIED=YES \
NOTE4C_SECURITY_STATE_COMPATIBLE=YES \
NOTE4C_ALLOW_PARTITION_OVERWRITE=YES \
NOTE4C_PORT=/dev/cu.usbmodemXXXX \
scripts/flash-server.sh target/xtensa-esp32s3-espidf/release/panelet-os
```

runner 拒绝隐式串口，也会分别拒绝未确认备份、安全状态或分区表覆盖的请求。刷写过程会写 bootloader、partition table 和 `ota_0` app，并擦除 `otadata`，但不会调用 eFuse 命令；它不会自动为 `ota_1` 安装恢复固件。刷写完成后的安全操作顺序：

1. 先以 USB 供电启动，不按任何键，确认串口出现 `boot`、`mode` 和 `menu` 事件。正常启动可能呈现配网/连接或前台画面；不要立即运行额外显示测试。通过管理页显式进入诊断模式后再操作菜单，具体步骤见[诊断说明](diagnostics.md)。
2. 运行 `SYS`，记录 reset reason、free heap 和 minimum free heap。
3. 运行 `KEYS_LED`，逐个确认上键、下键 / 电源键、BOOT / 确认键事件，并目视确认低有效绿色 LED。
4. 复位时持续按住上键，确认进入 safe mode；松开上键后依次尝试 `EPD_QUICK`、`EPD_PARTIAL_EXP`、`STATUS_SCREEN`、`I2C_SCAN` 与 `RTC_READ`，确认都被 `Skipped`，GPIO6/21/42/46 仍保持 low。`BATTERY` 不开这些 rail，可单独观察。
5. 不按上键重新启动，确认已退出 safe mode；再通过管理页进入诊断模式，运行 `I2C_SCAN`。确认 GPIO21 始终 low；用逻辑分析仪或万用表核对 GPIO46 PA low -> GPIO42 high -> 约 3 ms settle -> 访问 -> GPIO42 low。无总线错误时日志应完整扫描 `0x08..=0x77`；NACK/`ESP_FAIL` 继续，首个 timeout 或其它错误必须记录地址并提前中止。只要求 `0x18`、`0x51`，不要求 `0x55`。
6. 运行 `RTC_READ`。确认只写 register pointer `0x02`、每次读取 7 B、约 1100 ms 后重读；记录 BCD/date 校验、VL、century bit 原值及跨分钟/日期推进。不设置 RTC，不把 century bit 映射成具体年代；VL 置位只能记 `Inconclusive`。
7. 运行 `BATTERY`。记录约 2 s 的 GPIO1/2 raw、占空和稳定状态，以及 10 组 GPIO4 ADC raw/calibrated mV；同时用万用表测 ADC pin 与电池端，核对分压比。软件给出的 2:1 仅是假设，不能当作电池百分比或关机阈值。
8. 选择 `EPD_QUICK`，长按 BOOT / 确认约 1 秒。它使用 10 MHz SPI 进行一次全屏刷新，预计可能超过 10 秒。
9. 目视记录从左到右的黑、白、黄、红竖条、四个不等大的角标和非对称斜线，用来判断颜料映射、旋转、镜像、packing 和缺行。
10. 记录 `epd_refresh_begin` 到 `epd_refresh_finish` 的总时间、driver result、cleanup 和 forced-safe 状态。只有日志与画面都符合预期，才能记为本次实机通过。
11. 在完成其它安全测试后，从默认 `SYS` 按一次上键回绕到 `STATUS_SCREEN`，长按 BOOT 约 1 秒，只允许一次普通全刷。核对 8 项、`MODE`、`- P O ? F S A` RAM snapshot 与底部操作提示；选择变化不得刷屏，复位后历史应清空。transport 完成只记 `Observed`，还需目视判读字形与方向。
12. 只有第 8–10 步全刷基线已稳定通过并准备好记录画面和阶段时序时，才选择 `EPD_PARTIAL_EXP`。长按 BOOT 约 1 秒、完全释放，再长按约 1 秒；确认按上/下键会撤销 armed，并且本次启动不能再次运行该实验。
13. 记录 `baseline_full`、`partial_experimental`、`recovery_full` 三个阶段。确认每周期仍发送 30,000 B，实验窗口是 inclusive `x=160..239, y=110..189`；观察中心黑白变化、窗口外变化、ghosting、各 PON/DRF bounded-stage 和总耗时。无论传输是否成功，都不能在未人工判读前记为支持。
14. 短按 RESET，确认 RAM session snapshot 清空且固件仍能重启；最后再做一次 BOOT + RESET，确认 ROM download 退路没有因自定义固件消失。

诊断测试仍由同步 runner 执行，开始后不能通过按键取消；独立输入任务会继续采样，但主循环不会在测试中途分派菜单事件。双长按保护只发生在测试开始前，不是运行中断机制。异常时的最终恢复入口仍是硬件 BOOT + RESET；驱动会尽力清理并断 EPD rail，但清理日志不能替代万用表或逻辑分析仪确认。

## 8. Bring-up 测试矩阵

以下保留 2026-09-02 Phase 1 的验收计划与当时证据边界，不是当前支持矩阵。表中的禁用 PSRAM、Wi-Fi 尚未实现和 heartbeat 均描述早期固件；当前默认启用 PSRAM、已有 Wi-Fi 实现，输入与诊断模式也已演进。当前行为见[设备入口](README.md)与[诊断说明](diagnostics.md)，后续实机证据见[历史索引](history/README.md)。未覆盖的电气、校准、功耗与寿命项目不能因软件更新视为通过。

| 项目 | Phase 1 覆盖 | 实机步骤 / 观察 | 通过条件 | 当时未覆盖边界 |
|---|---|---|---|---|
| ROM 恢复 | 手工流程 | BOOT + RESET 连续两次，刷写后再复测一次 | 每次都出现明确 ESP32-S3 download 设备 | 不证明完整镜像一定可恢复 |
| 原厂 Flash 备份 | 脚本与 mock 测试 | 读取 16 MiB、双份 SHA-256 比对 | 精确 16,777,216 B，摘要一致，安全状态已复核 | 不自动恢复、不判断加密镜像可写性 |
| 早期电源保持 | GPIO17 静态实现 | USB 供电先测；随后单独做电池冷启动 20 次 | 无意外掉电，日志连续，RESET/ROM 路径可用 | GPIO17 时限、硬关机极性尚未测量 |
| 输出安全态 | 启动与每项测试前重置 | 测 GPIO6/21/42/46，检查 safe mode | 未选择测试时 EPD/NFC/audio/PA 都不被使能 | 仅 GPIO 调用成功不等于 rail 电压已实测 |
| 系统信息 | `SYS` | 记录 reset reason、heap；确认 GPIO19/20 未被占用 | 日志完整且可重复 | 不覆盖 PSRAM；当时镜像明确禁用 PSRAM |
| 三键与 LED | `KEYS_LED` + host 状态机 | 上/下/BOOT 各短按和长按；目视菜单序号、运行、结果、heartbeat、safe/armed LED pattern | active-low 事件、菜单换项和 LED 脉冲一致；pattern 期间仍持续扫描按键 | Phase 1 已目视常用选择/运行/结果/heartbeat；未逐一计时所有 safe/armed/failure pattern |
| RAM 会话快照 | `SessionResults` host tests | 每项完成后核对状态码；RESET 后再开状态页 | `- P O ? F S A` 映射正确，复位后清空，不写 NVS/Flash | 不是持久化 crash counter 或历史记录 |
| safe mode | 启动按住上键 | 确认 safe 日志并尝试显示与 I2C/RTC 项 | EPD/状态页/I2C/RTC 被 `Skipped`，GPIO6/21/42/46 始终关闭 | `BATTERY` 仍可只读；没有持久化 crash counter |
| I2C scan | `I2C_SCAN` + target build | GPIO21 low；核对 PA low -> GPIO42 high -> 3 ms -> probe -> low，扫描 `0x08..=0x77` | 无总线错误时完整探测 112 地址并发现 `0x18`、`0x51`；首个 timeout/其它错误时记录并有界中止；`0x55` 非必需 | Phase 1 单板一次通过；地址 ACK 不证明器件功能，GPIO42 电气波形未测 |
| RTC 只读 | `RTC_READ` + host parser/calendar tests | `0x51` 从 `0x02` 读 7 B，两次约隔 1100 ms | BCD/日历有效、VL clear、时间合理推进；century bit 只记录 | Phase 1 秒推进一次通过，但内容未校时；不证明长期精度，VL 时为 `Inconclusive` |
| 电池/充电观测 | `BATTERY` + target build | GPIO1/2 约 2 s 统计；GPIO4 读 10 组 raw/calibrated mV；万用表对照 | API 全程成功记 `Observed`，并保存 pin/battery 实测与比例误差 | Phase 1 API/样本一次成功；2:1 未经万用表确认，不报告 SOC、不执行低电量关机 |
| framebuffer | 主机单元测试 | 检查 400 x 300、30,000 B、MSB-first 和 `0x1B` sentinel | 所有主机测试通过 | 黑白黄红映射只来自参考源码，必须目视确认 |
| SSD2683 命令/清理 | 主机 mock 测试 | 核对 DTM/PON/DRF `00`/POF `00`/DSLP `A5` 与 error cleanup | 顺序、频率上限、bounded BUSY 和 rail-off 测试通过 | mock 不证明 NOTE4C BUSY 极性或电气时序 |
| 四色全刷 | `EPD_QUICK` | 10 MHz、一次非对称测试图；记录 BUSY 和总耗时 | 四色、方向、完整画面正确；无 timeout；关屏后 GPIO6 low | full refresh 已单板实测一次，不外推到批次和寿命 |
| 状态页 | `STATUS_SCREEN` + host UI tests | 显式单次长按，只做一次 full refresh；核对标题、8 项、状态码和底部提示 | transport/cleanup 完成只记 `Observed`；人工确认 glyph、scale、bounds、方向 | Phase 1 页面与方向一次通过；不自动刷新、不用 PTLW；snapshot 复位失效 |
| 实验局刷 | `EPD_PARTIAL_EXP` + 主机 mock | 双长按放行；baseline full 后完整 30,000 B DTM + PTLW 中心窗口；约 10 s 观察后一次 full recovery | 只记录 baseline/partial/recovery、窗口内外变化、ghosting、bounded-stage/总耗时和关断；结论先记 `inconclusive` | Phase 1 单次窗口内容限制有效且无可见残影，但耗时等同全刷；仍无 waveform、温度、重复寿命或生产支持证据 |
| BUSY 极性/超时 | proposal 与参考源码均为 active-low；板上仍属 provisional，120 s ceiling | 首次用逻辑分析仪同步 GPIO8、CS、DC、SCK、RESET、EPD power | 各阶段电平与实际忙闲一致；等待有界 | proposal 不是 final datasheet，板级接线和批次尚未实测 |
| Wi-Fi、音频、NFC、sleep | 尚未实现 | 在显示和恢复路径稳定后分项加入 | 各自独立测试与功耗记录通过 | 编译或地址 probe 都不能替代功能验证 |

不要为了制造错误路径而在刷新期间拔 panel、短接 BUSY 或切断主电源。超时和 cleanup 先由主机 mock 覆盖；任何实物 fault-injection 都应另写方案并评估高压关断风险。

### 持久化小组件 MVP 附加验收清单

以下是后续 MVP 阶段增加的验收步骤，不是完成结果。实际执行前应核对设备当前存储状态；已有有效存储的设备不得为重复验收而格式化。各次完成范围见对应[实机记录](history/mvp-hardware-log-2026-09-04.md)。

1. 刷入后先读取 `system.status`，确认 storage 为 `uninitialized` 时硬件菜单与管理页仍可用，且 `.pnw` 上传返回 `storage_not_initialized`。
2. 在管理页输入精确的 `ERASE ASSETS`，仅执行一次显式初始化；记录耗时、reset reason、初始化前后 heap 和串口错误。
3. 用 `POST /packages/v1` 分别上传日历与相册 `.pnw`，确认 HTTP 202、job 成功、列表标记 persistent；再上传一个 CRC 错误包，确认旧版本不受影响。
4. 运行相册两次，记录 KV 推进；写入 private text 和 shared text/binary，设置 active widget，然后短按 RESET。
5. 重启后确认包列表、active widget、KV/private/shared 均恢复；再次运行相册应从上次状态继续。
6. 删除非 active 小组件并等待 delete job 成功，RESET 后确认包和 private namespace 不复活，shared 文件不被连带删除。
7. EPD 刷新进入约 24 秒 BUSY 后，用另一客户端重复请求 `system.status`、`job.get` 和 shared list，确认管理 HTTP 与显示 actor 没有互锁。
8. 最后重新做 BOOT + RESET ROM download 演练；初始化 assets 和持久化对象不得影响刷不死恢复路径。

该清单只覆盖常规 RESET 恢复。更新/删除中切断供电属于更高风险的掉电一致性测试，必须在已再次核对完整 16 MiB 恢复镜像后单独安排。

## 9. 实机记录模板

每次测试复制下面的字段到仓库外或不含敏感信息的实验记录。`Pass` 只能表示本次明确列出的步骤和硬件条件通过，不能概括整台设备。

```text
NOTE4C bring-up record
Date/time and timezone:
Operator:
PCB revision / visible hardware revision:
Panel/FPC marking (if inspected):
Firmware commit and dirty status:
Build profile / ESP-IDF / rustc / espflash / esptool versions:
Host OS:
Power source: USB / battery / both
Factory backup gate: size verified / SHA-256 verified / second copy verified
Security summaries reviewed: yes / unknown (do not paste identifiers)
Test item and exact steps:
Expected result:
Observed serial events (redacted):
Session status code before/after test; reset cleared snapshot: yes / no
LED pattern observed: selection / running / result / heartbeat / safe / armed
I2C found addresses; NACK/ESP_FAIL / timeout / other counts:
GPIO46 PA / GPIO42 rail / GPIO21 NFC sequence and measured levels:
RTC raw bytes / VL / century bit / elapsed / progress:
GPIO1 STDBY_H and GPIO2 CHRG_L counts/duty/stability:
ADC raw samples / calibrated pin mV / measured divider ratio / battery mV:
BUSY_N idle level and stage timing: reset / DTM / PON / DRF / POF
EPD total elapsed_ms:
Experimental PTLW window / packed_len / full DTM bytes:
Experimental phases: baseline_full / partial_experimental / recovery_full
Window-inside change / window-outside change / ghosting:
Measured rail voltage/current and instrument:
Visual result: colors / orientation / missing lines / ghosting
Cleanup: POF / DSLP / RESET low / GPIO6 low
Post-test RESET boot: pass / fail
Post-test BOOT+RESET ROM entry: pass / fail
Result: pass / fail / inconclusive
Artifacts: redacted log, scope capture, photo identifiers
Notes and next action:
```

串口路径只记录“人工选择并匹配目标板”，不要保存完整设备路径或 USB serial。图片若包含二维码、账号、通知或原厂个人内容，应先脱敏。

## 参考

- [Espressif esptool：ESP32-S3 基础选项](https://docs.espressif.com/projects/esptool/en/latest/esp32s3/esptool/basic-options.html)
- [Espressif esptool：读取 Flash 等基础命令](https://docs.espressif.com/projects/esptool/en/latest/esp32s3/esptool/basic-commands.html)
- [Espressif espefuse：`summary` 与按字段筛选](https://docs.espressif.com/projects/esptool/en/latest/esp32s3/espefuse/summary-cmd.html)
- [NOTE4C Quick Start 官方原址](https://wiki.zectrix.com/zh/hardware/note4c/quick-start)；历史快照信息见[来源清单](../../../3rd-res/SOURCES.md)。
