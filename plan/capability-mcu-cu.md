# MCU ↔ agenterm-cu 能力对照树

| 日期 | 2026-09-03 |
|---|---|
| MCU 实验室 | 兄弟仓 `moltbaby/skills/mcu`（Bun/TS，`bin/mcu`） |
| 产品面 | 本仓 `crates/agenterm-cu`（Rust，`agenterm-cu`） |
| 纪律 | ACU 最终替代 MCU；吸收**能力与分层教训**，不逐行搬 TypeScript / helper protocol-v40 |
| 切片史 | [`design-mcu-absorption.md`](design-mcu-absorption.md) 片 1–4 + web/empty-chrome/`page-js`；三平台补齐见 [`design-cu-multi-os-parity.md`](design-cu-multi-os-parity.md) 片 A–K |

**2026-09-04 PID 发现补强**：所有 CDP page 动词均可用 `--pid`
解析该进程显式声明的调试端口；解析前后绑定启动身份，不扫端口、
不公开完整命令行。`scripts/cu-cdp-smoke.sh` 已用真实浏览器 PID 通过。

**还差什么（2026-09-01，夜）**：~~读富内容仍未做~~ **已做**：`clipboard-read --type`（MCU `clipboard read`）按宿主自己的类型名读有界字节，回复 `sha256` + utf8/base64，可选 `--out`。macOS 只认 `clipboard info` 的 AppleScript class（`«class PNGf»` / `string`），不认 UTI。`clipboard write`/`write-file`/`clear --apply` 已接 ABI 1.24。
MCU 叶子 `dclick`/`rclick`/`shot`/`type`/`key`/`move`/`elements`/`launch`/`quit`/`hide`/`show`/`clipboard`/`page read --js`/`frame`/`movewin`/`resize`/`maximize` 已是 live 别名（几何四词走 `window-place`）；**2026-09-03 又收了一批**：`inspect`/`find`/`read` 是 `query` 的别名，`page targets` → `page-targets`，`page text` → `page-text`（a11y 阅读顺序），`page-js` 有 `--target-id/--target-url/--target-title` 选 tab，`tab list`/`tab select` 走 tab-strip 后台切 tab。**2026-09-04 desktop closure 第一格已活**：`hit`/`zoom`/`snapshot`/`diff`/`raise`/`minimize`/`restore` 除包级测试外，已进入 `cu-macos-smoke` 的真实 Cocoa/AX journey；其中 minimize→restore 还抓出并修正了 off-screen `CGWindowID` owner lookup。Linux/Windows 同组旅程与 `drag` 的独立全局 pointer court 仍待补齐。**同日开始非桌面迁移**：`ps --pid/--parent/--name/--offset/--max` 已走 `agenterm-platform::process::list`，与 qjswasm `process.list` 共用内核；`process-state --pid N` 又接入共享 process-observation，区分 `live|dead|unknown` 并返回可用的启动身份；`process-usage --pid N` 在前后相同身份间采样累计 CPU/内存/page-fault，并以十进制字符串无损发布宽计数，`--watch-ms` 进一步给出以单一启动身份约束、时长/间隔/样本数三重有界的序列；`process-wait` 再以调用方给出的启动身份绑定 pidfd/kqueue/Windows HANDLE 原生对象，取代 MCU 的 PID 轮询。MCU 对应的 `process state/usage N` 已可无损改写，native `process-wait` 也可由兼容入口直达。`process-kill` / `kill` 现又要求 PID + 启动身份 + `--expect exited`，先写 crash-persistent receipt，再通过同一个 native process reference 投递并等待：Linux x86_64 pidfd 与 Windows x86_64 retained-HANDLE 真机均绿；macOS 因不存在对 PID 重用原子的 signal primitive 而 typed 拒绝。更深的命令行/文件/端口过滤、任意 signal、suspend/resume 与进程树 kill 仍是 gap。其余未接 MCU 命令仍 typed 拒绝，不再 `unknown command`。

**同日 terminal 第一刀已活**：AgenTerm 自有会话新增
`terminal-list/read/send/wait`；轻量 control client 直连产品协议，身份钉在
scope + epoch + `@tab`。读明确是 bounded screen snapshot，不冒充增量 cursor；
写由 ACU 与 server 两层 receipt 验证。macOS 注册旅程已全绿；Linux 新 STEP
全过，但同套件后续旧 observe 段红而没有签出整套 evidence；Windows court
在投送产品前被 interactive nonce 阻断。任意 headless PTY/job、进程树管理与
loss-aware 增量输出 cursor 仍是下一层 gap。

**CDP 活证据已到（2026-09-03）**：`/json` reader 认了 `Content-Length` / chunked（`src/cdp/http.rs`），`scripts/cu-cdp-smoke.sh` PASS。**后台 tab 读+写也接上了**：`page text / find / click / fill / type / nav / screenshot --target-id|--target-url|--target-title|--match` 全走 CDP 目标自己的 websocket；MCU-compatible `--match` 搜 title+URL+description，但 ACU 要求恰好一项，拒绝 MCU 的 first-hit 猜测。`page type` 冻结已有 editable focus，以同元素 value-growth 验证且不记录明文。`src/cdp/` 以 `ws.rs` 一条 session、`Transport` 假转录单测、`ax.rs` 同形阅读行和 `page.rs` plan/perform 收据实现，从不调 `Target.activateTarget` / `Page.bringToFront`（只有 `page screenshot --activate` 显式例外，算 actuate），每条回复带 `focus_changed: false` + target 身份。`scripts/cu-cdp-actuate-smoke.sh` PASS：A 活跃、B 后台，唯一 `--match` 打在 B，宽匹配 typed ambiguous；各动词执行后 `/json` 首页仍是 A、`windows --focused` 不变。真机（用户的 Brave Origin）仍需 `--remote-debugging-port=9222` 重启才能跑同一条。
**截图三个平台都通了**：macOS 那句「被系统拿走」只对了一半——SDK 里确实没了，但符号还在框架里，
dlsym 拿到就能抓（和本仓够 SkyLight 是同一套办法），实测 macOS 26.5 抓到真内容；符号哪天真没了
就是点名 ScreenCaptureKit 的 typed 拒绝，不退化成整屏抓图；
`orderwin` 在 macOS 上做不到——`AXRaise` 是应用内排序、SkyLight 的 `SLSOrderWindow` 对跨
connection 的外部窗口返回 `kCGErrorIllegalArgument`（两条都实测过），cu 不激活应用，
所以带实测值 typed 拒绝。**`app hide`/`show` 和 `send-keys` 在 Linux 上已经补齐**
（前者对进程的每个窗口做 ICCCM 图标化并回读，后者把 `enter` 落到节点的默认动作上，
和 macOS 的 `AXConfirm` 同一个道理）。

**Windows 有自己的注册旅程了**：`cu-windows-smoke` 的既有 **11 STEP / 11 EVIDENCE**
曾在 Windows on ARM 上连跑两次通过；当前脚本已把 owned fixture 的
`process-state` / `process-usage` / `process-wait` 纳入同一条旅程，等待下一次原生 court 复跑。
`process-watch` 也已接入三平台旅程：PID/parent/name 可组合过滤，也可显式 all，先给
identity-bound baseline，再在 duration/interval/event/inventory 四重预算内发
started/exited；PID 重用表现为旧 identity exited + 新 identity started。macOS 公共
CLI 已抓到 owned child 的真实退出，三平台 qjswasm 证据等待最新 HEAD 原生复跑。
入口一直都在，只是在隔壁项目里：`minicon` 的
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
Xvfb/openbox/at-spi2/GTK 固件）：`cu-linux-smoke` 既有 20 STEP / 20 EVIDENCE 连跑多次稳定；
当前脚本已扩成 22 STEP / 23 EVIDENCE，把 owned fixture 的稳定进程身份、无损资源计数和 exact-object wait
纳入同一旅程，等待下一次原生 court 复跑。
PRD 的 Linux leaf 从 `[~] mapped` 改成 `[x]`。**代价是抓出 16 个 bug，其中 3 个让整个动词不可用、
1 个让代码在真实 feature 组合下根本编不过**——所以 **Windows 那一侧现在应当按「未验证」理解，
而不是「接近对齐」**：本仓没有那台机器，也没有可跑的模拟路径，leaf 仍写 `[~] mapped`。

图例：`[✓]` 有真机或本机旅程 · `[~]` 动词在、平台半截或 typed 诚实失败 · `[ ]` 未做。非桌面组也必须最终从 ACU 单入口可达；当前 typed refusal 是诚实缺口，不是永久边界。

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
│   cu  [✓] page-js：CDP Runtime.evaluate（需 --remote-debugging-port；无口 typed）+ --target-id/url/title 选 tab + page targets；
│        cu-cdp-smoke PASS 2026-09-03（headless 一次性实例）
├── 后台 tab 读+写（CDP，不抢 tab / 窗口）
│   MCU [✓] page click / nav / read（CDP）
│   cu  [✓] page text / find / click / fill / nav / screenshot --target-*：目标 websocket 上 Accessibility.getFullAXTree、
│        DOM.querySelectorAll、Input.dispatchMouseEvent、Input.insertText、Page.navigate、Page.captureScreenshot；
│        cu-cdp-actuate-smoke PASS 2026-09-03（每步后活跃 target / 前景窗口不变）；真机待开口
├── 网页文字 / 后台 tab（不开 CDP 口）
│   MCU [✓] page read（CDP）
│   cu  [✓] page text 阅读顺序 {id, role, text, bounds}；tab list / tab select 走 tab-strip radio-button，后台切、不抢焦点（Brave 真机 2026-09-03）
├── 浏览器扩展 / Native Messaging / tab 生命周期
│   MCU [✓] 实验室
│   cu  [~] `browser profiles` / `browser open` / `tab close` live（真实实例、按 profile 开窗）；MV3 桥 typed unsupported（日常网页走 AX）
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
│   cu  [~] process state/usage/wait/watch + exact-object kill 已进统一 facade；其余同名动词 typed unsupported（capabilities 可查）；这是迁移缺口，
│        后续由 ACU typed facade 调 owning mechanism，不在 CU crate 复制内核
└── 远程目标
    MCU [ ] 本机为主
    cu  [~] --target current|ssh|vnc；rdp 占位 rdp_unavailable
```

## 1. 动词对照（cu 命令面 × MCU 族）

| 族 | MCU | agenterm-cu | 状态 |
|---|---|---|---|
| 已安装 App | `app list --all` | `apps --all`：扫 `/Applications`、`/System/Applications`（含 Utilities）、`~/Applications`，每行带 `running`，`installed_available: false` 与「一个都没有」是两回事 | **已对齐**（mac；L/W typed） |
| 发现窗口 | `windows` `App#n` · Space/zIndex/occlusion · `--all` · `windows watch` | `windows` JSON `handle` + MCU `ref`；`--window` 接受 `N` 或 `App#N`；**每行带 `z_index` + `occluded_percent`**（ABI 1.17，矩形精确相减）+ **`spaces`**（macOS SkyLight 逐窗归属，id 与 `spaces` 清单对得上）；`windows-watch` poll-diff；`apps` 运行中窗口聚合 | **句柄+watch+zIndex/occlusion+Space 归属**；仍缺已安装未运行 |
| 有界树 | `query`/`tree` depth=12 · `--selector` · `--scan-max` · `treeMeta` · `inspect`/`find`/`read` | `query --selector` 与 `invoke --selector` 接受 `Role[idx] / Role@title / *@title / #desc`；其余 filter/budget 保留；**`inspect HANDLE` / `find HANDLE TEXT` / `read HANDLE SELECTOR` 是 `query` 别名**；`inspect --app` inventory 仍是 migration gap；截断时 `next_actions` 明说「页面不在这次回复里，加 `--max-nodes 6000 --depth 64`」 | **拼写与作用域对齐**（2026-09-03 别名补齐）；真实三平台旅程仍待补 |
| 页面文字 | `page read`（CDP） | **`page text --window H [--max-bytes] [--within]`**：a11y 阅读顺序 `{id, role, text, bounds}`（+ name/focused/actionable），链接/按钮合一行，网页文字在 AXValue 不在 name；默认 16 KiB / depth 64 / 6000 节点 | **已对齐**（mac；Brave 真机 94 行 2026-09-03；L/W typed） |
| empty-chrome | `inspect`/`unlock`；闲置 Chromium 浅树 ≠ 空页 | `ax` + `next_actions`；**`unlock` 真 poke**（macOS `AXManualAccessibility` + `AXEnhancedUserInterface`，再唤醒 renderer，ABI 1.15），有界重读（5 × 200 ms）后报 `poked`/`grew`/`returned_before`/**`web_nodes_before`/`web_nodes_after`/`rereads`/`poke`**（对比读 depth 64 / 6000，旧 depth 12 看不到 web-area 子树） | **已对齐**（cu 2026-08-31，字段 2026-09-03；L/W typed，两边后端不需要 poke） |
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
| 网页 JS | `page read --js` / `page targets` / `browser read --js` | `page-js` CDP Runtime.evaluate（默认 9222）+ **`--target-id` / `--target-url` / `--target-title` / `--match`** 选 tab（`--match` 跨 title+URL+description；零命中 `cdp_target_not_found`、多命中 `cdp_target_ambiguous`，候选在 `error.detail`；后台 tab 原地求值不切换）；**`page targets`** 列 `/json`（id/url/title/description/type/attached/websocket）；无 listener typed | **已对齐**（`scripts/cu-cdp-smoke.sh` PASS 2026-09-03，一次性 headless 实例）；MAIN Function constructor 拒绝 |
| 后台 tab 读+写 | `page read` / `page click` / `page nav`（CDP） | **`page text` / `page find` / `page click` / `page fill` / `page nav` / `page screenshot --target-*`**：同一选 tab 规则，目标自己的 websocket，行形 `{id, role, text}` 与 AX `page text` 同形（`backend: "cdp"`，id = backend DOM node id）；`find` 给 `{node, path, role, name, text, box}`，文字命中提升到外层 button/link；`click` 一个节点（多命中 `cdp_node_ambiguous`）滚入视口后按盒中心派 mouse 事件、文档+节点回读作证；`fill` DOM.focus + insertText + `.value` 回读；`nav` 等 load 事件；`screenshot` 后台可能被拒 `cdp_screenshot_unavailable`，绝不为此激活；三者都写收据；每条回复 `focus_changed: false` | **已对齐**（`scripts/cu-cdp-actuate-smoke.sh` PASS 2026-09-03：A 活跃、全部动词打在后台 B，每步后 `/json` 首页与 `windows --focused` 不变）；真机待 `--remote-debugging-port` |
| 后台 tab | `browser tabs`（扩展） | **`tab list --window H`**（index/title/selected）/ **`tab select --window H (--title SUB \| --index N)`**：按 tab-strip 的 radio-button 后台切，`selected` 回读作证，不抢焦点；`a11y_tab_not_found` / `a11y_tab_ambiguous` | **已对齐**（mac Brave 真机 2026-09-03，前后焦点窗口不变；无 CDP 口时的 a11y 兜底） |
| 浏览器桥 | `browser *` MV3 + Native Messaging | `browser profiles` / `browser open --profile` / `windows --browser-profile` / `tab close` live（真实运行实例，`open -na --profile-directory`，不重启）；MV3 桥 typed unsupported | 日常网页仍 AX（`page text` / `query` / `invoke`）；MV3 是待迁移 facade，不是永久排除项 |
| 开箱 | `setup`/`doctor`/`caps`/`permissions` | `capabilities` 里有**一级 `permissions` 块**：授权状态 + 修复路径 + **被它卡住的 24 个动词**（含输入类——macOS 同一份 Accessibility 授权也管投事件）；`setup`/`doctor`/`permissions` 仍 typed | **报告面对齐**；修复向导是 ACU migration gap |
| 授权 | session/lock/request-id | `--grant observe,actuate` / `--grant-id` | 形状不同，都 fail-closed |
| 目标 | 本机 | `current`/`ssh`/`vnc`；`rdp` 占位 | **cu 多一层 transport** |
| App 生命周期 | `app launch/quit/hide/show` | **四个都做了**（mac live）：`hide`/`show` 写应用级 `AXHidden` 且按 **pid** 寻址（隐藏后句柄就不存在了）；`quit` 按下应用**自己的 Quit 菜单项**并配 `close` 那套三件套 + 收据，**不是信号**；`launch` 走 LaunchServices，**回复明说没有 pid**（进程归 launcher 管），要 pid 就等窗口出现 | **已对齐** |
| 进程/文件/PTY/job/设备/提权/Simulator/spaces | MCU 工坊/库房/地库 | `ps` 基础 inventory、`process-state` 稳定身份、`process-usage` 单次/有界序列、`process-wait` 原生稳定对象等待、`process-watch` identity-bound lifecycle diff 已 live；`file-inspect` / `file inspect` 已有末级链接不跟随、稳定对象身份与无损元数据（macOS qjswasm 41 STEP / 42 evidence live，L/W 叶已编译、native journey 待跑）；`pty`/`job`/`process` 其余形状仍 **typed unsupported** | 已开始共用 platform/qjswasm 内核；其余属于 ACU 替代门的待迁移 facade，不是永久留 MCU |

## 2. 互相该补的缺口（文档已点名，实现另立项）

**ACU 从 MCU 迁移（桌面环先行，随后覆盖完整机器控制）**

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
   `--target-id` / `--target-url` / `--target-title` / `--match` 选 tab（零命中 `cdp_target_not_found`、多命中 `cdp_target_ambiguous`）；
   `page targets` 列 `/json`；`tab list` / `tab select --window H (--title|--index)` 走 a11y tab-group radio-button 后台切 tab，不抢焦点。
   `page text --window H` 按阅读顺序给 {id, role, text, bounds}（网页文字在 AXValue，不在 name）；`unlock` 同时设 AXManualAccessibility + AXEnhancedUserInterface 并唤醒 renderer，对比读用 depth 64 / 6000 节点（旧 depth 12 看不到 web-area 子树）。

8. **2026-09-03 账本**：关闭 —— `inspect`/`find`/`read` 别名、`page targets`、`page text`、`page-js` 选 tab、`tab list`/`tab select`、`unlock` 的 web-node 字段、`windows[].browser_profile`（从 Chromium 标题尾巴 ` - <App> - <profile>` 解析）、CDP 活证据（`cu-cdp-smoke` PASS）、**后台 tab 读+写**（`page text/find/click/fill/nav/screenshot --target-*`，`cu-cdp-actuate-smoke` PASS，前景不变）。
   仍开 —— 真机 Brave Origin 开 `--remote-debugging-port` 后重跑同一条、`drag` 独立 pointer court、Linux/Windows 的 desktop-closure 旅程、`ps` 等 process 组、`browser *` 扩展桥。

**MCU 树应承认 cu 已产品化（勿再写「迁入 [ ]」）**

1. 默认环 + 四条不变量 + menu/focused/observe/close/receipts 已在 `agenterm-cu` mac 旅程落地。
2. empty-chrome/`titleIncludes`/`page-js` 后端名已进 cu JSON；`page text` / `tab list` / `tab select` / `page targets` 是 cu 拼写，MCU 树引用时按这些名字。
3. cu 多了 MCU 没有的：`--target ssh|vnc`、`--grant`、Spectacle `window-place`、始终 JSON stdout。
4. 写刀、扩展、PTY、job、process、device、privilege 等能力不能永久留在
   MCU；应由 ACU 通过 AgenTerm、qjswasm、libagenterm 或 platform 的 typed
   facade 调用其 owning mechanism，禁止在 CU crate 复制一套内核。

## 3. 日常怎么选入口

| 场景 | 用谁 |
|---|---|
| Agent 编排第三方 App（mac 语义树） | 优先 `agenterm-cu`（产品 ABI + grant） |
| Chromium 网页列节点 | 两边都走 AX `query`；不要先装扩展 |
| chatgpt.com composer / closed-shadow / tab-id | **MCU** `browser read --js`；cu `page-js` / `page find|click|fill --target-*` 需 CDP 口 |
| 后台窗口里的后台 tab（不能抢人的前景） | **cu** `page targets --browser-profile` → `page find` → `page click`/`page fill`（CDP，`focus_changed: false`）；AX 动词得先 `tab select` |
| 本机 PTY/job/设备/提权/Simulator | **迁移期**仍可能由 MCU/AgenTerm 本体执行；目标入口是 ACU typed facade |
| 远程桌面 worker | **cu** `--ssh`/`--vnc` |
