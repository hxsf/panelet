# Panelet Rune 示例小组件

此目录保存可审查的 Rune 源码、manifest 和 UTF-8 包内资源。源码与资源是唯一事实来源；不要手工维护一份重复嵌入源码的 JSON 或 `.pnw` 文件。

`counter/`、`calendar/`、`todo/` 和 `photo-album/` 中的原创小组件源码、manifest、资源和说明按 [MIT](LICENSE) 发布，可以复制修改用于自己的小组件。管理页的默认 Rune 模板也按 MIT 发布；宿主固件、编辑器和本目录的 `build-upload.mjs` 打包工具仍按仓库的 MPL-2.0 许可。

复制或分发示例时保留版权与 MIT 许可通知。每个 `main.rn` 保留版权声明及 MIT 标识，`resources/LICENSE.txt` 保留完整的许可与免责条款；两者会一同进入命令行、桌面编辑器及浏览器预览导出的 `.pnw`。条款正文为 1,023 字节，符合现有单资源 1 KiB 上限，完整的组合通知另见本目录 `LICENSE`。独立编写的小组件仍由其作者自行选择许可。

- `todo`：定时拉取统一 JSON 待办 feed，支持独立安装配置与按像素去重；详见 [TODO 说明](todo/README.md)。
- `calendar`：读取 RTC/time，绘制当月日历，并把最近日期写入私有 KV。
- `photo-album`：三张四色矢量画面；每次 mounted/timer/BOOT click 只提交一张，并通过私有 KV 循环切换。

`.pnw`（Panelet Widget）是小组件包的默认扩展名，与旧 `.n4w` 使用同一套 ZIP 内容和校验规则。已有 `.n4w` 可继续导入，无需重新打包；设备内部存储文件名、ABI 和上传 MIME 保持兼容。

生成可直接发送给管理 API 的 STORE-only `.pnw` 二进制包：

```sh
node examples/widgets/build-upload.mjs todo > /tmp/panelet-todo.pnw
node examples/widgets/build-upload.mjs calendar > /tmp/panelet-calendar.pnw
node examples/widgets/build-upload.mjs photo-album > /tmp/panelet-photo-album.pnw
```

在已连接 NOTE4C 管理网络并取得屏幕上显示的 admin token 后，可以自行发送：

```sh
curl --fail-with-body \
  -H 'Content-Type: application/vnd.note4c.widget+zip' \
  -H "X-Note4c-Token: $NOTE4C_ADMIN_TOKEN" \
  --data-binary @/tmp/panelet-calendar.pnw \
  http://192.168.4.1/packages/v1
```

不要把 admin token 写入示例、命令历史或仓库。包、资源和 KV 持久化在设备的
`assets` 文件系统中；首次使用前需在管理页显式初始化（会擦除整个 assets
分区），之后复位或断电不会自动清除已提交内容。

本轮只完成主机侧 Rune 编译、执行、Canvas 配额与 `.pnw` 上传包检查；这两个示例尚未重新刷入 NOTE4C 做 Wi-Fi、管理 API、RTC 和 EPD 的端到端实机验证。尤其日历首次实测必须用已知日期核对 RTC 的 weekday 列映射，相册每次运行仍会触发一次约 24 秒的显示刷新。

打包器只依赖 Node.js 标准库，并在输出前检查当前 MVP 的主要传输限制：STORE-only ZIP、5 KiB 源码、最多 8 个资源、单项正文 1 KiB、资源路径与正文合计 4 KiB、8 KiB manifest，以及 64 KiB `.pnw` body。设备端仍会执行完整的 manifest、capability、路径、CRC、Rune 编译和 admission 校验；压缩、ZIP64、加密、重复路径、目录穿越、symlink 和尾随数据都会被拒绝。

## 当前与计划中的能力

所有示例使用 ABI 3、组件宏和 `os`；设为前台后 mounted 首次执行，定时及输入 hook 串行分派。源码自己调用 Canvas.flush 提交画面，宿主合并后呈现。详见 [生命周期契约](../../docs/os/widget-lifecycle-v1.md)。

为后续调度测试保留的顶层 handler 只能使用当前已经存在的 ABI。后续扩展事件调度时，激活流程应运行一次 `main`，成功后再按统一事件序列串行调用 handler；显示刷新期间产生的输入与 timer 事件由有界 mailbox 保存或合并，而不是重入同一个 widget。

“相册”目前只能验证四色 Canvas、资源和状态切换。当前资源仅支持 UTF-8，Canvas 没有 bitmap/blit，因此它不是任意照片解码器。

生命周期示例：[counter](counter/README.md)，演示常驻 State、mounted/render 与 BOOT 确认键 hook。
