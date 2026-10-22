# Panelet Calendar Rune widget

Calendar 0.3.0 使用 `note4c.rune/3`，需要对应新固件。包由 `manifest.json`、`main.rn` 和 `resources/` 组成，入口由组件宏生成，不再填写 `entry`。

```sh
node examples/widgets/build-upload.mjs calendar > /tmp/panelet-calendar.pnw
```

在管理页上传后设为前台。mounted、BOOT 短按、timer hook 都调用同一个普通 `refresh()` 方法；上、下键与 BOOT 长按未绑定。定时间隔通过组件配置设置，0 表示关闭。配置更新、取消前台和重新进入会重建会话。

脚本通过 `os::time::local()` 读取按系统时区换算的时间。顶部显示 `YYYY-MM-DD`、星期、干支年和农历月日；七列月历保持等宽，周末红字、今天黄底。五周月份使用 43 px 日期行和 3 倍数字，六周月份使用 36 px 日期行和 2 倍数字；保持原有排版和绘图预算。

`os::time::lunar(year, month, day)` 支持 2000..=2099 公历日期并保留闰月。系统时间不可用时提交 `TIME NOT SET`，不猜测日期。中文字形来自固件已有字库。

Canvas 通过 `os::screen::get_context()` 获取，由脚本主动 `flush().await`。成功仅表示画面快照已接收；`CALENDAR SUBMITTED` 日志不是物理呈现确认。宿主合并提交并比较最终像素，内容不变则跳过刷新。

旧版输入/定时设计草稿已由当前生命周期契约取代。当前使用静态属性宏，不支持动态订阅、翻月或多后台 VM。本版硬件行为以新的实测记录为准，旧版实测不能代替验证。
