# 通用拉取待办小组件

每次运行执行一次 HTTP GET，验证完整 JSON 快照，显示前 6 条未完成事项。固件负责定时再次调用与按像素去重；小组件没有无限循环，也不在每次轮询时写 Flash。

## 安装和配置

当前源码为 0.4.0，要求 `note4c.rune/3` 固件。使用 `os` 模块和组件宏，旧 ABI 固件不能执行此包；迁移保留组件 ID、配置和授权。Basic 认证仍要求 HTTPS。旧版本的硬件记录不能替代本版实测。

```sh
node examples/widgets/build-upload.mjs todo > /tmp/panelet-todo.pnw
```

在管理页上传 `.pnw`，进入“小组件 → TODO → 配置”：

- **API 地址**：返回下述 JSON 的 HTTP(S) 地址。
- **检查间隔**：建议 300 秒；0 为关闭自动检查，最小 60 秒。
- **认证方式**：无认证、Basic（用户名与密码/应用密码）、Bearer Token。使用认证时必须为 HTTPS，API 不得重定向；Basic 由程序编码，无需手填 `Authorization`。
- Basic 用户名不得为空或含冒号；用户名/密码不得含控制字符，合计最多 1024 UTF-8 字节。旧配置中的非空 `bearer_token` 仍兼容。
- **屏幕标题**：默认“待办事项”。
- **授权访问地址**：根据 API 地址生成，确认它是本组件允许访问的 origin/path。

保存成功后点击“设为前台”。mounted 自动执行首次检查；BOOT 短按和 timer hook 都调用同一个普通 refresh 方法。API 地址、token 和间隔都独立持久化，更新包不需要重新配置。管理页面关闭后轮询仍由设备执行。

不要将真实 token 写进源码包或示例文件。当前固件管理接口与存储未加密，配置只适用于可信开发网络。若设备未初始化 assets，应按既有恢复与存储流程处理；本例不会自动格式化。

## BOOT 输入与画面提交

正常模式下短按释放 BOOT 会排入一次检查事件；上、下键及 BOOT 长按未绑定。事件串行处理，定时检查由宿主合并；按键不会穿透进入诊断菜单。诊断模式须在管理页显式进入，期间前台暂停。

按键采样独立于显示刷新，刷新期间仍可采集。输入队列最多 32 项，满时丢弃新事件并报告溢出；长按后释放不再产生短按。BOOT + RESET 的 ROM 恢复路径保持不变。

脚本使用 `canvas.flush().await` 提交画面，成功只表示宿主接收了快照。宿主合并、按像素去重并安排全刷；管理页的事件完成与物理呈现状态分别报告。刷屏失败不会把已成功的 HTTP 检查变成脚本失败。

## API 契约

这是统一的待办 feed 格式，不是任意第三方 API 的自动适配器。Apple Reminders、Todoist 或其他来源由外部服务转换；本组件不绑定它们的认证或 JSON 结构。

HTTP 200，UTF-8 JSON，完整响应最多 16 KiB：

```json
{
  "version": 1,
  "items": [
    {"text": "买牛奶", "completed": false, "important": true, "detail": "今天 18:00"},
    {"text": "提交项目报告", "detail": "明天"}
  ]
}
```

- `version` 必须为整数 1，`items` 必须是数组，最多 64 项。
- `text` 必填，1～512 UTF-8 字节。
- `completed`、`important` 可省略，默认 false；提供时必须为 boolean。
- `detail` 可省略，默认空串；最多 256 UTF-8 字节。
- 保持服务端顺序，过滤 completed 后显示前 6 项；important 使用红色。截止日期、时区、排序及循环事项由服务端决定，`detail` 仅为显示文本。
- `items: []` 是有效空清单，会显示“暂无待办”。缺字段、错误类型、异常 HTTP 状态和网络错误均使 job 失败，不提交新画面。
- 不使用 ETag/304；每次必须返回完整快照。像素不变时固件跳过物理刷新。画面不显示每次检查的时间，避免时间戳导致无效全刷；检查状态通过 job/event 查询。

每项两行：标题 18px，备注 16px 并缩进；没有 `detail` 时第二行留空，不是自动换行。每屏最多 6 条，页脚显示当前展示数/未完成总数。字库覆盖 3,767 个汉字/标点；英文字母和数字使用放大的 10×14 点阵、大写字形，使实际可见高度接近中文。补充标点使用 GNU Unifont。长行按像素截断为 `...`，不支持的字符（例如 emoji、生僻字）显示 `?`，不承诺完整 Unicode 覆盖。

此版使用 `canvas.text_sized` / `os::screen::fit_text_sized`；旧 `canvas.text` 字体和日历排版保持兼容。

[feed.example.json](feed.example.json) 可作为静态 API。调试时可在电脑上运行：

```sh
python3 -m http.server 8080 --directory examples/widgets/todo
```

将 URL 配为 `http://<电脑局域网IP>:8080/feed.example.json`，留空 token；电脑需与设备互通。测试文件只含示例事项，修改它可验证内容变化和去重。不要将真实待办或凭据放进此仓库或公开静态目录。

接口和调度的完整说明见 [配置与定时运行](../../../docs/os/widget-configuration-and-polling.md)。轮询不包括休眠唤醒，不等同于低功耗模式。

## 验证

主机测试执行真实 Rune 源码、模拟 HTTP/显示 adapter，验证认证头、数据错误不呈现、空清单、行数与 UTF-8/像素预算；渲染检查使用与固件相同的 Canvas 和字体。可输出预览：

```sh
NOTE4C_TODO_PREVIEW=/tmp/panelet-todo-preview.ppm \
  cargo +stable test -p panelet-rune-abi-poc \
  --target "$(rustc +stable -vV | sed -n 's/^host: //p')" todo_rendered_frame
```

这些结果不代表设备端 HTTPS、周期时序、恢复或屏幕效果已验证。

从 iOS 提醒事项生成文件并上传 WebDAV，见 [快捷指令配置步骤](../../../docs/guides/ios-reminders-webdav.md)。
