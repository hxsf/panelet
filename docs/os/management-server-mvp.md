# Panelet 管理服务

2026-10-22 的 Panelet 更名只完成软件验证与目标构建，本次没有重新刷写或验收真实设备；下文历史记录不能作为新构建已经上板的证明。

## 验证状态与边界

管理服务是 `panelet-os` 固件的默认组成部分，不再受 Cargo feature 控制；它使用 ESP-IDF `std` 栈。2026-09-09 将小组件包格式和宿主 ABI 迁移为 `note4c.rune/3`，已完成目标构建和 app-only 刷写，验收范围及限制见 [ABI 3 实机记录](../devices/note4c/history/abi3-hardware-log-2026-09-09.md)。

主界面当前由 server 状态驱动：无有效网络时显示 SoftAP 配网信息，STA 连接成功后自动显示管理地址和 admin credential；小组件调用屏幕 API 成功后，`system.status.main_screen` 切换为 `widget`。网络回退到配网状态时，server 会重新提交配网页。所有 EPD 操作仍由主硬件循环串行执行。

以下是历史版本记录，不代表 ABI 3 验收：持久化 v2 管理服务、PSRAM、双二维码、串口配网、Wi-Fi 原因诊断、系统时间和农历 ABI 已经 app-only 刷入 NOTE4C。新版日历包已完成 Rune 编译、SPIFFS 持久化、运行和 activate，但尚未验证 active widget 的 RESET/掉电恢复。2026-09-05 在 Apple Silicon macOS 上生成的完整 app image 为 3,904,096 / 4,128,768 B，占 OTA 槽 94.56%，剩余 224,672 B；该数字不代表运行时 RAM 余量。

HTTP handler 只负责长度限制、认证、JSON 解码和入队。Wi-Fi 切换、硬件测试、Rune 编译/运行、网络请求及 EPD 刷新必须在各自 worker/actor 中执行，不能阻塞管理 HTTP task。

安装配置与定时检查已增加到管理页，新增 `widget.configuration` / `widget.configure` RPC；详细契约和验证边界见 [配置与定时运行](widget-configuration-and-polling.md)。

## 启动、认证与持久化存储

固件只读取自己的 `note4c_net` NVS namespace。没有有效配置时开启 WPA2 SoftAP：

1. SSID 为 `NOTE4C-xxxx`，后缀取工厂 eFuse MAC 的末两字节；AP 密码由完整工厂 MAC、版本号和独立用途标签稳定派生为 `XXXX-XXXX-XXXX`；
2. 管理地址为 `http://192.168.4.1/`；
3. 屏幕以文字显示 AP 密码；独立 ADMIN QR 包含 8 位 bootstrap token，不再显示 token 明文。bootstrap token 使用与 AP 密码不同的用途标签派生；两者不得写日志或出现在 job/event 响应中；
4. `wifi.configure` 先保存 candidate，显式断开并排空旧 STA/IP 状态后再连接；只有本次 SSID 的新关联事件和有效 IPv4 同时出现才提升为 active，失败时保留 SoftAP，连接 timeout 会取消仍在进行的异步连接；有密码网络的最低认证门槛为 WPA-Personal，以兼容当前 WPA1 IoT 网络，同时仍接受 WPA2/WPA3 Personal；
5. SoftAP 和 STA 模式的所有 RPC 请求都必须提供 `X-Note4c-Token`；HTTP handler 还要求 `application/json`，并拒绝不匹配当前 `Host` 的浏览器 `Origin`。

配网页同时显示标准 Wi-Fi QR 和独立 ADMIN QR。STA 成功页把不含凭据的管理 URL QR
与 ADMIN QR 分开。管理页的 `POST /admin/v1/token` 要求当前 token，可将 6..32 位
ASCII token 写入 `admin_custom` NVS key 并立即轮换运行时认证；响应不返回新旧 token。
一旦存在合法自定义值，重启后不再使用派生 bootstrap。

工厂 MAC 在无线侧可观察，无密钥 SHA-256 派生只解决“单机固定、一机一值”，不提供
真正的秘密性。8 位 bootstrap、可见二维码和未加密 HTTP 都只适合当前开发调试环境；
量产需要每机秘密、失败限速和安全传输，不能沿用本方案。

Wi-Fi 现在最多保存 5 组凭据：连接成功才更新列表，同 SSID 替换密码，优先尝试最近成功项；启动和断线后尝试其余已保存网络，全部不可用时保留配网入口并每 60 秒重试。满额不静默淘汰，新增前先忘记一组。旧单组 NVS 凭据可迁移，新列表写入后清理旧明文键。`wifi.saved` 只返回 SSID 和容量，`wifi.forget` 删除保存项；忘记当前网络不会立即断开这次连接。真实第二个 AP 的切换仍需对应环境验证。

串口配网使用一行式 `N4C-PROVISION/1` JSON 协议。USB reader 只负责有界解析、凭据
校验和向同一 broker 入队；唯一 network worker 仍负责 candidate、连接验证和 promote。
回复只含随机 request ID、状态、job ID 或稳定错误码。设备等待 40 秒仍未终结时返回
`pending/device_timeout`，因为 worker 可能随后完成；脚本会以超时退出并明确提示该延迟
副作用，不会将仍运行的 job 表述为失败。明确断开会按白名单报告 AP 未找到、认证超时、
认证/密钥交换失败、安全模式不兼容、关联失败、弱信号或 beacon timeout；关联成功但未
取得地址则报告 DHCP timeout。HTTP job 额外保留数值 reason/RSSI，串口不透传任意设备
字符串。`wifi_ap_not_found` 也可能是 SSID、频段/范围、密码或安全模式不匹配，不能直接
等同于错误密码。主机入口为：

```sh
scripts/provision-wifi.py --port /dev/cu.usbmodemXXXX \
  --ssid 'YOUR-WIFI' --password 'YOUR-PASSWORD'
```

脚本不自动选择串口，也不打印串口原始流。参数密码可能出现在 shell history 和进程
列表；运行前必须退出 `espflash monitor`。设备 reader 在没有 USB host SOF 时清除半条
命令并休眠重试，设计上允许之后插入电脑；该断开重连路径尚未完成实机验证。

当前 NVS 未加密，管理服务也没有 TLS。这是可信本地网络中的开发固件，不是量产安全方案。
WPA1/TKIP 同样属于仅为现有 IoT 互操作保留的弱安全兼容路径；可控制 AP 时应迁移回
WPA2-Personal/AES 或更强配置。

系统时间配置同样保存在 `note4c_net` namespace。默认 NTP server 为
`pool.ntp.org`，默认时区为 UTC+08:00。时区当前保存为 -720..=840 的固定分钟偏移，
不包含 IANA 时区数据库，也不会自动处理夏令时。STA 取得地址后，ESP-IDF SNTP 以
immediate 模式设置系统 UTC 时钟；成功回调只发布有界事件，由 network worker 再通过
主硬件所有者把 UTC 一次性写入 PCF8563 的 `0x02..0x08` 并读回校验。NVS ownership
marker 只在这次写入成功后建立；后续启动只信任带 marker、VL 未置位、century bit 为
0 的 RTC 数据，按本仓库定义的 2000..2099 UTC 约定先恢复系统时钟，再等待 NTP 校准。
RTC 协调 timeout 为 45 秒，以覆盖 NOTE4C 一次约 25 秒的全屏刷新，但 RTC I2C 事务
本身仍使用 100 ms 有界 timeout。

8 MiB `assets` 作为 SPIFFS 使用，但启动时 `format_if_mount_failed=false`，不会静默格式化。只有未初始化状态下，管理页向 `POST /storage/v1/initialize` 发送 `{"confirm":"ERASE ASSETS"}` 才会擦除整个分区、写入 `NOTE4C-WIDGET-STORE` v1 marker 并启用持久化；Ready 状态拒绝在线重格式化。旧格式、无 marker 或无法挂载时，包上传和脚本存储失败关闭；硬件菜单、恢复入口和管理页仍可使用。

SPIFFS 是 flat filesystem，代码以截断 SHA-256 物理文件名保存对象，逻辑目录仅存在于经校验的索引和 `.pnw` 内。private 索引持久化并验证 owner ID，防止命名碰撞造成跨 widget 访问。小组件包、active widget、私有 KV、private 文件和 shared 文件均持久化；Flash 是 authoritative storage。MVP broker 为列出和运行保留最多 20 KiB 的已解码包缓存，该缓存会在重启时从 Flash 重建，不承担持久化。包更新使用临时文件、`fsync`、回读 ZIP/CRC 校验和可恢复替换；删除先写 tombstone，再删除包和整个 private namespace。启动恢复 torn temp/tombstone，并对索引未引用 blob 做 GC。它们仍需要通过实机复位和受控断电测试，不能把主机文件系统测试当成 Flash 原子性证明。

## JSON-RPC 2.0 子集

唯一入口是 `POST /rpc/v1`，请求体上限 64 KiB，不支持 batch。耗时操作返回 HTTP 202 与 `job_id`，调用 `job.get` 查询最终状态；事件由固定 ring 保存，通过 `event.list` 分页读取。

当前 RPC 方法：

- 状态与网络：`system.status`、`wifi.status`、`wifi.scan`、`wifi.configure`
- 时间：`time.status`、`time.configure`、`time.sync`
- 观察：`event.list`、`job.get`
- 显示和测试：`screen.retry` 重试最新画面；诊断模式下使用 `display.text`、`display.status`、`test.list`、`test.run`
- 模式：`system.mode.set {mode:"normal"|"diagnostic"}`；访问诊断页面本身不调用该方法
- 总览：`dashboard.get`，一次返回 system、widgets 与 summary（总数、脚本处理数、前台 ID）；沿用管理认证，不包含配置凭据。
- 小组件：`widget.deactivate`、`widget.list`、`widget.get`、`widget.delete`、`widget.activate`、`widget.status`

`dashboard.get` 的 `system` 增加以下只读信息，不返回组件配置凭据：

- `mode`：`normal` 或 `diagnostic`。
- `input`：`{captured, overflow, pending}`，分别为采集计数、队满丢弃数和待处理数。
- `presentation`：`{submission_id, state, refreshing, error}`。state 为 `idle/pending/presenting/presented/unchanged/failed`，refreshing 单独指示硬件正在刷新，可能正在完成已退出前台的旧帧。

脚本处理状态来自组件 job，不与物理刷新合并。flush 接收成功不是屏幕呈现完成；呈现失败报告到 presentation。`screen.retry {}` 请求重试最新目标，不重新执行脚本，也不等于物理完成。无目标时拒绝重试。

`system.mode.set` 显式进入诊断时销毁前台会话、废弃旧待呈现帧，但保留持久化前台选择；回到 normal 重新 mount。模式不持久化，重启回到正常模式，安全启动仍保留。诊断菜单页面只读取状态；正常模式三个按钮交给组件，未绑定事件忽略。

`widget.upload` 不属于 JSON-RPC。上传与共享二进制数据使用独立 HTTP 接口：

- `POST /packages/v1`：请求体为原始 `.pnw`，content type 为 `application/vnd.note4c.widget+zip`；返回 202 和 `job_id`；
- `POST /shared/v1`：请求体为原始文件 bytes，逻辑路径放在 `X-Note4c-Path`；
- `GET /shared/v1`：无 path header 时列出逻辑路径，有 header 时读取原始 bytes；
- `DELETE /shared/v1`：按 `X-Note4c-Path` 删除；
- `POST /storage/v1/initialize`：唯一允许格式化 assets 的显式入口。

以上接口与 RPC 一样要求 `X-Note4c-Token` 和同源检查。包与单个共享文件当前都限制为 64 KiB；shared 总配额为 1 MiB。

浏览器编辑器的版本化资源另使用 `/web/v1/*` 管理接口及 `/web/<bundle_id>/<path>`
静态文件接口。资源包格式、各接口鉴权、容量限制、续传／提交／回退和 A/B 兼容规则见
[assets 资源的存储与传输契约](web-assets.md#存储与传输契约)。

HTTP 200 只表示 RPC 调用成功，不表示异步 job 成功。RPC 级结构化错误形态为：

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "error": {
    "code": -32006,
    "message": "widget already pending or running",
    "data": {
      "kind": "job",
      "code": "widget_busy",
      "details": {"optional": "diagnostic data"}
    }
  }
}
```

异步失败体现在 `job.get` 的 `result.error` 中。v2 worker 应保留稳定的 `kind/group`、`code`、`message`、`retryable` 和可选 `details`，页面和自动化只能根据稳定 code 分支，不得解析人类消息：

```json
{
  "job_id": 9,
  "method": "widget.input",
  "state": "failed",
  "result": {
    "error": {
      "kind": "job",
      "code": "widget_host_error",
      "message": "widget host operation returned an error",
      "details": {
        "group": "http",
        "code": "timeout",
        "message": "operation exceeded timeout",
        "retryable": true
      }
    }
  }
}
```

Rune widget host error 使用上述结构；管理页也兼容既有测试/网络 worker 的旧字符串或 ad-hoc JSON 错误。后两者尚待迁移，调用方暂时不能假设所有非 widget job 都有相同字段。

## `note4c.rune/3` 二进制上传包

`.pnw`（Panelet Widget）是语义明确的 STORE-only ZIP；旧 `.n4w` 包继续接受，HTTP MIME 仍使用 `application/vnd.note4c.widget+zip`，既有含 `n4w` 的错误码保持不变。扩展名不替代内容校验，包包含：

```json
{
  "id": "com.example.weather",
  "name": "Weather",
  "version": "0.2.0",
  "abi": "note4c.rune/3",
  "capabilities": [
    "time.read",
    "http.fetch",
    "screen.present",
    "storage.kv",
    "fs.private",
    "fs.shared.read"
  ],
  "host_permissions": ["https://api.example.com/v1/*"]
}
```

```text
manifest.json
main.rn
resources/labels/title.txt
```

请求体不是 JSON、multipart 或 base64。设备拒绝压缩、加密、ZIP64、data descriptor、extra field、archive comment、目录项、symlink、重复/非法路径、CRC 错误、local/central header 不一致、空洞/重叠以及 EOCD 后尾随数据。

Manifest 不接受 `entry`，源码文件仍为 `main.rn`。一个包声明一个 `#[widget::component] impl State`，由宏生成内部入口；不保留旧 main(std)、注入式生命周期或 render hook 兼容层。宏校验绑定和签名，执行时再验证动态结果为 `Result<()>`。上传先验证 JSON、字段、ABI、能力、网络权限、源码和资源限额，再在 worker 编译；全部成功才替换同 ID 包，无效 candidate 不得先删除旧版本。

完整的十二项 capability 词表：

| capability | 对应能力 |
|---|---|
| `time.read` | uptime、Unix 时间和 RTC 读取 |
| `power.read` | 电池原始观测 |
| `wifi.read` | 只读 Wi-Fi 状态 |
| `device.read` | 固件、型号与固定屏幕参数 |
| `http.fetch` | 有界 HTTP 请求 |
| `http.insecure_tls` | 允许单次 HTTPS 请求关闭证书验证；危险能力 |
| `screen.present` | Canvas、文本或状态页提交 |
| `storage.kv` | 小组件私有 KV |
| `log.write` | 有界日志 |
| `fs.private` | 当前小组件独占的私有文本文件根 |
| `fs.shared.read` | 公共根的列出、存在检查和 UTF-8 文本读取 |
| `fs.shared.write` | 公共根的 UTF-8 文本写入和删除 |

`os::runtime` 的 ABI/限制查询、`os::resource` 的包内只读访问以及 `os::json` 的本地解析不额外消耗 capability；`os::device` 的型号、固件和屏幕参数需要 `device.read`。宿主仍要在每次受控调用执行时再次检查 capability，不能只相信上传校验。

宿主模块将硬件 I/O 分组放在 `os::power` 与 `os::wifi`，例如 `os::power::battery().await?` 和 `os::wifi::status().await?`，不再嵌套在 `os::device` 下。`os::time::rtc().await?` 同时返回 PCF8563 的日期时间、`century_bit`、`voltage_low` 等原始字段以及可选 `unix_ms`/`valid`，字段语义与完整示例见 [Rune 小组件管理与宿主 ABI MVP](widget-runtime-mvp.md)。

### host permissions

`host_permissions` 使用严格的 Chrome 风格模式：`http[s]://host[:port]/path`。主机只能是精确域名/IP 或单个前导 `*.` 子域通配；path 可含一个 `*`。不允许宽泛的 `*` host、通配端口、userinfo、query 或 fragment。默认端口会规范化，规范化后的重复项会被拒绝。

例如：

```json
["https://api.example.com/v1/*", "https://*.weather.example/data"]
```

每次请求及每次 redirect 都必须重新匹配 pattern，并重新校验解析后的地址。loopback、unspecified、multicast、broadcast 和设备自身地址始终拒绝；LAN 地址只有显式 permission 才能访问。

当前 ESP-IDF Rust 安全封装不暴露已连接 socket 的 peer IP，因此实现会在连接前后各做一次 DNS/IP 授权，但还不能把已校验 IP 固定到实际连接。这只能降低、不能消除 DNS rebinding/TOCTOU 风险；在获得可验证 peer 或自定义 pinned connector 前，`http.fetch` 仍是开发能力，不应处理内网管理凭据。跨 origin redirect 直接拒绝，303 在同 origin 内改为 GET（HEAD 除外）。

`http.insecure_tls` 具有两层显式授权：manifest 必须同时声明 `http.fetch` 和 `http.insecure_tls`，脚本还必须对具体 `HttpRequest` 调用 `set_tls_verify(false)`。它仅适用于 HTTPS，不允许成为全局开关。管理页选择该能力时会显示红色警告并在上传前再次确认。

## 资源与 RAM 配额

当前包限制：

- 最多 4 个小组件，包含不可替换的内置项；
- 单个源码最多 5 KiB；
- 每包最多 8 个 UTF-8 资源，单项正文最多 1 KiB，资源路径与文本合计最多 4 KiB；
- 资源路径最长 64 B，必须是相对路径，不允许空 segment、`..`、反斜杠、冒号或控制字符；
- manifest 最多 8 个 host permissions；
- 已安装包加一次 pending replacement 的动态内容 admission accounting 上限为 20 KiB；
- `.pnw` 请求体最多 64 KiB；解码后的动态内容仍受 20 KiB admission accounting 限制。HTTP handler 当前会按 `Content-Length` 一次性分配临时缓冲，因此峰值必须在实机 heap 验收中测量。

`widget.get` 当前返回 manifest、源码、`resource_count`、RAM accounting 和状态，并在 `include_source=true` 时以 `resources: [{path,text}]` 回传资源正文，供完整编辑与重新打包。持久化并不代表执行时零 RAM：运行注册表仍缓存最多四个小包的已解码 manifest、源码和资源，并按 20 KiB admission 上限控制。

脚本通过 `os::resource::read_text(path).await?` 只读访问自己包内资源。`os::fs::private` 和 `os::fs::shared` 是另外两个根，不与 package resource 路径互通。脚本侧目前只提供 UTF-8 text API；管理 HTTP 的 shared 接口可保存任意二进制，以便后续增加 bytes ABI 而不迁移磁盘格式。

## ABI 3 设备迁移

刷写前把旧包、配置和前台选择导出到仓库外受控目录，准备 TODO 0.4.0、Calendar 0.3.0、Photo Album/Counter 0.2.0 及设备其他已安装包。逐包迁移脚本，不用简单字符串替换代替行为检查。新固件拒绝执行旧 ABI，但保留其持久化数据；新包沿用 ID，保留配置、网络授权、间隔、资源、KV/private 数据及前台选择。迁移失败保留导出和原件，不格式化存储。

固件继续使用现有 app-only 流程，分区上限 4,128,768 B；以保留约 256 KiB 余量为目标，不修改分区、不删除 TLS 校验。脚本接收回执、物理 BUSY/清理、三键队列及内部 RAM/PSRAM 水位需分别验证。最终构建和实机记录完成后另行更新结果。

## 管理页行为

当前工作树管理页采用菜单导航、按钮旁任务状态、按设备地址保存 Admin token，以及进入页面自动加载。总览使用 `dashboard.get`；旧固件缺少该方法时回退到 `system.status` + `widget.list`，运行状态显示未知。2026-09-09 用户已验收上一版页面；本轮新增诊断切换、提交/呈现状态与重试已通过本机浏览器交互检查，设备接口验证见 ABI 3 实机记录。生命周期与前台契约见 [生命周期 v1](widget-lifecycle-v1.md)，预览范围见 [本机 UI 预览](../development/server-ui-preview.md)。

内置页面支持：

- 修改 NTP server 与固定 UTC 偏移、查看同步/RTC 状态并触发立即同步；

- 编辑 ABI 3 manifest（无 entry）、十二项 capability、host permissions、异步 Rune 源码；
- 添加、编辑、移除 UTF-8 包内资源，并在浏览器生成 STORE-only `.pnw`；
- 选择并上传预构建 `.pnw`，自动记录并轮询 job；
- 卡片列出小组件及前台/脚本处理状态，查看、配置、编辑、设为前台/取消前台、删除；
- 显式初始化 assets，以及上传、列出和删除 shared 二进制文件；
- 禁止对 built-in 项显示可执行删除操作，busy 项暂时禁用冲突操作/删除；
- 醒目标记 assets 擦除风险、尚未实机验证和 `http.insecure_tls`；
- 将 RPC error 与 job `result.error` 显示成 `kind/code/message/retryable/details`。

删除当前前台、修改 built-in 或与 busy 操作冲突时按 broker 规则拒绝；包更新需保留配置与私有数据，并使受影响会话失效。删除现在是 worker job：Flash tombstone、包和整个 private namespace 清理成功后，才从内存注册表删除并报告 job 成功。

## 待实机验证

刷机前按恢复/分区文档核实同设备原厂备份、受控副本与安全状态；已有核验记录可复用。ABI 3 板测至少记录：

- 固件 commit、镜像尺寸、OTA slot 余量；
- 启动、SoftAP、STA candidate/rollback、token 鉴权；
- 管理页在桌面和手机浏览器中的上传、资源编辑、列表、前台选择、删除与 job 错误展示；
- Rune 编译/运行峰值 heap、最大连续块和 widget task stack high-water mark；
- HTTP DNS、redirect、timeout、响应上限和 TLS 验证；
- Canvas 四色 packing、裁剪和实际显示结果；
- RTC、电池、Wi-Fi 只读值与 host error；
- 约 24 秒 BUSY 期间 HTTP 页面持续可用，且刷新结束后 EPD 完成安全断电；
- 上传包、KV、private 文件、shared 文件和 active widget 在 RESET 后恢复；
- 删除后 RESET 不复活包或私有数据；
- 更新/删除过程中受控复位时，恢复逻辑只加载 CRC 和 manifest 均有效的包。

ABI 3 之前的 2026-09-09 生命周期、前台恢复、多 Wi-Fi 与尺寸实测见 [硬件记录](../devices/note4c/history/lifecycle-hardware-log-2026-09-09.md)。

### 自动差异窗口刷新

`system.status` / `dashboard.get` 的 `presentation` 增加 `refresh_mode`（`full` / `partial` / null）和 `window`（`{x,y,width,height}` 或 null）。字段描述最新提交已开始的刷新方式；pending/unchanged 没有新刷新方式。管理页显示全屏/局部窗口和区域尺寸。策略、基线内存和回退条件见 [显示策略](../devices/note4c/screen-partial-refresh.md)。
