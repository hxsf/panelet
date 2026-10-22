# 小组件画面预览与编辑器

2026-10-22 起，设备管理页的 `/editor` 已接入纯浏览器 WASM 预览：页面、JS/CSS 随
固件；大型 WASM、示例和字体许可资料放到独立版本的 assets 资源包。兼容的 A/B 固件
可以共用同一资源版本，安装与版本规则见 [assets 资源](../os/web-assets.md)。本轮仅完成
软件和目标构建验证，按用户要求暂不实机部署。

以下原生桌面编辑器仍保留，用于开发、离线验证与原生/WASM 像素对照。

`panelet-studio` 是 `app/os` 内的独立 macOS / Linux 程序。它执行与固件相同的
Rune ABI 3、生命周期、字体和 Canvas 实现，读取 400 × 300、30,000 字节的
MSB-first 2 bpp 帧缓冲，在浏览器显示黑、白、黄、红四色预览。

## 启动与使用

需要 stable Rust 和系统 C/C++ 编译工具，不需要 ESP-IDF 工具链或连接设备：

```sh
scripts/run-widget-editor.sh
# 可选端口
scripts/run-widget-editor.sh --port 18746
```

启动后在浏览器打开 <http://127.0.0.1:18745>。程序只监听本机回环地址，Ctrl-C 退出。
设备管理页的「代码编辑」入口现在打开同设备 `/editor`，不再依赖该本机端口。
启动脚本优先使用 rustup stable，也兼容已安装 stable 工具链但 rustup 启动器缺失的环境；
macOS 已安装独立 Command Line Tools 时默认使用它，不改变系统的 Xcode 选择。

1. 载入计数器、日历、待办或相册示例，或导入 `.pnw` 包；旧 `.n4w` 包仍接受。
2. 编辑源码、manifest 和 UTF-8 资源，按需设置配置、时间和模拟数据。
3. 点击「重新预览」运行 mounted；用三键短按／长按或定时事件检查交互。
4. 导出 PNG 检查画面，或导出 `.pnw` 后在设备管理页上传。

设备上的已有组件可先在管理页载入编辑器，再「导出 .pnw」，到本机编辑器导入。旧版本管理页导出的 `.n4w` 也可直接导入，无需修改设备存储或包内容。
编辑器使用同一个严格包解析器和组件验收规则。导出检查包结构；Rune 编译与执行结果
由「重新预览」提供。组件安装配置和模拟数据不属于 `.pnw`，导出不保存这些输入。
页面编辑仅保留在当前浏览器页面，关闭前须导出需要保存的包。

## 预览语义

- 每次请求创建隔离会话，从 mounted 开始重放输入，单个请求最多 64 个事件。
  按键保留会话 State 和内存存储；重新预览、修改源码或模拟环境会清空事件历史。
- 每个事件可推进虚拟时间，范围 0～31 天；不会自动运行后台定时器。
  `sleep_ms` 立即完成，不推进时钟。短按 `held_ms=50`，长按 `held_ms=1000`。
- 网络仅按完整 URL 匹配用户提供的 HTTP 响应，仍检查 capability 和 manifest／配置
  的 host permissions。未配置响应时返回明确错误，不访问真实网络。
- resources 来自包；KV、private、shared 文件全部位于请求内存。shared 可指定初值，
  不读写电脑文件、设备或 NAS。电池、RTC 和 Wi-Fi 硬件调用返回 `unavailable`；
  `os::time` 的 Unix／本地日期来自模拟时间。
- 浏览器直接解码固件产生的打包像素，不另实现文字栅格化。脚本失败时，已接受的最新
  flush 仍可显示；没有提交时显示「没有提交画面」，不会把旧画面当作新成功结果。
- 屏幕颜色采用浏览器标准色。预览不验证真实屏幕颜色、BUSY、刷新时间、耗电或显示寿命；
  主机分配器、平台调度与设备不同，也不能用预览证明设备 RAM 峰值或硬件服务可用。

编译使用 1 MiB Rune 分配限额，执行使用 64 KiB 分配限额和 100,000 指令预算，与当前
固件一致。每事件另有 2 秒墙钟限制，每次 HTTP 预览在子进程执行，超过 10 秒终止。
HTTP 输入上限 64 KiB；只接受本机 Host 和同源浏览器请求。Rune 分配限额不是整个进程
的内存上限。

## 编译隔离与容量

固件的 `firmware` feature 默认开启，普通固件构建仍得到完整 server。
`desktop-editor` 显式开启独立 bin，设备依赖按 `target_os = "espidf"` 限定。
原生预览执行模块不进入设备 bin。当前设备会内嵌浏览器编辑器 HTML/JS/CSS，但大型
WASM 独立存在 assets 中，且不会在设备上执行。
`scripts/build-mvp.sh` 显式选择固件 bin，避免将桌面程序作为固件构建目标。

```sh
NOTE4C_HOST_TARGET="$(rustc +stable -vV | sed -n 's/^host: //p')"
cargo +stable build -p panelet-os --no-default-features \
  --features desktop-editor --bin panelet-studio --target "$NOTE4C_HOST_TARGET"
cargo +stable test -p panelet-os --no-default-features \
  --features desktop-editor --bin panelet-studio --target "$NOTE4C_HOST_TARGET"
```

以下为 2026-09-22 历史尺寸，不代表后续 assets 功能构建。改动前，`c0747b6` 默认固件 app image 为 **3,731,760 B**，
OTA 槽 **4,128,768 B**，剩余 **397,008 B**。原有 `opt-level=z`、LTO、
单 codegen unit、管理页 gzip 已启用。本次选择独立程序，既保留余量，也避免预览占用
设备组件会话或触发实体屏幕。

最终目标构建 app image 为 **3,732,080 B（90.39%）**，较基线仅增加 **320 B**，
余 **396,688 B**。管理页 gzip 从 21,426 B 增至 21,719 B；独立编辑器路由、HTML、
模拟资源未进入固件镜像。目标编译、非 merge app image 生成及用户授权的 app-only
刷写已完成，启动与管理页验证及待办拉取异常见 [2026-09-22 实机记录](../devices/note4c/history/editor-hardware-log-2026-09-22.md)。

设备端执行预览的隔离容量实验另见 [历史测量记录](../devices/note4c/history/widget-preview-size-2026-09-22.md)。
该实验只测量 Flash，不属于当前浏览器 WASM 实现，也未进行实机运行验收。

2026-09-22 验证：macOS Apple Silicon 的完整 host workspace（排除 server）171 项
测试、editor bin 12 项测试通过；另有 5 项真实 HTTP／worker、包往返和恢复、请求边界及
启动脚本 mock 测试通过，入口为 `scripts/tests/test-widget-editor.py`：

```sh
python3 scripts/tests/test-widget-editor.py \
  --bin "target/$NOTE4C_HOST_TARGET/debug/panelet-studio" -v
```

Linux 使用同一 `std` 实现和显式 host target 构建路径，本轮未在 Linux 机器执行构建。
浏览器检查覆盖初始画面、短按／长按、重新预览、中文日历和 flush 后报错的画面保留；
Chrome 导出的 PNG 已核对 400 × 300，`.n4w` 已核对 CRC、完整源码和 manifest。

这与 [管理页本机预览](server-ui-preview.md) 不同：旧 Python 服务在 18744 端口
模拟管理接口；本编辑器在 18745 端口执行真实 Rune 小组件。
