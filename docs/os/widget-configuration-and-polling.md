# 小组件配置与定时运行

2026-10-22 的 Panelet 更名只完成软件验证与目标构建，本次没有重新刷写或验收真实设备；下文历史记录不能作为新构建已经上板的证明。

本文描述 ABI 3 配置与调度契约，实现、主机测试及目标构建已完成，2026-09-09 的实测见 [ABI 3 实机记录](../devices/note4c/history/abi3-hardware-log-2026-09-09.md)。2026-09-08 的 [实机记录](../devices/note4c/history/todo-hardware-log-2026-09-08.md) 属于历史版本，不证明新输入/提交调度已上板通过。

## 配置归属

安装配置与 `.pnw` 源码包分离，按 widget ID 保存在 SPIFFS private 索引。包更新保留配置；删除包时随私有数据一起删除。配置不占用脚本 KV key，也不能被其他 widget 的 KV/文件接口访问。

管理页的“已安装小组件 → 配置”提供检查间隔、网络授权与配置对象；TODO 提供 API 地址、标题、无认证/Basic/Bearer 的专用表单。ABI 3 的 TODO 0.4.0 使用相同认证配置字段。设置完成后设为前台；页面关闭不影响运行。间隔设为 0 关闭定时检查，但进入前台仍会首次加载。前台选择和取消均持久化；配置更新销毁重建会话，详见 [生命周期 v1](widget-lifecycle-v1.md)。

配置接口沿用管理 HTTP 的认证与 Origin 检查。配置可能含凭据，只通过显式 `widget.configuration` 返回给编辑器，不在 `widget.list/get/status`、普通 job/event 或 Debug 中回显。管理页不把小组件配置保存到 localStorage 或通用输出区；Admin token 与上次菜单单独按设备地址保存在 localStorage，可从连接页清除。当前管理 HTTP 与 SPIFFS 都未加密，仅适用于可信开发网络；API 凭据不能放进仓库或示例包。

## JSON-RPC

`widget.configuration` 参数：

```json
{"id":"com.note4c.todo"}
```

返回 `{id, settings}`。读取配置本身需要 admin token。

`widget.configure` 参数（整个 settings 替换，非 merge）：

```json
{
  "id": "com.note4c.todo",
  "settings": {
    "interval_seconds": 300,
    "host_permissions": ["https://example.com/todo.json"],
    "values": {
      "url": "https://example.com/todo.json",
      "title": "待办事项",
      "bearer_token": ""
    }
  }
}
```

`interval_seconds` 为整数：0 或 60～86400。`values` 必须是 JSON object。整个 settings 序列化后最多 2048 UTF-8 字节，同时受 broker RAM admission 限制；未知顶层字段、非法 interval 或网络模式会被拒绝。`values` 的具体含义由各小组件定义。

`host_permissions: null`（默认）沿用 manifest；数组是管理员在这次配置中显式授予的替代列表，空数组禁止网络访问。它不增加 manifest capability，不关闭 TLS、DNS/IP 或 redirect 检查。模式格式与 manifest 一致，见 [运行时契约](widget-runtime-mvp.md)。TODO 表单根据输入 URL 生成精确 origin/path 授权，并展示给用户；查询参数不作为授权模式的一部分。

配置写入在唯一 widget worker 上执行，返回 HTTP 202 与 job ID。仅 job `succeeded` 表示已持久化并应用；202 不等于保存完成。配置期间禁止同组件运行/删除/替换；持久化失败不发布新内存设置。固件启动恢复配置，非法或不可读配置保持自动运行关闭，不静默覆盖磁盘原件。

## Rune 接口

以下是由 mounted/timer/input 调用的普通辅助函数；完整组件见 TODO 示例。

```rune
pub async fn load_feed() {
    let config = os::config::get()?;
    let url = config.get("url")?.unwrap().string()?.unwrap();
    let response = os::http::get(url).await?;
    if !response.is_success() {
        return os::runtime::fail("API request failed");
    }
    let feed = response.json()?;
    let items = feed.get("items")?.unwrap();
    let first = items.at(0)?;
    os::runtime::ok(())
}
```

- `os::config::get()` 同步返回当前安装配置的 `values`，类型是现有 `JsonValue`；仅能读自己的配置，不另设 capability。
- `JsonValue.at(index)` 返回数组元素的 `Option<JsonValue>`；非数组或越界返回 None。
- `os::runtime::fail(message)` 返回 `Err(HostError)`，code=`widget_rejected`，message 最多 160 个字符。用于报告可诊断的验证失败，不应传入凭据或完整接口正文。
- `os::screen::fit_text(text, width, scale)` 按实际字宽裁切，必要时添加 `...`。输入最多 1024 UTF-8 字节、width 为 1～400、scale 为 1～4；控制字符变为空格，ASCII 字母使用大写字形，字库外字符变为 `?`。直接 `canvas.text` 仍严格拒绝不支持的 glyph。
- `canvas.text_sized(point, text, color, height)` 与 `os::screen::fit_text_sized(text, width, height)` 支持 16～32 的偶数像素行高。汉字使用原 16×16 字形，英数以旧 5×7 字形扩为 10×14 可见高度，再统一缩放，避免仅声明 16px 行高、实际英数仍很小的问题。补充标点使用 Unifont 8×16。旧 `text` / `fit_text` 行为保持兼容。

## 调度和显示语义

前台会话保持 VM、编译结果和 State，new/mounted 完成初次加载。间隔为 0 只关闭 timer，不关闭 mount 或输入；配置变化销毁重建会话及宿主权限上下文。safe/诊断模式不自动调度前台。

timer 与输入串行执行，沿用指令、分配、宿主调用和墙钟限额，不能永久循环。定时截止从调用完成后计算，失败沿用有界退避；最多保留一个待处理 timer，不补跑历史 tick。输入不合并为 timer 或已有 job。切换后丢弃旧代次事件，已开始的物理刷新仍完成清理。

脚本在任意正常 hook 自行绘图并 `canvas.flush().await`，没有 render hook 或 bool 重绘返回值。Canvas 继承本会话最后成功提交的画面；同一次调用共用工作区，新会话白底。flush 返回 `submission_id`，只代表接收快照。后续脚本失败不回滚成功提交，未提交修改丢弃。

宿主固定合并 200ms，刷新中只保留最新目标，比较最终像素指纹后决定跳过或全刷；清理也成功才更新已呈现身份。像素池最多四份 30,000 B，PSRAM 保存像素，小同步对象留内部 RAM。失败保留最新目标，不忙循环重试；新提交或 `screen.retry {}` 请求重试。管理页将脚本处理、已接收和物理呈现分开显示。相同 JSON 的空白或不可见字段变化不会改变像素身份。

这是设备保持运行时的轮询，不是 deep-sleep 定时唤醒；不承诺电池续航、多后台并行或分钟级硬实时。

## 三键输入与诊断

正常模式将上、下、BOOT 三键交给前台组件，通过 `#[widget::input(click(boot), long_press(up, down))]` 等宏声明。event 提供只读 keycode、kind、held_ms。长按只触发一次，释放不补短按；未绑定事件忽略，不进入旧测试页。

输入 GPIO 由独立采样任务持有，20ms 采样、3 样本消抖、约 1 秒长按，32 项 FIFO 在 HTTP/全刷期间继续采集。队满丢弃新事件并报告 overflow，不宣称任意速率零丢失。切换代次后按住的键须释放再触发。LED 和任务结果不代表物理刷新成功；刷新期间三键短按/长按六项实测已通过，事件数量与测试边界见本轮实机记录。

诊断需显式调用 `system.mode.set {mode:"diagnostic"}`，返回正常使用 `{mode:"normal"}`。仅打开网页菜单不切模式，诊断保留前台选择，重启回到正常模式。ROM/USB 恢复和硬件电源行为保持不变。

widget worker 保持现有 64 KiB 栈和编译/执行水位记录；本轮不因新接口放宽 VM 配额。

## 验证入口

完整验收应覆盖配置边界/认证/不泄露、存储恢复与删除隔离、定时不重入/退避/停用、待办数据校验/中文布局和显示缓存失效。固件构建与镜像尺寸单独检查。以下为完整场景清单；本版已执行的子集以 ABI 3 实机记录为准，未执行项保持待测：

1. 保存配置、激活、关闭管理页，观察连续三轮 API 请求及间隔。
2. 相同画面不触发 EPD；修改一条事项后只有一次全刷；合法空清单清除旧待办。
3. 超时、401/500、错误 JSON 时保留画面并形成失败 job，后续恢复成功。
4. RESET 后恢复配置并重新获取；禁用定时、切换组件和 safe mode 不继续旧周期。
5. 记录实际 BUSY 时序、供电条件及显示效果；凭据不进入测试日志。

### 历史软件验证记录（2026-09-08；不适用于 ABI 3 验收）

- Apple Silicon macOS 完整 host workspace：120 个测试通过；最后一次 admission 调整后又重跑 management/package 的 29 个相关测试，全部通过。
- 管理页在本地模拟 RPC 上验证了配置打开、间隔拒绝、URL/授权联动、保存 job 完成、重新读取和普通输出不回显凭据；未连接真实设备。修复了输入 API 地址后授权列表没有及时更新的问题。
- 真实 Rune + Canvas 生成并检查 400×300 待办预览；相同数据不同 JSON 空白的渲染像素一致。
- 当时未提交工作树的 ESP32-S3 release 构建成功。`espflash save-image` 仅生成本地应用镜像，尺寸 4,067,168 / 4,128,768 B，使用 98.51%，剩余 61,600 B；没有改变分区或刷写设备。这是 Flash 容量测量，不是 RAM、功耗或实机行为验证。
- `.n4w` 为 4,448 B，已核对 STORE 格式及内含源码与当前 `main.rn` 一致。字体快照校验和与生成过程可复现。
