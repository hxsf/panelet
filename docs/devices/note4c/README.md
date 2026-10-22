# Panelet on NOTE4C

适用于 ZECTRIX NOTE4C Devkit、PCB V1.0、ESP32-S3 N16R8、4.2 英寸 400 × 300 SSD2683 四色屏。NOTE4 黑白型号不属于本页适用范围。

该设备已有电源/恢复、三键、LED、RTC、电池原始观测、Wi-Fi/管理服务、小组件和四色显示的实机记录；具体 commit、条件与限制见下方日志。蓝牙、音频、NFC、低功耗指标和电池百分比未作为已验证功能提供。2026-10-22 的浏览器 WASM 预览与版本化 assets 完成软件验证和目标构建，当时未上传或刷写到设备；不能把它与先前实机结果合并成整版验收。

## 使用入口

1. 首次安装前完成[恢复与原厂备份](bringup-and-recovery.md)，核对[硬件资料](note4c-hardware-and-esp32-rs.md)与[分区策略](firmware-partition-layout.md)。
2. 按[构建说明](../../development/building.md)生成当前固件；有对应授权和核验条件后，使用受控脚本刷写。
3. 用设备 SoftAP/管理页或下方串口脚本配网。已配置设备通过其网络地址进入管理页。
4. 选择[示例小组件](../../../examples/widgets/README.md)，导入、配置并激活；编辑和预览见 [Studio](../../development/widget-editor.md)。浏览器预览先安装匹配的 [assets 资源](../../os/web-assets.md)。

## 恢复、备份与刷写

在首次写入前，为同一台设备完成 BOOT + RESET 恢复演练、完整 16 MiB 原厂 Flash 读取、SHA-256 核对和第二份受控副本。备份放在仓库外；后来的开发固件备份不能替代原厂记录。

```sh
scripts/backup-factory-flash.sh \
  --port /dev/cu.usbmodemXXXX \
  --output-dir /Volumes/PRIVATE/note4c-backups
```

所有命令从仓库根目录执行，串口必须替换为本次明确识别的设备。保留 GPIO0 BOOT 与 GPIO19/20 原生 USB 恢复路径；进入 ROM download 时按住 BOOT，短按 RESET 后再松开 BOOT。

当前布局为 16 MiB、双 OTA app 槽和 8 MiB assets。常规更新只有在只读核对分区表、OTA 选择记录 CRC/状态及实际映射均指向 `ota_0` 后，才能使用 app-only 模式：

```sh
NOTE4C_FACTORY_BACKUP_VERIFIED=YES \
NOTE4C_SECURITY_STATE_COMPATIBLE=YES \
NOTE4C_FLASH_MODE=app-only \
NOTE4C_EXISTING_OTA0_LAYOUT_VERIFIED=YES \
NOTE4C_PORT=/dev/cu.usbmodemXXXX \
scripts/flash-server.sh target/xtensa-esp32s3-espidf/release/panelet-os
```

上述 `YES` 代表已经完成对应核验，不能预先设置来跳过检查。app-only 从 ELF 生成非 merge app image，检查容量后仅写 `ota_0 @ 0x20000`，不修改 bootloader、分区表、NVS、assets 或 otadata。未知/不兼容的设备安全状态不能使用该示例。

首次迁移、full 模式和分区覆盖的独立要求见[恢复流程](bringup-and-recovery.md)。双 OTA 布局不等于已实现 HTTP OTA，也不保证另一个槽中已有可恢复固件。刷写脚本名与 `NOTE4C_*` 确认变量保留为设备专用接口。

## 串口配网

关闭占用端口的 monitor 后执行：

```sh
scripts/provision-wifi.py \
  --port /dev/cu.usbmodemXXXX \
  --ssid 'YOUR-WIFI' \
  --password 'YOUR-PASSWORD'
```

脚本只输出协议状态，不回显密码；命令行参数仍可能保留在 shell history 或进程列表。设备先写 candidate，连接验证成功后才 promote 为有效配置；失败不覆盖现有配置。`pending` 表示任务仍可能继续修改 Wi-Fi，不能当作失败后立即重复提交。

管理页支持保存多组 Wi-Fi，协议与认证细节见[管理服务](../../os/management-server-mvp.md)。芯片 BLE 能力不代表当前已有蓝牙配网入口。

## 按键与硬件诊断

完整菜单行为、总线访问与安全清理限制见[诊断操作说明](diagnostics.md)。

正常模式下，三个键只交给前台小组件；未绑定的事件忽略，不会切换诊断菜单或自动运行组件。需要诊断时，在管理页“硬件诊断”中点击“进入诊断模式”，或调用 `system.mode.set {mode:"diagnostic"}`；仅打开该页面不会切换模式。完成后点击“返回正常模式”，或调用 `{mode:"normal"}`，宿主会重建所选前台会话。模式不持久化，普通重启回到正常模式；safe mode 仍限制外设访问。完整契约见[生命周期](../../os/widget-lifecycle-v1.md)。

以下菜单操作只用于诊断模式，菜单输出在 USB 日志中，选择变化不会自动刷新屏幕：

- 上/下键切换菜单；启动时持续按住上键进入 safe mode。
- 菜单顺序为 `SYS`、`KEYS_LED`、`EPD_QUICK`、`EPD_PARTIAL_EXP`、`I2C_SCAN`、`RTC_READ`、`BATTERY`、`STATUS_SCREEN`。
- `SYS` 默认选中；按 BOOT 执行系统信息检查，不调用前台组件。
- `EPD_QUICK`、`STATUS_SCREEN` 需长按 BOOT 约 1 秒；`EPD_PARTIAL_EXP` 需长按、完全释放、再次长按，每次启动最多进入一次。
- safe mode 保持 EPD、NFC 与音频/I2C 上拉 rail 关闭，拒绝涉及这些供电的显示/I2C/RTC 诊断。

诊断范围须按结果解释：I2C scan 仅探测响应；RTC 检查 BCD/日期与秒推进；电池采样报告原始状态和电压估计，不输出 SOC；屏幕命令成功与清理完成也不等于画面已经人工确认。详细电平、时序与操作依据见[硬件资料](note4c-hardware-and-esp32-rs.md)和[恢复文档中的验收矩阵](bringup-and-recovery.md#8-bring-up-测试矩阵)。

全刷和 PTLW 窗口刷新都可能超过 10 秒。普通小组件的自动差异策略与错误恢复见[屏幕刷新说明](screen-partial-refresh.md)；它不承诺高速局刷或长期寿命。实验诊断不是日常刷新入口。

## 实机与历史记录

以下文件保存当次条件和产物数据，不代表后续构建或不同板卡自动通过：

- [首次 bring-up](history/bringup-log-2026-09-02.md)与 [Phase 1](history/phase1-hardware-log-2026-09-02.md)：恢复、供电、按键、外设、显示和 PTLW 初始证据。
- [持久化小组件](history/mvp-hardware-log-2026-09-04.md)与 [开发交接](history/develop-handoff-2026-09-04.md)：PSRAM、Wi-Fi、时间/RTC 与存储验证。
- [待办](history/todo-hardware-log-2026-09-08.md)、[生命周期/多 Wi-Fi](history/lifecycle-hardware-log-2026-09-09.md)、[ABI 3](history/abi3-hardware-log-2026-09-09.md)：应用与运行时迁移。
- [自动差异窗口刷新](history/partial-refresh-hardware-log-2026-09-09.md)：时序、目视结果和未确认区域。
- [编辑器入口固件](history/editor-hardware-log-2026-09-22.md)与 [预览容量实验](history/widget-preview-size-2026-09-22.md)：历史刷写和尺寸实验，后者未做实机运行验收。
