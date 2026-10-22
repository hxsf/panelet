# 小组件生命周期 v1 · ABI 3

本文是 `note4c.rune/3` 的实现契约。运行时、固件和示例包迁移已完成；初次 ABI 3 验证为 167 项主机测试，后续自动差异窗口刷新版本为 [171 项及实机验证](../devices/note4c/history/partial-refresh-hardware-log-2026-09-09.md)。目标构建通过；三键、显示提交、模式切换和启动恢复的实测范围见 [ABI 3 实机记录](../devices/note4c/history/abi3-hardware-log-2026-09-09.md)。

v1 只有一个前台会话，通过宏声明生命周期，不引入 trait、动态订阅、多后台 VM、拼页或状态磁盘快照。用户不编写入口 `main`，宿主也没有特殊 `render` hook。

## 源码契约

完整示例见 [计数器](../../examples/widgets/counter/main.rn)。

```rune
struct Counter { count }

#[widget::component]
impl Counter {
    pub fn new() { Counter { count: 0 } }

    #[widget::mounted]
    pub async fn mounted(self) {
        self.draw().await
    }

    #[widget::input(click(boot), long_press(up, down))]
    pub async fn on_input(self, event) {
        if event.keycode == os::input::DOWN {
            self.count -= 1;
        } else {
            self.count += 1;
        }
        self.draw().await
    }

    pub async fn draw(self) {
        let canvas = os::screen::get_context()?;
        canvas.clear(os::screen::WHITE)?;
        canvas.text_sized([20, 30], self.count.to_string(), os::screen::RED, 32)?;
        canvas.flush().await?;
        os::runtime::ok(())
    }
}
```

`new()` 是必需的同步零参数状态构造函数，只构造数据，不提供有效的宿主调用上下文。一个包只定义一个 component；manifest 使用 `abi: "note4c.rune/3"`，不再接受 `entry`。宏生成内部入口，用户不要定义或调用 `__note4c_component_*` 保留名称。

| 宏 | 签名与结果 | 调用时机 |
| --- | --- | --- |
| `#[widget::mounted]` | 可选，`async fn name(self)` → `Result<()>` | 新会话创建状态后一次 |
| `#[widget::input(click(boot), long_press(up, down))]` | 可选，`async fn name(self, event)` → `Result<()>` | 对应输入事件 |
| `#[widget::timer]` | 可选，`async fn name(self)` → `Result<()>` | 前台组件的定时检查 |
| `#[widget::before_destroy]` | 可选，`async fn name(self)` → `Result<()>` | 正常释放前的有界清理，最多 5 秒 |

每个方法只声明一个 hook 属性，input 属性内部可以组合多项绑定。`long_press(up, down)` 是两个独立绑定，不是组合键。同一 `(keycode, kind)` 重复绑定、未知名称、参数数量及同步/异步要求错误在编译时拒绝。Rune 为动态语言，结果类型仍在执行时检查。

`event` 只读，包含 `keycode`、`kind`、`held_ms`。按键常量为 `os::input::{UP, DOWN, BOOT}`，动作常量为 `CLICK / LONG_PRESS`。短按在释放时产生 click；达到长按阈值触发一次 long_press，释放不补 click。held_ms 从消抖后的按下计时。

## 宿主能力与画面提交

`os` 是全局可访问、也可导入的宿主能力模块，例如 `use os::screen;`。它不保存组件状态；网络和存储函数仍有副作用。生命周期不注入 `std`、`ctx` 或 Canvas。完整能力和限制见 [运行时契约](widget-runtime-mvp.md)。

脚本在任意常规 hook 中主动取得 Canvas、绘图并 `flush().await`；普通 `draw` 方法只供脚本自己复用，宿主不识别它。字段修改和 hook 返回都不会自动刷新画面。

- 同一次 invocation 的 `get_context()` 指向同一工作画面，继承本会话最后成功提交的快照；新会话为白底。
- flush 接收不可变快照，返回 `submission_id`；完成只表示接收成功，不表示物理显示完成。后续修改 Canvas 不影响旧快照；未 flush 的修改在 invocation 结束时丢弃。
- 首次待提交画面建立固定 200ms 合并窗口，后续提交替换最新帧但不延长窗口。正在全刷时只保留最新待呈现帧。
- 宿主对最终像素做指纹去重，变化时按 [差异窗口策略](../devices/note4c/screen-partial-refresh.md) 选择 PTLW 或全刷；清理也成功后才能记录已显示身份。失败保留最新目标，不忙循环重试；新提交或 `screen.retry` 触发下一次尝试。
- 四份 30,000 字节缓冲组成有界提交池，另有一份 30,000 字节的成功显示基线；大像素区放 PSRAM，小引用计数/锁/队列留内部 RAM。invocation 撤销后释放工作缓冲，旧 Canvas 句柄不能占有像素区。

已经成功接受的 flush 不因后续脚本失败回滚。显式切换前台、配置重建、包替换或进入诊断模式则使旧待呈现帧失效；已经开始的物理刷新允许安全完成。

## 会话、输入和退出

前台会话保留编译结果、VM 和 State，串行执行事件。每次调用有独立指令、宿主调用和墙钟上限；Rune 分配额度跨事件保留，不能通过不断发事件重置预算。这不是覆盖所有 Rust 分配、引用环和碎片的完整 OOM 沙箱。

每个 VM 的宿主入口绑定当前 invocation 的权限、配置、取消标记与配额，不使用进程级“当前组件”全局变量。Future 创建时捕获调用身份，首次执行时检查并计入配额；未 await 不产生 IO。调用结束撤销旧 Future/Canvas 等操作句柄，不能保存它们跨事件访问设备。

handler 错误或超时使会话作废、旧待处理输入失效；后续新事件重新创建会话，不继续使用部分修改的 State。已接受画面仍保留。配置更新持久化后销毁重建，使用新权限和配置执行 new/mounted。

独立采样任务持有三个 GPIO 输入，按 20ms、3 样本消抖、约 1 秒长按阈值工作，不执行脚本或显示。32 项 FIFO 在 HTTP/全刷期间仍采集；队满丢弃新事件并累计 overflow。输入不合并为同一个运行任务；定时事件最多保留一项。切换代次后丢弃旧输入，跨切换按住的键须先释放再识别。

正常卸载调用 before_destroy，随后释放状态；清理允许有界 IO，但不能再提交画面。失败会记录并继续释放，断电、复位或损坏的 VM 不保证执行。重要数据显式使用 KV/private 文件；v1 不提供挂起快照。

## 前台、诊断和迁移

管理页提供“设为前台 / 取消前台”，两种选择均持久化。启动状态页后自动进入所选组件，间隔为 0 也会 mount；安全模式不自动进入。

正常模式三个键只交给前台组件，未绑定事件忽略，不穿透测试菜单。`system.mode.set {mode:"diagnostic"}` 显式暂停前台并进入诊断；`{mode:"normal"}` 返回并重建所选会话。访问诊断网页本身不改模式。模式不持久化，重启正常进入；ROM/USB 恢复路径不变。

ABI 3 不保留旧 `main(std)`、注入式生命周期、render hook 或 bool 重绘返回值兼容层。迁移前导出设备包、配置和前台选择到受控目录；新固件拒绝执行旧 ABI，保留其持久化数据。按原 ID 更新新包，保留配置、网络授权、间隔、资源、KV/private 和前台选择。不得格式化存储来处理 ABI 不兼容。
