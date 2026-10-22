# NOTE4 支持计划

**状态：规划中，尚未适配和验收。** NOTE4 黑白屏是 Panelet 的下一款候选设备；当前 `panelet-os` 固件、默认构建配置、刷写脚本和编辑器预览仍面向 NOTE4C，不应直接用于 NOTE4。

接入前需要独立确认：

- 准确型号、PCB revision、MCU、Flash/PSRAM 规格和 ROM/USB 恢复入口。
- 显示控制器、分辨率、黑白编码与位序、BUSY 极性、供电/复位和刷新协议。
- 引脚与按钮布局、power-hold、电池、RTC、网络和可选外设。
- 原厂备份、安全状态、分区、镜像容量以及可授权的写入范围。

代码准备不止更换 GPIO：需要解除 UI、Rune DisplayBroker 和原生/WASM 预览中的 400 × 300 四色 framebuffer 绑定，增加黑白显示描述和明确的颜色兼容行为。设备页面与预览必须使用同一 profile，不能用 NOTE4C 的 PTLW 结果作为 NOTE4 的时序依据。

按[新硬件接入流程](../../development/adding-device.md)推进。开始实现后将状态改为“适配中”；有明确设备、固件版本和观测结果后，将日志放到本目录的 `history/` 并逐项更新[设备列表](../README.md)。当前没有 NOTE4 的主机驱动测试、目标构建或实机通过记录。
