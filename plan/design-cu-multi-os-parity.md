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
| `select-option` | ✓ 弹出+子项 AXPress | **✓ 已映射** Selection.SelectChild（名字唯一） | **✓ 已映射** 展开→按名唯一→SelectionItem.Select→复原 |
| `set-checked` | ✓ 期望态 | **✓ 已映射** 期望态 + StateSet 回读 | ✓ TogglePattern |
| `set-expanded` | ✓ 期望态 | **✓ 已映射** 期望态 + StateSet 回读 | **✓ 已映射** ExpandCollapse 期望态 + 回读 |
| `increment` / `decrement` | ✓ AXIncrement | **✓ 已映射** Value + MinimumIncrement，钳到区间并回读 | **✓ 已映射** RangeValue + CurrentSmallChange，同样钳+回读 |

### 0.2 节点级读写（cu 动词 `scroll` / `get-extents` / `select` / `get-selection` / `set-caret` / `get-caret` / `send-keys`）

| 机制 | macOS AX | Linux | Windows UIA |
|---|---|---|---|
| `send_node_keys` | **✓ 语义映射** `enter`→AXConfirm / `escape`→AXCancel，其余 typed（见下） | ✓ | ✓ focus + input-inject |
| `scroll_node` | **✓** AXScrollToVisible | ✓ | **✓ 已映射** ScrollItem.ScrollIntoView |
| `get_node_extents` | **✓** AXPosition+AXSize | ✓ | **✓ 已映射** BoundingRectangle 独立重读 |
| `set/get_node_selection` | **✓** AXSelectedTextRange | ✓ | **✓ 已映射** TextPattern：文档区间量偏移，`Select()` 后回读 |
| `set/get_node_caret_offset` | **✓** AXSelectedTextRange 零长 | ✓ | **✓ 已映射** 退化区间 + `Select()` |

**macOS 这一整列是 2026-08-30 之前的 `PLACEHOLDER cut` 遗留**——AX 三个属性都在，只是没写。

### 0.3 后台动词

| 机制 | macOS | Linux | Windows |
|---|---|---|---|
| `menu_tree_for_window` | ✓ AXMenuBar | **✓ 已映射** 有界树里的 `menu bar` 子树 | **✓ 已映射** UIA `MenuBar` 子树 |
| `invoke_menu_path` | ✓ 唯一解析 + AXPress | **✓ 已映射** 标题路径唯一解析 + AT-SPI 动作 | **✓ 已映射** 标题路径唯一解析 + Invoke |
| `focused_node_for_window` | ✓ AXFocusedUIElement | **✓ 已映射** 有界树里最深的 `STATE_FOCUSED` | **✓ 已映射** 有界树里最深的 `HasKeyboardFocus`（两者 `capabilities` 写明 `mode: state-search`） |

### 0.4 cu 层（非 a11y）

~~`close` Linux 无映射~~ 已接 EWMH `_NET_CLOSE_WINDOW`；~~`orderwin` Linux typed~~ 已接 `ConfigureWindow(Above)`（不碰焦点）；`spaces` 仅 macOS；`observe` 的原生通知只有 macOS（L/W 后端都有事件机制，本轮未接）。

**`screenshot` 在 macOS 上是被系统拿走的**（2026-08-31 实测）：`CGWindowListCreateImage`
**在 macOS 15.0 已 obsoleted 并从 SDK 移除**（本机编译器原话：
"'CGWindowListCreateImage' is unavailable: obsoleted in macOS 15.0 - Please use ScreenCaptureKit
instead"）。替代品 ScreenCaptureKit 是 block 异步 API，且要另一份 TCC 授权（Screen Recording，
不是 Accessibility）。本片只把拒绝理由改成这句实测，**不退化成整屏抓图**——截错东西比 typed
拒绝更糟，而且产品规则本来就是「截图不替代树」。

## 1. 切片（每片一次提交，顺序按「能不能就地拿到证据」排）

| 片 | 内容 | 验收 |
|---|---|---|
| **A** | macOS §0.2 整列：`AXScrollToVisible` / `AXPosition+AXSize` / `AXSelectedTextRange`（选区 + 零长插入点）/ `send-keys`（先 `AXFocused` 再 `CGEventPostToPid`，不激活） | `cu-macos-smoke.qjs` 新 STEP 真机通过 |
| **B** | Windows §0.1 四个动作 + `scroll`/`get-extents` + `focused` | `cargo check --target {x86_64,aarch64}-pc-windows-msvc` + vtable 槽位单测（无真机；只报"已映射"不报"已验证"）；选区/插入点与 menu 留片 G |
| **C** | Linux §0.1 五个动作 + `focused` + 双向状态词 | `cargo check --target x86_64-unknown-linux-gnu`（menu 两个机制留给片 G） |
| **D** | cu 层 §0.4：`close`/`orderwin`/`screenshot` 的缺平台，`capabilities` 逐平台如实报 | 单测 + 交叉编译 |
| **F** | 网页 AX：`unlock`（`AXManualAccessibility` poke，ABI 1.15）+ 自有 `WKWebView` 固件 + `scroll` 正向证据 | `cu-macos-web-smoke.qjs` 真机通过 |
| **N** | `capabilities` 的一级 `permissions` 块：状态 + 修复路径 + 被卡住的动词清单 | 旅程断言（含 `pointer-move` 也在 accessibility 的 gates 里） |
| **M** | Linux 截图（X11 `GetImage` → 共用 PNG writer）+ 逐窗 Space 归属（SkyLight，无需 ABI） | 交叉编译 + clippy；Space 归属有旅程交叉校验 |
| **L** | 剪贴板类型清单（ABI 1.19）：三平台各用原生枚举，不归一化名字 | 本机实测（PNG 在剪贴板时 `text` 空而 `types` 有 9 项）；无旅程步（旅程不该写用户剪贴板） |
| **K** | Linux `close`/`orderwin`/topmost：EWMH client message + `ConfigureWindow(Above)` | 交叉编译 + clippy（未真机） |
| **J** | L/W 后台菜单：在窗口自己的有界树里找 `menu bar`，按标题路径唯一解析后按下 | 交叉编译 + clippy（未真机；`capabilities` 写 `mode: tree-search`） |
| **I** | `observe --mode notifications`（ABI 1.18，macOS AXObserver）；poll-diff 仍是默认 | `cu-macos-smoke` 26 STEP / 25 EVIDENCE 真机通过 |
| **H** | `windows` 的 z 序与遮挡（ABI 1.17）：三平台都用各自的原生栈序，矩形精确相减 | `cu-macos-smoke` 25 STEP / 24 EVIDENCE + 6 条契约层单测 |
| **G** | 最后三个 MCU `invoke` 拼写（`set-selected`/`cancel`/`show-default-ui`，ABI 1.16）+ 补齐 `selected`/`unselected` 两向词 | `cu-macos-smoke` 24 STEP / 23 EVIDENCE 真机通过 |
| **E** | macOS 输入注入：`pointer-move`/`pointer-click`/`type-text`/`send-keys`（全局 HID tap） | `cu-macos-pointer-smoke.qjs` 真机通过（只动指针并复位，不点不打字） |

### 片 E 的两个测量与一个词汇缺口

**测量 1：键送不进非活跃 App。** accessory app 的 ordered-front 窗口 `keyWindow = no`，
`CGEventPostToPid` 投的键事件连 `sendEvent:` 都进不去（探针 app 逐事件打印确认）。
**测量 2：鼠标事件能进 `sendEvent:`，但没有窗口。** 同一个探针收到了
`LeftMouseDown`/`LeftMouseUp`，真实指针没动——**但按钮不响应**，因为事件不带窗口，
AppKit 无从路由（补 `kCGMouseEventWindowUnderMousePointer` 也一样）。
**结论：macOS 没有窗口局部注入**，只有全局 HID tap。所以 `pointer-move --to <handle>`
是 typed 拒绝而不是近似成全局。

**词汇缺口（留给下一片）**：`agenterm.tasks.json` 的 `side_effects` 是**封闭词表**
（`repository_write`/`artifact_write`/`process_spawn`/`gui_spawn`/`network_loopback`/
`git_mutation`/`remote_publish`），**表达不了「动用户的真实指针」**。本片没有偷偷造一个
未经校验的值，只在 task 描述里写清楚；要真正声明它得改 schema + 文档 + 校验，另立一片。

### 片 F 的两个测量

1. **AppKit 发布不了 `AXScrollToVisible`，WebKit 每个节点都发布。** 所以 `scroll` 的正向证据
   只能来自网页目标：固件里 1200px 空白之后的链接，`scroll` 前 y=1955（视口外），
   之后 y=905——**用独立的 `get-extents` 读回来的，不是信 scroll 自己的回复**。
2. **`AXManualAccessibility` 的返回状态是假的。** 它返回 `kAXErrorAttributeUnsupported`(-25205)
   的同时 poke 确实生效（同一个 WKWebView：poke 前 3 个节点，poke 后 14 个）。
   所以 `unlock` 的 `poked` 只表示「请求送到了」，唯一关于树的断言 `grew` 来自前后两次读。
   **另一个测量**：一个会话里只要有任何辅助功能客户端启用过 AX，之后新起的 WebKit 进程
   就直接发布整棵树——所以 `grew: false` 在已建好的树上是正确回报，不是 poke 失败。

### 片 B 的纪律：手写 vtable 必须钉槽位

Windows 适配器不是用 `windows` crate 的接口，而是手抄 vtable 偏移调 COM。
**槽位写错 = 在一台本仓跑不到的机器上调错方法**，编译期完全看不出来。所以三个新 pattern
（ExpandCollapse / RangeValue / ScrollItem）的每一个方法偏移都按 SDK 的 IDL 顺序写进
`raw_vtable_prefix_offsets_match_windows_sdk_slots`，和既有的 Invoke/Toggle/Value 一样。
IID 与顺序取自本机 `windows-0.61.3` 的生成代码，不是凭记忆。

### 片 C 的发现：AT-SPI 只发布「已置位」的状态

`StateSet` 里没有 `unchecked`。所以一个没勾的复选框只带 `checkable`，
和一个**根本没有勾选状态**的控件在 JSON 里长得一模一样——`verify --expect checked:false`
在 Linux 上因此既不能通过也说不出原因。本片让适配器在后端说「这个状态存在」时补出反向词
（`checkable` → `checked`/`unchecked`/`mixed`，`expandable` → `expanded`/`collapsed`），
与 macOS AX、Windows UIA 同一套词。**不 checkable 的节点一个词都不加**，
所以 `checked:false` 对普通按钮仍然 fail-closed（有单测钉住这一条）。

### 片 A 的两个诚实缺口

1. **`scroll` 没有正向证据。** AppKit 不发布 `AXScrollToVisible`：在 `NSButton`、
   同时重写现代与传统 action API 的裸 `NSView`、`NSTableView` 行上都量过，都没有。
   Chromium / WebKit 的网页内容发布（实测一个 Brave 窗口 130 个节点）。所以正向证据需要
   一个旅程自己拥有的网页目标——固件里塞一个 `WKWebView`（`loadHTMLString`，不联网）
   同时还能把 **WebArea 网页 AX** 这条 mcu 对照里的 `[~]` 补成自有证据，记为片 F。
2. ~~**`send-keys` 仍 typed**~~ **已解决，但结论和预期相反**（片 E，2026-08-31）：
   先按「focus + `CGEventPostToPid`」实现，真机不生效——**量出来的原因**是
   accessory app 的 `orderFrontRegardless` 窗口 `keyWindow = no`，投给 pid 的键事件
   连它的 `sendEvent:` 都进不去（写了一个只打印收到的键事件的探针 app 确认）。
   macOS 根本没有「把键给一个非活跃 App」这条路；激活它就破坏后台不变量，
   全局 `CGEventPost` 更糟（落到用户正在打字的地方）。所以改成**语义映射**：
   `enter`→`AXConfirm`、`escape`→`AXCancel`，其余和弦 typed 拒绝并写明这条约束，
   回复里 `via` 是 `ax-action` 而不是 `device-event`——不能读成「键送到了」。
   **macOS 指针注入仍是缺口**（只接了读），`pointer-move`/`pointer-click` 保持 typed。

**诚实条款**：B/C 片在本机只能交叉编译。凡未在真机跑过的映射，`capabilities` 与本页都写 `mapped`（已映射未验证），**不写 `available`**，也不在 PRD 里翻 `[x]`。

## 1.5 还剩一批：App 生命周期（`apps --all` + `app launch/quit/hide/show`）

这是 [`design-mcu-absorption.md`](design-mcu-absorption.md) §1 里唯一没做的「第三批」。
**没有在本轮硬塞进去**，因为 `quit` 是破坏性动词，要配 PRD 31 的三件套 + 收据 + 固件旅程，
在一轮长会话的末尾赶出来会比这份仓库其余部分做得糙。设计先写在这，谁接手都能照着做。

### 为什么 `apps --all` 不能单独做

「列出已安装但没在跑的 App」本身对 agent 是死路：cu 没有 `launch`，列出来也用不上。
它的价值全在「列 → 起」这条链上，所以要么整批做，要么都不做——本轮选后者，并把判断写进
[`capability-mcu-cu.md`](capability-mcu-cu.md) 的「还差什么」，不装作是遗漏。

### 机制（三平台）

| 动作 | macOS | Linux | Windows |
|---|---|---|---|
| 列已安装 | 扫 `/Applications`、`/System/Applications`（含 `Utilities`）、`~/Applications` 的 `*.app` | XDG `applications` 目录的 `*.desktop`（`Name=` 要解析） | 开始菜单 `*.lnk`（用户 + 全局两处） |
| `launch` | `LSOpenCFURLRef` / `open -a` —— 注意 `open -a` 把 pid 交给 LaunchServices，**旅程杀不掉**（片 1 踩过） | `.desktop` 的 `Exec=`，经 `process_spawn` | `ShellExecuteExW` |
| `quit` | 目标进程的 AX 菜单 `Quit`（后台、可回读），**不是 SIGKILL** | `_NET_CLOSE_WINDOW` 逐窗（已接） | 逐窗 `WM_CLOSE`（已接） |
| `hide`/`show` | `AXHidden` 写应用元素 | `_NET_WM_STATE_HIDDEN` / `ConfigureWindow`（raise 已接） | `ShowWindow(SW_MINIMIZE/SW_RESTORE)` |

### 纪律（照片 4 的 `close`）

`quit` 与 `close` 同级：**精确目标 + 前置快照 + 可验证后置条件**，缺一即 typed `refused`
且 `effect: not_performed`；每次动作一张崩溃持久收据；回读进程是否真的没了，不信调用返回值。
`launch` 不是破坏性动词但**必须可回收**：旅程只起自有固件，且要能 SIGTERM 收掉——
所以 macOS 那条路不能用 `open -a`。

### 判据

一条 `cu-macos-app-smoke.qjs`：起自有固件 → `apps --all` 里能按名字找到它 → `hide` 后
`windows` 读不到、`show` 后读得到 → `quit` 走三件套并回读进程消失 → 无孤儿、前台窗口与真实指针不变。

## 2. 状态

| 片 | 提交 | 状态 |
|---|---|---|
| A macOS 节点读写 + E 语义 send-keys | `7b577624` + 见下 | **已落地**：`get-extents` / `select` / `get-selection` / `set-caret` / `get-caret` / `send-keys` 真机通过（`cu-macos-smoke` **23 STEP / 22 EVIDENCE**，81.3M 步 / 287 ops / 73 页；前台句柄与真实指针不动，无孤儿）；`scroll` 已映射 `AXScrollToVisible`，只有 typed 拒绝证据 |
| N 权限报告面 | 见下 | **已落地**：修复路径本来埋在 `tree` 动词里（要先知道是树被拒才找得到），现在一级；macOS 同一份 Accessibility 授权还管所有输入动词，gates 列出全部 24 个；没有权限模型的宿主写 `model: none` 而不是空集 |
| M Linux 截图 + Space 归属 | 见下 | **已落地**：Linux `GetImage` 只认 24/32 位 TrueColor，别的 visual typed 拒绝（不乱解释字节）、64 MiB 像素上限；Space 归属每行带 `spaces`，旅程校验它的 id 都在 `spaces` 清单里 |
| L 剪贴板类型 | 见下 | **已落地**：mac `clipboard info` / Linux 复用 TARGETS 探针 / Win `EnumClipboardFormats`（按系统给的优先序）；`types_available: false` 与「空清单」是两回事 |
| K Linux 窗口操作 | 见下 | **已落地（映射，未真机）**：`close` = `_NET_CLOSE_WINDOW`（是请求不是杀进程，所以闸仍然回读句柄）；`orderwin` = `ConfigureWindow(Above)`，**不动键盘焦点**；topmost = `_NET_WM_STATE_ABOVE`；iconify/maximize/restore 保持 typed（那是 WM 策略，猜了会报假成功） |
| J L/W 后台菜单 | 见下 | **已落地（映射，未真机）**：复用各自已验证的有界树 walk，不新开机制；每段先解析后按下，缺失/重名/禁用/非叶子都在动手前拒绝；勾选状态用树里已有的 `checked`/`unchecked`/`mixed` |
| I 原生事件流 | 见下 | **已落地**：macOS AXObserver 订阅在应用元素上，run loop 分片跑到期限；**不替换 poll-diff**——两者互不包含（poll 有 `before`/`after`、看不见「改了又改回去」；通知有到达时序、没有 before/after），默认仍 poll-diff，回复写明 mode |
| H z 序 + 遮挡 | 见下 | **已落地**：mac（CGWindowList 前到后）/ Windows（EnumWindows z 序）/ Linux（`_NET_CLIENT_LIST_STACKING` 反转，**拒绝退回创建序**）；矩形相减在契约层有 6 条单测；真机：两个自有窗口 z=0/1 且 occl=0，把前窗盖到后窗上后者变 100 |
| G MCU 三动作 + selected 两向词 | 见下 | **已落地**：`set-selected` 在 NSTableView 行上 verified + 幂等 no-op（另两行仍 `unselected`）；`cancel`/`show-default-ui` typed 拒绝并列出节点真有的动作；没有该状态的节点在碰机制前就 `state_unobservable` |
| E macOS 输入注入 | 见下 | **已落地**：`cu-macos-pointer-smoke` 4 STEP / 4 EVIDENCE（8.1M 步 / 51 ops / 10 页）；指针移动后**读回并精确复位**；窗口作用域 `--to <handle>`、缺 `--to`、observe-only grant 三种 typed 拒绝且都没动指针 |
| F 网页 AX + unlock poke | 见下 | **已落地**：`cu-macos-web-smoke` 6 STEP / 6 EVIDENCE（5.58M 步 / 80 ops / 5 页），WebArea 树、`scroll` 正向（链接 y 1955→905）、网页 `invoke press`/`set-value`、未聚焦写入 fail-closed |
| B Windows | 见下 | **已落地（映射，未真机）**：`set-expanded`/`select-option`/`increment`/`decrement`/`scroll`/`get-extents`/`focused` + 快照补 `expanded`/`collapsed`；**选区/插入点走 TextPattern**（文档区间量 UTF-16 偏移，`Select()` 后回读）；五个新 pattern/接口的 vtable 槽位全部单测钉死 |
| C Linux | 见下 | **已落地（映射，未真机）**：五个动作 + `focused` + `checked`/`unchecked`/`mixed`、`expanded`/`collapsed` 双向状态词；两条平台单测；linux/aarch64 两个 target `check` + `clippy` 干净 |
| D cu 层 | — | 未开工 |
