# Rune 0.14.2 嵌入 footprint PoC

日期：2026-09-03。此实验只生成本地构建产物；没有连接、刷写或读取任何 NOTE4C，也没有触碰 factory backup。

## 结论

Rune `=0.14.2` 以 `default-features = false, features = ["std"]` 成功链接到当前的 `xtensa-esp32s3-espidf` / ESP-IDF `v5.5.3` release 固件。为使数字代表最小可运行嵌入场景，PoC 没有调用 `Context::with_default_modules()`，而只安装脚本所需的 `core + clone + cmp + vec` 模块。

第三变体已经实测：host 使用同一模块/ABI 把 fixture 编成普通 `Unit<ArrayUnit>`，以 72 B 自有头 + CBOR 封装为 **1,467 B**，target 只做容器校验、反序列化、RuntimeContext/VM 和执行。host `42`、`while true {}` budget 终止、损坏/截断/版本拒绝均通过；但这依赖一个**仅供实验的 Rune-core ItemBuf serde 修复**，不能当作官方 Rune 0.14.2 的可生产预编译格式。

更关键的是，最终 target map 仍链接 Rune 的 `compile`、`query`、`parse::parser` 和 `indexing` 符号。也就是说应用层没有编译源代码的调用路径，但 Rune 0.14.2 的 `std` 产物没有把前端从链接结果剥掉；加上 CBOR decoder，所谓 runtime-only image 反而比现有 compiler+VM 变体大 **51,328 B**。

最初的 `factory` app 分区只有 1 MiB；新的 runtime-only image 是 **2,162,912 B**，当时 `espflash save-image` 以 `image_too_big` 拒绝它。后续已增加并本地验证 NOTE4C 16 MiB 双 OTA 分区，当前同一实验变体为 2,162,816 B，可装入 4032 KiB 的 `ota_0`/`ota_1`，并保留 8 MiB `assets`。详情见[固件分区与恢复策略](../../docs/devices/note4c/firmware-partition-layout.md)与[新分区构建证据](artifacts/partition-layout-16m-2026-09-03.txt)；仍未连接或刷写设备。

| 指标 | 基线 | compiler+VM | 预编译 Unit consumer（实验补丁） |
| --- | ---: | ---: | ---: |
| `.flash.text` | 331,044 B | 1,761,288 B | 1,788,932 B |
| `.flash.rodata` | 105,524 B | 269,012 B | 292,692 B |
| `.flash.appdesc` | 256 B | 288 B | 288 B |
| 直接 Flash section 合计 | 436,824 B | 2,030,588 B | 2,081,912 B |
| ESP-IDF app image（含镜像头/对齐） | 517,792 B | 2,111,584 B | **2,162,912 B** |
| 1 MiB factory 分区余量 | +530,784 B | -1,063,008 B | -1,114,336 B |
| `.dram0.data` | 14,578 B | 14,610 B | 14,610 B |
| `.dram0.bss` | 3,664 B | 3,664 B | 3,664 B |

`size -B` 的 `text/data/bss` 总数包含 ESP-IDF linker 的 `*.dummy` 容量占位，不能当作真实 Flash/RAM 使用量；本表采用直接 section 与 `espflash save-image` 的 app/partition 校验。原始 `size -A/-B` 输出均已保留在 [artifacts](artifacts)；最终链接 map 保留在忽略的 `target/rune-footprint-poc/*/.../libespidf.map`，并已确认 Rune rlib 位于 Rune map 中。

## 同源比较条件

三侧均为同一个 `panelet-os`、同一个 `Cargo.lock`、同一 ESP-IDF、target、linker、rustflags 和 release profile（`opt-level = "s"`）。构建变量是：

- 基线：不启用 feature。
- compiler+VM：`--features rune-footprint-poc`，使可选的 `panelet-rune-footprint-poc` 生效。
- 预编译 Unit consumer：`--features rune-runtime-only-footprint-poc`，使可选的 `panelet-rune-runtime-only-poc` 生效，并显式使用实验目录的 `rune-core 0.14.2` ItemBuf serde patch；其 host-only package generator 不参与 target 依赖图。

构建命令、工具链版本、ELF SHA-256 和初始隔离构建的错误记录在 [build-and-toolchain-2026-09-03.txt](artifacts/build-and-toolchain-2026-09-03.txt)。`CARGO_WORKSPACE_DIR="$PWD"` 是必要的：单独改 `CARGO_TARGET_DIR` 会使 embuild 误判 workspace。

```sh
CARGO_WORKSPACE_DIR="$PWD" ESP_IDF_SYS_ROOT_CRATE=panelet-os \
  CARGO_TARGET_DIR="$PWD/target/rune-footprint-poc/baseline" \
  cargo +esp rustc --locked --release -p panelet-os -- \
  -C link-arg=-Wl,--Map="$PWD/target/rune-footprint-poc/maps/baseline-note4c-server.map"

CARGO_WORKSPACE_DIR="$PWD" ESP_IDF_SYS_ROOT_CRATE=panelet-os \
  CARGO_TARGET_DIR="$PWD/target/rune-footprint-poc/rune-std" \
  cargo +esp rustc --locked --release -p panelet-os \
  --features rune-footprint-poc -- \
  -C link-arg=-Wl,--Map="$PWD/target/rune-footprint-poc/maps/rune-std-note4c-server.map"

CARGO_WORKSPACE_DIR="$PWD" ESP_IDF_SYS_ROOT_CRATE=panelet-os \
  CARGO_TARGET_DIR="$PWD/target/rune-footprint-poc/rune-runtime-only" \
  cargo +esp rustc --locked --release -p panelet-os \
  --features rune-runtime-only-footprint-poc -- \
  -C link-arg=-Wl,--Map="$PWD/target/rune-footprint-poc/maps/rune-runtime-only-note4c-server.map"
```

依赖固定在 `crates/panelet-rune-footprint-poc/Cargo.toml`；参考 [Rune 0.14.2 发布页](https://crates.io/crates/rune/0.14.2)、[`budget::with`](https://docs.rs/rune/0.14.2/rune/runtime/budget/fn.with.html) 和 [`limit::with`](https://docs.rs/rune/0.14.2/rune/alloc/limit/fn.with.html)。实验补丁所需的 `rune-core 0.14.2` 快照位于 `3rd-res/vendor/`，来源、许可证和唯一修改已登记在 `3rd-res/SOURCES.md`。

## 静态镜像与分区证据

当前目标的分区为 `nvs 24K @ 0x9000`、`phy_init 4K @ 0xf000`、`factory app 1M @ 0x10000`。基线 `espflash save-image --merge --skip-padding` 已生成 583,328 B 的本地合并文件；Rune 使用完全相同命令时，`espflash 4.5.0` 以 `image_too_big` 拒绝它。这是预期的本地容量检查，不是板上失败。

- [基线 size 原始输出](artifacts/size-baseline-2026-09-03.txt)
- [Rune size 原始输出](artifacts/size-rune-std-2026-09-03.txt)
- [预编译 Unit consumer size、map 反证与静态 RAM](artifacts/size-rune-runtime-only-patched-2026-09-03.txt)
- [Unit package、host round-trip 与无补丁失败证据](artifacts/precompiled-unit-runtime-only-2026-09-03.txt)
- [分区解析、镜像命令与 espflash 输出](artifacts/image-and-partition-2026-09-03.txt)

## 板上 RAM 采样固件（尚未执行）

`app/os/src/rune_footprint.rs` 只在 `rune-footprint-poc` feature 下运行，在安全 Board 初始化和日志初始化之后、正常按键菜单之前输出 JSON。每个阶段采样：

1. `pre_rune_after_safe_board_init`
2. `source_created`
3. `context_created`
4. `runtime_context_created`
5. `unit_compiled`
6. `vm_created`
7. `simple_script_executed`
8. `instruction_budget_checked`
9. `memory_limit_checked`
10. `vm_cleared`
11. `all_rune_objects_dropped`

每条 `rune_footprint_heap` JSON 同时记录系统默认 heap、`MALLOC_CAP_8BIT`、`MALLOC_CAP_INTERNAL` 与 `MALLOC_CAP_SPIRAM` 的 `free_bytes`、`minimum_free_bytes`、`largest_free_block_bytes`（系统默认项无 largest API）。`minimum_free` 是自开机以来的低水位，故不是每阶段瞬时差值。这里保留的 2026-09-03 构建证据来自当时明确禁用 PSRAM 的配置；日志现在按实际构建配置动态报告 `psram_configured`，不能把旧制品或 macOS RSS 当作当前 ESP32 RAM 证据。

没有板上日志，所以本 PoC **没有任何实机 RAM 数字或实机 sandbox 结论**。

预编译 Unit consumer 的 `app/os/src/rune_runtime_only_footprint.rs` 同样只在其 feature 下、在安全 Board 初始化和日志初始化之后输出 `rune_runtime_only_footprint_heap` JSON。它逐阶段采样 `pre_decode`、`decoded_unit`、`runtime_context_created`、`vm_created`、`script_executed`、`instruction_budget_checked` 和 `all_runtime_objects_dropped`（另有 context 构造阶段）。字段同样区分默认 heap、8-bit、internal 和 SPIRAM，并按实际配置报告 `psram_configured`。该代码已静态链接，但没有刷写或执行，因此不是实机 RAM 数据。

## 预编译 Unit 的格式与边界

`fixtures/minimal-array-unit-v1.rnu` 是 72 B 固定头加 1,395 B CBOR payload。头含 magic、container/Rune 版本、ArrayUnit 标识、模块集/host ABI、长度、payload SHA-256、codec 和保留位；target 在 decode 前限制 blob ≤ 65,608 B、payload ≤ 65,536 B，并以 Rune allocator 的 192 KiB 限额进行 decode。它拒绝多余 CBOR 字节。

新生成的 Panelet 预编译 Unit 使用 `.pnu` 后缀；容器格式不变，旧 `.rnu` 文件仍可读取。上面的历史 fixture、校验值和 `include_bytes!` 引用保留原名，用于复现实验。当前主机生成命令为：

```sh
PANELET_HOST_TARGET="$(rustc +stable -vV | sed -n 's/^host: //p')"
cargo +stable run -p panelet-rune-unit-compiler-poc \
  --target "$PANELET_HOST_TARGET" -- \
  --output target/minimal-array-unit-v1.pnu
```

编译器要求显式 `--output`，按调用者指定的路径写入；它不会仅因旧后缀拒绝文件。这里使用明确的主机 target，不会调用设备刷写 runner。

这些是输入面缩小措施，不是上传者认证。Rune 0.14.2 没有可用于完整 hostile-Unit 验证的 API；生产系统只能接受签名的可信 package generator 输出，并另行补齐 key rotation、anti-rollback、资源限制审计和 fuzzing。Unit header 锁定精确 Rune/ABI，但它不承诺跨 Rune 版本或跨平台的稳定二进制协议；本实验尚无板上执行证据。

## 官方与社区先例

Rune 0.14.2 的官方 CLI loader 确实会把 `Unit` 作为 `.rnc` 本地缓存，以 bincode 写入并在后续运行时读回；官方 release notes 也记录过 byte-code 和将 Unit 指令、调试信息分开序列化的工作。这证明 Rune 项目考虑并使用过“先编译、后加载”，但 `.rnc` 只按源文件修改时间失效，没有 Rune 版本、Context/ABI、校验或签名，因此不是公开承诺的发布格式或跨架构 ABI。参见 [0.14.2 loader.rs](https://github.com/rune-rs/rune/blob/0.14.2/crates/rune/src/cli/loader.rs)、[Rune releases](https://github.com/rune-rs/rune/releases) 和 [相关 PR #549](https://github.com/rune-rs/rune/pull/549)。

本实验还直接复现了官方 `.rnc` 所走 bincode 路径无法序列化该最小 Unit 的问题，详见 host evidence artifact。检索官方仓库、issues/discussions 和公开代码后，没有找到可核验的 macOS/其他桌面架构产出 Rune Unit、再部署到 ESP32/Xtensa 执行的案例。准确表述应是“未找到已验证先例”，而不是断言社区中绝对不存在。

## sandbox 探针与已验证边界

可移植 crate 的主机测试运行以下三件事：简单脚本返回 `42`、`while true {}` 包在 `budget::with(1024, ...)` 中并返回错误、通过连续 `Vec::push` 的压力脚本包在 `limit::with(4096, ...)` 中并返回错误。

```sh
cargo +stable test -p panelet-rune-footprint-poc \
  --target aarch64-apple-darwin
```

这证明 Rune 0.14.2 在主机上以本 PoC 的 API 路径能终止该 Rune VM 无限循环、也能以普通错误返回内存额度失败。它不证明板上已执行；`budget` 只约束 Rune VM 指令，不是 Rust/native 回调或阻塞 I/O 的墙钟 watchdog。`limit` 只追踪由 Rune allocator 计入的 payload，不是 ESP-IDF allocator 元数据、Context/Unit 预先分配或任意 Rust 堆分配的全局 OOM 上限。

## 下一步（需另行授权）

16 MiB 双 OTA 分区现已完成本地构建和容量验证，详见[固件分区与恢复策略](../../docs/devices/note4c/firmware-partition-layout.md)。它会改变分区表并擦除 `otadata`；只有完成仓库既有完整 16 MiB factory backup、确认安全状态、明确串口和接受分区覆盖后，才可用 `scripts/flash-server.sh` 受控刷写 feature 变体并采集上述 JSON。首次串口刷写只安装 `ota_0`，在 `ota_1` 也被写入并验证之前不能宣称已有 A/B 回退。
