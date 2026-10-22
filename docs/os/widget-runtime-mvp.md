# Panelet 小组件运行时：`note4c.rune/3`

2026-10-22 的 Panelet 更名只完成软件验证与目标构建，本次没有重新刷写或验收真实设备；下文历史记录不能作为新构建已经上板的证明。

本文描述已实现的 ABI 3 接口。2026-09-09 的目标构建、app-only 刷写及实测范围见 [ABI 3 实机记录](../devices/note4c/history/abi3-hardware-log-2026-09-09.md)；历史版本的刷写记录不替代本版证据。生命周期、多事件绑定和迁移见 [生命周期 v1](widget-lifecycle-v1.md)，配置和定时行为见 [配置与定时运行](widget-configuration-and-polling.md)。

组件通过宏声明生命周期，通过 `os` namespace 使用宿主能力，主动 flush 画面。没有注入的 std/ctx、用户 main 入口或宿主 render hook。

## 1. 包与入口

上传体是 STORE-only ZIP 格式的 `.pnw`（Panelet Widget），仍接受旧后缀 `.n4w`。后缀更名不改变 ZIP 内容、ABI 或资源限制：

```text
manifest.json
main.rn
resources/labels/title.txt
```

`POST /packages/v1` 直接接收 ZIP bytes，不经过 JSON-RPC、multipart 或 base64。包内只允许上述两个固定文件和零到八个 UTF-8 `resources/` 文件；设备执行 ZIP 结构、STORE method、路径、CRC、大小、manifest 和 Rune 编译校验。

`abi` 必须为 `note4c.rune/3`，manifest 不接受 `entry`。`main.rn` 保留为源码文件名，不表示需要定义 main。源码只声明一个 `#[widget::component] impl State`，由宏生成内部入口；签名见生命周期文档。旧 ABI 包明确拒绝执行，升级通过保留相同 ID 替换包，不删除配置或私有数据。

设备上传 worker 先完整校验包并用与运行时相同的 Rune Context 编译 candidate，随后写临时文件、`fsync`、回读校验，再替换持久化包和内存注册项。上传的是源码，不是未经验证的跨版本 Rune Unit。

## 2. 十二项 capability

| capability | 授权的分组 API |
|---|---|
| `time.read` | `os::time::uptime()`、`unix()`、`local()`、`rtc()` |
| `power.read` | `os::power::battery()` |
| `wifi.read` | `os::wifi::status()` |
| `device.read` | `os::device::info/model/firmware/screen_width/screen_height/color_count` |
| `http.fetch` | `os::http::request/get/get_text/fetch` |
| `http.insecure_tls` | 请求级 `set_tls_verify(false)`；同时要求 `http.fetch` |
| `screen.present` | `os::screen::get_context`、Canvas 绘图与 `flush` |
| `storage.kv` | `os::storage::has/get/set/delete` |
| `log.write` | `os::log::write/debug/info/warn/error` |
| `fs.private` | `os::fs::private::exists/read_text/write_text/delete/list` |
| `fs.shared.read` | `os::fs::shared::exists/read_text/list` |
| `fs.shared.write` | `os::fs::shared::write_text/delete` |

`os::runtime` 的 ABI/配额/取消查询和休眠、`os::resource` 的包内只读资源、`os::json` 的本地 JSON 解析不额外要求 capability。`os::device` 固定信息需要 `device.read`；资源和设备信息仍只能来自当前 widget sandbox，不能映射任意文件或全局状态。

Capability 采用两次检查：上传时拒绝未知/重复声明，实际敏感调用开始时再次检查。创建一个 Future 不消耗 call budget，也没有宿主副作用；第一次 poll/`await` 才执行 capability 检查、配额预留和 host I/O。

## 3. `os` 模块 API

`os::...` 可直接使用或通过 `use os::screen;` 导入。模块自身不可变；宿主调用绑定当前 VM/invocation 的权限和配额，不使用可变进程全局状态。下面的代码片段是从生命周期调用的普通辅助函数或函数体，不是完整包。

| 分组 | 主要 API | 调用性质 |
|---|---|---|
| `os::config` | `get()` | 同步、只读当前小组件安装配置 |
| `os::time` | `uptime()`、`unix()`、`local()`、`rtc()` | 前三项立即返回 `Result`；RTC 为 cold Future |
| `os::http` | `request()`、`get()`、`get_text()`、`fetch()` | request builder 同步；I/O 为 cold Future |
| `os::screen` | `get_context()`、`get_size()`、`get_width()`、`get_height()`、`get_palette()`、文字裁切 | 取得调用内 Canvas；`canvas.flush()` 为 cold Future |
| `os::runtime` | `abi_version()`、`capability()`、`remaining_ms()`、`cancelled()`、`limits()`、`ok()`、`sleep()`、`wait_all()`、`timeout()` | 调度与限制 |
| `os::storage` | `has()`、`get()`、`set()`、`delete()` | cold Future、按 widget ID 隔离 |
| `os::resource` | `exists()`、`read_text()`、`list()` | cold Future、只读当前包 |
| `os::fs::private` | `exists/read_text/write_text/delete/list(prefix)` | cold Future、当前 widget 独占根 |
| `os::fs::shared` | `exists/read_text/write_text/delete/list(prefix)` | cold Future、公共根、读写分权 |
| `os::log` | `write()`、`debug()`、`info()`、`warn()`、`error()` | cold Future |
| `os::json` | `parse()` | 本地、有界同步解析 |
| `os::power` | `battery()` | cold Future |
| `os::wifi` | `status()` | cold Future |
| `os::device` | `info/model/firmware/screen_width/screen_height/color_count` | 有界同步读取 |

所有 host-facing 失败都使用 `Result::Err(HostError)`。稳定字段为：

- `code`：程序分支依据，例如 `capability_denied`、`invalid_argument`、`quota_exceeded`、`timeout`、`cancelled`、`unavailable`；
- `group`：`http`、`screen`、`runtime`、`power` 等来源；
- `message`：面向人的说明，不保证可供程序解析；
- `retryable`：是否适合由调度器稍后重试。

推荐用 `?` 把错误交给 widget job，而不是把失败伪装成空字符串或零值。MVP 为减小固件没有安装 Rune 默认 prelude，因此用 `os::runtime::ok(value)` 构造入口的成功 `Result`：

```rune
pub async fn load_data() {
    os::log::info("START").await?;
    let text = os::http::get_text("https://api.example.com/v1/message").await?;
    let canvas = os::screen::get_context()?;
    canvas.clear(os::screen::WHITE)?;
    canvas.text_sized([12, 20], text, os::screen::BLACK, 24)?;
    canvas.flush().await?;
    os::runtime::ok(())
}
```

## 4. Canvas

Canvas 固定为 400 × 300、30,000 B、MSB-first 2 bpp。颜色编码沿用面板 framebuffer：

| 值 | 颜色 |
|---:|---|
| `0` | 黑 |
| `1` | 白 |
| `2` | 黄 |
| `3` | 红 |

一帧最多 128 条命令，所有 `text` 的 UTF-8 数据合计最多 1 KiB。支持 `clear`、`pixel`、`line`、`rect`、`fill_rect`、`text` 和新增的 `text_sized`；host 会做 checked conversion，渲染核心对屏幕外几何执行裁剪，零宽或零高矩形是 no-op。旧 `text` 使用 5×7 ASCII / 16×16 中文，scale 为 `1..=4`。`text_sized` 支持 16～32 偶数像素行高，英数可见字高与中文接近；具体字体和裁切语义见配置与定时运行文档。不支持的 glyph 返回结构化错误。

坐标数组分别为 `[x, y]`、`[x0, y0, x1, y1]` 与 `[x, y, width, height]`：

```rune
pub async fn load_data() {
    let canvas = os::screen::get_context()?;
    canvas.clear(1)?;
    canvas.line([-20, 0, 419, 299], 0)?;
    canvas.rect([8, 8, 384, 284], 2)?;
    canvas.fill_rect([16, 40, 160, 48], 3)?;
    canvas.text([24, 54], "NOTE4C", 1, 2)?;
    let receipt = canvas.flush().await?;
    os::runtime::ok(receipt.submission_id)
}
```

`get_size()` 返回逻辑区域 `[width, height]`，宽高快捷函数提供单维尺寸；`get_palette()` 返回可用颜色列表。使用 `os::screen::{BLACK, WHITE, YELLOW, RED}` 常量。v1 区域就是整屏，不实现拼页。

同一 invocation 多次取得的 Canvas 共享工作区，继承当前会话最后成功提交的帧，新会话是白底；未 flush 的改动丢弃。flush 生成不可变快照，后续绘图不影响它。`submission_id` 是接收回执，不是 job ID，也不是物理完成证明；成功接受的画面不随后续脚本失败回滚。

宿主以首份待呈现帧为起点合并 200ms，更新不延长窗口。全刷中只保留最新目标，最终像素指纹相同则跳过，不同则按 [差异窗口策略](../devices/note4c/screen-partial-refresh.md) 自动选择 PTLW 或全刷。失败保留目标并报告，不循环重试；新提交或管理 `screen.retry {}` 重试。物理刷新与安全清理全部成功才记录成功指纹；其他显示入口改屏时使缓存失效。

缓冲池最多四份 30,000 B（工作、最新、刷屏中及提交短暂副本），另保留一份 30,000 B 成功显示基线；基线分配失败时使用全刷，像素在 PSRAM，同步/引用计数在内部 RAM。调用撤销释放工作缓冲，不能因脚本保留过期 Canvas 而占住缓冲池。显式切换、配置重建和诊断模式清除旧待呈现帧，已开始的刷新须完成有界 BUSY 等待与安全断电。PTLW 保留完整 DTM 传输，不宣称刷新加速或长期寿命已验证。

## 5. HTTP 与 JSON

`host_permissions` 使用 `http[s]://host[:port]/path` 严格模式，只接受精确 host/IP 或单个前导 `*.` 子域通配；path 最多一个 `*`。每个 redirect 都重新执行 permission、DNS/IP、设备自身地址和 capability 检查。

常用 GET 与 JSON：

```rune
pub async fn load_data() {
    let response = os::http::get("https://api.example.com/v1/status").await?;
    if !response.is_success() {
        os::log::warn("HTTP STATUS IS NOT SUCCESS").await?;
        return os::runtime::ok(());
    }

    let json = response.json()?;
    let title = json.get("title")?.unwrap().string()?.unwrap();
    os::storage::set("last_title", title.clone()).await?;
    let canvas = os::screen::get_context()?;
    canvas.clear(os::screen::WHITE)?;
    canvas.text_sized([12, 20], title, os::screen::BLACK, 24)?;
    canvas.flush().await?;
    os::runtime::ok(())
}
```

完整 request builder：

```rune
let request = os::http::request("GET", "https://api.example.com/v1/data")?;
request.add_header("Accept", "application/json")?;
request.set_timeout(5000)?;
request.set_max_redirects(1)?;
let response = os::http::fetch(request).await?;
```

ABI 限制 URL 512 B、请求 header 最多 16 个/总计 2 KiB、请求或响应正文最多 16 KiB、redirect 最多 3 次；默认 timeout 为 10 秒，显式设置最多 30 秒。平台可以更严格，不能放宽。

### 危险 TLS 例外

只有同时声明 `http.fetch` 和 `http.insecure_tls` 时，以下调用才可能成功：

```rune
let request = os::http::request("GET", "https://lab.example.com/data")?;
request.set_tls_verify(false);
let response = os::http::fetch(request).await?;
```

这会关闭该请求的服务端证书验证，可能遭受中间人攻击。它不是访问普通 HTTP、绕过 host permission 或跳过 redirect/DNS 检查的开关，也不能设为全局默认。

## 6. 资源与 KV

资源只读且属于当前包：

```rune
pub async fn load_data() {
    let title = os::resource::read_text("labels/title.txt").await?.unwrap();
    let paths = os::resource::list().await?;
    os::log::info(title.clone()).await?;
    let canvas = os::screen::get_context()?;
    canvas.clear(os::screen::WHITE)?;
    canvas.text_sized([12, 20], title, os::screen::BLACK, 24)?;
    canvas.flush().await?;
    os::runtime::ok(paths.len())
}
```

资源路径最长 64 B，必须为规范化相对路径，不允许绝对路径、空 segment 或 `..`。当前每包最多 8 个 UTF-8 资源，单项正文最多 1 KiB，路径与文本合计最多 4 KiB。

KV 按 widget id 隔离：

```rune
pub async fn load_data() {
    os::storage::set("last-run", "done").await?;
    let value = os::storage::get("last-run").await?;
    if value.is_some() {
        os::log::info(value.unwrap()).await?;
    }
    os::storage::delete("last-run").await?;
    os::runtime::ok(())
}
```

Key 为 1–32 B ASCII 字母、数字、`_`、`-` 或 `.`；单 value 最多 1 KiB。KV 持久化在当前 widget 的 private state 中，删除 widget 时与整个 private namespace 一起清除。

文件系统根严格分离：

```rune
os::fs::private::write_text("cache/state.txt", "READY").await?;
let own = os::fs::private::read_text("cache/state.txt").await?;
let shared = os::fs::shared::read_text("feeds/message.txt").await?;
let paths = os::fs::shared::list("feeds").await?;
```

路径最长 96 B，必须是相对规范路径；绝对路径、反斜杠、冒号、控制字符、空 segment 和 `..` 均被拒绝。脚本 ABI 当前只读写 UTF-8 文本：单次最多 4 KiB，每次 invocation 累计 16 KiB、最多 32 次 FS call、list 最多 64 项。平台存储的 private 单文件上限 16 KiB/每组件总计 64 KiB，shared 单文件上限 64 KiB/总计 1 MiB，因此 ABI 限制更严格。管理 HTTP 可上传 shared 原始二进制，但 Rune 需要未来的 bytes ABI 才能读取非 UTF-8 内容。

## 7. 时间、RTC、电池与 Wi-Fi

这些接口只提供原始或明确可信度的读数，不虚构电池 SOC、世纪映射或网络可用性：

```rune
pub async fn load_data() {
    let uptime_ms = os::time::uptime()?;
    let unix_ms = os::time::unix()?;
    let local = os::time::local()?;
    let rtc = os::time::rtc().await?;

    let battery = os::power::battery().await?;
    let wifi = os::wifi::status().await?;

    if !rtc.valid || rtc.voltage_low {
        os::log::warn("RTC REGISTER SAMPLE IS NOT TRUSTED").await?;
    }
    if rtc.unix_ms.is_none() {
        os::log::info("RTC CENTURY MAPPING IS UNKNOWN").await?;
    }
    if battery.soc_percent.is_none() {
        os::log::info("BATTERY SOC IS UNKNOWN").await?;
    }
    if !wifi.connected {
        os::log::warn("WIFI DISCONNECTED").await?;
    }
    os::runtime::ok(uptime_ms)
}
```

`BatteryReading` 暴露 `adc_raw`、`adc_pin_mv`、`standby_pin_high`、`charge_pin_low` 以及可选 `cell_mv`/`soc_percent`。只有适配器掌握经过验证的换算时才能填写后两项。

`RtcReading` 原样暴露 `year_00_99`、`month`、`day`、`weekday`、`hour`、`minute`、`second`、`century_bit`、`voltage_low`，另提供可选 `unix_ms` 与 `valid`。`valid = !voltage_low` 只表示本次寄存器样本未被 VL 标记，不证明 RTC 长期精度；世纪映射不可信时 `unix_ms = None`，脚本不能自行猜测世纪。

`os::time::unix()` 和 `os::time::local()` 在一次 invocation 开始时取同一份已验证系统时钟
快照。`local()` 返回可选 `LocalTimeReading`，包含完整四位 `year`、月日、星期、时分秒、
`utc_offset_minutes` 和原始 `unix_ms`；系统时钟尚未由 owned RTC 或 NTP 建立时返回
`None`。固定偏移跨日/跨年由宿主完成，widget 不应自行猜世纪或重复应用偏移。当前偏移
不处理夏令时。

`os::time::lunar(year, month, day)` 把 2000..=2099 范围内的公历日期转换为
`LunarReading`。返回数值 `year`、`month`、`day`、`leap_month`，以及可直接显示的
`year_name`（两字干支）、`month_name`（含“月”，闰月含“闰”）和 `day_name`。
换算是本地纯计算，但仍受 `time.read` 能力控制。Canvas 的原 ASCII 5×7 字体保持不变，
并提供 GNU Unifont 16×16 的常用简体/日历/标点子集（3,767 字形）；直接 `canvas.text` 对子集外字符仍整帧拒绝。动态文本可先通过 `os::screen::fit_text` 裁切并替换不支持的字符。

`WifiStatus` 暴露 `connected`、`ssid` 和可选 `ip`，不包含密码、admin token 或其他小组件的网络数据。

## 8. 调度、并发与取消

每次 invocation 有 Rune instruction budget、allocator 限额、最多 64 个已启动 host calls，以及总 wall-time。`os::runtime::wait_all` 最多接收 8 个 Future，并按输入顺序返回结果；`timeout` 为单个 Future 添加 deadline。Future 只有被 await/poll 后才算启动：

```rune
let weather = os::http::get_text("https://api.example.com/weather");
let clock = os::http::get_text("https://api.example.com/time");
let results = os::runtime::wait_all([weather, clock]).await?;
```

v1 不允许同一 widget 重入。一个独立 widget worker 串行执行生命周期，host adapter 把真正的网络、显示和硬件工作交给有界 actor/queue。墙钟或 VM 失败会设置共享取消标记；尚未开始的 HTTP/硬件请求会跳过，已返回 VM 的后续 `cancelled()` 也会反映该状态。`os::runtime::timeout(future, ...)` 只停止脚本对该 Future 的等待，不会撤销已经入队或开始的 host 副作用；MVP 尚无 per-operation cancellation。已经进入 ESP-IDF 阻塞 HTTP 调用的请求只能等待其有界超时；已经开始的 EPD 刷新必须完成 BUSY 处理和安全断电，不能被强行切断。

本轮不实现多后台 VM 或额外 worker pool；显示/I2C 外设继续由单一宿主所有者管理。

## 9. 管理与验证闭环

```text
POST /packages/v1 (.pnw) -> ZIP校验/编译 -> 临时写/回读 -> 持久化替换
widget.list/get/status -> 查看 manifest、源码和状态
widget.activate/configure -> HTTP 202 + job_id
buttons/timer -> bounded foreground event queue
widget worker -> lifecycle/input hook -> os modules -> actors
                                       -> Canvas flush -> latest frame -> display
job.get -> queued/running/succeeded/failed + structured result/error
event.list -> 有界事件 ring
widget.delete -> worker tombstone -> 删除包/private -> 注册表提交
```

首次实机验收必须至少覆盖：显式 assets 初始化、二进制 `.pnw` 上传与拒绝样例、入口/ABI/capability 拒绝、未 await Future 无副作用、Canvas 四色和裁剪、HTTP permission 与 redirect、资源/private/shared 隔离、KV 与 active widget 的 RESET 恢复、删除后不复活、RTC/VL、电池、Wi-Fi、取消与 timeout，以及长 BUSY 期间管理 HTTP 仍可响应、三个按钮仍被独立采样，溢出明确计数。新旧 ABI 迁移不得重复格式化已就绪 assets。
