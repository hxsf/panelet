# 可跨固件共享的 assets 资源

2026-10-22：已完成软件实现、主机／浏览器验证与目标构建；按用户要求，本轮没有连接、
刷写或上传到真实设备。设备实际剩余空间、SPIFFS 写入耗时和物理断电恢复仍待验证。

## 存放边界

浏览器预览的 Rune 编译器、VM、Canvas 和字体编译为 WASM，作为 gzip 静态文件存入
已有 `assets` SPIFFS。浏览器下载后在 Web Worker 中执行，设备只读文件并用 1 KiB
缓冲区流式发送，不执行预览脚本。分区布局继续使用双 OTA + 8 MiB assets。

管理页、编辑器 HTML、页面 JS/CSS 与 Worker 脚本仍随固件编译，因为它们容易随设备
接口变化。资源缺失或损坏时，内嵌管理页仍能登录、管理设备并重新安装资源。

| 内容 | 当前处理 | 原因 |
|---|---|---|
| 浏览器 WASM 引擎 | assets | 体积大、接口稳定后更新少，兼容固件可共享 |
| 四个示例与模拟数据 | assets | 与预览引擎兼容版本配套，可独立更新 |
| 字体来源／许可文本 | assets | 静态资料，可跨固件共享，随含字体的 WASM 一起分发 |
| 管理页／编辑器 HTML、JS、CSS | 固件 | 与管理接口和页面流程联动，不因少量节省拆开版本 |
| 设备端 CJK 字库（128,078 B） | 暂保留固件 | 下一项值得外置的资源；需要先增加可验证的字库格式、加载与缺失回退接口 |
| 设备端 ASCII 字库（1,520 B）和基本字形 | 固件 | 体积小，配网／恢复画面必须不依赖 assets 可用 |
| 设备显示时序、驱动、TLS 信任配置 | 固件 | 属于启动／硬件／信任边界，不作为可替换网页资源 |
| 用户 `.pnw`、KV/private/shared 文件 | 既有 assets 命名空间 | 原本就在资源分区，本功能不搬动或重新格式化它们 |

后续大图标、背景图片和音频素材也适合使用同类稳定资源；当前固件没有这些需要立即
搬迁的大文件。归档第三方固件中的素材不等于本固件已经嵌入，未把归档内容算作节省。

## 简单兼容版本与 A/B 共存

`app/os/web-assets-api-version` 是当前固件需要的资源兼容版本，目前为 `1`。
它独立于固件版本和 A/B 槽：

- A、B 都兼容资源 v1 时，读取同一份 v1；普通固件升级不重建或重复上传 WASM。
- 只有资源接口不兼容时才改为 v2，并把 v1 留给仍使用它的固件。可先安装 v2 再更新固件。
- 同一兼容版本内修正引擎或示例时，更新内容即可，不需要递增兼容版本；使用内容哈希
  区分修订和浏览器缓存。SHA-256 文件按内容去重，兼容版本之间也可共享相同文件。
- 每个兼容版本保存两份带 generation／checksum 的小索引用于恢复；它们不是为 A/B
  各复制一份 WASM。更新只替换较旧索引，新包完整验证前当前包始终保留。
- 不自动删除其他兼容版本。当前上限为 4 个兼容版本；未来清理旧版本前必须确认回滚
  固件已不再依赖它，不能简单按最近更新时间删除。

资源入口先获取指定兼容版本的清单，再使用 `/web/<bundle_id>/<path>` 固定到该修订。
这些哈希 URL 可以长期缓存；页面和当前版本查询则 `no-store`。未安装对应兼容版本时
明确报错，不悄悄使用另一版本。

## 构建与安装

仅在预览引擎、示例或资源契约变化时构建资源：

```sh
scripts/build-preview-wasm.sh
python3 scripts/build-web-assets.py
```

产物为 `target/web-assets/bundle.pna`（Panelet Assets），旁边保存 manifest 和大小／哈希摘要。旧 `.n4a` 包仍可安装；后缀更名不改变 `N4A1` 容器格式或资源 API 版本，完整映射见[名称与兼容性](../development/naming-and-compatibility.md)。
默认版本从上述兼容版本文件读取；准备不同兼容版本时可显式使用 `--api-version N`。
常规 `scripts/build-mvp.sh` 只构建固件及随固件的网页，不构建或嵌入 WASM。

设备安装新固件后，在「文件与存储 → 预览资源」选择 `.pna`（也接受旧 `.n4a`），点击安装。已安装相同包时
管理页和 CLI 都只读查询状态，不发写请求。取消／中断保留当前已提交资源，重新选择同一
包可从已确认的偏移续传；确认提交阶段不提供误导性的“取消已提交结果”。

开发部署也可以使用已有 Admin token 文件，避免把 token 放进命令行参数：

```sh
python3 scripts/deploy-web-assets.py \
  --url http://DEVICE_ADDRESS \
  --token-file /PRIVATE/PATH/admin-token.txt \
  --bundle target/web-assets/bundle.pna
```

安装器先校验完整包的 SHA-256，再联系显式指定的设备；不跟随重定向，不格式化存储，
不改分区、NVS、bootloader 或 app。写入 assets 本身仍是设备 Flash 写入，本轮未执行。

## 更名前的构建测量（历史）

以下为 2026-10-22 Panelet 更名前的本地产物，不是新名称构建的尺寸，也不是实机剩余空间。重命名和字体通知补齐后的数据另见[更名验证记录](../development/panelet-migration-2026-10-22.md)：

| 项目 | 大小 |
|---|---:|
| 设备 app image | 3,826,160 B / 4,128,768 B（92.67%） |
| 应用槽余量 | 302,608 B |
| 相对上一已刷写固件增加 | 94,080 B（资源管理、文件服务及固件内网页） |
| 浏览器 WASM 原始文件 | 3,016,668 B |
| assets 中的 gzip WASM | 913,381 B |
| 示例 gzip／字体许可 gzip | 4,742 B / 8,934 B |
| 完整 `.n4a` 包（含 manifest） | 927,638 B |

确认原始 WASM 和 gzip WASM 均不在设备 app image 中。app SHA-256 为
`906d2b11cfd1476a3cdc0c748f6936f0816d9beb2ec3546e40d06b3eba6fc942`；资源包 SHA-256 为
`d74995efea5a56e0bd212a089e4c435ae53ba7925fb9685b36aea68dcf0a35d7`。

本机沙箱预装相同内容的资源 v2 后，内容文件数量不变，当前 v1 继续可用；管理页回退
v1 后 v2 仍被保留。浏览器实际运行计数器和中文月历通过。文件选择自动化受 Chrome
扩展文件权限限制，安装通过同一 HTTP 协议的 CLI 完成；页面上传／取消／续传事件另由
真实 Blob 的 Node 测试覆盖。没有修改浏览器扩展权限。

## 存储与传输契约

- `.pna`（旧 `.n4a` 同格式）：`N4A1` 魔数、4 字节小端 manifest 长度、UTF-8 manifest，然后按路径排序的
  原始存储字节（本包各文件已 gzip）。schema v1，manifest 最大 16 KiB。
- 最多 24 文件，单文件最多 1536 KiB，包内文件合计最多 2 MiB。上传每请求最多 32 KiB，
  使用文件索引＋offset，可幂等重试。一次保留一个 pending 上传。
- 每文件 SHA-256 校验、同步和回读；全部文件验证后才发布索引。完整 SHA 和 size
  会检查文件名短哈希冲突。物理名不超过 SPIFFS 的 31 字节限制。
- 启动重新验证索引与数据，损坏新版时可选用同一兼容版本的有效旧版；不跨兼容版本回退。
- 垃圾清理只处理 `wa-*` 自有内容命名空间，并保留所有兼容版本的索引引用与 pending。
- 写前使用 `esp_spiffs_info` 的实际 total/used；保持至少 25% 报告容量用于 GC 和其他数据。
  容量查询与 begin/chunk/commit/rollback 在同一个存储锁内，后台组件不能消耗过期预算。
- 页面带允许 WASM 的 CSP；公共资源单独使用 sandbox CSP，上传 HTML/SVG 不会获得管理
  页同源脚本权限。写接口均要求现有 Admin token 和同源请求。

| 接口 | 鉴权 | 作用 |
|---|---|---|
| `GET /web/v1/status` | Admin | 真实空间、当前／其他兼容版本、pending 和问题 |
| `GET /web/v1/manifest?api=N` | 公共 | 选择指定兼容版本，返回 bundle_id 和文件元数据 |
| `GET /web/<bundle_id>/<path>` | 公共 | 只读流式资源，MIME、gzip、ETag／304、immutable 缓存 |
| `POST /web/v1/begin` | Admin | 验证 manifest、检查空间、准备或恢复上传 |
| `POST /web/v1/chunk?bundle=...&file=...&offset=...` | Admin | 原始二进制块，回读确认并返回完整进度 |
| `POST /web/v1/commit` | Admin | 全量校验后发布；已发布的同包可幂等确认 |
| `POST /web/v1/rollback` | Admin | 恢复当前固件兼容版本的上一份有效资源 |

## 本机验证

保留原生桌面编辑器，并新增显式 assets 沙箱：

```sh
scripts/run-widget-editor.sh --web-assets /tmp/note4c-assets --port 18747
```

此模式用真实本机文件和同一个资源存储模块，容量明确模拟 8 MiB／75% 安全线；开发
token 为 `preview-token`。页面使用真实 WASM，所有原生 render/package 接口均关闭。
它不能证明设备剩余容量、SPIFFS 物理断电保证或 Wi-Fi 下载速度。

本轮验证通过：host workspace 190 项、桌面编辑器 12 项、WASM 原生像素／行为对照
19 组、浏览器逻辑 14 项、资源 HTTP 9 项、构建／部署工具 5 项、原生 HTTP 回归 5 项。
最终 ESP32-S3 release 构建通过，0 warning。

```sh
NOTE4C_HOST_TARGET="$(rustc +stable -vV | sed -n 's/^host: //p')"
cargo +stable test --workspace --exclude panelet-os --all-features --target "$NOTE4C_HOST_TARGET"
cargo +stable test -p panelet-os --no-default-features --features desktop-editor \
  --bin panelet-studio --target "$NOTE4C_HOST_TARGET"
node scripts/tests/test-preview-wasm.mjs --bin "target/$NOTE4C_HOST_TARGET/debug/panelet-studio"
node scripts/tests/test-browser-preview.cjs
python3 scripts/tests/test-web-assets-tools.py
python3 scripts/tests/test-web-assets-http.py \
  --bin "target/$NOTE4C_HOST_TARGET/debug/panelet-studio" --bundle target/web-assets/bundle.pna
```

WASM 在 Worker 内运行，下载最多 45 秒，下载完成后的编译／执行最多 10 秒；终止会回收
整个 Worker。Rune 编译 1 MiB、执行 64 KiB、100,000 指令预算保留，WASM 线性内存上限
64 MiB。这些都是浏览器侧限制，不计入 ESP32 的运行内存。
