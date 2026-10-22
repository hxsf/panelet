# 管理页本机预览 · ABI 3

2026-10-22 的 Panelet 更名只完成软件验证与目标构建，本次没有重新刷写或验收真实设备；下文历史记录不能作为新构建已经上板的证明。

需要执行小组件并预览真实像素时，使用[小组件画面预览与本机编辑器](widget-editor.md)
（`scripts/run-widget-editor.sh`，默认端口 18745）。本文的 Python 服务仅用于管理页交互模拟。

2026-09-09 的页面增加了 ABI 3 编辑模板、显式诊断模式、输入统计和提交/呈现状态，当时固件已完成目标构建、刷写与 [限定范围实机验证](../devices/note4c/history/abi3-hardware-log-2026-09-09.md)。预览仅模拟管理接口，不能验证 Rune、刷新时序或真实按键。

## 打开预览

```sh
python3 scripts/preview-server-ui.py --port 18744
```

浏览器打开 `http://127.0.0.1:18744`。首次自动使用 `preview-token`，清除登录后可在连接设置中重新输入。模拟器只绑定回环地址，所有写操作作用于内存 fixture，不连接设备或 NAS；重启后重置。此服务模拟管理接口，不运行 Rune，也不模拟屏幕硬件。

旧 Python 模拟器尚未实现 `/editor` 和 `/web/v1/*`。当前管理页新增的「预览资源」卡片
与浏览器编辑器入口无法在这套 fixture 中验收：资源状态请求会失败，编辑器入口不可用。
真实 WASM 与资源安装／恢复流程的本机验证见 [assets 资源的本机验证](../os/web-assets.md#本机验证)。

## 页面与接口

- 设备总览、小组件、代码编辑、网络与时间、文件与存储、硬件诊断、任务记录、连接设置分别导航；移动端导航横向排列。
- 总览自动请求 `dashboard.get`，合并资源初始化状态、小组件数量、脚本处理状态及前台项。该 RPC 沿用 token 认证，不返回小组件配置凭据；旧固件缺少该方法时回退至独立查询，无法确认的运行状态显示未知。
- 进入相应菜单自动加载列表和设置。异步操作在按钮附近显示等待、完成或失败，并记录到任务页；HTTP 接受不等于 job 成功。
- Admin token 和最后菜单按设备地址保存在浏览器 localStorage；连接页可清除。TODO 密码、Bearer token 与其他小组件配置不写入 localStorage。
- 代码编辑取回源码及 UTF-8 包资源，生成 ABI 3 STORE-only `.pnw`，无 entry 字段。新建模板采用 component/new/mounted、多输入绑定及普通 draw，使用 os 模块和 canvas.flush。assets 初始化保持显式确认，不随进入页面执行。
- TODO 0.4.0 表单支持无认证、Basic 和 Bearer；认证要求 HTTPS。Basic 编码在 Rune host 完成，旧 Bearer 配置保持兼容。

- 诊断模式通过 `system.mode.set` 的 normal/diagnostic 按钮切换；仅访问菜单不切换，正常模式禁用硬件测试操作。进入诊断保留前台选择，返回后重新进入组件。
- 总览每 2 秒在可见页面读取状态，分别展示脚本处理、最新 submission_id、物理 refreshing，以及 idle/pending/presenting/presented/unchanged/failed。显示输入 captured/pending/overflow，溢出和呈现错误明确提示。
- “重试最新画面”调用 `screen.retry {}`，提交重试请求成功不等于画面呈现完成。预览模拟 pending → presenting → presented/failed，可验证失败后显式重试。
- 修改预览 Python 后需要重启服务；HTML 则随每次请求读取。重启清除全部模拟编辑，不影响设备。

HTML 保持单一源文件。构建时 `scripts/build-server-web.py` 生成确定性 gzip，固件通过 `Content-Encoding: gzip` 返回，减少新增 UI 的 Flash 占用。Python 3 是该打包步骤的依赖；实际镜像尺寸须在 UI 验收后的目标构建中核对。

## 本轮已完成的软件检查与待验收项

- JS 语法、114 个 HTML ID 与字面量引用检查通过。
- 独立 JS 执行验证六种呈现状态、诊断按钮门控、三个新按钮的准确 RPC 参数，并检查菜单导航不修改模式。
- Python 语法及内存 mock 的模式切换、提交、刷新、失败、重试、取消前台状态迁移通过。
- gzip 往返检查通过；页面压缩约 21 KiB。最终固件体积仍以目标镜像为准。

本轮还在本机浏览器验证了：进入诊断菜单仍保持普通模式、显式进入/退出诊断模式及按钮门控、无 entry 的 ABI 3 编辑模板、总览分开展示脚本与呈现状态。设备上的三键、呈现和启动恢复结果另见 ABI 3 实机记录。本轮没有重新完成全部手机布局验收。预览场景下拉框可切换正常、未初始化、任务失败和连接中断，控件仅在本机预览出现。

上一版已验收总览密度、菜单、按钮旁反馈、TODO Basic 配置及手机布局；历史 47 项管理/Rune 测试和此前浏览器结果不替代本次 ABI 3 验收。真实 NAS Basic 需要对应服务端环境。

## TLS 边界

设备对外 HTTPS 请求已接入 ESP-IDF 证书包，默认验证证书。`http.insecure_tls` 仍要求 manifest 和单次请求双重显式授权；TODO 不关闭证书验证。当前没有私有 CA 导入配置。管理服务自身仍是 HTTP，与小组件对外 HTTPS 是两个独立方向。

上一版 UI 验收后的刷写、前台/Wi-Fi 实测和镜像尺寸见 [2026-09-09 硬件记录](../devices/note4c/history/lifecycle-hardware-log-2026-09-09.md)。
