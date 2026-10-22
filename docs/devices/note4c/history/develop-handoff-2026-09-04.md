# NOTE4C develop 分支交接（2026-09-04）

本文供另一台开发机或新会话继续 NOTE4C 小组件 MVP 上板验证。禁止把设备
SSID、Wi-Fi 密码、管理 token、MAC、实际私网地址或 Flash 备份写入仓库、终端
转储和测试制品。

## 当前板上状态

- 板卡仍可通过 BOOT + RESET 进入 ESP32-S3 ROM download；完整 16 MiB 原厂
  Flash 已由用户在仓库外备份并核对另一份副本。
- bootloader 与 16 MiB 双 OTA 分区表已经写入；当前从 `ota_0 @ 0x20000`
  启动，`ota_1` 仍不能当作已经验证的回退镜像。
- 当前实际刷入的是默认启用 server 与 PSRAM 的 `note4c-server` 固件。2026-09-04
  续测只写了 `ota_0 @ 0x20000`，未重写 bootloader、partition table、NVS、
  assets 或 otadata。保存的 Wi-Fi 配置仍在 NVS；启动时先建立 SoftAP/HTTP，
  成功连接已保存网络后切换为纯 STA。
- 当前精确镜像复位后已用保存的 active 配置连接 WPA1 AP、取得 IPv4，并从
  AP+STA 切换为纯 STA；这验证了本次保存配置的复位回连，但不改变 WPA1/TKIP 仅属
  开发兼容路径的安全边界。
- `assets` 的 8 MiB SPIFFS 已由用户明确授权格式化，随后状态接口确认
  `ready=true`、`persistent=true`。格式化超过了客户端 30 秒 timeout，但操作
  最终成功；初始化接口应改成异步 job 或提供更长且明确的进度反馈。
- 日历 `.n4w` 已更新为全屏大字月历：顶栏显示公历、星期、干支年和农历月日，日格
  显示单行农历，仅当天黄色、周末红字。包已重新编译、持久化、运行并 activate；
  状态接口确认版本 `0.2.0`、active widget 与 persistent storage。最终字形和颜色仍需
  用户直接目视确认，active widget 的 RESET 恢复也尚未复测。
- 系统时间默认使用 `pool.ntp.org` 和 UTC+08:00，管理页可修改 NTP server 与固定 UTC
  分钟偏移。SNTP 设置系统 UTC 时钟后，network worker 通过唯一主硬件所有者把 UTC
  写入 PCF8563 并读回校验；写入成功后才保存 RTC ownership marker。实机已验证 NTP
  同步、RTC 回写，以及下一次 RESET 在联网前从 owned RTC 恢复系统时钟。

## 本轮发现并修复的问题

1. SPIFFS VFS 不实现 `mkdir`，挂载点却可能被 `Path::exists` 判为不存在。
   `WidgetStore::initialize_mounted` 现在会直接在已挂载根写 marker，不再创建
   目录；对应主机测试通过。
2. SSID 后缀与固定 AP 密码改由工厂 eFuse MAC 派生；AP 密码为
   `XXXX-XXXX-XXXX`。admin bootstrap 使用独立域标签派生为 8 位，屏幕只显示独立
   ADMIN QR，不显示 token 明文。管理页可把自定义 token 写入 `admin_custom` NVS；
   一旦设置，重启不再使用 bootstrap。
3. `scripts/flash-server.sh` 现在显式传入 16 MiB Flash、bootloader、分区表、
   `ota_0` 和 `otadata` 擦除范围，避免 `--erase-parts` 在没有分区表时拒绝运行。
4. 曾有一次人工执行普通 `cargo build` 时漏掉 `management-server` feature，导致
   临时刷入的 472 KiB 纯硬件测试固件既不开 Wi-Fi 也不开 AP。随后只覆盖
   `ota_0` 修复，NVS、assets、bootloader 和 partition table 均未重写。当前源码
   已迁移为 `note4c-server` 且彻底移除该 feature 门控，普通构建不会再裁掉 server。
5. 旧固件把所有串口配网失败压成 `network_rejected`。当前固件订阅 typed STA 事件，
   只复制数值 reason/RSSI 并映射到固定错误码；切换 candidate 前显式断开旧连接，成功
   必须同时看到本次新关联和有效 IPv4，timeout 会取消底层连接。带密码网络启用可选
   PMF。脚本只显示白名单提示，不回显设备任意字段、SSID 或密码。
6. 实机串口随后返回 `wifi_security_incompatible`，用户确认目标 AP 因另一个 IoT 设备
   的 WPA2 握手缺陷被强制为 WPA1。根因是本站固件原先把 scan authmode threshold 设为
   WPA2；现已降为 WPA-Personal、完成 app-only 刷写，仍接受更强的 WPA2/WPA3
   Personal。WPA1/TKIP 是弱安全兼容配置，不应作为量产默认。失败 candidate 未
   promote。用户后续重新配置后，当前 NVS active 已在复位启动中完成 WPA1 握手、
   DHCP 和纯 STA 切换。
7. PSRAM 版首次重传日历时，`mvp_context()` 已成功，但 256 KiB Rune compile
   allocator limit 明确耗尽。主机同上下文测试确认日历在 512 KiB 可编译，而相册在
   512 KiB 仍会耗尽；1 MiB 下两个示例均通过，因此只把编译期限额提高为 1 MiB，
   运行期 64 KiB 限额不变。刷入后日历 upload job 已实机成功。
8. 加入本地时间 ABI 后，启动时为每个持久化包重复构建 Rune compiler context 曾触发
   内部 heap allocator 断言并重启。包上传提交前已经编译，Flash 写入也会回读并重新
   decode/CRC 校验；启动恢复因此只重建 broker registry，真正运行仍走同一个 1 MiB
   有界编译路径并返回跨版本错误。修复后重复启动稳定，新日历重新上传和运行均成功。
9. 首次 NTP 回调恰逢联网成功页约 24.6 秒全刷，2 秒 RTC queue wait 先超时；这不是
   RTC/I2C transaction failure。协调等待已改为有界 45 秒，I2C 每笔仍为 100 ms。
   随后实机连续两次 NTP→RTC 写入/读回通过，第二次启动也先从 RTC 恢复成功。

## PSRAM 与 Rune 编译验证

未启用 PSRAM 的历史固件上传日历时只有约 130 KiB free heap，失败精确发生在
`mvp_context()`。工作树因此加入了：

- 80 MHz Octal、8 MiB PSRAM 配置；来源依据是 NOTE4C N16R8 官方容量与归档参考
  固件配置，当前单板已有一次启动探测和 memtest 通过记录，仍需重复与压力测试；
- `CONFIG_SPIRAM_USE_MALLOC=y`、`MALLOC_ALWAYSINTERNAL=512`，使较大普通分配优先
  使用 PSRAM；
- 64 KiB internal reserve，保护 FreeRTOS 栈和 DMA/internal-only 分配；
- 启动 PSRAM memtest，以及 Rune Context 成功/失败时的 internal/PSRAM heap
  诊断。

当前默认 `note4c-server` 已在 Apple Silicon macOS 上完成 release target 构建并
app-only 刷入。加入农历 ABI、CJK 字形和新版日历后的 app image 为
3,904,096 / 4,128,768 B（约 94.56%，余 224,672 B）。单板启动识别到 8 MiB
Octal PSRAM、80 MHz，
启动 memtest 通过；
这仍只是资格测试，不应在取得长期压力与断电测试前写成“PSRAM 已完整验证”。Rune
`Context`/`RuntimeContext` 后续可以考虑复用。当前实机上传时 Context 已成功：一次
记录的 internal free heap 为 132,315 -> 64,779 B，PSRAM free heap 为
8,357,692 -> 7,524,840 B。256 KiB 编译边界随后成为独立失败点；提高到经主机两个
示例验证的 1 MiB 后，日历已完成编译、SPIFFS 持久化、RESET 列表恢复和一次运行；
仍未测量精确编译峰值。

配网页现在分别显示标准 Wi-Fi QR 与 ADMIN QR；STA 成功页分别显示不含凭据的管理
URL QR 与 ADMIN QR。2026-09-04 已刷入并完成一次双二维码配网页全刷 transport/关电
验证；较早镜像的 `pon_drf_refresh` 为 19.815 秒，driver 总耗时 20.092 秒，最新
1 MiB 编译预算镜像的成功页对应值为 24.314 秒和 24.585 秒，均以 `PoweredOff`
结束。二维码是否可扫、成功页布局和改密后重启仍需人工端到端确认。

`scripts/provision-wifi.py` 已加入标准库实现，要求显式 `--port`、`--ssid` 和
`--password`。设备端 USB Serial/JTAG 握手已实机返回 `ready`；当前保存的目标网络
配置已在复位后完成连接和 DHCP。固件对“无 USB host”状态和瞬时 VFS 读错
采用休眠重试，不再永久退出 reader；无 host 启动后再插电脑仍待实机验证。运行脚本前
退出 serial monitor。设备侧 40 秒仍未终结的 job 返回 `pending`，脚本会警告它仍可能
随后修改网络，而不是误报 `failed`。此前用合成的不存在 SSID 跑通
accepted→`wifi_ap_not_found`，确认失败 candidate 不会 promote；这项历史失败测试
不能从旧版 `network_rejected` 反推原因。

本轮还定位到 `esp-idf-svc 0.52.1` 遇到 ESP-IDF 5.5.3
`WIFI_EVENT_AP_WRONG_PASSWORD` 会 panic。Cargo 已固定到上游
`d15fb4e7abc98d5e68bd83b6569105d91a90a5ca`，包含新事件映射及未知事件降级告警；
错误密码不应再导致整机 abort，仍需一次实机错误密码回归。

## 下一会话建议顺序

1. 检查分支和工具链，运行主机测试与 `scripts/build-mvp.sh`。确认 app image 小于
   单个 `0x3f0000` OTA slot。
2. 连接设备后只读检查芯片安全状态和串口路径。不要重新格式化已 ready 的
   assets，也不要擦除 NVS。
3. 已完成非 merge app-only 写入；后续刷写仍只写 `ota_0 @ 0x20000`，不要重复写
   bootloader 或 partition table。
4. 继续记录重复启动与 Rune 编译时的 internal/PSRAM free、largest block 和峰值；
   若启动失败，直接用 BOOT + RESET 回到 ROM download，并恢复上一版 app。
5. 新版日历上传、运行与 activate 已通过；下一步人工核对顶栏、农历、当天黄色和
   周末红字，再 RESET 验证 active widget 恢复，并在 EPD BUSY 期间请求
   `system.status`。
6. RESET 后继续检查 active widget 和 KV 恢复；再测试 private/shared、删除语义、
   错误 token/origin、CRC 损坏包和受控断电恢复。

## 包存储方向

`.n4w` 应作为不可变、STORE-only ZIP 原样持久化，不要上传后整包展开。这样上传
提交只需原子替换一个文件，并避免 SPIFFS flat namespace 上的多文件事务和 inode
开销。运行层最终应抽象 `read_at(offset, length)`，依据 ZIP 中央目录随机读取条目；
当前 SPIFFS 可用 seek/read，未来裸机可由 LittleFS 或 Flash object store 实现。

组件运行产生的 private/KV 与用户共享的大媒体独立存储；ZIP 索引、Rune 编译产物
和图片转码结果只能作为可重建缓存。当前 MVP 虽原样保存 ZIP，但启动和执行仍会把
最多 64 KiB 的包读入 RAM；file-backed reader 是后续内存优化。若以后支持 gzip，
应只把它当传输外壳，设备解压后仍以 `.n4w` 为权威格式，否则会失去随机读取。

## 常用验证命令

```sh
cargo +stable test --workspace \
  --exclude note4c-server \
  --all-features \
  --target "$(rustc +stable -vV | sed -n 's/^host: //p')"
scripts/tests/test-backup-factory-flash.sh
scripts/tests/test-flash-server.sh
scripts/build-mvp.sh
```

上板 HTTP 命令必须在本地临时输入 token，不要把真实值加入 shell script、文档、
history 或日志。详细接口、恢复门槛和验收矩阵见
[`management-server-mvp.md`](../../../os/management-server-mvp.md)、
[`widget-runtime-mvp.md`](../../../os/widget-runtime-mvp.md) 与
[`bringup-and-recovery.md`](../bringup-and-recovery.md)。
