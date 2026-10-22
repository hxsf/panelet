# 构建与验证

以下命令从仓库根目录执行。当前固件目标仍是 NOTE4C 的 ESP32-S3；项目更名没有完成其他板卡的适配。设备安装与刷写见 [NOTE4C 指南](../devices/note4c/README.md)。

## 主机逻辑与原生 Studio

需要 stable Rust、系统 C/C++ 编译工具。仓库默认 target 是 Xtensa，因此所有主机命令都显式指定 host target：

```sh
PANELET_HOST_TARGET="$(rustc +stable -vV | sed -n 's/^host: //p')"
cargo +stable test --workspace --exclude panelet-os --all-features \
  --target "$PANELET_HOST_TARGET"
```

只改某个 crate 时可用 `-p <crate>` 缩小范围；涉及跨 crate 契约时运行完整主机套件。原生 Studio 是 `app/os` 中通过 feature 隔离的独立程序：

```sh
cargo +stable build -p panelet-os --no-default-features \
  --features desktop-editor --bin panelet-studio --target "$PANELET_HOST_TARGET"
cargo +stable test -p panelet-os --no-default-features \
  --features desktop-editor --bin panelet-studio --target "$PANELET_HOST_TARGET"
python3 scripts/tests/test-widget-editor.py \
  --bin "target/$PANELET_HOST_TARGET/debug/panelet-studio" -v
```

直接使用编辑器可执行 `scripts/run-widget-editor.sh`，默认只监听 <http://127.0.0.1:18745>。该脚本保留原名，启动的是 `panelet-studio`；操作与模拟行为见[编辑器说明](widget-editor.md)。

## 浏览器 WASM 与资源包

浏览器预览需要 stable Rust 的 `wasm32-unknown-unknown` target。行为对照使用 Node.js 与上面生成的原生 Studio：

```sh
rustup target add wasm32-unknown-unknown --toolchain stable
scripts/build-preview-wasm.sh
python3 scripts/build-web-assets.py
node scripts/tests/test-preview-wasm.mjs \
  --bin "target/$PANELET_HOST_TARGET/debug/panelet-studio"
node scripts/tests/test-browser-preview.cjs
python3 scripts/tests/test-web-assets-tools.py
python3 scripts/tests/test-web-assets-http.py \
  --bin "target/$PANELET_HOST_TARGET/debug/panelet-studio" \
  --bundle target/web-assets/bundle.pna
```

WASM 产物为 `target/web-assets/raw/preview.wasm`，安装包为 `target/web-assets/bundle.pna`。构建、主机 HTTP 测试和本机沙箱不写真实设备；运行 `deploy-web-assets.py` 或在设备管理页安装则会写入设备 assets，按设备操作范围另行执行。兼容版本、A/B 共用和恢复规则见 [web assets](../os/web-assets.md)。

可在本机启动使用真实文件存储的 assets 沙箱：

```sh
scripts/run-widget-editor.sh --web-assets /tmp/panelet-assets --port 18747
```

它使用模拟容量，不代表设备剩余 Flash 或物理断电恢复保证。预览也不能验证真实颜色、刷新时序、电流或设备运行时内存。

## NOTE4C 固件

当前采用 Espressif Rust 工具链、`ldproxy`、`espflash`，target 为 `xtensa-esp32s3-espidf`，ESP-IDF 固定 `v5.5.3`。工具链准备沿用：

```sh
cargo binstall espup ldproxy espflash
espup install
. "$HOME/export-esp.sh"
scripts/build-mvp.sh
```

若未安装 `cargo-binstall`，先按其安装说明准备该命令，或用 `cargo install` 安装上述工具。macOS 还需要系统 Command Line Tools。

`scripts/build-mvp.sh` 只执行 release 构建并显式选择 `panelet-os` bin，产物是 `target/xtensa-esp32s3-espidf/release/panelet-os`。它不构建预览 WASM、不上传资源、不刷写设备。

`.cargo/config.toml` 为设备 target 配置了刷写 runner：**`cargo run` 会触发设备写入，不是编译检查。** 固件编译通过后仍须根据 [NOTE4C 分区策略](../devices/note4c/firmware-partition-layout.md)核对最新 app image 的大小；历史日志中的字节数不代表新版本容量。

## 脚本与文档检查

修改备份、刷写、配网脚本时运行相应 mock 测试，避免把连接真实设备当作脚本验证：

```sh
scripts/tests/test-backup-factory-flash.sh
scripts/tests/test-flash-server.sh
python3 scripts/tests/test-provision-wifi.py
```

文档修改检查内容、链接和 `git diff --check`。代码改动按影响运行相关检查，修复失败后重跑受影响部分；通过后不必重复扩大测试。

提交或交付时分别记录主机测试、WASM/浏览器验证、目标构建和实机结果。目标构建或预览通过不能替代设备显示、Wi-Fi、输入和恢复的实测。
