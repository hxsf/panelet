# NOTE4C 持久化小组件 MVP 实机记录（2026-09-04）

## 环境

- 板卡：NOTE4C PCB V1.0
- 电源与调试：USB
- 固件：本工作区 `management-server` release，未提交工作树
- 分区：16 MiB 双 OTA；本轮首次写入 bootloader、partition table、otadata 和 `ota_0`，后续修复只写 `ota_0 @ 0x20000`
- 敏感信息：Wi-Fi SSID、密码、admin token、设备标识和实际私网地址均不写入仓库

## 已观察结果

1. ESP32-S3 rev 0.2、16 MiB Flash；Secure Boot 和 Flash Encryption 均未启用。
2. 双 OTA 分区表正常加载，应用从 `ota_0 @ 0x20000` 启动。
3. 未初始化的原 `assets` 内容无法作为 SPIFFS 挂载；固件没有静默格式化，管理服务保持可用。
4. WPA2 SoftAP、DHCP server 和 HTTP server 启动成功；客户端获得 `192.168.4.x` 地址。
5. 第一版配网页面因随机凭据包含小写十六进制、超出大写 5x7 字体而渲染失败；凭据统一为大写十六进制后，页面全刷成功。该次刷新 driver 总耗时约 20.08 秒并正常关电。
6. 管理页提交 candidate 后，设备成功加入目标 Wi-Fi、获得私网 IPv4，并切换为纯 STA；HTTP 页面随后仍有访问记录。
7. 手动触发一次状态页全刷成功，driver 总耗时约 20.03 秒并正常关电。
8. `assets` 第一次显式格式化已执行，但初始化 marker 写入前因 SPIFFS 不支持
   `mkdir` 返回 `ENOTSUP`；改用已挂载根初始化后再次显式执行，客户端等待超过
   30 秒而超时，但后续状态确认 `ready=true`、`persistent=true`。
9. 曾误刷未启用 `management-server` 的纯硬件测试 app，表现为 RESET 后既不连接
   Wi-Fi 也不开 SoftAP；随后只重写 `ota_0` 为正确构建，网络与管理服务恢复，
   NVS、assets、bootloader 和 partition table 未改写。
10. 日历 `.n4w` 二进制上传返回 202，但后台编译在 `mvp_context()` 失败为
    `rune_context_failed`，发生于包持久化之前。失败时 free heap 约 130 KiB；
    这是未启用 PSRAM 的历史固件结果，不能据此断定是包格式或脚本语法错误。

## 随后实现的配网 UX

1. 首次配网画面生成标准 Wi-Fi QR code，方便手机直接加入 SoftAP。
2. AP 密码使用稳定派生的 `XXXX-XXXX-XXXX`；8 位 admin bootstrap 以不同用途标签
   独立派生。字母表排除 `D/O/0`、`1/I/L`、`5/S`、`8/B`。
3. STA 配网成功并取得地址后，自动提交新页面，显示当前地址，并把不含凭据的管理
   URL QR 与独立 ADMIN QR 分开。
4. 管理页允许用当前 token 设置 NVS 自定义值；响应、日志和 job/event 都不返回
   token。
5. 这是可信本地网络的开发调试折中，不是量产认证方案；失败限速和正式凭据生命周期
   仍需另行设计。

## 尚待验证

- PSRAM 重复启动、压力测试、internal/PSRAM heap 峰值与分配策略；
- 运行已通过二进制 `/packages/v1` 编译、持久化的 `.n4w`；
- RESET 后验证 package、active widget、KV、private/shared 恢复；
- 删除后验证 package/private/KV 不复活而 shared 保留；
- 最大包、并发 busy、错误 token/origin 和受控断电恢复。

## Server/PSRAM app-only 刷写（同日续测）

- 板卡：NOTE4C PCB V1.0；USB 供电与调试；未测量电流或电压。
- 固件：`52a8168-dirty` 工作树的默认 `note4c-server` release；非 merge app
  image 为 3,814,224 / 4,128,768 B（92.38%）。本次只写
  `ota_0 @ 0x20000`，没有重写 bootloader、partition table、NVS、assets 或
  otadata。
- 写入前只读确认 ESP32-S3 rev 0.2、16 MiB Flash，Secure Boot 与 Flash
  Encryption 均未启用，与此前记录一致。
- 启动识别到 64 Mbit（8 MiB）Octal PSRAM，报告 80 MHz；启动 PSRAM memory
  test 为 `OK`，并将 8192 KiB 加入 heap。64 KiB internal reserve 已启用。
  这是本板本次冷启动的运行证据，不代表长期稳定性或所有分配策略已经验证。
- `assets` 恢复为 `Ready`；Wi-Fi driver、SoftAP DHCP 和 HTTP handlers 均成功
  启动。日志未记录 SSID、地址或 credential。
- 保存网络连接后，server 提交了一次主界面全刷。`pon_drf_refresh` 约
  24.313 秒，driver 总耗时约 24.577 秒，结束状态为 `PoweredOff`，安全清理
  成功。由于本记录没有独立目视确认，只把 transport/关电路径记为通过；页面
  内容与字符布局仍待人工确认。
- 本轮没有重新上传或运行日历 `.n4w`，因此 Rune Context、编译峰值和包持久化
  仍未验证。

## 配网二维码 app-only 刷写（同日续测）

- 固件：同一未提交工作树的默认 `note4c-server` release；非 merge app image 为
  3,826,784 / 4,128,768 B（92.69%，余 301,984 B）。只写
  `ota_0 @ 0x20000`，保留 bootloader、partition table、NVS、assets 和 otadata。
- 写入前再次只读确认 ESP32-S3 rev 0.2、16 MiB Flash，Secure Boot 与 Flash
  Encryption 均未启用。
- 启动再次识别 8 MiB Octal PSRAM、80 MHz，memory test 为 `OK`；`assets` 为
  `Ready`，SoftAP DHCP 与全部 HTTP handlers 启动成功。
- server 提交一次配网页全刷；`pon_drf_refresh` 为 24.313 秒，driver 总耗时
  24.572 秒，结束状态 `PoweredOff`。串口没有出现凭据。
- 本轮只验证新配网页 framebuffer 能生成并完成全刷及安全关电；Wi-Fi QR 可扫性、
  STA 成功页二维码、浏览器自动填入和真实认证仍待人工端到端确认。

## 固定凭据、双二维码与串口配网刷写（同日续测）

- 在上版日志中，手机使用重启前保留在墨水屏/系统缓存中的旧 SoftAP 密码连接，产生
  ESP-IDF 5.5.3 event ID 45（`WIFI_EVENT_AP_WRONG_PASSWORD`）；发布版
  `esp-idf-svc 0.52.1` 将未知事件 panic，导致固件 abort 重启。不是目标路由器拒绝
  STA 凭据。
- 新构建固定到含相关上游修复的 `esp-idf-svc` 精确提交。SSID 后缀、固定 AP 密码和
  独立 8 位 admin bootstrap 都从工厂 eFuse MAC 做版本化、域分离派生；MAC 不是秘密，
  因此仅作为开发默认值。
- 配网页/成功页不再显示 admin token 明文；管理 URL QR 不含 token，并新增独立
  ADMIN QR。管理页新增已鉴权 token 轮换和 NVS 自定义值。
- 非 merge app image 为 3,850,784 / 4,128,768 B（93.27%，余 277,984 B）；本次仍
  只写 `ota_0 @ 0x20000`，没有改写 bootloader、partition table、NVS、assets 或
  otadata。
- 启动再次通过 8 MiB PSRAM memtest，`assets` 为 `Ready`，SoftAP DHCP、HTTP 以及
  `/admin/v1/token` handler 启动。设备端串口协议对固定测试 request ID 返回
  `state=ready`，未传输真实 SSID 或密码。
- 双二维码配网页全刷 transport 成功：`pon_drf_refresh` 24.316 秒，driver 总耗时
  24.592 秒，最终 `PoweredOff`。二维码内容仍需用户扫码确认。
- 审查修复后的最终 3,850,784 B app 再次 app-only 写入并复位启动；8 MiB PSRAM
  memtest、`assets=Ready`、SoftAP/DHCP/HTTP 均通过。该精确镜像的配网页全刷
  `pon_drf_refresh` 为 19.815 秒，driver 总耗时 20.092 秒，最终 `PoweredOff`；日志未
  输出派生密码或 admin token。
- reader 已针对 ESP-IDF USB Serial/JTAG VFS 在无 host SOF 时返回错误的行为改为持续
  休眠重试，并在断开时丢弃半条命令；当前只验证了 USB 已连接时的握手，尚未执行
  “电池/无 host 启动后再插电脑”的物理重连测试。
- 设备侧等待 40 秒仍处于 queued/running 的串口 job 现在报告 `pending`，不会错误声称
  已失败；该 job 仍可能随后完成并修改网络，主机脚本会把这个边界明确提示为超时。
- 尚未用实际参数执行串口配网的 accepted→succeeded、NVS promote 与 RESET 回连；
  也尚未实机触发一次错误密码确认不再 panic。

## Wi-Fi 原因诊断 app-only 刷写（同日续测）

- 板卡：NOTE4C PCB V1.0；USB 供电与调试；未测量电流或电压。
- 固件：`52a8168-dirty` 未提交工作树的默认 `note4c-server` release；非 merge app
  image 为 3,855,184 / 4,128,768 B（93.37%，余 273,584 B）。只写
  `ota_0 @ 0x20000`，未改写 bootloader、partition table、NVS、assets 或 otadata。
- 同一精确镜像复位启动识别到 8 MiB Octal PSRAM、80 MHz，memory test 为 `OK`；
  `assets=Ready`、SoftAP DHCP、全部 HTTP handlers 和串口 reader 均启动成功。
- 保存的 active 网络在本次环境中返回 disconnect reason 201，固件串口安全日志为
  `wifi_ap_not_found`，未输出 SSID、BSSID 或密码。该码也可能来自 SSID、2.4 GHz
  可见性/范围、密码或安全模式不匹配，且 active 不一定是升级前失败的 candidate，
  因此本记录不把它解释为“密码错误”。
- 使用合成的不存在 SSID 和非秘密测试密码执行串口脚本，完整走过 ready、accepted 和
  failed，主机安全输出 `wifi_ap_not_found`；失败 candidate 未 promote，复位后仍加载
  原 active 配置。该测试只验证失败原因链路，不代表真实网络配网成功。
- 复位后的配网页全刷 transport 成功：`pon_drf_refresh` 24.315 秒，driver 总耗时
  24.588 秒，最终状态 `PoweredOff`。未独立目视确认本次页面内容。

## WPA1 兼容 app-only 刷写（同日续测）

- 用户确认目标 AP 因另一个 IoT 设备的 WPA2 握手缺陷被强制为 WPA1；上一版 STA
  scan authmode threshold 为 WPA2，因此其 `wifi_security_incompatible` 与 ESP-IDF
  reason 211 的条件一致。没有据此读取或记录实际 SSID、密码。
- 有密码网络的最低门槛改为 WPA-Personal。ESP-IDF 将该字段作为最低认证等级，因此仍
  接受 WPA2/WPA3 Personal；可选 PMF 保留给 WPA2/WPA3 协商。WPA1/TKIP 属于弱安全
  兼容配置，不是量产默认。
- 新 release 非 merge app image 仍为 3,855,184 / 4,128,768 B（93.37%，余
  273,584 B）；只写 `ota_0 @ 0x20000`，保留 bootloader、partition table、NVS、
  assets 和 otadata。
- 精确镜像复位启动再次通过 8 MiB Octal PSRAM、80 MHz 和 memory test；
  `assets=Ready`、SoftAP DHCP 与全部 HTTP handlers 正常。NVS 中旧 active 配置仍返回
  reason 201；上一轮失败 candidate 按设计未 promote，所以这次启动不能代替用户用
  WPA1 目标参数重跑串口配网。
- 本轮配网页全刷 transport 成功：`pon_drf_refresh` 24.315 秒，driver 总耗时
  24.590 秒，最终 `PoweredOff`。未独立目视确认页面内容。

## Rune 编译限额与日历上传（同日续测）

- 板卡：NOTE4C PCB V1.0；USB 供电与调试；未测量电流或电压。
- PSRAM 版第一次重传 4,664 B 日历包时，`mvp_context()` 已成功；记录的 internal
  free heap 为 132,151 -> 64,779 B，PSRAM free heap 为 8,327,480 ->
  7,494,504 B。后台仍返回 `rune_compile_failed_or_memory_limit`，包未持久化。
- 主机用同一 `mvp_context()` 和源码复现：日历在 256 KiB 边界触发 Rune allocator
  failure，在 512 KiB 成功；相册在 512 KiB 仍触发 allocator failure，两个示例在
  1 MiB 均成功。固件编译期边界据此从 256 KiB 调整为 1 MiB，运行期 64 KiB
  memory limit 未改变，并加入两个示例的限额编译回归测试。
- 96 项主机单元测试通过；ESP32-S3 release target 构建通过。非 merge app image
  仍为 3,855,184 / 4,128,768 B（93.37%，余 273,584 B）。只写
  `ota_0 @ 0x20000`，保留 bootloader、partition table、NVS、assets 和 otadata。
- 写入前只读确认 ESP32-S3 rev 0.2、16 MiB Flash，Secure Boot 与 Flash
  Encryption 均未启用。复位启动再次通过 8 MiB Octal PSRAM、80 MHz 和启动
  memtest；`assets=Ready`，已保存 WPA1 配置完成关联、DHCP，并切换到纯 STA。
- 本次成功页全刷 `pon_drf_refresh` 24.314 秒，driver 总耗时 24.585 秒，最终
  `PoweredOff`；未独立目视核对画面。
- 同一个 4,664 B 日历包重新上传后 HTTP 返回 202，upload job 最终
  `succeeded`，结果为 `persistent=true`、`stored=assets_spiffs`；`widget.list`
  同时列出内置 demo 与状态为 `ready` 的日历，存储总体标记为 persistent。
- 本轮没有 activate 或 run 日历，也没有在上传成功后复位，因此组件画面、RTC
  weekday 映射、KV、active widget 和 RESET 后恢复仍未验证。

## 系统时间、RTC 持久化与日历运行（2026-09-05 续测）

- 板卡：NOTE4C PCB V1.0；USB 供电与调试；未测量电流或电压。写入前只读确认芯片
  revision、16 MiB Flash 和未启用的 Secure Boot/Flash Encryption 与既有记录一致；
  本节不记录 MAC、网络地址、SSID 或管理凭据。
- 固件：`52a8168-dirty` 未提交工作树的默认 `note4c-server` release；最终非 merge app
  image 为 3,888,672 / 4,128,768 B（94.18%，余 240,096 B）。各次都只写
  `ota_0 @ 0x20000`，保留 bootloader、partition table、otadata、NVS 和 assets。
- 第一版在启动恢复已存包时为每个包重新构建 Rune compiler context，实机于
  `mvp_context()` 内触发 TLSF allocator assert 并重启。上传提交前已经执行同一编译，
  包落盘又执行 ZIP/CRC 回读验证；修复为启动只恢复 broker registry、真正运行时仍做
  1 MiB 有界编译。修复后设备稳定启动，8 MiB PSRAM memtest、`assets=Ready`、保存
  网络关联/DHCP、HTTP 和纯 STA 切换均通过。
- SNTP immediate 同步成功并设置系统时钟。首次 RTC 回写因同时开始的成功页全刷占用
  主硬件所有者而在 2 秒 queue wait 超时；该轮没有执行 I2C 写。协调 timeout 改为
  45 秒后，成功页 `pon_drf_refresh` 约 24.325 秒、driver 总耗时约 24.598 秒，随后
  PCF8563 `0x02..0x08` UTC 单次写入及读回校验成功，并建立 NVS ownership marker。
- 下一次 RESET 在 Wi-Fi 初始化前从带 marker 的 RTC 恢复系统时钟成功；联网后再次
  SNTP 校准，并在约 24.586 秒成功页全刷结束后再次完成 RTC 写入/读回。这里验证的是
  本仓库定义的 C=0、2000..2099、UTC ownership 约定，不外推为芯片通用世纪语义。
- 管理 RPC 的 `time.status` 报告 `clock_valid=true`、`source=ntp`、`sync_state=synced`、
  `rtc_updated=true`；以默认 NTP server 和 UTC+08:00 执行 `time.configure` job 成功，
  随后的状态读取保持一致。本地换算结果与测试主机同一时刻一致。
- 更新后的日历包重新上传 job 成功；RESET 后列表仍恢复内置 demo 与状态为 `ready` 的
  日历。一次 `widget.run` 完成，返回 100,000 instruction budget 与 64 KiB runtime
  limit，EPD 刷新链路无 job error。屏幕日期/星期/排版尚待用户目视确认，且本轮未
  activate 日历或验证 KV/active 的再次复位恢复。

## 大字月历、农历 ABI 与激活（2026-09-05 续测）

- 主机实现并验证了 2000..2099 公历转农历、干支年、日历所需 16×16 CJK 字形子集，
  以及新版全屏月历。主机侧实际 Rune 源码渲染为 400×300 2 bpp framebuffer 后与选定
  效果图并排检查：7 列等宽、四角安全退让、仅当天黄色、周末红字、每日农历单行、
  顶栏公历/星期/干支年/农历月日均满足设计；结果记录在仓库根 `design-qa.md`。
- 104 项 workspace 主机测试、脚本 mock/伪终端测试、格式检查和 ESP32-S3 release
  构建均通过。非 merge app image 为 3,904,096 / 4,128,768 B（94.56%，余
  224,672 B）；本次只写 `ota_0 @ 0x20000`，保留 bootloader、partition table、
  otadata、NVS 和 assets。
- 精确镜像两次启动均识别 8 MiB Octal PSRAM、80 MHz，启动 memory test 通过；
  `assets=Ready`、owned RTC 恢复、保存网络关联/DHCP、纯 STA、HTTP 和 NTP→RTC
  回写均正常。成功页全刷的 `pon_drf_refresh` 为 24.315 秒，driver 总耗时
  24.587 秒，最终 `PoweredOff`。
- 新日历包为 4,972 B、版本 `0.2.0`。旧管理 token 与 NVS 中自定义值不符时上传按
  预期返回 401；取得当前 token 后上传 job、`widget.run` job 和 `widget.activate`
  job 均 `succeeded`，状态接口确认日历为 active、存储为 persistent。
- 日历实机运行的 `pon_drf_refresh` 为 24.314 秒，driver 总耗时 24.591 秒，最终
  `PoweredOff`；widget 日志报告 `CALENDAR PRESENTED`。串口验证了完整刷新与安全
  关电，最终字形、对齐和颜色仍以用户目视验收为准。
