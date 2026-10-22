# Panelet

Turn your display board into a personal dashboard with customizable widgets.

把带屏幕的开发板变成你的专属看板，用小组件展示你关心的信息。

Panelet 为设备提供显示、按键、网络连接、存储和小组件运行能力，并提供浏览器管理页与
编辑预览工具。你可以安装现成的小组件，也可以修改或编写自己的小组件。

## 可以用它做什么

- 在屏幕上显示日历、待办事项、相册和其他自定义内容。
- 通过浏览器管理小组件、设置网络、调整配置并选择当前显示的内容。
- 在编辑器中预览画面、模拟按键，导入或导出小组件包。
- 让小组件按设定的时间或按键事件更新，保留各自的配置和数据。

仓库提供[日历、待办、相册和计数器示例](examples/widgets/README.md)。预览不会操作设备
上的真实屏幕；显示效果和刷新能力仍以具体设备为准。

## 支持的设备

| 设备 | 状态 | 使用入口 |
|---|---|---|
| ZECTRIX NOTE4C Devkit，PCB V1.0 | 当前已接入；400 × 300 四色墨水屏 | [NOTE4C 说明](docs/devices/note4c/README.md) |
| ZECTRIX NOTE4（黑白屏） | 计划支持，尚无可用适配 | [接入准备](docs/devices/note4/README.md) |
| 其他带屏幕设备 | 欢迎适配，需要独立驱动与验证 | [添加新设备](docs/development/adding-device.md) |

目前处于开发阶段，安装仍需要构建固件并按设备说明完成备份与受控刷写。当前软件不提供
一键通用安装；不要将 NOTE4C 固件用于 NOTE4 或其他型号。

## 开始使用

1. 打开对应的[设备说明](docs/devices/README.md)，确认型号与支持状态。
2. 按设备指引完成备份、构建与安装，再设置 Wi-Fi 并打开设备管理页。
3. 安装示例小组件，设置需要的数据源或内容。
4. 需要自定义时，使用[编辑与预览工具](docs/development/widget-editor.md)。设备上的
   浏览器预览还需要安装[预览资源包](docs/os/web-assets.md)。

想先在电脑上试用编辑器，可安装 stable Rust 后运行：

```sh
scripts/run-widget-editor.sh
```

它只监听本机回环地址，不需要连接或刷写设备。工具链准备、测试和固件构建见
[构建与验证](docs/development/building.md)。

## 文档与开发

[文档首页](docs/README.md) 按用途组织：

- [设备文档](docs/devices/README.md)：型号、接线与供电、安装恢复、显示限制和实测记录。
- [OS 文档](docs/os/README.md)：管理接口、小组件生命周期、权限、存储和共享资源契约。
- [架构与目录规划](docs/development/architecture.md)：当前实现、模块边界和后续拆分路线。
- [添加新设备](docs/development/adding-device.md)：硬件适配步骤、能力声明和验收要求。
- [名称与兼容性](docs/development/naming-and-compatibility.md)：Panelet 更名后的程序入口及保留的旧协议标识。

当前小组件使用 Rune，板端仍包含编译与执行能力；独立编译器、VM 以及更多设备适配属于
后续规划。项目不会因为更名而改动已安装设备的包格式、凭据或持久化数据。

## 许可证

项目自有代码按 [MPL-2.0](LICENSE) 许可；[小组件示例](examples/widgets/README.md)单独按
[MIT](examples/widgets/LICENSE) 许可，方便复制、修改和用于自己的小组件。
第三方代码、字体与参考资料保留各自许可和权利边界，详见[许可证与第三方材料](docs/development/licensing.md)
及[来源清单](3rd-res/SOURCES.md)。
