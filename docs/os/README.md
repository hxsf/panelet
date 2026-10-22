# Panelet OS 契约

这里记录管理服务和小组件使用的系统契约。具体设备的接线、供电、分区容量、屏幕能力及
实测结果见[设备文档](../devices/README.md)，不能由通用接口推断每块板都具备全部能力。

| 文档 | 内容 |
|---|---|
| [管理服务](management-server-mvp.md) | RPC、认证、Wi-Fi 配置、任务与存储接口 |
| [小组件运行时](widget-runtime-mvp.md) | 当前 Rune ABI、包格式、capability 和宿主调用 |
| [生命周期](widget-lifecycle-v1.md) | mounted、事件、前台会话和 Canvas 提交 |
| [配置与调度](widget-configuration-and-polling.md) | 实例配置、网络权限和定时更新 |
| [版本化资源](web-assets.md) | 浏览器 WASM 资源、兼容版本、安装和回退 |

文件名中的 `mvp` 和 `v1` 是现有文档名称，不代表所有内容停留在第一版；每篇内部的
明确 ABI、格式与验证记录是判断依据。重命名与协议兼容规则见
[名称与兼容性](../development/naming-and-compatibility.md)。

当前绘图与刷新实现仍含 NOTE4C 的 400 × 300、四色 2 bpp 和 SSD2683 约束。通用显示
profile 与平台适配层的提取属于[后续架构工作](../development/architecture.md)，不是本次
文档归类后自动获得的多硬件能力。
