# qjswasm native door — 目标资格账（S5 第一片）

本文件是**机器事实账本**：每格标注 **真机执行 / 仅编译 / BLOCKED**，不把编译当运行，不把 debug 当 release。
它**不**声称任何未列出的能力。所有数字都附可复跑命令。

## 1. Source identity 与工具链

| 项 | 值 |
|---|---|
| 仓库 | `partnernetsoftware/agenterm` |
| **HEAD（source identity）** | `93a46fcd2291c69e1ee58f77348ab8f12d690e3a`（short `93a46fcd`） |
| 采集时间 | 2026-09-12（本机 macOS，Apple Silicon） |
| `rustc -V` | `rustc 1.97.0 (2d8144b78 2026-07-07)` |
| `cargo -V` | `cargo 1.97.0 (c980f4866 2026-06-30)` |
| `cargo xwin --version` | `cargo-xwin-xwin 0.23.1` |
| `zig` | 可从 `PATH` 解析 |
| `cargo zigbuild` | `cargo-zigbuild` 子命令存在 |
| 编译 lane | `CARGO_TARGET_DIR=target/native-s5-linux`（采集后清理） |

```sh
# From repository root.
git rev-parse HEAD
rustc -V; cargo -V; cargo xwin --version; command -v zig
```

## 2. Host（darwin/arm64）——**真机执行**

| 格 | 命令 | 结果 | 状态 |
|---|---|---|---|
| native-door **schema** | `cargo test -p agenterm-qjswasm --test native_door_schema` | `test result: ok. 9 passed; 0 failed` | **真机执行** |
| native-door **runtime** | `cargo test -p agenterm-qjswasm --test native_door` | `test result: ok. 10 passed; 0 failed` | **真机执行** |

两者都在本机把 guest 真正跑起来（不是 `check`/`clippy`）。S4 遗留的 `a_fourth_capability_is_only_an_additional_wat_guest`
在**本 HEAD 已恢复为 ok**（见 §5「第五点偏差」）。

## 3. 跨目标格

| 目标 | 命令 | 结果 | 状态 |
|---|---|---|---|
| Windows **x86_64** MSVC | `cargo xwin clippy --target x86_64-pc-windows-msvc -p agenterm-qjswasm --all-targets -- -D warnings` | `Finished`（无诊断） | **仅编译**（不声称 Windows runtime） |
| Windows **aarch64** MSVC | `cargo xwin clippy --target aarch64-pc-windows-msvc -p agenterm-qjswasm --all-targets -- -D warnings` | `Finished`（无诊断） | **仅编译**（不声称 Windows runtime） |
| Linux **x86_64** | `cargo zigbuild --target x86_64-unknown-linux-gnu -p agenterm-qjswasm --all-targets` | `Finished`（4 个写集外 `agenterm-platform` dead-code warnings） | **仅编译**（不声称 Linux runtime） |

先前把 `check` 放在 `cargo zigbuild` 后面会得到 usage 与 `rc=2`；这不是环境阻塞，而是命令形状错误。
更正后的命令完成了 Linux x86_64 编译。它没有执行 Linux 二进制，因此这里只登记**仅编译**证据。

## 4. 门面计数复算

| 计数 | 声明值 | 本片复算 | 依据 |
|---|---|---|---|
| raw imports | **8** | **8 ✓** | `crates/agenterm-qjswasm/src/host.rs:209`：`const SIGNATURES: [(&str, usize, usize); 8] = [`（源码直接可数） |
| compiler declarations | **5 + native 1** | **5 + 1 = 6 ✓** | 运行期读数：`door_declarations().len() + native_door_declarations().len() == 6`，由 `tests/native_door.rs` 的第五能力测试断言并通过；`native_door_declarations()` 按 `field == "native_call"` 过滤，见 `src/lib.rs:468-473` |
| exact stubs | **49** | **49 ✓（生产 admission + 独立枚举）** | `22b79c0f` 让生产 invocation 与 cardinality 共用唯一的七-family admission helper；私有单测独立枚举七种 exact Rust 类型 × arity 0..=6，逐项确认 49 个组合 admitted，并确认异构、pointer、窄整数、void、f32 与 arity 7 被拒绝 |
| schema patterns | **381** | **381 ✓（独立枚举）** | `crates/agenterm-qjswasm/tests/native_door_schema.rs:410` 起的 `register_pattern_cardinality_is_derived_from_classes_and_arity`，在 `:436` 断言 `assert_eq!(independently_enumerated, 381)`，该测试**在本机通过**；派生自 `MAX_NATIVE_ARITY = 6`（`src/native.rs:25`）与 class 基数 |

复跑：
```sh
rg -n 'const SIGNATURES' crates/agenterm-qjswasm/src/host.rs
cargo test -p agenterm-qjswasm --test native_door_schema register_pattern_cardinality -- --exact
cargo test -p agenterm-qjswasm --test native_door a_fifth
rg -n 'NativeType::(I32|U32|I64|U64|Isize|Usize|F64)|invoke_[0-6]' crates/agenterm-qjswasm/src/native.rs
```

## 5. S4 基线 → 本 HEAD：生产 Rust 不变性与第五点偏差

- **生产 Rust 聚合哈希相等（复算证明，非引用）**：
  ```sh
  # 本 HEAD
  find crates/agenterm-qjswasm/src -name '*.rs' | sort | xargs shasum -a 256 | shasum -a 256
  # => 44241fd41b550705a6de8d0c6bf24caf3300243d2071b4e3ffff45aaf68d0e51
  # S4 基线 b6755b0f（在 repo-local 临时目录展开同一子树后复算）
  mkdir -p target/s5-baseline && git archive b6755b0f crates/agenterm-qjswasm/src | tar -x -C target/s5-baseline
  find target/s5-baseline/crates/agenterm-qjswasm/src -name '*.rs' | sort | xargs shasum -a 256 | sed 's#target/s5-baseline/##' | shasum -a 256
  # => 44241fd41b550705a6de8d0c6bf24caf3300243d2071b4e3ffff45aaf68d0e51
  ```
  ⇒ **`b6755b0f` 与 `93a46fcd` 的 `crates/agenterm-qjswasm/src/**/*.rs` 聚合哈希相等**：
  S4 新增第五能力（与 S1–S4 各片）**没有改动任何生产 Rust**。
- **第五点偏差（S4 报告的既有异常）现状**：S4 时 `a_fourth_capability_is_only_an_additional_wat_guest`
  以 `Door("native_spec_malformed: expected library|symbol|result(parameters)")` **在基线提交上即为红**（当时已用
  "以 HEAD 版文件原地替换 + 哈希还原"证明与 S4 改动无关）。**在本 HEAD 该测试已为 `ok`** ⇒ 偏差已被后续提交消化，
  **不再**是未解释红。复跑：`cargo test -p agenterm-qjswasm --test native_door a_fourth`。

## 6. Release 配置

| 层 | 状态 |
|---|---|
| **L1** | **未测定** |
| **L2** | **未测定** |
| **L3** | **未测定** |

上表 §2/§3 的全部数据都来自 **debug**（`dev` profile）构建；**不得**用 debug 结果替代 release 结论。
本片未运行任何 release 构建或 release 资格。

## 7. 本片未声称项（逐条列白）

- **未声称** Windows 上的 native-door **runtime**（§3 两格只有 clippy 编译证据）；
- **未声称** Linux x86_64 runtime（§3 只有编译证据）；
- **未声称** release L1/L2/L3（§6 全部未测定）；
- **未声称** door 的 "eighth door" 名称等值于计数 6——本片只登记**实测**的 `5 + 1 = 6` 与 `SIGNATURES = 8`，两者的关系留给裁决；
- 本片**未**改任何代码 / Cargo / plan / PRD（唯一新增文件即本文件）。

## 8. 复核方式

```sh
./scripts/doc-redact-check.sh research/qjswasm-native-door/RESULTS.md   # 期望输出：clean
git diff --check                                                      # 期望：无输出
```
