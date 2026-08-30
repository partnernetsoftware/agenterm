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

### 片 1 —— macOS `current` 的默认环有界、有证据（先只读）

- platform：macOS `tree_for_window` 已经活着；补 node budget（1..20000，遍历期与 depth 同时生效）与 `truncated` 标记；把 AX 的 `AXActionNames` 填进节点 `actions`（今天全空）；`capability_status` 报 TCC 未授权为 typed `denied` + 修复路径，而不是空树。
- cu：`windows` 输出稳定句柄（PID + `_AXUIElementGetWindow` 号）与 inventory 过滤；`tree --depth N --max-nodes N --flat`；新动词 `query`（role/text/identifier/actionable/within 过滤 + 分页）。
- 旅程：`scripts/qjs/cu-macos-smoke.qjs`——启动一个 owned 固件 App（TextEdit 或 agenterm 自己的窗口），`windows` 找到它，`query --role AXTextArea` 命中，`tree` 截断标记在 `--max-nodes 5` 下为 true。
- 判据：旅程 PASS + 一条 `verified` 的观察收据；`capabilities` 在无 TCC 时答 `denied` 而不是空树。

### 片 2 —— `invoke` + `verify`（写，先 fail-closed）

- cu：`invoke --window H --name PAT|--node PATH <action>`（press / set-value / select-option / set-checked / set-expanded / increment / decrement）经 platform `perform_node_action` / `set_node_text`；`verify --window H --expect '[{role,name,checked,value,…}]'`；`wait` 加 `--expect`。
- 不变量落地（PRD 31）：动作不 activate/raise；目标歧义或缺 action → typed 拒绝；每个动作答 `verified|unverified` + 收据。
- 旅程：cu-macos-smoke 加一段——`invoke ... set-value` 写 TextEdit 文本，`verify --expect` 回读；再 `invoke press` 一个 checkbox 并回读 checked。

### 片 3 —— 后台 `menu`、`focused`、`observe`，与 PRD 32 的 `frame` 事务合流

- 每个动词一行 PRD 29 leaf + 一段旅程；此时把 qjs 门的 `process.window_*` 评估收敛：要么门调用 cu 的 `Command`，要么 PRD 28 明写两条面各自的边界。

## 3. 不做 / 明确排除

- 不移植 mcu 的 TS；不引入 Bun；不复制其 helper 协议。
- 不在 cu 里建 shell/PTY/process 域。
- Linux/Windows 已有的 AT-SPI/UIA 动词不因为吸收而改拼写；新动词三平台一个拼写，未接的平台 typed `unsupported`。

## 4. 完成判据（对齐 PRD 28 的「current 层有 shipped 版本」）

macOS 上 `cu-macos-smoke.qjs` 三段 PASS（观察 / 语义写 / 后台菜单），旅程在 `agenterm.tasks.json` 里是一个 task，
quick lane 之外单独一条 GUI lane；PRD 29/30/31 对应 leaf 从 `[ ]` 变 `[x]` 且每条都指向旅程里的 STEP 行。
