# 把 `moltbaby/skills/mcu` 的 PRD 吸收进 `agenterm-cu`

| 日期 | 2026-08-30 |
|---|---|
| 目的 | 评审 mcu（活的桌面控制实验室），决定哪些进 agenterm-cu 的 PRD、以什么顺序实现、怎么用 `.qjs` 旅程驱动 |
| 来源 | `~/repos/moltbaby/skills/mcu/{PRD.md,PRD-MAP.md,CAPABILITY-TREE.md,SKILL.md}`（2026-08-30 读的版本） |
| 纪律 | PRD 14：吸收的是**命令集与分层的教训**，不搬 TypeScript；实现按 agenterm 的 `Command` / grant / `libagenterm` ABI 写 |
| 范围 | 只改 `crates/agenterm-cu/**`、`crates/agenterm-platform` 的 a11y 契约、`prd/PRD_02_28..32`、`scripts/qjs/cu-*.qjs`；引擎侧由另一条线推进 |

## 0. 两边现状（数出来的，不是印象）

**mcu**（Bun/TS，~40 个子命令族）：一个统一的 machine control plane——desktop / browser / shell / PTY / process / storage / device / privilege / runtime。
自评：门厅 runtime 与正殿 desktop 的 macOS 部分 `[✓]`（真机证据），Linux/Windows 语义树「有合约缺真机」，
privilege/storage/device 多为 plan-only。它自己的北极星写得很清楚：**默认环 = `windows` → 有界 `query`/`tree` → `invoke <selector>`，
网页走系统 AX WebArea，`elements` 编号是次路径，截图最后手段**；四条不变量（后台不抢前景、fail-closed、投递≠成功、destructive 三件套）已成闸。
PRD-MAP 末行：「agenterm 迁入 → 桌面环稳定 + 不提前搬仓」。

**agenterm-cu**（Rust，32 k 行，5 份 PRD，22 个动词）：`windows / tree / click / focus / send-text / send-keys / copy / paste / wait /
scroll / get-text / select / get-selection / set-caret / get-caret / get-extents / screenshot / pointer-* / clipboard-read / capabilities /
window-place`，寻址是 `--window HANDLE --name PAT [--role ROLE]` 或 `--node <path>`；三层 target（current / ssh / vnc，rdp 占位）；
Linux AT-SPI2 与 Windows UIA 有证据（`cu-windows-smoke` 七张收据），macOS `current` 在 PRD 30 里写成占位（AX observe stub「code present, live NOT claimed」，
actuation「not started」；`executor.rs:755`）——**但 2026-08-30 本机实探：`windows` 返回真句柄（Brave Origin#14278 …），`tree --window 14278` 返回真 AX 树
（`backend: ax`, `degraded: false`，节点有 id/role/name/bounds/states），只是 `actions` 全空、没有 node budget、没有 `query`。观察半边其实已经活着，缺的是证据、动作与有界性。**授权/审计（PRD 31）17 条 `[ ]`。

**交叉点**：`agenterm-platform` 已有 typed `accessibility_tree` 契约（`tree_for_window / perform_node_action / set_node_text / get_node_text /
scroll_node / get_node_extents / set_node_selection / set_node_caret_offset …`）且 macOS 适配器里有 AXUIElement FFI；qjs 的 tool 门另有一套
`process.window_{key,pointer,message,resize,rect,control}` 直连 platform——**同一台机器上现在有两条计算机使用面**（cu 的动词、门的 window ops），
mcu 的教训之一正是「一条入口」。

## 1. 评审结论：吸收什么、不吸收什么

| mcu 的东西 | 决定 | 落到哪 |
|---|---|---|
| 默认控制环 `windows → query/tree → invoke selector`，`verify --expect`，`wait` | **吸收，第一优先** | PRD 29 新动词 `query`、`invoke`、`verify`；PRD 30 macOS `current` 成为第一平台 |
| 稳定句柄 `App#n`、窗口 inventory 过滤（pid/app/title/focused/onscreen/minimized/occluded）+ 分页/截断计数 | 吸收 | PRD 29 `windows` 扩展 |
| 有界树采集（depth + node budget 在遍历期同时生效，不先造无界树）、`treeMeta` 截断标记 | 吸收（已有 depth，补 node budget + 截断标记） | platform `accessibility_tree` 契约 + PRD 30 |
| selector 文法（`AXToolbar / AXButton@Run`、`[0]`、`@identifier`）与 `find` | 吸收文法思想，拼写按 agenterm 现有 `--name/--role/--node` 收敛 | PRD 29 |
| `elements` 编号 flatten（次路径） | 吸收为 `tree --flat`（同一 flatten 下标供 `invoke --index`） | PRD 29 |
| `focused` App-local 读写、`observe` 事件流 | 第二批 | PRD 29 |
| `menu inspect / invoke`（后台、不 activate） | 第二批（macOS 特有价值高） | PRD 29/30 |
| app `launch/quit/hide/show`、窗口 `frame` 事务、Space/z-order 只读 | 第三批；`frame` 与 PRD 32 window-place 合并 | PRD 32 |
| 四条不变量（后台不抢前景/键焦/鼠标；fail-closed；verified/unverified；destructive 三件套）+ typed `unsupported/degraded/denied/needs-privilege` | **吸收为 PRD 31 的闸**，先于任何新动词 | PRD 31 |
| session/lock/receipt、crash-persistent effect receipt、audit query | 吸收其**形状**（每个写动作一张收据）；实现复用 PRD 31 的 grant store + audit | PRD 31 |
| browser：系统 AX WebArea 主路径 | 随默认环一起来（WebArea 只是 role）；扩展/Native Messaging/CDP **不吸收** | PRD 29 注一句 |
| shell / PTY / job / process introspect | **不进 cu**——这是 AgenTerm 本体（tabs/PTY/mux）与 qjs 门 `process.*` 的地盘；重复建就是第三条入口 | 记在 PRD 28「边界」 |
| Simulator / UTM guest、storage / device / network / power / login-session / privilege broker | **不吸收**（B 区：需求为零或平台策略未定） | PRD 28 边界 |
| runtime daemon / helper v40 协议 | 不吸收；agenterm 的等价物是 `libagenterm` + worker 协议 | — |

## 2. 实现安排：三片，每片一条 `.qjs` 旅程作验收

qjswasm 在这里的角色：**每一片的黑盒证据都是一段 `.qjs`**（像 `cu-windows-smoke.qjs` 那样，`--profile tool`，
`process.command` 调 `agenterm-cu --json`），旅程越写越多，引擎的账单/取消/命名拒绝就越被真实脚本练到。

### 片 1 —— macOS `current` 的默认环有界、有证据（先只读）—— **已落地 `5abb85ee`（2026-08-30 深夜）**

验收（评审者本机重跑）：`cu-macos-smoke.qjs` `success`，7 STEP / 6 EVIDENCE，固件被 SIGTERM 回收无孤儿；账单 `steps 6.97M / host_ops 182 / host_bytes 40 KB / waited 0 / heap 8 页`。
意外：TextEdit 不能当固件（系统 App 直接 exec 被 launch constraints SIGKILL，`open -a` 把 pid 交给 LaunchServices）→ 用编译的 Cocoa 固件 `examples/objc/agenterm_ax_fixture.m`；旧适配器读的是不存在的 `AXActions` 属性，所以 `actions` 一直空；ABI 升到 1.12（`agt_a11y_tree_snapshot_bounded`）。

- platform：macOS `tree_for_window` 已经活着；补 node budget（1..20000，遍历期与 depth 同时生效）与 `truncated` 标记；把 AX 的 `AXActionNames` 填进节点 `actions`（今天全空）；`capability_status` 报 TCC 未授权为 typed `denied` + 修复路径，而不是空树。
- cu：`windows` 输出稳定句柄（PID + `_AXUIElementGetWindow` 号）与 inventory 过滤；`tree --depth N --max-nodes N --flat`；新动词 `query`（role/text/identifier/actionable/within 过滤 + 分页）。
- 旅程：`scripts/qjs/cu-macos-smoke.qjs`——启动一个 owned 固件 App（TextEdit 或 agenterm 自己的窗口），`windows` 找到它，`query --role AXTextArea` 命中，`tree` 截断标记在 `--max-nodes 5` 下为 true。
- 判据：旅程 PASS + 一条 `verified` 的观察收据；`capabilities` 在无 TCC 时答 `denied` 而不是空树。

### 片 2 —— `invoke` + `verify`（写，先 fail-closed）—— **已落地 `26142e3e`（2026-08-31 凌晨）**

验收（评审者本机重跑）：`cu-macos-smoke.qjs` `success`，12 STEP / 11 EVIDENCE，无孤儿；账单 `steps 20.5M / host_ops 327 / host_bytes 117 KB / waited 0 / heap 22 页`。
落地形状：契约 `AccessibilityNodeAction` 九个动作、状态双向（`checked/unchecked/mixed`、`expanded/collapsed`）；macOS 只在 `AXActionNames` 列出时才 press，`AXValue` 写要 `IsAttributeSettable` + 回读；ABI 1.13 `agt_a11y_node_invoke`；cu 新码 `ambiguous`；PRD 31 三条 leaf 按旅程证明的范围翻 `[x]`（键焦/鼠标不变量未读、`degraded/needs-privilege` 未练到）。

- cu：`invoke --window H --name PAT|--node PATH <action>`（press / set-value / select-option / set-checked / set-expanded / increment / decrement）经 platform `perform_node_action` / `set_node_text`；`verify --window H --expect '[{role,name,checked,value,…}]'`；`wait` 加 `--expect`。
- 不变量落地（PRD 31）：动作不 activate/raise；目标歧义或缺 action → typed 拒绝；每个动作答 `verified|unverified` + 收据。
- 契约层的准备（读过 `contract/accessibility_tree.rs` 后）：今天 `AccessibilityNodeAction` 只有 `Click` / `Focus`。片 2 先把它扩成
  `Press / SetValue(String) / SelectOption(String) / SetChecked(bool) / SetExpanded(bool) / Increment / Decrement`，每个平台各自映射
  （macOS：`AXPress` / `AXValue` 写 / 子项 `AXPress` / 期望态 = 读→不同才 press→回读 / `AXIncrement` `AXDecrement`；Linux AT-SPI `Action` + `EditableText`；
  Windows UIA Invoke / Value / Toggle / ExpandCollapse / SelectionItem），缺映射的平台答 typed `unsupported`——这是 mcu「desired-state 幂等」教训的落点：
  `set-checked true` 在已勾选时是 no-op + `verified`，不是再按一次。
- 旅程：cu-macos-smoke 加一段——`invoke ... set-value` 写 TextEdit 文本，`verify --expect` 回读；再 `invoke press` 一个 checkbox 并回读 checked。

### 片 3 —— 后台 `menu`、`focused`、`observe`，与 PRD 32 的 `frame` 事务合流 —— **已落地 `96d52316`，评审者本机重跑通过（2026-08-31 凌晨）**：`success`，16 STEP / 15 EVIDENCE，前台窗口句柄前后相同（15122 → 15122），无孤儿；账单 `steps 40.0M / host_ops 196 / host_bytes 257 KB / waited 809 ms / heap 37 页`。§4 的三段完成判据（观察 / 语义写 / 后台菜单）到此都 PASS。

落地形状：ABI 1.14 三个后台导出（`agt_a11y_menu_snapshot` / `agt_a11y_menu_invoke` / `agt_a11y_focused_snapshot`，都复用既有节点读取器）；
macOS 适配器读 `AXMenuBar`、逐段按 `AXTitle` 唯一解析并在 `AXPress` 前拒绝禁用/歧义/非叶子，`AXFocusedUIElement` 经 `AXParent` 链 + `CFEqual`
算回窗口树里的子索引路径；`observe` 是 cu 侧对有界树的 poll-diff（平台层没有接 AXObserver，回复写明 `mode: "poll-diff"`）；
`invoke --focused` 在同一次树读里绑定 PID + 窗口 + focused 身份。旅程加四段 STEP（`cu.macos-ax-menu-inspect` / `menu-invoke` / `focused` / `observe`），
观察者是经门 `process_spawn` 的第二个 `agenterm-cu`；本机首跑 `success`，16 STEP / 15 EVIDENCE，`steps 39.96M / host_ops 196 / host_bytes 257 KB / waited 804 ms / heap 37 页`，11.4 s。
意外：accessory App 的主菜单 AppKit 会自动补 Apple / Services 等项（菜单栏 75 节点）；`menu inspect --depth 0` 下 `has_submenu` 只反映已走到的层。
PRD 32 的 `frame` 只写成 `[ ]` leaf + 一段映射（沿用既有 apply pipeline），没有代码。


### 片 4 —— destructive `close` 三件套、crash-persistent receipts、`click`/`focus`/`pointer`/`frame` 真机证明 —— **已落地 cut 3.52（2026-08-31）**

验收（本机重跑）：`cu-macos-smoke.qjs` `success`，**21 STEP / 20 EVIDENCE**，固件被 SIGTERM 回收无孤儿；账单 `steps 75.8M / host_ops 264 / host_bytes 445 KB / waited 819 ms / heap 67 页`，约 14.4 s。片 4 在片 1–3 之后新增五段 STEP（`cu.macos-ax-click` / `cu.macos-ax-frame` / `cu.macos-ax-destructive-refusals` / `cu.macos-ax-destructive-close` / `cu.macos-ax-receipts`）。

落地形状：

- **PRD 31 destructive gate（`close`）**：新动词 `close --window H [--pid N] [--title T] --snapshot --expect gone`。三件套在碰任何机制之前检查——精确目标（`--window`，可用同一次 inventory 读里的 `--pid`/精确 `--title` 绑定）、前置快照（`--snapshot`：目标窗口的有界树写进 reserved 收据）、可验证后置条件（`--expect gone`：句柄从 inventory 回读为缺席）——缺任何一件即 typed `refused`（`detail.reason=destructive_gate`，`missing` 指名，`effect:not_performed`）。机制是平台自己的关闭控件（macOS `AXCloseButton` + `AXPress`，从不 activate/raise）。旅程关掉固件的第二个窗口，回读它 gone、主窗口与进程仍在、指针与前台窗口不变；错 `--pid`/`--title` → `window_identity_mismatch`，未知句柄 → `window_not_found`，observe grant → `refused`，坏后置条件 → `invalid_input`；重复关同一句柄 → `window_not_found`。
- **crash-persistent receipt（新模块 `receipt.rs`）**：每个 actuation（`invoke` / `menu invoke` / `click` / `focus` / `close`）在 audit 目录旁开一个 per-target JSONL（`<audit dir>/cu-receipts/<target>.jsonl`），动作前 flush 一行 `reserved`（target/node/action/value/before/snapshot），回读后 flush 一行 `completed`/`failed`（after/verified/method/reason）。只有 `reserved` 没有配对行 = 崩溃签名（uncertain，绝不当"没发生"）。开不了收据即拒动作（`receipt_unavailable`）。`receipts [--window H] [--max N]`（Observe）按序回读，默认 50、上限 1000、`--max 0` 是 `invalid_input`。
- **`click`/`focus` 真机化**：片 2 只映射未走旅程；片 4 用 `--node` 与 `--name` 各按一次 `Fixture Press`（tree-diff verified，计数标签前进），`focus --node` 移一 responder（focused-readback verified），都带收据；`--name "Fixture Twin"` 两命中 → `a11y_node_ambiguous`。注意 `click`/`wait` 的 name matcher 用**归一化** role 拼写（`button`），与 `invoke`/`query` 接受 `AXButton` 不同。
- **pointer invariant（PRD 31）**：`pointer-position` 在 macOS 从 unsupported 变为**只读**实现（`CGEventCreate(NULL)` + `CGEventGetLocation`，不投任何事件）；旅程在每次 `click`/`close` 前后读它并要求坐标不变，证明真实光标从不移动。ABI 里 `agt_input_pointer_position` 的可用性闸放宽：只读观察不再要求 `input-inject` capability 为 `Available`（macOS 只读、不注入）。
- **`frame` 事务（PRD 32）**：`window-place --action frame --window H --x --y --w --h` 复用既有 apply pipeline（rect 替换 geometry step，其余 preflight/quantize/clamp/单次 AX 写/独立回读/grant/audit/undo 不变）；非 resizable 窗口 typed `window_not_resizable`，catalog 动作带 `--x…` → `invalid_input`，缺维度 → `usage`。旅程把固件主窗口移到请求 rect 并用 `windows --pid` 独立回读证实。

平台侧发现（值得记）：headless 启动固件时，附属（accessory）App 的 `orderFrontRegardless` 窗口会成为其**内部** key window（让固件 App 发布 `AXFocusedUIElement`，片 3 `focused` 需要它），但 `AXFocusedApplication`（系统级）在没有真正活跃 GUI App"守位"时会指向该固件——旧 `mark_focused` 因此把固件误报为 focused。修复：`focused_window_id()` 只在 focused App 同时 `AXFrontmost` 时才采信，并去掉"猜第一个窗口"的兜底。这样 `windows --focused` 回到真正前台（Brave），而固件 `focused` 动词照常工作。这是把 mcu"后台不抢前景"的不变量做实、且不改片 1–3 已验收行为的关键一步。固件本身仍是普通 `NSWindow`（`+resizable`，为 `frame`）+ 第二个可关窗口 `agenterm-ax-second-<pid>`；NSPanel/nonactivating 试过但破坏片 3 的 `focused`。

意外：(1) 第二窗口初名 `<main>-second` 是主标题的超串，破坏片 1 的 `--title` 过滤（matched 2）→ 改名 `agenterm-ax-second-<pid>`；(2) 收据 head 的 `target`（层级串 `current`）与 body 的 `target`（身份对象）键冲突、`merged` 保 head → body 对象被丢，qjs 读 `.title` 于 String 触发引擎缺口 → body 改名 `window_identity` / `spec`。

Linux/Windows：新动词三平台一个拼写，未接的平台 typed `unsupported`（`close` 在 Linux `window_op` 无映射，capabilities 明说；Windows `WM_CLOSE` 有映射但无旅程）；`-p agenterm-platform --features a11y-tree` 对 `x86_64-unknown-linux-gnu` 与 `x86_64-pc-windows-msvc` 均 `cargo check` 通过。ABI 无新导出、无版本 bump（只放宽了 `agt_input_pointer_position` 的可用性闸并给 macOS 接了只读实现）。

- 每个动词一行 PRD 29 leaf + 一段旅程。
- 两条面的边界（2026-08-30 读过门的实现后定）：qjs 门的 `process.window_{key,pointer,rect,control}` 是**按 PID + 数字控件 id 的原始窗口操作**
  （`agenterm_platform::process_window`），存在的理由是旅程要驱动 **agenterm 自己的窗口**做产品自检；cu 的动词是**面向 agent 的 a11y 语义面**，
  驱动任意 App。不合并：门保留自检原语，旅程要碰第三方 App 的语义树时经 `process.command` 调 `agenterm-cu --json`（cu-macos-smoke 就是这么写的）。
  PRD 28「边界」写一句；门不再新增 window 原语。

## 3. 不做 / 明确排除

- 不移植 mcu 的 TS；不引入 Bun；不复制其 helper 协议。
- 不在 cu 里建 shell/PTY/process 域。
- Linux/Windows 已有的 AT-SPI/UIA 动词不因为吸收而改拼写；新动词三平台一个拼写，未接的平台 typed `unsupported`。

## 4. 完成判据（对齐 PRD 28 的「current 层有 shipped 版本」）

macOS 上 `cu-macos-smoke.qjs` 三段 PASS（观察 / 语义写 / 后台菜单），旅程在 `agenterm.tasks.json` 里是一个 task，
quick lane 之外单独一条 GUI lane；PRD 29/30/31 对应 leaf 从 `[ ]` 变 `[x]` 且每条都指向旅程里的 STEP 行。

## 5. 状态（2026-08-31 凌晨，四片都由评审者本机重跑验收）

| 片 | 提交 | 旅程（评审者重跑） | 账单 |
|---|---|---|---|
| 1 观察有界 | `5abb85ee` | 7 STEP / 6 EVIDENCE | 6.97M 步 / 182 ops / 8 页 |
| 2 `invoke`/`verify` | `26142e3e` | 12 / 11 | 20.5M / 327 / 22 页 |
| 3 `menu`/`focused`/`observe` | `96d52316` | 16 / 15，前台句柄不变 | 40.0M / 196 / 37 页 |
| 4 `close` 三件套 / 收据 / `click` / `frame` | `ac6c2d72` | 21 / 20，真鼠标 (88,658) 不动 | 73.8M / 261 / 67 页 |

mcu 的默认环、四条不变量、后台动词、destructive 三件套、崩溃持久收据，在 macOS `current` 上都有证据了；ABI 1.12 → 1.14。
片 4 顺手修了一个真缺陷：`AXFocusedApplication` 跟的是 key-window 归属，附属 App 把窗口 ordered-front 后会被误判为前台——现在要求 `AXFrontmost`。

**还没做（不在本页范围，另立题）**：Linux/Windows 对新动词的实现（现为 typed `unsupported`）；`observe` 接 AX 通知（现为 poll-diff，答复里明写）；macOS 指针注入（只接了读）；远程层（ssh/vnc）上的新动词；PRD 32 `frame` 的黑盒证据仍 `[~]`。
**引擎侧从旅程里得到的需求**（已交引擎线）：`concat` / `join` / 数组 `indexOf` / `sort` / `charCodeAt`·`substring` / `stringify` 的 `space` / `Array.isArray`。

