# 预览集成固件容量测量（2026-09-22）

本记录是隔离工作树中的目标构建实验，未刷写设备，不是设备端预览的运行验收。
实验期间原工作区和已刷写制品保持不变；当时正式固件只含包导出与本机预览链接。

2026-10-22 整理说明：本文保留设备端执行预览的历史容量实验，未纳入当前产品路径。
当前实现让浏览器执行 WASM，设备仅提供静态资源，见 [assets 资源](../../../os/web-assets.md)。
整理时已将实验源码差异、未跟踪文件和最终构建产物备份到仓库外；隔离工作树不再用于
当前实现。源码差异以 `c0747b6` 为基线保存，可独立于工作树归档流程恢复。

## 结果

三次均使用同一工作树、ESP32-S3 target、release、`opt-level=z`、LTO、
单 codegen unit 和现有 4,128,768 B OTA 槽，使用 `espflash save-image` 生成非 merge image。

| 构建 | app image | 相对基线增加 | 槽剩余 | 占用 |
|---|---:|---:|---:|---:|
| 默认固件 | 3,732,080 B | 0 | 396,688 B | 90.39% |
| 预览引擎与认证 API | 3,778,176 B | 46,096 B（45.016 KiB） | 350,592 B | 91.51% |
| 再含完整网页、四示例、包导入导出 | 3,805,296 B | 73,216 B（71.5 KiB） | 323,472 B | 92.17% |

默认构建尺寸与实验时已刷写镜像一致。引擎复用现有 Rune 编译器、运行时、Canvas 和字体，
没有嵌入第二份独立运行时。确认新增 API 和模拟宿主字符串在引擎/full 镜像中保留，
默认镜像中不存在；确认三个 gzip 资源逐字节包含于 full 镜像，未被 LTO 删除。

完整档额外网页资源采用 gzip level 9、mtime=0：

| 资源 | 原始 | gzip |
|---|---:|---:|
| 编辑器 HTML | 25,095 B | 8,565 B |
| 管理页复用 CSS | 11,167 B | 3,416 B |
| 四个完整示例及模拟配置 | 18,538 B | 4,742 B |
| 合计 | 54,800 B | 16,723 B |

## 实验实现与复现

实验工作树以当时实现快照为基础，加入两个默认关闭的测量 feature（仅存在于归档
实验中，当前主工作区不包含，以下命令须在恢复实验快照后运行）：

- `firmware-preview-size`：链接实际 `editor/runtime.rs`，注册认证后的 `/preview/v1`，
  64 KiB 输入限制、单个预览任务、64 KiB 专用工作栈，通过独立 DisplayBroker 捕获帧。
- `firmware-preview-ui-size`：包含上项，再注册 `/editor`、`/editor.css`、`/examples`、
  `/package/decode`、`/package/encode`，使用真实包解析与验收；页面读取现有管理页的
  登录 token。HTTP 默认允许 32 个 handler，足以容纳全部路由。

```sh
scripts/build-mvp.sh
scripts/build-mvp.sh --features firmware-preview-size
scripts/build-mvp.sh --features firmware-preview-ui-size
```

每次构建后单独执行 `espflash save-image --chip esp32s3 --flash-size 16mb`
并指定现有双 OTA CSV、`--target-app-partition ota_0`；没有运行 flash runner。

三份 app image SHA-256：

- baseline：`06f48e412c5014d7c84c4e6b783d00387ef7014f85f847679629b16cd085c217`
- engine：`a8ef40c23844835cc06c3daa67de600470b79373741043b359fe6d8e385730ec`
- full：`e6d2bb19d20d29fb76ef89e121b1139cebc430a1a802db08b7ad5981fe144b26`

实验时原始测量值保存于 `/tmp/note4c-integrated-size-results.json`，镜像和构建日志使用
`/tmp/note4c-integrated-{baseline,engine,ui}` 前缀。2026-10-22 整理时这些临时文件已不存在；
上表与 app image 哈希保留为当时记录，本轮没有重新构建三档镜像核验。full 构建当时有
一条不影响链接的 unused import 警告；三档目标编译均成功。

归档前另保存实验工作树最后的 ELF、bootloader 与分区表到仓库外的本机制品目录，
逐文件复制并核对 SHA-256。ELF 为 5,632,384 B，SHA-256：
`b9e69a4ff5d67258c3adc8afe865018ac027eff65855bc3e42a60282420df949`。
它与已刷写固件不同，也不是上面三份 app image 的替代验证。

## 尚不能由容量证明的事项

Flash 容量足够，最终产品实现的字节数会随调度和内存改造略有变化。本实验未验证：

- 新 64 KiB 工作栈和编译 context 的内部 RAM 峰值、PSRAM 压力及同步对象放置；
- 桌面版 10 秒子进程终止机制的设备端替代；当前 PoC 的 HTTP 等待超时不取消 worker；
- 编译期间管理 HTTP 响应、64 事件重放的整体时间限制与取消回收；
- 把 30,000 像素字节转为 JSON Value 数组产生的额外内存。正式版本宜直接传二进制帧，
  并复用或扩展已有组件 worker；
- 连续预览与正在运行组件共存、异常恢复、实体显示不受影响的实机表现。

这些是运行时集成工作，不是 Flash 装不下的证据。
