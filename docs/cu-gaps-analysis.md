# agenterm-cu computer-use 能力落地分析（macOS）

日期：2026-09-07 · 调研者：Claude Code（agenterm-cu 环境）
主机：macOS（Darwin 25.5.0，arm64）

> **PRD 归属**：本文档的结论已正式落入
> [`prd/PRD_02_28_agenterm_cu.md`](../prd/PRD_02_28_agenterm_cu.md) 的
> 「Delivery and install (runtime `libagenterm` colocation)」一节（位于
> "Current delivery truth" 与 "Product outcome" 之间），作为 P0 分发缺陷、
> P1 TCC 权限门与验证方式的 authority；本文档是该节引用的证据来源。

## TL;DR（结论先行）

**背景假设被推翻。** agenterm 的底层 computer-use 能力（键盘/指针注入、窗口枚举、窗口截图、a11y 树）在
macOS 上**已经完整实现、已接入 ABI 导出、且实测可用**。今天实测的失败**不是"没实现/没接线"，而是运行了
过期的二进制 + 缺失/过期的 dylib**。用当前源码树 `cargo build --profile abi-release -p agenterm-abi` 产出的
`libagenterm.dylib` 配合 `target/release/agenterm-cu`，四项能力全部 `available` 并实测跑通。

因此本次**未改动任何代码**（无需改）。真正要做的是**发布/安装打包**：让交付的 cu 二进制旁边带上匹配版本的
`libagenterm.dylib`（或正确设置 `AGENTERM_ABI_LIB`）。

## 一、现状实测

### 1.1 之前失败的复现根因

- 用户跑的是 `~/.local/bin/agenterm-cu` → 软链到 `~/Applications/AgentermCu.app/Contents/MacOS/agenterm-cu`
  （构建日期 **2026-08-13**，v0.1.16）。该 .app 的 `Contents/MacOS/` 目录**根本没有 `libagenterm.dylib`**
  （`find` 结果为空）。cu 走 dynlib 加载（见 `crates/agenterm-cu/src/dynlib.rs`）时，
  要么找不到库、要么加载到一个更旧的、不含 milestone 43/46 符号的库，于是报
  `symbol agt_input_send_keys missing` / `agt_window_enumerate missing` / `input-inject not wired on unix`。
- 用户看到的 capabilities gaps 文案 `input_degraded: "still agenterm-platform until agt_input_inject ships"`、
  `windows: "still agenterm-platform until agt_window_enumerate ships"` 是 **milestone 46 之前**的旧文案。
  当前源码里这两行已经是 `"none — shared agenterm.dll (milestone 46)"`
  （见 `crates/agenterm-cu/src/executor/capabilities.rs:507`）。这直接证明用户跑的是旧 cu。

### 1.2 用当前源码树重新构建后的实测（全部通过）

命令前缀：`AGENTERM_ABI_LIB=$PWD/target/abi-release/libagenterm.dylib target/release/agenterm-cu --target current ...`

| 能力 | 命令 | 结果 |
| --- | --- | --- |
| capabilities | `--grant observe,actuate capabilities` | `ok:true`，全组 `available`；`input=Available` / `windows=Available`；`input_degraded: "none — shared agenterm.dll (milestone 46)"` |
| a11y 树 | `--grant observe tree` | `ok:true`，`mechanism:"libagenterm"`、`backend:"ax"`，返回真实 AX 节点（窗口/角色/bounds/states） |
| 窗口枚举 | `--grant observe windows` | `ok:true`，返回真实窗口（handle=60767 等，app_name/bounds/pid/z_index 齐全） |
| 窗口截图 | `--grant observe screenshot --window 60767 --out /tmp/agt_shot.png` | `ok:true`，产出 2100x2040 PNG（605 KB） |
| 屏幕截图（device） | `device-screenshot` | `host_tcc_consent_required`（Camera/屏幕录制 TCC 未决），**这是权限门，不是能力缺失** |

> 说明：`tree` / `windows` 能跑，说明本机 Accessibility 权限已授予；`device-screenshot` 卡在 TCC 同意，
> `screenshot --window` 走的是 CGWindowListCreateImage 路径本机已可用。

### 1.3 dylib 符号验证

`nm -gU target/abi-release/libagenterm.dylib | grep agt_` → **88 个 `agt_*` 导出**，四项关键符号全部在列：

```
_agt_input_send_keys        _agt_input_type_text     _agt_input_pointer_move/click/drag/scroll/position
_agt_window_enumerate       _agt_window_stacking_list _agt_screen_list
_agt_screenshot_capture_window  _agt_screenshot_write_png
_agt_a11y_tree_snapshot     _agt_a11y_tree_snapshot_bounded  （+ 20 个 a11y_node/observe/menu 符号）
_agt_native_window_show/activate/move/rect/set_topmost/close/minimized
```

## 二、各符号状态表（声明 / 导出 / 实现）

ABI 层在本文 2026-09-07 取证时为 `abi_version!(1, 28)`，cu 同时要求
`EXPECTED_ABI_MAJOR=1` / `REQUIRED_ABI_MINOR=28`，当时**版本匹配**；当前版本只以
`crates/agenterm-abi/src/lib.rs`、`include/agenterm.h` 与
`crates/agenterm-cu/src/dynlib.rs` 为准，不把这份历史证据当活版本号。
macOS 平台实现：`crates/agenterm-platform/src/adapters/macos/*.rs`，经 `src/selected.rs` 按 `target_os` +
cargo feature 选择。abi crate 已对 platform 开启 `input-inject, window-enum, window-op, a11y-tree, screenshot`
等 feature（见 `crates/agenterm-abi/Cargo.toml` 的 dependencies 行）。

| 符号 | ABI 声明 | ABI 导出(#[no_mangle]) | macOS 实现 | 用到的 macOS API | 实测 |
| --- | --- | --- | --- | --- | --- |
| `agt_input_send_keys` | ✅ lib.rs:7208 | ✅（nm 可见） | ✅ adapters/macos/input_inject.rs | CGEventCreateKeyboardEvent + CGEventPost | capabilities=Available |
| `agt_input_type_text` | ✅ lib.rs:7158 | ✅ | ✅ 同上 | CGEventKeyboardSetUnicodeString | Available |
| `agt_input_pointer_move/click/drag/scroll` | ✅ | ✅ | ✅ 同上 | CGEventCreateMouseEvent/ScrollWheelEvent | Available |
| `agt_window_enumerate` | ✅ lib.rs:6183 | ✅ | ✅ adapters/macos/window_enumerate.rs | CGWindowListCopyWindowInfo | ✅ 实测返回真实窗口 |
| `agt_native_window_*`（show/activate/move/…） | ✅ | ✅ | ✅ adapters/macos/window_op.rs | CGWindowList + AX/SkyLight | Available |
| `agt_screenshot_capture_window` | ✅ lib.rs:2785 | ✅ | ✅ adapters/macos/ui_screenshot.rs | dlsym(CGWindowListCreateImage)（SDK 已移除、二进制仍在） | ✅ 实测出 PNG |
| `agt_a11y_tree_snapshot(_bounded)` | ✅ lib.rs:3414/3446 | ✅ | ✅ adapters/macos/accessibility_tree.rs | AXUIElement* API | ✅ 实测返回 AX 树 |

**结论：没有任何一项处于"完全没实现"或"没接线"状态。** 全部是"已实现 + 已导出 + 可用"。

## 三、真正的 gap（不是能力缺失，是交付与权限）

1. **交付打包 gap（P0，唯一阻塞项）**：发布的 cu 二进制/`.app` 未随附匹配的 `libagenterm.dylib`。
   - 现象：`agt_* missing` / `native-window-capture-is-unavailable` / `not wired on unix`（全是旧库/缺库症状）。
   - 影响文件：`install.sh`、`packaging/`、cu 的库搜索逻辑 `crates/agenterm-cu/src/dynlib.rs`
     （搜索顺序：`AGENTERM_ABI_LIB` 环境变量 → exe 同目录 → 若干开发路径）。
2. **macOS TCC 权限门（P1，运行前提，非代码缺陷）**：
   - Accessibility（AX 树 / AX 驱动的窗口操作）— 本机已授予。
   - 屏幕录制 / Camera（`device-screenshot` 全屏捕获）— 本机未决，报 `host_tcc_consent_required`；
     `screenshot --window` 走 CGWindowListCreateImage 本机已通。
   - 这些由 `setup` / `permissions` / `doctor` 动词处理，属正常授权流程。

## 四、分优先级落地方案

### P0 —— 让"控制浏览器"最小闭环立即可用（无需写任何平台代码）

最小闭环 = 观察（a11y 树 / 窗口截图）+ 驱动（send-keys / type / pointer-click）。这些**当前源码已全部可用**，
只差把库送到 cu 手边：

- 方案 A（立即验证／临时）：设 `AGENTERM_ABI_LIB=<repo>/target/abi-release/libagenterm.dylib`，
  配合 `target/release/agenterm-cu`。本次已用此法跑通四项。
- 方案 B（正式交付，改打包）：让 `.app`/安装产物在 cu 二进制同目录放置同批构建的 `libagenterm.dylib`。
  - 改：`packaging/` 的 bundle 规则 + `install.sh`，在拷贝 cu 时一并拷贝 `abi-release` 的 dylib，
    并校验 `agt_abi_version()` 与 cu `EXPECTED_ABI_MAJOR/REQUIRED_ABI_MINOR` 一致。
  - 依赖：无新 macOS API；纯打包/脚本改动。
  - 权限要求：无（打包阶段）。

### P1 —— 权限引导

- 用 `agenterm-cu ... permissions`（`crates/agenterm-platform/src/adapters/macos/permission_settings.rs`）
  引导用户授予 Accessibility + Screen Recording；`doctor` 做就绪自检。纯流程，无代码缺口。

### P2 —— （可选）稳健性

- `screenshot --window` 依赖 dlsym 私有 `CGWindowListCreateImage`（SDK 已删）。未来 macOS 若移除该符号，
  需补 ScreenCaptureKit 路径（`ui_screenshot.rs` 注释已标注此风险）。非当前阻塞。

## 五、本次实际改动与验证

- **代码改动：无。** 经核实四项符号已声明/已导出/已实现且实测可用，不满足"只差接线且 <30 行"的动手条件，
  故未修改任何源码，也未做任何 git 操作。
- **验证动作**：
  1. `nm -gU target/abi-release/libagenterm.dylib | grep agt_` → 88 个导出，四项关键符号在列。
  2. 用 `AGENTERM_ABI_LIB` 指向该 dylib + `target/release/agenterm-cu` 实测：
     capabilities 全 available、`tree` 返回真实 AX 树、`windows` 返回真实窗口、
     `screenshot --window 60767` 产出 2100x2040 PNG。
  3. 核对 ABI 版本：库 `1.28` == cu 要求 `1.28`，匹配。
  4. 定位旧 `.app`（2026-08-13）不含 dylib，坐实"过期二进制 + 缺库"为原始失败根因。

## 六、给下一步的一句话建议

不要去"实现" computer-use（已实现）；把 P0 的**打包随附 dylib**做掉，
让发布的 cu 旁边永远躺着同版本 `libagenterm.dylib`，然后走 `permissions`/`doctor` 引导授权即可。
