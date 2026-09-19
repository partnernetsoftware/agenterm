# uuctrl — 远控技术笔记

**这是什么**：一份 lab 技术笔记，记录商用远控产品（以网易 UU 远控为样本）的分层拆解，
以及它对 AgenTerm / MiniCon 未来「移动端 + 终端级远控」方向的启示。

**这不是什么**：不是 roadmap，不是 source map，不是版本承诺。产品状态的唯一真相仍在
[`prd/PRD_02_33_mobile_reach.md`](../prd/PRD_02_33_mobile_reach.md)（移动端）和
[`prd/PRD_02_22_decentralized_network.md`](../prd/PRD_02_22_decentralized_network.md)（传输）。
本文若与 PRD 冲突，以 PRD 为准。

记录日期：2026-09-19。

---

## 1. 样本：UU 远控的公开技术要点

需要先标清楚证据等级：网易**没有**公开架构文档。下表来自官网宣传页和第三方评测，
属于**指标级 / 宣传级**信息，不是设计细节，不要当作可验证事实引用。

| 层 | 公开说法 |
|---|---|
| 传输 | UDP + 私有协议，DTLS 握手，AES 加密 |
| 穿透 | P2P 直连优先，宣称 ~95% 穿透率；失败回落中继 |
| 中继网 | 复用 UU 加速器的 300+ 全球节点做优化中继链路 |
| 视频 | H.265 硬编为主，动态码率；4K@144Hz、HDR、4:4:4 真彩 |
| 延迟 | 宣称千兆内网 10ms 级，5G 远控 18–25ms |

**结论性判断**：UU 远控本质是「云游戏串流引擎 + 已有加速器专网」两件既有资产的组合，
不是新发明的技术。编码和 P2P 都是标准工程；真正难复制的是**中继网**——那是加速器业务
白送给远控的，第三方自建时唯一需要认真设计的部分。

## 2. 通用远控的六层分解

任何远控/串流产品都是这六层：

1. **采集** — Windows: DXGI Desktop Duplication / Windows.Graphics.Capture；
   macOS: ScreenCaptureKit；Linux: PipeWire/KMS。音频走 WASAPI loopback。
2. **编码** — NVENC / AMF / QSV / VideoToolbox。低延迟关键不在选 H.265 还是 AV1，
   而在编码器参数：CBR、零 B 帧、ultra-low-latency tuning、
   用 intra-refresh 代替周期性 IDR（避免关键帧码率尖峰）、按 slice 边编边发。
3. **传输** — UDP + FEC（丢包不重传，前向纠错）+ 自适应码率 + 极小 jitter buffer。
4. **穿透** — STUN 打洞 + ICE 选路 + TURN 中继兜底。
5. **输入注入** — `SendInput` 够用；游戏要 raw/相对鼠标；手柄要 ViGEm 虚拟设备。
6. **会话面** — 设备注册、配对、权限分级、审计。

## 3. 核心判断：终端级远控 ≠ 像素级串流

**这是整篇笔记最重要的一条，也是反直觉的一条。**

把上面六层套到「手机远程操作一个 AgenTerm/MiniCon 终端」上，会发现大半层被直接砍掉：

| 层 | 像素级串流 | 终端级远控 |
|---|---|---|
| 采集 | DXGI / ScreenCaptureKit | **不需要** — 源头就是结构化的 cell grid + PTY 字节流 |
| 编码 | H.265 硬编、HDR、4:4:4 | **不需要** — 文本 diff，压缩用通用 zstd 即可 |
| 传输 | UDP + FEC，允许丢帧 | **必须可靠有序**（见下） |
| 穿透 | STUN/ICE/TURN | 同样需要 — 这是唯一完整保留的一层 |
| 输入 | 键鼠/手柄注入 | 按键 + 少量 resize/焦点事件，量级极小 |
| 会话面 | 设备配对、权限 | 同样需要，且要求更严（终端 = 任意命令执行） |

两个量级差：

- **带宽**：1080p60 像素流约 15–20 Mbps。终端交互式输入 < 1 KB/s，
  大量输出（`cat` 大文件）突发也只到几百 KB/s，且可以在 server 侧合并/节流。
  **差 3–4 个数量级。** 这意味着中继节点的成本模型完全不同：
  像素流的中继成本是流量，终端流的中继成本几乎只是连接数。
- **延迟容忍**：游戏串流要 < 30ms 才不难受；终端本地回显在 50–100ms 内是可接受的。
  **门槛低一个档次。**

**传输语义的反转**（最容易踩的坑）：
像素流「宁可丢一帧也不要延迟」，所以用 UDP + FEC 不重传是对的。
终端流是**有状态**的——丢一个 escape sequence，后续所有渲染都错位，而且无法自愈。
所以终端远控**必须可靠有序**，TCP / QUIC / Noise+Yamux 反而是正确选择，
而 UU 那套「UDP + FEC + 私有协议」在这个场景下**不适用**。

不要因为「远控产品都用 UDP」就把这个结论搬过来。

## 4. 已有资产对照（2026-09-19 读码核实）

**本节在初稿后整体重写。初稿凭 PRD 文字推断，读码后发现推断错了两条，见 §11 撤销记录。**

终端级远控所需的线协议**已经存在**，不在 22 的 research 里，而在主仓 `src/ui_bridge.rs`
（1080 行，`UI_BRIDGE_SCHEMA_VERSION = 7` / `UI_BRIDGE_PROTOCOL_VERSION = 1`）：

| 能力 | 代码事实 |
|---|---|
| 续传位置 | `UiEventPosition { server_epoch, sequence }` — `src/ui_bridge.rs:240` |
| 增量帧与缺口判定 | `UiDeltaBatch` 带 `after_sequence` / `through_sequence` / `current_sequence` 三元序 + `complete` / `truncated` — `src/ui_bridge.rs:388-400` |
| 终端 cell 线格式 | `UiCellRun { row, column, columns, text, style }` 行程编码 + `UiScreenSnapshot` — `src/ui_bridge.rs:271, 280` |
| 版本协商 | `negotiate()` → `UiCompatibility::{Compatible, ClientTooOld, ClientTooNew}` — `src/ui_bridge.rs:649` |
| 交互租约 | `UiLeaseGrant`；TTL 常量 `UI_LEASE_TTL_MS = 5_000` — `src/ui_lease.rs:11` |
| 硬上限 | bootstrap 8 MiB / tabs 1024 / screen 512×512 / delta 64 events / input 256 KiB — `src/ui_bridge.rs:15-29` |
| 可替换 UI 已声明 | `headless_server_facts()` 打开 `replaceable_ui` / `interactive_lease` / `reconnect` / `rollback_proven` — `src/ui_bridge.rs:133-142` |

**结论：`server_epoch` + `sequence` 三元序就是我初稿里说「缺」的那套 resume 机制，
而且比我提的方案完整**——它同时区分「权威换代」（epoch 变 → 丢投影缓存重取 bootstrap）
和「本代内有缺口」（truncated → 补取或回退全量）。不要再设计第二套。

执行投影已写在 [`plan/plan-mobile.md`](../plan/plan-mobile.md)：M-A 抽 `agenterm-protocol`、
M-B 做 `agenterm-client-core`、M-C 才是壳。本笔记不重复它。

## 5. 真正的缺口（只剩一个半）

### (a) 传输：`validate_local()` 是硬墙 —— 唯一真需要新东西的地方

`resolve_ipc_endpoint` 对显式 endpoint 和 legacy address 都调 `validate_local()`
（`src/platform/contract/ipc.rs:344, 356`），legacy 路径的错误文案直接写着
"must use a loopback IP"，单测钉死相对路径 unix socket 必须 err（同文件 523-528）。
**手机从物理上连不到今天的 server。**

这是有意的桌面安全边界，33 明确要求配对「不得把本地 IPC 打穿」。所以正确形状是：
**新开一条授权面，而不是放宽 `validate_local()`**。22 的 attach 证明（Noise + Yamux +
签名邀请 + nonce/过期/scope + typed 拒绝）正是为这条面准备的，且它有意**不带**终端输入、
shell、PTY 或 `agenterm server` 权限路径——那个「缺」是设计，不是欠账。

中继可达性仍然是要自建的部分（见 §6）。对照 UU：他们靠加速器业务白送 300 节点；
我们靠终端流量比像素流小 3–4 个数量级，把节点成本压到可自建。**这是我们唯一的结构性优势。**

### (b) 客户端内核尚未独立

`UiClientModel` 的方法全是 `pub(crate)`——`connect` / `maintain_lease_if_due` /
`poll_deltas` / `select_tab` / `send_input` / `paste_input` / `resize_request` /
`detach`（`src/ui_client.rs:94, 202, 228, 255, 259, 264, 285, 434`），且直接吃 crate 内
本机 IPC。协议面已半独立，**客户端内核还没有**。这就是 M-A/M-B 的全部内容，
是搬家 + 把传输收成 trait，不是新设计。

### (c) 半个缺口：租约 TTL 对移动网络太短

`UI_LEASE_TTL_MS = 5_000`。手机切网/锁屏/后台挂起随便超过 5 秒。
plan-mobile 的 K3 已标为待决：失租后应当**保留只读投影**而不是黑屏。这是产品语义决策。

## 6. 穿透与公网入口

> **Owner 决定（2026-09-19）：不自建 relay 主机，复用 Cloudflare。**
> 这条决定让下面的打洞分析在产品路径上**大部分不再需要执行**——见 §6.5「决定之后」。
> 分析本身保留，因为它解释了这个决定为什么成立，也是将来换承载时的判据。

**「要不要公网入口」这个问题本身是错问的**，因为公网入口是三个可以分开的角色，
成本与必要性完全不同。混在一起谈才会得出「必须有公网服务器」这个过强结论。

| 角色 | 干什么 | 成本 | 能否省掉 |
|---|---|---|---|
| 会合 / 信令 | 双方交换候选地址 | 每次配对几 KB | 有带外信道就能省 |
| 地址发现（STUN） | 告诉你自己的映射地址 | 极小 | IPv6 或带外可省 |
| 数据中继（TURN） | 真的转发流量 | 带宽，持续 | 打洞成功就不用 |

严格答案：**不一定需要自己的公网服务器，但一定需要某种双方都能到达的会合点。**
会合点不等于服务器。

### 能真正做到零公网入口的三种情况

1. **同网段** — mDNS 发现，零基础设施。33 的「LAN first」就是这条，
   而且大概率覆盖真实使用的大部分场景（在家、在公司）。
2. **带外会合 —— 33 的二维码已经是这个东西。**
   QR 本来是为配对授权设计的，但它天然是一条带外信道，可以直接携带候选地址/端口。
   在 LAN 与 IPv6 场景下，**信令服务器可以完全省掉**。
   已有组件顺手多担一个角色，不需要新造件。
3. **IPv6** — 有 IPv6 就没有 NAT，端点本身可路由，只需防火墙打孔（出站包通常就开了）。
   国内移动网与家宽覆盖已相当好。**建议先测量实际链路的 IPv6 可达性再决定是否建 relay**：
   测量成本极低，但可能直接省掉一整层基础设施。

### 一条硬事实

**双侧 CGNAT + 对称 NAT = 无论多聪明都得中继。** 国内 IPv4 移动网是重度 CGNAT。
要覆盖真实场景，relay 兜底跑不掉。

### 两组公开数据（差距本身就是信息）

- **Tailscale**：典型条件下直连率 >90%。但它的设计是**每条连接都先经 DERP 起步**，
  再尝试升级为直连。
- **libp2p DCUtR 在 IPFS 生产网的大规模测量**：打洞阶段条件成功率 **70% ± 7.1%**
  （前提是 relay reservation 与公网地址发现都已成功）。TCP 与 QUIC 都约 70%，
  **推翻了「UDP 打洞比 TCP 容易」这个流传很广的说法**。

第一条正面回答了本节的问题：**即使直连率 90%+，公网入口在「起步」阶段仍是必需的**——
打洞本身需要有人协调。第二条说明别照抄 UU 宣称的 95%，那个数字高度依赖实现与网络环境。

### 6.5 决定之后：打洞整条被绕开

选了 Cloudflare Tunnel，**就不再做 NAT 穿透**——两端各自**拨出**，中间在 Cloudflare 边缘会合。
于是 STUN / ICE / 打洞 / DCUtR / AutoNAT、以及上面那些 70% 与 90% 的成功率，
**全部不用关心**。这不是妥协，是对终端流量剖面的正确取舍：
带宽小 3–4 个数量级、延迟预算 50–100ms，全程中继完全够用。

剩下的形状只有两层：

| 层 | 承载 | 说明 |
|---|---|---|
| **L0 LAN 直连** | mDNS + QR 带地址 | 保留。零依赖、最低延迟，覆盖在家/在公司 |
| **L1 Cloudflare Tunnel** | `cloudflared` 出站 → 域名，手机走 WSS | 其余全部场景，含 CGNAT、跨国、企业网 |

要点：

- **桌面端只出站，不开监听。** 这比自建 relay 更符合 22/33 已写死的
  「装了不能静默开监听」——`cloudflared` 是一条出站连接，不是一个 listener。
- **走 HTTP/WSS 模式，不要走裸 TCP 模式。** WSS 让 PWA 用浏览器原生能力直连，
  客户端**不需要装任何东西**；`cloudflared access tcp` 则要求客户端也装 cloudflared，
  那会直接违背 33「PWA 第一宿主」的排序。
- **443/TLS 天然穿企业网与酒店网**，原 L3 兜底层一并消失。
- **Cloudflare 只搬字节，不做授权。** 22 的签名邀请 + Noise 握手仍然在隧道**内部**
  端到端跑，Cloudflare 看到的是密文。不要用 Cloudflare Access 替代应用层配对
  （它按用户计费、免费档 50 人，且那是**接入控制**，不是产品的授权模型）。
- **Workers + Durable Objects 是可选项，不是必需项。** 有隧道就不需要会合点。
  将来若要多设备扇出或推送再考虑；DO 的 WebSocket Hibernation 现已覆盖免费档，
  长连接不再按驻留时间计费。

已核的额度（2026-09 查，属易变信息，动手前复核）：
Tunnel 本身免费、带宽不计量、隧道数不限；WebSocket 默认开启；
Zero Trust Access 免费档 50 用户——**上限只在接入控制那一侧，不在隧道传输上**。

前置条件：Tunnel 需要一个托管在 Cloudflare 的域名。`agenterm.work` 已在 33/M13 的
路线上（当前 Pages CNAME 是过渡别名），DNS 是否迁到 Cloudflare 需单独确认。
`try.cloudflare.com` 快速隧道无需域名但主机名随机，只适合开发，不能当产品形态。

### 代价（如实记录）

- **全程中继，永不直连。** 路径是 手机 → CF PoP → 桌面。对终端预算够用；
  若将来真做 §8 的像素级远控，这条会变紧，届时需重新评估。
- **引入第三方依赖**，与「自管理」有张力。缓解的是 plan-mobile 的硬约束：
  `agenterm-protocol` **传输无关**——Cloudflare 只是一个承载，协议仍然是自己的，
  将来换 relay、换 libp2p、换自建都不动协议层。**这正是那条硬约束的价值兑现处。**

### 安全与成本目标是一致的，不冲突

公网入口 = 攻击面。22 与 33 都写死了「装了不能静默开监听」。
所以 relay 必须显式 opt-in，桌面端默认不公网监听——
**「最小化公网入口」既是成本目标也是安全目标**，不需要在两者之间权衡。

### 参考实现

| 项目 | 看什么 |
|---|---|
| Tailscale / Headscale + DERP | DERP 是「最小中继」的参考设计；Headscale 是可自建控制面。最贴近自管理形态 |
| libp2p AutoNAT + circuit-relay-v2 + DCUtR | 直接相关，已有 relay-v2 |
| coturn | 标准 TURN，WebRTC 生态 |
| Nebula（lighthouse）/ ZeroTier（moon 可自建） | 另两种会合点形态 |
| frp / rathole | **反例参照**：设计上就要求公网入口，帮你看清哪些部分可以省 |

上表在 Owner 决定后**降级为备选**：它们是「若将来离开 Cloudflare」时的替代路径，
不是当前要建的东西。

## 7. 建议的增量顺序（执行归属仍在 33/22，本文不认领）

- **L0 observe**：手机看到 fleet 树 + 某个 tab 的只读 cell 流。LAN 内先跑通。
- **L1 resume**：加序号与断线续传，能扛 WiFi↔5G 切换。
- **L2 relay**：一个自建公网 relay，离开 LAN 可用；保留 TCP/443 兜底。
- **L3 input**：显式 grant 后可发按键；修饰键 UX 单独设计。
- **L4（可选，很远）**：像素级 GUI 远控 —— 见下节，届时是另一条技术栈。

每一层都要有黑盒证据才算数，这条沿用 33 的 "Design text is not evidence"。

## 8. 如果将来真要做像素级（备忘，非当前方向）

记下来免得以后重新调研：

- **Sunshine（被控）+ Moonlight（控制端）**：开源，协议源自 NVIDIA GameStream
  （RTSP 信令 + ENet 控制 + RTP over UDP + FEC），支持 HEVC/AV1、HDR、YUV 4:4:4、高刷。
  画质上限对标甚至超过商用产品，缺的正是穿透和中继。
- **穿透补齐**：Headscale（自建 Tailscale 控制面）+ 自建 DERP 中继 —— 这就是
  自管理版本的「全球加速节点网」。
- **RustDesk 自建 hbbs/hbbr**：办公向远控的现成答案，跨平台，但串流引擎不是游戏级。
- **自研路线**：Pion / libwebrtc 拿到 ICE/DTLS/SRTP 全家桶，代价是 WebRTC 默认的
  GCC 拥塞控制是为视频会议调的，对串流偏保守，要改。
- **测量陷阱**：端到端延迟大头常常不在网络，而在显示端的 vsync 和合成器（16–33ms）。
  要用外接相机拍屏，不要信应用自报数字。

## 9. AgenTerm 与 MiniCon 的分工 —— 远控归 AgenTerm，MiniCon 不碰

**初稿在这里写错了，见 §11 撤销记录。**

产品线里已经有三个终端级 attach 面，它们**故意不互通**：

| 面 | 归属 | 性质 |
|---|---|---|
| `ui_bridge` 可替换 UI 协议 | agenterm | server ↔ 客户端，有 epoch/sequence/租约 —— **远控唯一正确的接入点** |
| `agenterm cli` 控制面 | agenterm 07/15 | 典型操作、快照、确定性等待 |
| `ATC1` 私有帧 | minicon 26 | GUI 生命周期内的本地 socket/pipe |

MiniCon 的宪章把「不做远控」写成了产品定义，不是欠账：
PRD 26 原文 —— 「`--control` 绑定的是 **GUI 进程内部**的本地 pipe/socket；
没有默认端口，**没有远程传输**，没有 Fleet、mux、会话持久化或 Agent 权限模型。
AgenTerm 的 server 身份可以比一个窗口活得久；MiniCon 的**不允许**。
监听器在 GUI 里面是设计，不是缺一个 server。」
PRD 根还钉了 idle 单 tab 10 MiB 的 RSS 意图。

**给 MiniCon 加远控会同时摧毁这三条**（进程寿命、无 server、内存地板）。
它和 agenterm 的共享面是 `minicon-core`（composer / json / tree / 滚动条几何，
零平台依赖，由测试强制），远控不属于那一层。

## 10. 明确不做

- 不在手机上跑 PTY / server / workspace 权威（33 已拒绝 F6）。
- 不用「远控」当理由放松 loopback IPC 的桌面安全边界（33 明确写死）。
- 不把 22 的 attach 证明直接提升成远程控制通道 —— 那条证明**没有**终端输入、
  shell、命令、PTY 或 `agenterm server` 权限路径，这是有意的。
- 不做 always-on 静默远程访问；不做撤销后仍存活的配对。
- 不为了追 UU 的画质指标去引入视频编码依赖——那和终端级远控是两条栈。

## 11. 撤销记录与剩余开放问题

### 撤销（2026-09-19，读码后）

初稿是在只读 PRD 文字、未读代码的情况下写的，有三条判断作废：

1. **「缺 resume 机制，需要在 wire 上带序号」** —— 作废。
   `UiEventPosition` + `UiDeltaBatch` 的三元序早就在 `src/ui_bridge.rs`，
   且比初稿设想的完整（同时区分权威换代与代内缺口）。
2. **「observe-only 的 cell 流要不要复用 snapshot 契约还是新设计 diff 帧」** —— 伪问题。
   `UiCellRun` 行程编码 + `UiScreenSnapshot` + `UiDeltaBatch` 已经是那套 diff 帧。
3. **「MiniCon 做被 attach 的一侧」** —— 作废且方向相反。
   那会违反 MiniCon PRD 23/26 的宪章（无 server、无远程传输、窗口关闭即结束）。
   远控整条归 AgenTerm。

教训与仓级纪律第 5 条一致：**断言「缺什么」之前先读代码给 file:line。**
凭 PRD 散文推断缺口，三条错了两条半。

### 仍然开放

- ~~自建 relay 的最小拓扑~~ —— **已由 Owner 决定关闭：复用 Cloudflare Tunnel，
  不自建主机，不做打洞（§6.5）。** 剩下的待确认项只有一个：
  `agenterm.work` 的 DNS 是否迁到 Cloudflare。
- `UI_LEASE_TTL_MS` 在移动网络下的产品语义（plan-mobile K3）：失租后保留只读投影的
  具体表现是什么。
- 壳选型 K1 未拍板（Flutter+flutter_rust_bridge / RN / Tauri 2）。终端是高频重绘字符网格，
  起稿人倾向 Flutter；这条与远控传输无关，可并行。
- UDP 承载是否值得做：终端流需要**可靠有序语义**，但不必须是 TCP 协议。
  自己做 seq+NACK+重传（即 QUIC 的做法）能同时拿到打洞与 0-RTT 重连，对移动端切网更优。
  `agenterm-protocol` 传输无关是硬约束，所以这条**不阻塞** M-A/M-B，可以最后定。

## 12. 参考

UU 远控公开信息（宣传/评测级，非架构文档）：

- 官网 <https://uuyc.163.com/>
- 网易UU远程全功能技术解构，CSDN <https://blog.csdn.net/2301_81073317/article/details/154915779>
- UU远程半年使用记，CSDN <https://blog.csdn.net/sinat_41617212/article/details/164154172>
- 品玩实测报告 <https://www.pingwest.com/a/310002>

NAT 穿透（§6 数据来源）：

- DCUtR in IPFS 大规模测量 <https://arxiv.org/html/2604.12484>
- Challenging Tribal Knowledge — 去中心化 NAT 穿透测量 <https://arxiv.org/pdf/2510.27500>
- How NAT traversal works，Tailscale <https://tailscale.com/blog/how-nat-traversal-works>
- Tailscale NAT traversal improvements pt.1 <https://tailscale.com/blog/nat-traversal-improvements-pt-1>
- DERP servers，Tailscale Docs <https://tailscale.com/docs/reference/derp-servers>
- libp2p punchr <https://github.com/libp2p/punchr>

Cloudflare 承载（§6.5 数据来源，额度易变，动手前复核）：

- Cloudflare Tunnel 文档 <https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/>
- Workers 定价 <https://developers.cloudflare.com/workers/platform/pricing/>
- Durable Objects 定价 <https://developers.cloudflare.com/durable-objects/platform/pricing>
- Cloudflare 连接限制 <https://developers.cloudflare.com/fundamentals/reference/connection-limits/>

仓库内相关：

- [`prd/PRD_02_33_mobile_reach.md`](../prd/PRD_02_33_mobile_reach.md) — 移动端产品状态权威
- [`prd/PRD_02_22_decentralized_network.md`](../prd/PRD_02_22_decentralized_network.md) — 传输/配对权威
- [`prd/PRD_02_07_agent_control_plane.md`](../prd/PRD_02_07_agent_control_plane.md) — 统一控制面契约
- [`research/agenterm-net/`](../research/agenterm-net/) — attach / mesh 现有证据
