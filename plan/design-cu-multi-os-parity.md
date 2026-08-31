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
| **R** | 在 guest 里接上真的 GTK3 树（Xvfb+at-spi2+openbox+PyGObject 固件），把节点动作跑通 | `invoke press`/`set-checked`/`focus`/`focused`/`get-text`/`query` 全部真机通过；抓到 2 个 bug |
| **Q** | 在 lima Ubuntu 26.04 aarch64 上**真跑** Linux 二进制（zig cc 交叉链接，VM 里不装 Rust） | 105 条平台单测在 Linux 执行通过；Xvfb+at-spi2 下 `capabilities` Available、`displays` 读出真实分辨率；抓到 3 个 bug |
| **P** | `apps --all` + `app launch`（ABI 1.21；LaunchServices，不假装有 pid） | `cu-macos-smoke` 28 STEP / 27 EVIDENCE 真机通过 |
| **O** | `app hide/show/quit`（ABI 1.20；`quit` 走应用自己的 Quit 菜单项 + `close` 那套三件套） | `cu-macos-smoke` 27 STEP / 26 EVIDENCE 真机通过 |
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

### 片 Q：把 Linux 真跑一次，抓到三个 bug

**「交叉编译干净」值不了多少钱**——这是本轮最该记住的一条。三个 bug 全是交叉编译 +
clippy 全绿的代码里的：

1. **剪贴板类型探针根本编不过。** 我写了 `Backend::probe()`，真名是
   `ClipboardBackendFacts`。之前每次 `cargo check --target ...` 都没开 `clipboard` feature，
   所以「0 errors」是在一段**编不过的代码**上报出来的。**特性窄的 check 不算 check。**
2. **`capabilities` 把 Rust `Debug` 输出当成状态值发出去了。** `status()` 是把能力的 Debug
   形式转小写：`Available` 恰好变成 `"available"` 看着没问题，其它变成整个结构体——Linux 上
   真的发出了 `"status": "unsupported { reason: \"host adapter unavailable\" }"`。
   macOS 上每个能力都是 Available，**永远看不到这个**。现在动词状态是稳定单词，reason 单独一个字段。
3. **空桌面上 `windows` 直接失败。** 两段式探针假定 cap=0 一定回 `buffer_too_small`；
   **零个窗口时 `cap < required` 是 `0 < 0`，ABI 回的是 `AGT_OK`**，cu 把它当成
   `unexpected_status` 报错。macOS 上永远有窗口，所以永远碰不到。三个 list 探针
   （`windows` / `stacking` / `screens`）都有同一个洞，一起修了。

### 片 R：接上真的 GTK 树，`invoke` 在 Linux 上原来根本走不通

4. **`invoke` 在 Linux 上完全不可达。** cu 在按之前要求节点**列出**这个动作，
   而 AT-SPI 的 walk **故意不读 action 名字**（WebKitGTK 的 `GetActions` 会挂），
   于是每个 Linux 节点都 `actions: []`，守卫把「没问过」当成了「没有」。
   契约里明写「空列表表示后端报告了没有，绝不表示没问」——**是 Linux 适配器违反了契约**。
   现在守卫只在空列表确实是一个断言的后端上生效；其余交给机制自己判（它会 `GetActions`
   再 typed 失败），晚一个来回，但诚实。
5. **GTK 的复选框不发布 `STATE_CHECKABLE`。** 真机上一个没勾的复选框只报
   `enabled,focusable,sensitive,showing,visible`。我的两向补全键在 `checkable` 上，
   所以从不触发，`set-checked` 把每个 GTK 复选框都拒成「状态不可观测」。
   改成**角色词汇**也算信号（`check box`/`radio button`/…），并且角色必须够格——
   普通 button 仍然一个状态词都不加（有单测钉住）。

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

## 1.5 App 生命周期（片 O + P：四个动词都做了）

[`design-mcu-absorption.md`](design-mcu-absorption.md) §1 的「第三批」。**控制那半已经做了**
（`app hide|show|quit`，见下面片 O）；**发现那半（`apps --all`）与 `launch` 没做**，
理由在下一段。下面的机制表与纪律就是片 O 照着落的。

### 片 O 实做时冒出来的两个东西（设计里没预料到）

1. **`show` 不能按窗口句柄寻址。** 隐藏会把这个 App 的窗口从 inventory 里拿掉，
   于是「取消隐藏」时那个句柄已经解析不了了——第一版就是这么写的，真机直接失败。
   所以平台函数改成收 **pid**（pid 活得比句柄久），`app show` 必须给 `--pid`，
   `app hide` 两个都收（趁窗口还在的时候自己查 pid）。
2. **隐藏的回读必须轮询。** 适配器已经等到 `AXHidden` 读回来了，但 window server
   要慢一拍才把窗口从自己的列表里删掉——写完立刻读一次，会把一个成功的 hide 报成 unverified。
   现在按 `close` 的做法轮询（实测 56 ms 落定）。

### 为什么 `apps --all` + `launch` 仍然没做

「列出已安装但没在跑的 App」对 agent 是死路：cu 没有 `launch`，列出来也用不上。
它的价值全在「列 → 起」这条链上，而 `launch` 要新一层机制（LaunchServices / `.desktop`
的 `Exec=` / `ShellExecuteExW`），且 macOS 那条路有个已知坑：`open -a` 把 pid 交给
LaunchServices，**旅程杀不掉自己起的东西**（片 1 踩过）。所以这两个一起留着，
判断写进 [`capability-mcu-cu.md`](capability-mcu-cu.md) 的「还差什么」，不装作是遗漏。

### 机制（三平台）

| 动作 | macOS | Linux | Windows |
|---|---|---|---|
| 列已安装（**未做**） | 扫 `/Applications`、`/System/Applications`（含 `Utilities`）、`~/Applications` 的 `*.app` | XDG `applications` 目录的 `*.desktop`（`Name=` 要解析） | 开始菜单 `*.lnk`（用户 + 全局两处） |
| `launch`（**未做**） | `LSOpenCFURLRef` / `open -a` —— 注意 `open -a` 把 pid 交给 LaunchServices，**旅程杀不掉**（片 1 踩过） | `.desktop` 的 `Exec=`，经 `process_spawn` | `ShellExecuteExW` |
| `quit`（**已做**） | 目标应用**自己的 Quit 菜单项**（后台按下、按 pid 回读），**不是信号** | 同机制未接（`_NET_CLOSE_WINDOW` 是逐窗的，不是退应用） | 同上 |
| `hide`/`show`（**已做**） | `AXHidden` 写应用元素，**按 pid** | 没有应用级隐藏态（那是窗口操作），typed | 同上，typed |

### 纪律（照片 4 的 `close`）

`quit` 与 `close` 同级：**精确目标 + 前置快照 + 可验证后置条件**，缺一即 typed `refused`
且 `effect: not_performed`；每次动作一张崩溃持久收据；回读进程是否真的没了，不信调用返回值。
`launch` 不是破坏性动词但**必须可回收**：旅程只起自有固件，且要能 SIGTERM 收掉——
所以 macOS 那条路不能用 `open -a`。

### 判据

**已达成**（并进 `cu-macos-smoke` 而不是单开一条）：起第二个自有固件 → `apps --all` 里能按名字找到它 → `hide` 后
`windows` 读不到、`show` 后读得到 → `quit` 走三件套并回读进程消失 → 无孤儿、前台窗口与真实指针不变。

## 2. 状态

| 片 | 提交 | 状态 |
|---|---|---|
| A macOS 节点读写 + E 语义 send-keys | `7b577624` + 见下 | **已落地**：`get-extents` / `select` / `get-selection` / `set-caret` / `get-caret` / `send-keys` 真机通过（片 A 当时 23 STEP / 22 EVIDENCE；这条旅程后来长到 **27 STEP / 26 EVIDENCE**，96.7M 步 / 85 页，前台句柄与真实指针始终不动、无孤儿）；`scroll` 已映射 `AXScrollToVisible`，只有 typed 拒绝证据 |
| P 已安装清单 + launch | 见下 | **已落地**：83 个已安装 App，逐行 `running`；起的是本轮自己打的 `.app` 包；**等窗口必须按 App 等**——按标题等会立刻匹配到已经在跑的那个固件，那一步就什么都没证明（第一版就是这么错的） |
| O App 生命周期（控制半） | 见下 | **已落地**：`hide` 后窗口从 inventory 消失但进程还在；`show` 必须按 pid（句柄已经不存在了）；`quit` 无三件套 / 错 pid 都是 `not_performed` 拒绝，带齐了则按下 `ax_fixture/Quit ax_fixture` 并回读进程消失、退出码 0 |
| N 权限报告面 | 见下 | **已落地**：修复路径本来埋在 `tree` 动词里（要先知道是树被拒才找得到），现在一级；macOS 同一份 Accessibility 授权还管所有输入动词，gates 列出全部 24 个；没有权限模型的宿主写 `model: none` 而不是空集 |
| M Linux 截图 + Space 归属 | 见下 | **已落地**：Linux `GetImage` 只认 24/32 位 TrueColor，别的 visual typed 拒绝（不乱解释字节）、64 MiB 像素上限；Space 归属每行带 `spaces`，旅程校验它的 id 都在 `spaces` 清单里 |
| L 剪贴板类型 | 见下 | **已落地**：mac `clipboard info` / Linux 复用 TARGETS 探针 / Win `EnumClipboardFormats`（按系统给的优先序）；`types_available: false` 与「空清单」是两回事 |
| K Linux 窗口操作 | 见下 | **已真机（片 T）**：`close` = `_NET_CLOSE_WINDOW`（是请求不是杀进程，所以闸仍然回读句柄）；`orderwin` = `ConfigureWindow(Above)`，**不动键盘焦点**；topmost = `_NET_WM_STATE_ABOVE`；iconify/maximize/restore 保持 typed（那是 WM 策略，猜了会报假成功） |
| J L/W 后台菜单 | 见下 | **Linux 已真机；Windows 仍映射**：复用各自已验证的有界树 walk，不新开机制；每段先解析后按下，缺失/重名/禁用/非叶子都在动手前拒绝。Linux 真跑（片 S）抓出 **4 个 bug**，见下 —— 这条盲写的路径没有一次是对的 |
| I 原生事件流 | 见下 | **已落地**：macOS AXObserver 订阅在应用元素上，run loop 分片跑到期限；**不替换 poll-diff**——两者互不包含（poll 有 `before`/`after`、看不见「改了又改回去」；通知有到达时序、没有 before/after），默认仍 poll-diff，回复写明 mode |
| H z 序 + 遮挡 | 见下 | **已落地**：mac（CGWindowList 前到后）/ Windows（EnumWindows z 序）/ Linux（`_NET_CLIENT_LIST_STACKING` 反转，**拒绝退回创建序**）；矩形相减在契约层有 6 条单测；真机：两个自有窗口 z=0/1 且 occl=0，把前窗盖到后窗上后者变 100 |
| G MCU 三动作 + selected 两向词 | 见下 | **已落地**：`set-selected` 在 NSTableView 行上 verified + 幂等 no-op（另两行仍 `unselected`）；`cancel`/`show-default-ui` typed 拒绝并列出节点真有的动作；没有该状态的节点在碰机制前就 `state_unobservable` |
| E macOS 输入注入 | 见下 | **已落地**：`cu-macos-pointer-smoke` 4 STEP / 4 EVIDENCE（8.1M 步 / 51 ops / 10 页）；指针移动后**读回并精确复位**；窗口作用域 `--to <handle>`、缺 `--to`、observe-only grant 三种 typed 拒绝且都没动指针 |
| F 网页 AX + unlock poke | 见下 | **已落地**：`cu-macos-web-smoke` 6 STEP / 6 EVIDENCE（5.58M 步 / 80 ops / 5 页），WebArea 树、`scroll` 正向（链接 y 1955→905）、网页 `invoke press`/`set-value`、未聚焦写入 fail-closed |
| B Windows | 见下 | **已落地（映射，未真机）**：`set-expanded`/`select-option`/`increment`/`decrement`/`scroll`/`get-extents`/`focused` + 快照补 `expanded`/`collapsed`；**选区/插入点走 TextPattern**（文档区间量 UTF-16 偏移，`Select()` 后回读）；五个新 pattern/接口的 vtable 槽位全部单测钉死 |
| C Linux | 见下 | **已落地（映射，未真机）**：五个动作 + `focused` + `checked`/`unchecked`/`mixed`、`expanded`/`collapsed` 双向状态词；两条平台单测；linux/aarch64 两个 target `check` + `clippy` 干净 |
| D cu 层 | 并进 K / M | **已落地**：`close` 三平台都有原生关闭控件（片 K 补上 Linux）；`orderwin` 三平台（片 K 补 Linux，不动焦点）；`screenshot` Win GDI + Linux X11（片 M），macOS 是系统拿走了 API；`capabilities` 逐平台如实报，不再硬编码 Linux 拒绝 |

## 3. 片 S：Linux 菜单与几何真机（2026-09-01）

片 J（后台菜单）和树里的 `bounds` 是**整段盲写**的，从没在 Linux 上执行过。给 GTK 固件加上
`Gtk.MenuBar`（File → Do Thing / Disabled Thing / Marked Thing）之后逐条跑，**六个 bug 全部只有
执行才会现形**，其中三个让整个动词不可用：

| # | 症状 | 根因 | 修在哪 |
|---|---|---|---|
| 1 | `menu inspect` 找到了菜单栏但 `items: []` | 调用方的 depth 预算数的是**菜单层数**，平台却把它当成**从窗口根算起的树深度**。macOS 的菜单栏挂在应用根下面，Linux 的被 GTK 埋在 frame→filler 里三层，预算全花在走到菜单栏的路上 | `linux/accessibility_tree.rs`：先用搜索深度定位菜单栏，再按它实际所在的深度重走一遍 |
| 2 | `inspect` 报的路径 `menu invoke` 不接受 | 两边模型不同：macOS 命名的是**条目**（`AXMenuBarItem` "File"）、下面挂一个重名的 `AXMenu`；AT-SPI **没有条目节点**，GTK 的 "File" 本身就是 `menu` 角色。扁平化只认条目角色，于是 Linux 丢了 "File" 这一段 | `cu/observe.rs`：命名的 `menu` 也算一段，除非它是 macOS 那个「被条目直接拥有」的重复节点 |
| 3 | 灰掉的条目报 `enabled: true` | AT-SPI **没有 `disabled` 这个状态词**，禁用就是不发 `enabled`；判据写的是「没有 disabled 就是 enabled」 | 同上：要求正的 `enabled`，两种词汇表都答得对，且错也错在**拒绝**那一侧 |
| 4 | 可勾选条目在 `inspect` 里**根本不存在**，`invoke` 却按得动 | AT-SPI 给它单独的角色 `check menu item`；角色白名单只有 `menuitem`/`menubaritem`。于是 inspect 和 invoke 对「有哪些条目」的回答不一致 | 同上：白名单加 `checkmenuitem`/`radiomenuitem` |
| 5 | 回执 `mark_after: "u"` | ABI 把 mark 定义为**一个 Unicode 标量**；Linux 适配器返回的是状态词 `"unchecked"`，于是被截成首字母。**macOS 永远看不见这个 bug**，因为真的 `AXMenuItemMarkChar` 就是一个字符 | `linux/accessibility_tree.rs`：把状态映射成它代表的字符（✓ / –），并补上原本没读的 `mark_before` |
| 6 | 树里每个节点 `bounds` 都是 `{0,0,0,0}` | 走树时**硬编码为零**，`bounds_from_proxy` 写了但一个调用点都没有。而 `{0,0,0,0}` 正是这个适配器在别处（`extents_or_unavailable`）明确称为「不可用」的哨兵——等于把「没读」当成「读到了一个零面积矩形」发出去，macOS 同一字段发的是真矩形 | 接上 `component_proxy_for`（和文本读一样绕开会挂的 `proxies()`），`ACTION_TIMEOUT` 有界；整棵树多花约 0 ms（33 ms） |

顺带（同一次执行里现形，不属于菜单）：

- **`i32::MIN` 当坐标发出去**：GTK 给未分配的控件（关着的菜单里的条目）报 `1x1 at i32::MIN`，
  只判 `width/height > 0` 放行了它。现在 `is_readable_rect` 一并拒绝未分配原点，`get-extents`
  改成 typed 拒绝并把真实矩形写进消息。
- **失败被读成空清单**：`read_two_stage` 把 `AGT_FAILED` + `out_len==0` 一律当作空载荷。但
  `AGT_FAILED` 同时承担「缓冲区太小（探针的正常答复）」和「调用失败」两件事，只有
  `agt_last_error` 的 code 能分开。后果是**一台根本没装 xclip/xsel/wl-paste 的机器，
  `clipboard-read` 报 `types_available: true, types: []`** —— 把「探针没跑起来」说成了
  「剪贴板上没有别的类型」。同一轮里我先前为「空桌面」加的三个
  `AGT_FAILED if needed == 0 => Ok(vec![])` 分支同样是这个错：空清单本来就走 `AGT_OK`，
  那三个分支只会吞掉真失败，已删除。
- **`AGT_UNSUPPORTED` 被拍平**：它按约定不记录错误，读错误槽会拿到上一次的残留
  （干净时是 `"ok: no error"`）。现在直接映射成 `Unsupported`。
- **Debug 渲染漏进 JSON**：`types_reason` 是 `format!("{error:?}")`，发出去的是
  `Failed { code: "...", message: "..." }`。同一类 bug 本轮已经修过一次。

macOS 旅程（28 STEP / 27 EVIDENCE）在这些改动之后全绿——那才是「Linux 的修法没有掰坏 macOS」
的证据，不是我读代码觉得没事。

## 4. 片 T：`orderwin` 在两个平台上都不是它自称的东西（2026-09-01）

`orderwin` 一直**从「发出了抬升请求」这件事报成功**，从来没读过结果。把读回加上之后，
两个平台各露出一个真问题：

**Linux：动词完全无效，但一直报 ok。** 两层原因，第二层是第一层的伪装：

1. 被管理的窗口被 WM 重父到自己的框架里，`SubstructureRedirect` 让针对客户端窗口的
   `ConfigureWindow` 变成**一个 WM 可以直接丢掉的请求**——openbox 就丢掉了。改用 EWMH 为
   「pager 重排自己不拥有的窗口、且不动焦点」准备的 `_NET_RESTACK_WINDOW`（openbox 在
   `_NET_SUPPORTED` 里确实登记了它）；不登记它的 WM 和无管理窗口仍回退 `ConfigureWindow`。
2. 光这样还是不动。**这个文件里每个 EWMH 发送点都是 flush 完就返回，连接在返回路上被
   drop，服务器还没处理的请求就此丢失。**实测：同一条 restack 消息，连接留着就生效、
   flush 后立刻 drop 就什么都不发生。三个发送点现在都在返回前做一次往返
   （`GetInputFocus`）——这也才让返回 `Ok` 这件事本身站得住。**受影响的不只是抬升，
   `_NET_CLOSE_WINDOW` 和移动/改尺寸走的是同一条路。**

修完在 openbox 上实测：窗口从背后升到最前，读回证实；已经成立的方向是「成功且没动」；
破坏性 `close` 缺三件套时拒绝，齐了则关掉并回读句柄消失。

**macOS：机制做不到，于是现在如实拒绝。** `AXRaise` 是**应用内部**的排序，对一个不在前台的
应用，它不会改变全局窗口列表。实测（两个自有固件窗口，同一个 pid）：请求把 z=1 的窗口放到
最前，结果它变成 z=2，也就是**更靠后**了。cu 不会为了让抬升生效去激活那个应用——那正是所有
动词都在避免的抢前台——所以诚实的答案是**带着实测到的前后 z 值 typed 拒绝**
（`window_order_not_applied`），而不是把 AXRaise 已经发出去当成成功。旅程第 29 步钉的就是
这个拒绝，外加「本来就成立的方向仍然成功」「同一个句柄写两次在动手前 `invalid_input`」
「observe-only grant 到不了机制」。

> 这条是「读回」这个纪律本身的回报：动词没变强，但**它不再说假话**，而且我们第一次
> 知道 macOS 这条路走不通——之前两个平台的成功回复都是假的。

### 还剩什么（2026-09-01，本轮结束时）

| 项 | 为什么还在 |
|---|---|
| ~~Linux **真机**~~ **已做**（片 Q/R/S） | **跑起来了**：本机有 lima VM + `zig cc` 能交叉链接，于是把 Linux 二进制在 Ubuntu 26.04 aarch64 上**真跑**。105 条平台单测在 Linux 上执行通过；起了 Xvfb + at-spi2 + dbus 之后 `capabilities` 报 `tree/windows: Available`、`displays` 读出真实的 1280x800。**代价是抓出三个只有真跑才会现形的 bug**（见下）。**片 R 又装了 openbox + PyGObject 固件**，于是节点动作也真跑了：`invoke press` 让计数标签
从 `pressed 0` 变 `pressed 1`；`set-checked true` performed+verified、再来一次 performed=false
仍 verified、`false` 又变回来；`focus`/`focused`/`get-text`/`query --role` 都通；`windows` 的
`z_index`/`occluded_percent` 来自真的 `_NET_CLIENT_LIST_STACKING`。**又抓到两个 bug**（见下）。**片 S 再加菜单栏**，把后台菜单和树几何也真跑了：**又抓到六个**（见 §3）——盲写的片 J 没有一条是对的 |
| Windows **真机证据** | 本仓没有那台机器，也没有可跑的模拟路径。映射交叉编译 + clippy 干净、vtable 槽位单测钉死，PRD leaf 写 `[~] mapped` |
| macOS `screenshot` | 系统拿走的：`CGWindowListCreateImage` 15.0 从 SDK 移除，ScreenCaptureKit 要另一份 TCC。**不退化成整屏抓图** |
| 剪贴板富内容读取 | 已报类型，读图片/文件字节是另一个策略问题 |
| ~~`apps --all` + `app launch`~~ | **已做（片 P）**。`launch` 用 `LSOpenCFURLRef`（不是 `open -a`，也不是 shell），**回复明说 `pid: null`**——进程归 launcher 管，这个调用没法知道 pid、也没法知道 App 是否真的起来了；要 pid 就等窗口 |
| `scroll` 在 Cocoa 上的正向证据 | AppKit 不发布 `AXScrollToVisible`（三种控件都量过）；正向证据在网页旅程里 |
