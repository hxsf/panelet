# Panelet 矢量相册小组件

Photo Album 0.2.0 使用 `note4c.rune/3`。三张“照片”仍由四色几何图形构成，验证包资源、KV 和 Canvas 主动提交；本例不读取或解码图片文件。

## 打包与使用

```sh
node examples/widgets/build-upload.mjs photo-album > /tmp/panelet-photo-album.pnw
```

在管理页上传并设为前台，mounted 显示下一帧；BOOT 短按或配置的 timer 事件再前进一帧。上、下键和 BOOT 长按没有绑定，未实现上一张或组合键。

`manifest.json` 不填写 `entry`，入口由 `#[widget::component]` 生成。组件 ID 仍为 `sample.photo-album`，配置和 KV 可以沿用。包使用仓库统一的 STORE-only 打包器，源码限制 5 KiB、上传包限制 64 KiB。不要将设备 token 写入仓库。

`resources/album.json` 被读取并记录，但帧顺序仍由脚本分支决定，其中旧的未来输入设想不代表已实现功能；`resources/captions/*.txt` 提供画面上的 ASCII 标题。

## 状态和画面提交

`os::storage` 的私有 KV 键 `album.current` 保存下一次要提交的帧：

1. 没有键时提交 `MORNING RIDGE`，然后保存 `market`。
2. 随后提交 `MARKET STREET`，保存 `harbor`。
3. 随后提交 `NIGHT HARBOR`，保存 `morning`，循环往复。

每次调用只执行一次 `canvas.flush().await`。成功表示宿主已接受快照，此后推进 KV；物理刷新由宿主独立安排，失败时不会自动回滚 KV。刷新错误及重试应查看管理页的呈现状态。连续提交可能被合并，只呈现最新帧。

重启后从持久化的下一帧继续，未对 VM 做磁盘快照。KV 写失败时脚本报告错误，但已经接受的画面不会回滚。删除组件的数据可能清除该 namespace。

三帧仍为 400×300，保留原图元位置、文字和绘制顺序；通过 `os::screen` 颜色常量表达黑、白、黄、红。单帧命令与文字量遵守现有配额。需要 `screen.present`、`storage.kv`、`log.write`；包内资源读取属于基础能力。

本版需要 ABI 3 固件；主机执行和设备呈现应分别验证，不将旧版测试视为本版实测。
