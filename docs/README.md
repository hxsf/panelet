# Panelet 文档

先按任务选择入口：设备差异与实测放在 `devices/`，跨设备的系统和小组件契约放在 `os/`。
这里的“跨设备”描述模块职责；目前实际固件仍只接入 NOTE4C，其他设备需要独立适配。

| 你想做什么 | 阅读入口 |
|---|---|
| 安装、配网、运行示例 | [设备支持与安装](devices/README.md) |
| 修改内容、预览或制作小组件 | [编辑器](development/widget-editor.md)、[示例](../examples/widgets/README.md) |
| 安装浏览器预览资源 | [assets 资源](os/web-assets.md) |
| 理解接口、权限与持久化行为 | [OS 契约](os/README.md) |
| 构建、测试或贡献代码 | [构建与验证](development/building.md)、[架构与目录](development/architecture.md) |
| 增加一种设备 | [新硬件接入](development/adding-device.md) |
| 判断旧名字是否可以替换 | [名称与兼容性](development/naming-and-compatibility.md) |
| 分发代码、固件或小组件 | [许可证与材料边界](development/licensing.md) |

## 设备与证据

- [NOTE4C](devices/note4c/README.md)：当前实现、恢复与备份、分区、显示策略及[历史记录](devices/note4c/history/README.md)。
- [NOTE4](devices/note4/README.md)：计划支持，尚未完成适配与实测。
- 设备文档中的尺寸、GPIO、供电和刷新结论不能直接推广到其他硬件。
- 历史记录保留当时的固件名、提交、制品哈希与限制；当前构建入口以开发文档为准。

## 其他指南

- [Apple Reminders / WebDAV 接入边界](guides/ios-reminders-webdav.md)
- [管理页本机模拟](development/server-ui-preview.md)
- [Agent 指令维护说明](development/agent-guidance-design.md)

## 最近的软件验证

[2026-10-22 Panelet 更名与文档迁移验证](development/panelet-migration-2026-10-22.md)记录本次主机检查、WASM 对照、目标构建和镜像容量；未进行实机操作。
