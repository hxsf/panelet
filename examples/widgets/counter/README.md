# 生命周期计数器

Counter 0.2.0 要求 `note4c.rune/3` 固件，演示组件宏、常驻 State、多事件绑定和脚本主动提交画面。

```sh
node examples/widgets/build-upload.mjs counter > /tmp/panelet-counter.pnw
```

设为前台后 mounted 显示 0。同一个 `#[widget::input(click(boot), long_press(up, down))]` handler 接收只读 `event`：BOOT 短按加 1，上键长按加 10，下键长按减 10。通过 `event.kind` 和 `event.keycode` 分支，`event.held_ms` 可用于读取持续时间。长按阈值约 1 秒，释放不再补发 click；多个参数不是组合键。

默认建议定时间隔保持 0；开启后 timer 每次加 1。所有入口串行处理，调用普通 `draw()` 方法，通过 `os::screen::get_context()` 绘图后 `canvas.flush().await`。没有专用 render 生命周期或注入的宿主参数。

提交成功仅表示快照已接收。宿主合并约 200ms 内的更新并按像素去重，变化时自动选择局部窗口或全刷，详见[差异窗口策略](../../../docs/devices/note4c/screen-partial-refresh.md)。独立采样可在显示刷新期间接收按键；队列上限 32，超出时丢弃新事件并计数。管理页分别报告脚本与物理显示状态。

取消前台、更新配置或重启后重新构造 State，计数回到 0；没有持久化计数。诊断模式在管理页显式开启，正常前台按键不进入测试菜单。[完整契约](../../../docs/os/widget-lifecycle-v1.md)。
