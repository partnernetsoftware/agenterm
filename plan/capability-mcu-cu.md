# MCU ↔ agenterm-cu 能力对照树

| 日期 | 2026-08-31 |
|---|---|
| MCU 实验室 | 兄弟仓 `moltbaby/skills/mcu`（Bun/TS，`bin/mcu`） |
| 产品面 | 本仓 `crates/agenterm-cu`（Rust，`agenterm-cu`） |
| 纪律 | 吸收**命令集与分层教训**，不搬 TypeScript / helper protocol-v40 |
| 切片史 | [`design-mcu-absorption.md`](design-mcu-absorption.md) 片 1–4 + web/empty-chrome/`page-js` |

图例：`[✓]` 有真机或本机旅程 · `[~]` 动词在、平台半截或 typed 诚实失败 · `[ ]` 未做。非桌面组在 cu 上必须 typed，不许静默缺失。

MCU 对 cua-driver 的桌面对照仍在 MCU `CAPABILITY-TREE.md`。本文件只对 **MCU ↔ cu**。

## 0. 两棵树（压缩）

```
machine-control
├── 默认环 windows → 有界 query/tree → invoke → verify/wait
│   MCU [✓] mac 真机；L/W 合约
│   cu  [✓] mac cu-macos-smoke；windows-watch poll-diff；apps running-only；L/W 同拼写
├── 不变量 后台不抢前景 / fail-closed / 投递≠成功 / destructive 三件套
│   MCU [✓] helper+receipt
│   cu  [✓] grant observe|actuate + receipts + close 闸（mac 旅程）
├── 网页 AX WebArea（无扩展）
│   MCU [✓] Brave Origin 真机；empty-chrome → query/unlock
│   cu  [~] tree/query.ax + next_actions + classify-only unlock；titleIncludes Heading↔WebArea
├── 网页 JS 第二刀
│   MCU [✓] CDP page read --js / 扩展 browser read --js（Runtime.evaluate）
│   cu  [~] page-js：CDP Runtime.evaluate（需 --remote-debugging-port；无口 typed）
├── 浏览器扩展 / Native Messaging / tab 生命周期
│   MCU [✓] 实验室
│   cu  [~] `browser` typed unsupported（日常网页走 AX）
├── 窗几何 / 关窗
│   MCU [✓] frame/orderwin/close/maximize…
│   cu  [✓] window-place（Spectacle+frame）+ close 三件套（mac）
├── 局部/全局输入
│   MCU [✓] --to handle|desktop；--private SkyLight
│   cu  [~] click/focus/send-text/keys/scroll/pointer-*；pointer-move 强制 `--to desktop`
├── shell / PTY / job / process / device / privilege / Simulator
│   MCU [✓/~] 实验室正殿外
│   cu  [~] 同名动词 typed unsupported（capabilities 可查）
└── 远程目标
    MCU [ ] 本机为主
    cu  [~] --target current|ssh|vnc；rdp 占位 rdp_unavailable
```

## 1. 动词对照（cu 命令面 × MCU 族）

| 族 | MCU | agenterm-cu | 状态 |
|---|---|---|---|
| 发现窗口 | `windows` `App#n` · Space/zIndex/occlusion · `--all` · `windows watch` | `windows` JSON `handle` + MCU `ref`（`App#n`）；`--window` 接受 `N` 或 `App#N`；`windows-watch` poll-diff；`apps` 运行中窗口聚合 | **句柄+watch+running apps**；仍缺 Space/occlusion/已安装未运行 |
| 有界树 | `query`/`tree` depth=12 · `--selector` · `--scan-max` · `treeMeta` | `query --selector` 与 `invoke --selector` 接受 `Role[idx] / Role@title / *@title / #desc`；其余 filter/budget 保留 | **拼写与作用域对齐**；真实三平台旅程仍待补 |
| empty-chrome | `inspect`/`unlock`；闲置 Chromium 浅树 ≠ 空页 | `ax` + `next_actions`；`unlock` 只重读并分类，明确 `poked:false` | **诚实 partial**；尚未映射 `AXManualAccessibility` poke |
| 语义写 | `invoke <sel> press\|set-value\|select-option\|set-checked\|set-expanded\|increment\|decrement\|…` | 原 7 动作 live；另 5 个 MCU 拼写可解析但在 ABI 未映射时 typed `unsupported` | **无 silent unknown**；不可把可解析误报为已实现 |
| 关闭环 | `verify`/`wait --expect` · `titleIncludes` · present/absent | `verify`/`wait --expect` · `name`/`titleIncludes` 可无 state；Heading↔WebArea | **对齐**（cu 2026-08-31） |
| flatten | `elements` / `invoke --index` | `tree --flat` / `invoke --index` | **对齐** |
| 后台菜单 | `menu inspect`/`menu invoke` path 唯一匹配 | `menu-inspect`/`menu-invoke` mac live；L/W unsupported | **对齐 mac** |
| App-local 焦点 | `focused <pid>` | `focused --window` / `invoke --focused` mac live | **对齐 mac** |
| 事件 | `observe` AXObserver；`query --watch` | `observe` poll-diff（明写非 AXObserver） | cu **弱一档** |
| 关窗 | `close` + `--expect-window absent` | `close` destructive 三件套 + `receipts` | **对齐思想** |
| 几何 | `frame`/`movewin`/`resize`/`maximize`/`orderwin` | `window-place`/`close` live；`orderwin` raise（mac AXRaise / Win Show；linux typed）；`spaces` macOS SkyLight 只读 | **orderwin/spaces 无 silent unknown** |
| 指针/键 | `click/type/key/scroll/drag --to` · `cursor` | `pointer-move --to desktop` 明确全局；窗口局部映射未实现即 typed `unsupported` | 防止漏写目标静默变成全局输入 |
| 剪贴板 | `clip` + 富 UTI | `clipboard-read` 纯文本 observe；节点 `copy`/`paste` | cu **窄** |
| 截图 | `shot` 可选权限 | `screenshot` Win GDI；mac/linux typed unsupported | MCU 实验室更完整 |
| 网页 JS | `page read --js` / `browser read --js` | `page-js` CDP Runtime.evaluate（默认 9222）；无 listener typed | **路径已接**；MAIN Function constructor 拒绝 |
| 浏览器桥 | `browser *` MV3 + CDP | `browser` typed unsupported | 日常网页仍 AX |
| 开箱 | `setup`/`doctor`/`caps`/`permissions` | `capabilities` + typed `setup`/`doctor`/`permissions` | 无 TCC 向导 |
| 授权 | session/lock/request-id | `--grant observe,actuate` / `--grant-id` | 形状不同，都 fail-closed |
| 目标 | 本机 | `current`/`ssh`/`vnc`；`rdp` 占位 | **cu 多一层 transport** |
| 进程/PTY/job/设备/提权/Simulator/spaces | MCU 工坊/库房/地库 | `pty`/`job`/`process`/… **typed unsupported** | 无静默 unknown；live 仍 MCU |

## 2. 互相该补的缺口（文档已点名，实现另立项）

**cu 可从 MCU 再吸收（仍限桌面环）**

1. ~~稳定句柄 `App#n`~~ **已做**：`windows[].ref` + `--window App#N`（仍同时接受整数；未做 live app 前缀核验）。
2. [~] `unlock` 动词与重读分类已做；真正的闲置 Chromium 只读 poke（depth≥8 hit-test / `AXManualAccessibility`）仍未映射。
3. ~~`query --selector` / `invoke --selector`~~ **已做**（MCU `Role[idx] / Role@title / *@title / #desc`；query 作用域=命中节点+子孙，invoke 绑定唯一节点）。
4. [~] `invoke` 已接受 `set-selected`、`set-selection`、`scroll-to`、`cancel`、`show-default-ui` 拼写；ABI 未映射前一律 typed `unsupported`，不算行为完成。
5. [✓] `windows-watch` poll-diff；`apps` running-only；`orderwin` raise（linux typed）；`spaces` macOS SkyLight 只读。
   `invoke scroll-to` / `set-selection` mapped；`cancel`/`set-selected`/`show-default-ui` typed.
6. ~~`--to`~~ `pointer-move` 必填 `--to desktop`。
7. [✓] `page-js` CDP Runtime.evaluate（`--port`，默认 9222）；无 listener typed；禁 MAIN Function constructor。

**MCU 树应承认 cu 已产品化（勿再写「迁入 [ ]」）**

1. 默认环 + 四条不变量 + menu/focused/observe/close/receipts 已在 `agenterm-cu` mac 旅程落地。
2. empty-chrome/`titleIncludes`/`page-js` 后端名已进 cu JSON。
3. cu 多了 MCU 没有的：`--target ssh|vnc`、`--grant`、Spectacle `window-place`、始终 JSON stdout。
4. **写刀仍留 MCU**；扩展/PTY/job/privilege **不要**再抄进 cu。

## 3. 日常怎么选入口

| 场景 | 用谁 |
|---|---|
| Agent 编排第三方 App（mac 语义树） | 优先 `agenterm-cu`（产品 ABI + grant） |
| Chromium 网页列节点 | 两边都走 AX `query`；不要先装扩展 |
| chatgpt.com composer / closed-shadow / tab-id | **MCU** `browser read --js`；cu `page-js` 需 CDP 口 |
| 本机 PTY/job/设备/提权/Simulator | **MCU** 或 AgenTerm 本体，不调 cu |
| 远程桌面 worker | **cu** `--ssh`/`--vnc` |
