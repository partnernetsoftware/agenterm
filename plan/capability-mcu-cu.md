# MCU ↔ agenterm-cu 能力对照树

| 日期 | 2026-09-01 |
|---|---|
| MCU 实验室 | 兄弟仓 `moltbaby/skills/mcu`（Bun/TS，`bin/mcu`） |
| 产品面 | 本仓 `crates/agenterm-cu`（Rust，`agenterm-cu`） |
| 纪律 | 吸收**命令集与分层教训**，不搬 TypeScript / helper protocol-v40 |
| 切片史 | [`design-mcu-absorption.md`](design-mcu-absorption.md) 片 1–4 + web/empty-chrome/`page-js`；三平台补齐见 [`design-cu-multi-os-parity.md`](design-cu-multi-os-parity.md) 片 A–K |

**还差什么（2026-09-01，晚）**：读富内容（图片/文件字节）仍未做，只报类型；
**截图三个平台都通了**：macOS 那句「被系统拿走」只对了一半——SDK 里确实没了，但符号还在框架里，
dlsym 拿到就能抓（和本仓够 SkyLight 是同一套办法），实测 macOS 26.5 抓到真内容；符号哪天真没了
就是点名 ScreenCaptureKit 的 typed 拒绝，不退化成整屏抓图；
`orderwin` 在 macOS 上做不到——`AXRaise` 是应用内排序、SkyLight 的 `SLSOrderWindow` 对跨
connection 的外部窗口返回 `kCGErrorIllegalArgument`（两条都实测过），cu 不激活应用，
所以带实测值 typed 拒绝。**`app hide`/`show` 和 `send-keys` 在 Linux 上已经补齐**
（前者对进程的每个窗口做 ICCCM 图标化并回读，后者把 `enter` 落到节点的默认动作上，
和 macOS 的 `AXConfirm` 同一个道理）。

**Windows 有自己的注册旅程了**：`cu-windows-smoke`，**11 STEP / 11 EVIDENCE**，
Windows on ARM 上连跑两次通过。入口一直都在，只是在隔壁项目里：`minicon` 的
`scripts/utm-court.sh` 把这两台 Windows 虚拟机登记为 `qemu-guest-agent` 适配器——
**代理走 virtio-serial 而不是 TCP**，所以扫端口当然什么都扫不到。用它的
`start`/`wait-ready`/`push`/`exec`/`pull` 把 `cargo-xwin` 编出来的
`agenterm-cu.exe` + `agenterm.dll` 推进去执行（客户机是 Windows on ARM 11 26200；
guest agent 在 session 0 看不见桌面，所以用一次性计划任务落到交互 session）。
`windows` / `tree`(uia) / `focused` / `screenshot` / `menu inspect` / `get-extents`
全部真机通过。**一路抓到五个真 bug**：`menu_items` 的祖先遍历没有环护栏（2 GB 不返回，我当天自己写进去的）；
而它的**根因**是节点 id 被 ABI 的 64 字节定长字段截断成碰撞、五个节点成了自己的父节点
（ABI 1.22 加了整 id 的读取路径）；qjs 路径层不认盘符，导致任何旅程都建不出运行目录；
`process_present` 调 `ps`，在 Windows 上让「无孤儿」断言恒真通过；以及固件必须不依赖客户机里的编译器。

**Linux 已经上真机，而且有自己的注册旅程了**（本机 lima VM + `zig cc` 交叉链接 +
Xvfb/openbox/at-spi2/GTK 固件）：`cu-linux-smoke`，20 STEP / 20 EVIDENCE，连跑多次稳定。
PRD 的 Linux leaf 从 `[~] mapped` 改成 `[x]`。**代价是抓出 16 个 bug，其中 3 个让整个动词不可用、
1 个让代码在真实 feature 组合下根本编不过**——所以 **Windows 那一侧现在应当按「未验证」理解，
而不是「接近对齐」**：本仓没有那台机器，也没有可跑的模拟路径，leaf 仍写 `[~] mapped`。

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
│   cu  [✓] 自有 WKWebView 固件旅程 cu-macos-web-smoke：WebArea 树 + unlock poke + scroll 正向 + 网页 invoke/verify
├── 网页 JS 第二刀
│   MCU [✓] CDP page read --js / 扩展 browser read --js（Runtime.evaluate）
│   cu  [~] page-js：CDP Runtime.evaluate（需 --remote-debugging-port；无口 typed）
├── 浏览器扩展 / Native Messaging / tab 生命周期
│   MCU [✓] 实验室
│   cu  [~] `browser` typed unsupported（日常网页走 AX）
├── 窗几何 / 关窗
│   MCU [✓] frame/orderwin/close/maximize…
│   cu  [✓] window-place（Spectacle+frame）+ close 三件套（mac 旅程；三平台都有各自的原生关闭控件）
├── 局部/全局输入
│   MCU [✓] --to handle|desktop；--private SkyLight
│   cu  [✓] 语义刀走 a11y 树永不动指针；全局刀 `pointer-move --to desktop` 有自己的旅程（移动→独立读回→复位）
│        实测：macOS 没有窗口局部注入（键进不了非活跃 App；鼠标事件到得了 sendEvent: 但不带窗口），所以 `--to <handle>` typed 拒绝
├── 事件流
│   MCU [✓] AXObserver
│   cu  [✓] 默认 poll-diff（带 before/after）+ `--mode notifications`（macOS AXObserver，有到达时序、能看见改回去的变化）
├── 窗口栈序/遮挡
│   MCU [✓] zIndex + occlusion
│   cu  [✓] 三平台各用原生栈序，`occluded_percent` 由矩形精确相减（契约层 6 条单测）
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
| 已安装 App | `app list --all` | `apps --all`：扫 `/Applications`、`/System/Applications`（含 Utilities）、`~/Applications`，每行带 `running`，`installed_available: false` 与「一个都没有」是两回事 | **已对齐**（mac；L/W typed） |
| 发现窗口 | `windows` `App#n` · Space/zIndex/occlusion · `--all` · `windows watch` | `windows` JSON `handle` + MCU `ref`；`--window` 接受 `N` 或 `App#N`；**每行带 `z_index` + `occluded_percent`**（ABI 1.17，矩形精确相减）+ **`spaces`**（macOS SkyLight 逐窗归属，id 与 `spaces` 清单对得上）；`windows-watch` poll-diff；`apps` 运行中窗口聚合 | **句柄+watch+zIndex/occlusion+Space 归属**；仍缺已安装未运行 |
| 有界树 | `query`/`tree` depth=12 · `--selector` · `--scan-max` · `treeMeta` | `query --selector` 与 `invoke --selector` 接受 `Role[idx] / Role@title / *@title / #desc`；其余 filter/budget 保留 | **拼写与作用域对齐**；真实三平台旅程仍待补 |
| empty-chrome | `inspect`/`unlock`；闲置 Chromium 浅树 ≠ 空页 | `ax` + `next_actions`；**`unlock` 真 poke**（macOS `AXManualAccessibility`，ABI 1.15），前后两次读报 `poked`/`grew`/`returned_before` | **已对齐**（cu 2026-08-31；L/W typed，两边后端不需要 poke） |
| 语义写 | `invoke <sel> press\|set-value\|select-option\|set-checked\|set-expanded\|increment\|decrement\|…` | **12 个动作全部映射**：mac 全部 live（含新的 `set-selected`/`cancel`/`show-default-ui`）；Linux 与 Windows 九个动作已映射未真机，三个新动作 typed | **无 silent unknown**；不可把可解析误报为已实现 |
| 关闭环 | `verify`/`wait --expect` · `titleIncludes` · present/absent | `verify`/`wait --expect` · `name`/`titleIncludes` 可无 state；Heading↔WebArea | **对齐**（cu 2026-08-31） |
| flatten | `elements` / `invoke --index` | `tree --flat` / `invoke --index` | **对齐** |
| 后台菜单 | `menu inspect`/`menu invoke` path 唯一匹配 | **三平台都映射**：mac 问应用要 `AXMenuBar`（live），L/W 在窗口自己的有界树里找 `menu bar` 节点并按标题路径唯一解析（`capabilities` 写 `mode: tree-search`，未真机） | **已对齐**（mac live，L/W mapped） |
| App-local 焦点 | `focused <pid>` | `focused --window` / `invoke --focused` mac live；**Linux/Windows 已映射**（有界树里最深的 `STATE_FOCUSED` / `HasKeyboardFocus`，`capabilities` 写 `mode: state-search`） | mac live，L/W mapped |
| 事件 | `observe` AXObserver；`query --watch` | `observe` **两种模式**：默认 poll-diff（每条事件带 `before`/`after`），`--mode notifications` 用 macOS AXObserver（有到达时序，能看见两次遍历之间「改了又改回去」的变化）；`capabilities` 列 `default_mode` + 每模式状态 | **已对齐**（两者互不包含，由调用方选） |
| 关窗 | `close` + `--expect-window absent` | `close` destructive 三件套 + `receipts` | **对齐思想** |
| 几何 | `frame`/`movewin`/`resize`/`maximize`/`orderwin` | `window-place`/`close`/`displays` live；`orderwin` raise **三平台都映射**（Linux 用 `ConfigureWindow(Above)`，不碰焦点）；`close` 三平台都映射（mac AXCloseButton / Win WM_CLOSE / Linux EWMH `_NET_CLOSE_WINDOW`）；`spaces` macOS SkyLight 只读 | **已对齐**；`spaces` 仍只有 mac |
| 指针/键 | `click/type/key/scroll/drag --to` · `cursor` | **macOS 注入已接**（HID tap：move/click/type/keys；`cu-macos-pointer-smoke` 真机移动+复位）；`--to desktop` 必填且明确全局，**`--to <handle>` typed 拒绝**（macOS 实测没有窗口局部注入）；节点级 `send-keys` 是语义映射（`enter`→AXConfirm） | **已对齐**；语义刀与全局刀分开 |
| 节点文本/几何 | 无独立动词（`query` 里带 rect） | **七个动词三平台都映射**：`get-extents`（mac AX 真机 / Win BoundingRectangle）、`select`/`get-selection`/`set-caret`/`get-caret`（mac+Linux 活，Win TextPattern 新映射）、`scroll`（mac AXScrollToVisible 网页旅程真机 / Win ScrollItem） | **cu 多一层** |
| 剪贴板 | `clip` + 富 UTI | `clipboard-read` 读纯文本，但**回复带 `types`**：剪贴板上所有表示的原生名字（mac class 名 / X11 TARGETS / Win 格式名，不做归一化），三平台都接。实测剪贴板放一张 PNG：`text` 空、`types` 有 `«class PNGf»` 等 9 项 | **已对齐发现面**；读富内容仍另说 |
| 截图 | `shot` 可选权限 | `screenshot` Win GDI + **Linux X11 `GetImage`**（只转 24/32 位 TrueColor，别的 visual typed 拒绝而不是乱解释字节）；**macOS 被系统拿走**（`CGWindowListCreateImage` 15.0 从 SDK 移除，ScreenCaptureKit 要另一份 TCC），拒绝理由写明 | **两平台对齐**；mac 是系统限制，不退化成整屏抓图 |
| 网页 JS | `page read --js` / `browser read --js` | `page-js` CDP Runtime.evaluate（默认 9222）；无 listener typed | **路径已接**；MAIN Function constructor 拒绝 |
| 浏览器桥 | `browser *` MV3 + CDP | `browser` typed unsupported | 日常网页仍 AX |
| 开箱 | `setup`/`doctor`/`caps`/`permissions` | `capabilities` 里有**一级 `permissions` 块**：授权状态 + 修复路径 + **被它卡住的 24 个动词**（含输入类——macOS 同一份 Accessibility 授权也管投事件）；`setup`/`doctor`/`permissions` 仍 typed | **报告面对齐**；向导有意留给 MCU |
| 授权 | session/lock/request-id | `--grant observe,actuate` / `--grant-id` | 形状不同，都 fail-closed |
| 目标 | 本机 | `current`/`ssh`/`vnc`；`rdp` 占位 | **cu 多一层 transport** |
| App 生命周期 | `app launch/quit/hide/show` | **四个都做了**（mac live）：`hide`/`show` 写应用级 `AXHidden` 且按 **pid** 寻址（隐藏后句柄就不存在了）；`quit` 按下应用**自己的 Quit 菜单项**并配 `close` 那套三件套 + 收据，**不是信号**；`launch` 走 LaunchServices，**回复明说没有 pid**（进程归 launcher 管），要 pid 就等窗口出现 | **已对齐** |
| 进程/PTY/job/设备/提权/Simulator/spaces | MCU 工坊/库房/地库 | `pty`/`job`/`process`/… **typed unsupported** | 无静默 unknown；live 仍 MCU |

## 2. 互相该补的缺口（文档已点名，实现另立项）

**cu 可从 MCU 再吸收（仍限桌面环）**

1. ~~稳定句柄 `App#n`~~ **已做**：`windows[].ref` + `--window App#N`（仍同时接受整数；未做 live app 前缀核验）。
2. ~~`unlock` 的 `AXManualAccessibility` poke~~ **已做**（2026-08-31，ABI 1.15）：poke 后重读，
   `poked`/`grew`/`returned_before` 三个字段分开——AppKit 对这个属性返回 unsupported 却照样生效，
   所以调用状态不能当结论。自有 `WKWebView` 固件旅程作证。
3. ~~`query --selector` / `invoke --selector`~~ **已做**（MCU `Role[idx] / Role@title / *@title / #desc`；query 作用域=命中节点+子孙，invoke 绑定唯一节点）。
4. ~~`invoke` 的 5 个 MCU 拼写~~ **已做**（2026-09-01，ABI 1.16）：`set-selected`（macOS `AXSelected`
   期望态，真机 verified + 幂等 no-op）、`cancel`（`AXCancel`）、`show-default-ui`（`AXShowDefaultUI`），
   加上早已映射的 `set-selection`/`scroll-to`。快照同时补出 `selected`/`unselected` 两向词
   （mac AX、Linux `selectable`），否则 `set-selected` 无从回读。节点没有这个状态时
   cu 在碰机制之前就 `state_unobservable` 拒绝。
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
