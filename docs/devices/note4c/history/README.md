# NOTE4C 历史记录

这些记录只适用于文中明确的板卡、固件与测试条件。项目在 2026-10-22 更名为 Panelet；
历史日志保留当时的 `note4c-*` 程序名、源码路径、提交标识和制品哈希，链接则指向当前
文档位置。旧构建命令应在对应历史源码上执行；当前命令见[构建与验证](../../../development/building.md)。

| 记录 | 范围 |
|---|---|
| [首次 bring-up](bringup-log-2026-09-02.md) | 备份、恢复与第一轮显示 |
| [Phase 1](phase1-hardware-log-2026-09-02.md) | 电源、按键、RTC、显示实验 |
| [开发交接](develop-handoff-2026-09-04.md) | 当时工具链、容量与待确认项 |
| [管理服务 MVP](mvp-hardware-log-2026-09-04.md) | 管理、存储与日历的阶段证据 |
| [日历设计检查](calendar-design-qa-2026-09-05.md) | 主机画面检查；不替代实机观测 |
| [待办](todo-hardware-log-2026-09-08.md) | 数据拉取和运行验证 |
| [生命周期与多 Wi-Fi](lifecycle-hardware-log-2026-09-09.md) | 启动恢复与网络 |
| [ABI 3](abi3-hardware-log-2026-09-09.md) | 输入、生命周期与受控迁移 |
| [差异窗口刷新](partial-refresh-hardware-log-2026-09-09.md) | NOTE4C SSD2683 有限条件下的显示证据 |
| [编辑器入口固件](editor-hardware-log-2026-09-22.md) | 当时 app-only 更新和验证边界 |
| [设备端预览容量实验](widget-preview-size-2026-09-22.md) | 隔离目标构建，未进行实机运行验收 |

日志引用的本机临时文件不保证仍存在，也不随源码分发。备份镜像、凭据、设备标识和原始
私有证据保留在仓库外。未验证事项继续视为未验证，不因文档迁移或代码更名自动变为通过。
