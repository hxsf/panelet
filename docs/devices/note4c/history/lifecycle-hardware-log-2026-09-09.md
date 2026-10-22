# 生命周期、前台与多 Wi-Fi 验证（2026-09-09）

> 本文记录 ABI 3 之前的版本；后续迁移和三键/显示调度验证见 [ABI 3 实机记录](abi3-hardware-log-2026-09-09.md)。

## 条件与最终镜像

- ZECTRIX NOTE4C PCB V1.0，四色 SSD2683，沿用项目单板记录。
- USB 数据连接供电；电池是否同时接入未确认，未测电流、电压或 BUSY 引脚波形。
- Git 基线 `52a816854c6045fa8fdabaf57ba71158b675de9d`，包含未提交工作树。
- 最终 ELF SHA-256：`29bb505d904acbb14894696eaa8ef21fd76611925a531bbb284267e01b5f8dc9`。
- 主要源码路径与内容的 SHA-256 指纹：`160dd976eab313c7387f53e25def0b08a1d43865456c7e4bf5d98ac0414edabb`。范围为 Cargo release 配置、server 源码/build/sdkconfig、生命周期与 ABI、管理/网络配置和存储实现，不替代完整 Git 提交。
- app image：**3,716,832 / 4,128,768 B（90.02%，余 411,936 B，约 402.3 KiB）**。

本轮初次 `opt-level=s` 构建为 4,101,472 B，接近分区上限；采用 `opt-level=z`、保留 LTO，并构建时 gzip 单文件管理页后，最终镜像减少 384,640 B。期间包含小量实现修正，因此不是仅改变编译参数的严格性能基准。未削减 TLS 验证、恢复路径、中文字体或功能，也未扩大分区。

## 刷写安全

复用 [首次备份记录](bringup-log-2026-09-02.md) 的同板 16 MiB 原厂备份与第二副本核验。本次刷写前重新读取 security info，确认 Secure Boot/Flash Encryption 均 disabled；读取分区表和 OTA 选择，核对 CRC、有效 `ota_0 @ 0x20000`、槽大小 `0x3f0000`，并在仓库外保存整个当前 app 槽及 SHA-256。

各次刷写均调用 `scripts/flash-server.sh` 的 app-only 模式，只写 ota_0。没有写 bootloader、分区表、otadata、eFuse 或格式化 assets。Wi-Fi 凭据和前台标记的应用层持久化更新属于本次功能；没有擦除整个 NVS。备份、网络地址、SSID、密码、token 和设备标识均未写入本记录。

## 实机观察

中间镜像 `915648b53…` 验证了主要功能；`10c351e36…` 增加 SPIFFS 调度修正；最终 `29bb505d9…` 再让旧 main 组件保留编译结果。以下列明对应证据，避免把前一镜像的全部观测当作最终镜像逐项重测。

| 项目 | 观察 | 结论 / 镜像 |
| --- | --- | --- |
| 启动恢复旧前台 | 网络状态页后生成 TODO mount job，assets Ready、前台 ID 保留 | Pass，915648b53 |
| 管理页 | 根页面内容与当前源码逐字节一致，前台切换/取消及配置、上传 API 可用 | Pass，915648b53；链路返回 identity，未将线上 gzip 编码记为 Pass |
| Basic 包更新 | TODO 0.3.0 上传并编译成功，原配置保留，原无认证示例源能拉取和呈现 | Pass，915648b53；不是 NAS Basic/TLS 实测 |
| 生命周期 mount / timer | Counter 成为前台，定时 job 成功并产生后续画面；事件处理期间复用会话 | Pass，915648b53 |
| 配置重建 | Counter 配置变更后重新编译/创建并呈现，定时间隔改为 0 | Pass，915648b53 |
| 零定时间隔的开机进入 | interval=0 的 Counter 在启动状态页后自动进入 | Pass，10c351e36 |
| 取消前台持久化 | 取消后重启，前台 ID 仍为空，停留在状态页 | Pass，10c351e36 |
| 已保存 Wi-Fi 迁移 | 旧单组凭据读入新列表，重启保持可连接；只读接口仅含 SSID 列表和容量 5 | Pass，915648b53 / 10c351e36 |
| 连接失败回退 | 请求不存在的测试 SSID，任务失败后恢复原已保存网络，保存列表仍为 1 组 | Pass，10c351e36 |

新确认键 hook 的路由、状态变化与错误处理有主机测试。用户在早期验证阶段按过 BOOT，当时前台是旧 TODO；之后出现状态页和 Counter 是测试脚本切换造成的，不能据此声称已目视验证 Counter 的实体按钮计数。

## 启动 watchdog 修正

首轮复现既有的非致命 watchdog 告警，栈位于启动恢复的 SPIFFS metadata / Path.exists 路径；随后程序仍能运行。存储恢复现在在目录遍历与初始化检查之间用 ESP-IDF 目标专用的短暂 sleep 让 idle task 获得运行机会，没有关闭或延长 watchdog。`10c351e36…` 的两次受监控重启未再出现该告警。

观测到的 PON/DRF 阶段约 24.3 秒，POF/deep-sleep 约 167 ms。它们是软件阶段时间，不是纯 BUSY 电平测量；job 成功代表既有驱动/forced-safe 清理条件满足，没有独立测 rail 电压。中间镜像复杂 TODO 路径的最低剩余 worker 栈记录为 8,408 B，适用于该包与本次条件，不推广为任意脚本的栈安全保证。

## 软件验证与限制

- 完整 host workspace（显式 aarch64-apple-darwin target、exclude server、all-features）：137 项通过。
- 生命周期覆盖实际宏编译、重复/非法签名拒绝、State 保留、false 不呈现、错误与取消作废会话、撤销旧 host 句柄、跨事件累计内存配额、旧 main 非 unit 成功值，以及真实 TODO 在同一 VM 连续执行 20 次。
- Wi-Fi 配置覆盖 5 组容量、同名替换/优先级、编码恢复、损坏/重复拒绝和 Debug 脱敏；前台空标记覆盖重新打开存储后恢复。
- 目标 release 构建、flash-server mock、provision-wifi 伪终端测试、JS 语法、HTML ID 引用、gzip 往返和 diff 检查通过。
- 没有第二个真实 AP 的凭据，本轮不声称实测了两组可连接 AP 间的选择；没有接入 NAS 实测 HTTPS Basic，默认校验证书和私有 CA 导入缺失的边界保持不变。
- v1 未实现 State 磁盘快照、多后台、拼页、动态订阅或浏览器屏幕模拟器。

## 最终交付状态

最终 `29bb505d9…` 镜像已 app-only 刷入，串口 ELF 前缀匹配。受监控启动先显示网络状态页，再进入 TODO 0.3.0；随后用兼容 RPC 再检查一次，`run_compile_finish` 总数仍为 1，出现 `canvas unchanged; full refresh skipped`，job 成功。该启动与检查窗口没有 watchdog 或捕获到的运行时错误。

前台为 `com.note4c.todo`，检查间隔 300 秒，保留原配置与本机示例 feed。测试 Counter 包及其私有数据已删除，设备仍保留原来的 3 个组件。Counter 源码留在仓库供使用者自行安装。没有把本轮当作真实 NAS、真实提醒事项数据或所有 Wi-Fi 场景的验收。
