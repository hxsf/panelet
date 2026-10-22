# NOTE4C 16 MiB 固件分区与恢复策略

日期：2026-09-03。本文只描述并验证本地构建产物；本次没有连接、读取或刷写设备，也没有访问原厂备份。

## 当前决定

暂不放弃 A/B OTA。默认使用 [`partitions-note4c-16m-ota.csv`](../../../app/os/partitions-note4c-16m-ota.csv)：

| 名称 | 范围 | 容量 | 用途 |
| --- | --- | ---: | --- |
| `nvs` | `0x009000..0x00efff` | 24 KiB | 系统设置；保持当前 Rust runner 的 offset 与 size |
| `phy_init` | `0x00f000..0x00ffff` | 4 KiB | PHY 数据 |
| `otadata` | `0x010000..0x011fff` | 8 KiB | 两份冗余 OTA 选择状态 |
| `ota_0` | `0x020000..0x40ffff` | 4032 KiB | 应用槽 A |
| `ota_1` | `0x410000..0x7fffff` | 4032 KiB | 应用槽 B |
| `assets` | `0x800000..0xffffff` | 8 MiB | SPIFFS：小组件包、active widget、private KV/files 与 shared blobs |

app 槽与 `assets` 的 offset/size 与归档参考固件 V2 16 MiB 表一致。唯一有意差异是：参考表把 16 KiB NVS 放在 `0x9000`，并把 `otadata` 放在 `0xd000`；本表保留本仓库原有 24 KiB NVS，把 `otadata` 移到旧 app 区内的 `0x10000`。ESP-IDF 构建产物已确认识别此布局，首个 app 写入地址为 `0x20000`。

`assets` 使用 SPIFFS，但固件以 `format_if_mount_failed=false` 探测，且只有存在 `NOTE4C-WIDGET-STORE` v1 marker 才启用。首次使用必须在管理页输入精确确认后显式格式化整个分区，不能把参考固件遗留内容当成可信文件。SPIFFS 没有真实目录且官方说明可靠使用空间约为分区的 75%，因此物理文件名保持在 21 B 以内，逻辑路径保存在有界索引中；MVP 的 shared quota 仅 1 MiB，远离满盘 GC 区域。

对外小组件包使用 STORE-only `.pnw` ZIP，旧 `.n4w` 包仍接受；SPIFFS 内部包与临时文件继续使用 `p<hash>.n4w`、`t<hash>.n4w`，不会为后缀更名迁移已有数据。包内资源、组件 private 数据和 shared 数据属于三个独立逻辑根。包替换使用临时文件、`fsync`、回读 ZIP/CRC 和恢复时 promotion；删除使用 tombstone。主机测试已覆盖 reopen 恢复，但 SPIFFS 掉电一致性仍必须实机验证。

## Rune 容量验证

固定 `Rune 0.14.2`、`xtensa-esp32s3-espidf`、ESP-IDF `v5.5.3`、release `opt-level=s`，启用 `rune-runtime-only-footprint-poc` 后：

| 布局 | app image | app 槽 | 使用率 | 余量 |
| --- | ---: | ---: | ---: | ---: |
| 双 OTA（默认） | 2,162,816 B | 4,128,768 B | 52.38% | 1,965,952 B |
| 单 app（备选） | 2,162,816 B | 8,323,072 B | 25.99% | 6,160,256 B |

双 OTA 槽仍有约 1.88 MiB，可继续评估 Wi-Fi、HTTP 服务、NFC 和 ABI。达到 80% 槽占用（约 3.15 MiB）时应重新做依赖/section 预算；在证明实际超过 4032 KiB 之前，不值得先牺牲固件回退能力。

上述表是 runtime-only footprint 实验，不是完整管理固件。2026-09-04 最初的完整 Wi-Fi/HTTP、Rune 编译器与 VM、Canvas、STORE ZIP、SPIFFS persistence 和 FS ABI release image 为 3,797,216 / 4,128,768 B（91.97%）。加入 PSRAM 配置与 Rune heap 诊断后的最新构建为 3,810,816 B（约 92.30%），仅余 317,952 B；该版本尚未刷机验证 PSRAM。它仍能放入双 OTA 槽，但已经越过 80% 复核线；新增图片解码、BLE、NFC 或音频前必须重新做 section/heap/stack 预算，不能把该余量视为功能预算。

`sdkconfig.defaults` 同时把 Flash header 配置从默认 2 MiB 修正为 16 MiB，并启用 `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE`。这个选项只为未来 OTA 提供机制：当前没有 HTTP OTA 实现；以后首次启动处于 `PENDING_VERIFY` 的 OTA image 时，应用必须先做有界自检，再调用确认 API，否则下次启动会回退。串口首次写入、`otadata` 为空时，因没有 factory partition，bootloader 选择 `ota_0`。

本地 `espflash save-image` 生成了两个不可提交的 merge image。默认双 OTA image 是 2,293,888 B；其中 `0x10000..0x11fff` 与 ESP-IDF 的 8 KiB `ota_data_initial.bin` 完全同 SHA-256（全 `0xff`）。完整命令、摘要和分区解析保存在 [`partition-layout-16m-2026-09-03.txt`](../../../experiments/rune-footprint-poc/artifacts/partition-layout-16m-2026-09-03.txt)。

## 为什么不推荐单 app

备选 [`partitions-note4c-16m-single.csv`](../../../app/os/partitions-note4c-16m-single.csv) 保留 24 KiB NVS、给 factory app 8128 KiB，并保留 8 MiB assets。它能容纳更大的 Rust/Rune 内核，但代价不是只有“失去方便的 OTA”：

- 运行中的 app 不能用 ESP-IDF OTA API 原地覆盖自身；远程更新内核没有独立 staging/验证槽。
- 下载、校验或写入中断后，没有上一版 app 可启动；常规恢复退化为物理 BOOT+RESET 和串口整机恢复。
- ESP-IDF 的 app rollback 只适用于 OTA partition，factory app 不能使用这套回滚。
- 小组件包仍能在 `assets` 内自行设计事务/回滚，但无法挽救内核本身的启动失败。

因此单 app CSV 只作为尺寸超过 4032 KiB 后的实验性逃生方案，不是默认构建配置。

## 首次迁移的真实风险

从此前 `factory @ 0x10000 / 1 MiB` 表切换到默认双 OTA 时：

- `0x0000` bootloader、`0x8000` partition table 和从 `0x20000` 开始的 app image 会被写入；受控 runner 还会通过 `--erase-parts otadata` 把 `0x10000..0x11fff` 恢复为初始擦除态，旧 factory app 必然不再可启动。
- `nvs @ 0x9000 / 24 KiB` 和 `phy_init @ 0xf000 / 4 KiB` 的物理范围不变，但这不保证原厂 NVS schema 与新程序兼容；新程序不得猜测或改写未知 namespace。
- 受控 ELF 刷写不会自动把已知恢复固件写满 `ota_1`，也不会自动格式化整个 `assets`。这些范围中的旧字节可能原样残留，却会被新分区表重新解释。runner 每次串口刷写都会清空 OTA 选择状态并回到 `ota_0`，不能用它模拟正常的网络 A/B 升级。
- 因此第一次刷入后还没有真正的 A/B 回退，直到 `ota_1` 被写入并验证为已知可启动 image。挂载 `assets` 前也必须校验文件系统/容器 magic 和版本，失败时显式初始化，不能把残留内容当成可信包。
- 分区表、bootloader 和一般 data partition 的在线更新不是安全 app OTA；网络管理服务不应暴露这些写入操作。

双 OTA 只降低应用更新失败风险，不取代 ROM download。它也不自动提供真实性：HTTP OTA 上线前仍需定义 TLS 信任、固件签名、版本策略、失败计数、启动自检和回滚确认。

## 恢复前置条件

发生迁移问题时仍使用这台设备自己的完整 16 MiB 原厂镜像恢复，而不是仅恢复旧 1 MiB app。执行任何写回命令前必须同时满足：

1. 对仓库外的备份重新做 SHA-256 校验，并确认精确长度为 16,777,216 B；
2. 确认镜像属于当前设备，第二份 NAS/离线副本摘要一致；
3. 重新确认 Secure Boot、Flash Encryption 和 ROM download 限制允许按原路径恢复；
4. 明确选择目标串口、进入 ROM download，不使用自动端口选择，不添加 `--force`，不先 mass erase；
5. 按备份目录中生成的 `restore-command.txt`，核对本机 esptool 主版本后人工执行从 `0x0` 写入完整镜像。

恢复会覆盖当前 bootloader、分区表、两个 OTA 槽、assets 和 NVS；这正是恢复原厂 16 MiB 状态的含义。完整安全流程见[首次刷机前的恢复与备份](bringup-and-recovery.md)。

## 官方行为依据

ESP-IDF v5.5.3 规定安全 app OTA 使用两个 OTA app 槽和一个 8 KiB `otadata`，更新写入非运行槽，验证后再切换启动；开启 rollback 后，新 image 若未完成自检确认会回退。没有 factory partition且 `otadata` 为空时，bootloader 会选择第一个 OTA 槽。另据分区表规范，app offset 必须 64 KiB 对齐，Flash sector/data partition 以 4 KiB 对齐。本仓库两张 CSV 均已由同一 ESP-IDF 的 `gen_esp32part.py` 验证。

- [ESP-IDF v5.5.3 OTA](https://docs.espressif.com/projects/esp-idf/en/v5.5.3/esp32s3/api-reference/system/ota.html)
- [ESP-IDF v5.5.3 分区表](https://docs.espressif.com/projects/esp-idf/en/v5.5.3/esp32s3/api-guides/partition-tables.html)
- [ESP-IDF SPIFFS 限制与 API](https://docs.espressif.com/projects/esp-idf/en/stable/esp32s3/api-reference/storage/spiffs.html)
