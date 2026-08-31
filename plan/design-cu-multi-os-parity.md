# agenterm-cu：三平台把动词做实（对齐 mcu 的可替代水平）

| 日期 | 2026-08-31 |
|---|---|
| 目的 | 把 `agenterm-cu` 从「macOS 有真机证据、L/W 同拼写但多处 typed unsupported」推到**三平台同能力**，达到可替代 mcu 的水平 |
| 前置 | [`design-mcu-absorption.md`](design-mcu-absorption.md) 片 1–4（macOS 默认环已闭环）、[`capability-mcu-cu.md`](capability-mcu-cu.md)（MCU ↔ cu 动词对照） |
| 纪律 | 直接在 `main` 上；每片一次原子提交；**typed unsupported 只有在后端真的没有这个机制时才允许**——本页把「没做」和「后端没有」分开数 |
| 范围 | `crates/agenterm-platform/src/adapters/{macos,linux,windows}/accessibility_tree.rs`、cu 侧动词与 capabilities、`scripts/qjs/cu-*.qjs` 旅程 |

## 0. 数出来的现状（2026-08-31 读代码，不是印象）

「✗」= 函数体是 typed unsupported 占位；「✓」= 有真实机制；括号里是该后端**存在但未接**的原生机制。

### 0.1 `perform_node_action` 九个动作

| 动作 | macOS AX | Linux AT-SPI2 | Windows UIA |
|---|---|---|---|
| `click` / `press` | ✓ AXPress | ✓ Action | ✓ Invoke/Legacy |
| `focus` | ✓ AXFocused | ✓ Component.GrabFocus | ✓ SetFocus |
| `set-value` | ✓ AXValue | ✓ EditableText | ✓ ValuePattern |
| `select-option` | ✓ 弹出+子项 AXPress | ✗（Selection iface） | ✗（SelectionItemPattern） |
| `set-checked` | ✓ 期望态 | ✗（Action toggle + STATE_CHECKED） | ✓ TogglePattern |
| `set-expanded` | ✓ 期望态 | ✗（Action expand + STATE_EXPANDED） | ✗（ExpandCollapsePattern） |
| `increment` / `decrement` | ✓ AXIncrement | ✗（Value iface + minimumIncrement） | ✗（RangeValuePattern） |

### 0.2 节点级读写（cu 动词 `scroll` / `get-extents` / `select` / `get-selection` / `set-caret` / `get-caret` / `send-keys`）

| 机制 | macOS AX | Linux | Windows UIA |
|---|---|---|---|
| `send_node_keys` | ✗（focus + CGEventPostToPid） | ✓ | ✓ focus + input-inject |
| `scroll_node` | ✗（AXScrollToVisible） | ✓ | ✗（ScrollItemPattern.ScrollIntoView） |
| `get_node_extents` | ✗（AXPosition+AXSize） | ✓ | ✗（CurrentBoundingRectangle） |
| `set/get_node_selection` | ✗（AXSelectedTextRange） | ✓ | ✗（TextPattern.GetSelection / Select） |
| `set/get_node_caret_offset` | ✗（AXSelectedTextRange 零长） | ✓ | ✗（TextPattern 退化选区） |

**macOS 这一整列是 2026-08-30 之前的 `PLACEHOLDER cut` 遗留**——AX 三个属性都在，只是没写。

### 0.3 后台动词

| 机制 | macOS | Linux | Windows |
|---|---|---|---|
| `menu_tree_for_window` | ✓ AXMenuBar | ✗（role menu-bar 子树） | ✗（HMENU / UIA MenuBar） |
| `invoke_menu_path` | ✓ 唯一解析 + AXPress | ✗ | ✗ |
| `focused_node_for_window` | ✓ AXFocusedUIElement | ✗（STATE_FOCUSED 搜索） | ✗（GetFocusedElement） |

### 0.4 cu 层（非 a11y）

`close` Linux 无 `window_op` 映射；`orderwin` Linux typed；`screenshot` mac/linux typed（Win GDI 有）；`spaces` 仅 macOS；`observe` 三平台都是 poll-diff（非原生通知）。

## 1. 切片（每片一次提交，顺序按「能不能就地拿到证据」排）

| 片 | 内容 | 验收 |
|---|---|---|
| **A** | macOS §0.2 整列：`AXScrollToVisible` / `AXPosition+AXSize` / `AXSelectedTextRange`（选区 + 零长插入点）/ `send-keys`（先 `AXFocused` 再 `CGEventPostToPid`，不激活） | `cu-macos-smoke.qjs` 新 STEP 真机通过 |
| **B** | Windows §0.1 四个动作 + §0.2 整列 + §0.3 三个后台机制 | `cargo check --target x86_64-pc-windows-msvc`（无真机；结论只报"已映射"不报"已验证"） |
| **C** | Linux §0.1 五个动作 + §0.3 三个后台机制 | `cargo check --target x86_64-unknown-linux-gnu` |
| **D** | cu 层 §0.4：`close`/`orderwin`/`screenshot` 的缺平台，`capabilities` 逐平台如实报 | 单测 + 交叉编译 |

### 片 A 的两个诚实缺口

1. **`scroll` 没有正向证据。** AppKit 不发布 `AXScrollToVisible`：在 `NSButton`、
   同时重写现代与传统 action API 的裸 `NSView`、`NSTableView` 行上都量过，都没有。
   Chromium / WebKit 的网页内容发布（实测一个 Brave 窗口 130 个节点）。所以正向证据需要
   一个旅程自己拥有的网页目标——固件里塞一个 `WKWebView`（`loadHTMLString`，不联网）
   同时还能把 **WebArea 网页 AX** 这条 mcu 对照里的 `[~]` 补成自有证据，记为片 F。
2. **`send-keys` 仍 typed。** macOS 把和弦送给某个节点只能向属主进程投 CGEvent，
   本适配器不投任何事件。它和 macOS 指针注入是同一个缺口，一起放片 E。

**诚实条款**：B/C 片在本机只能交叉编译。凡未在真机跑过的映射，`capabilities` 与本页都写 `mapped`（已映射未验证），**不写 `available`**，也不在 PRD 里翻 `[x]`。

## 2. 状态

| 片 | 提交 | 状态 |
|---|---|---|
| A macOS 节点读写 | 见下 | **已落地**：`get-extents` / `select` / `get-selection` / `set-caret` / `get-caret` 真机通过（`cu-macos-smoke` 22 STEP / 21 EVIDENCE，80.2M 步 / 274 ops / 72 页）；`scroll` 已映射 `AXScrollToVisible`，只有 typed 拒绝证据 |
| B Windows | — | 未开工 |
| C Linux | — | 未开工 |
| D cu 层 | — | 未开工 |
