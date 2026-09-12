# guest runtime 落点法庭：CU 静态 feature vs provider ABI

Status: **active follow-on court; measurement starts after native-door qualification**  
Depends on: `plan/design-qjswasm-native-door-experiment.md`

## 0. 要裁决的问题

若 qjswasm native door 通过资格实验，CU 如何调用 guest 程序而不产生一个“目录里存在、发布拓扑里永远返回
`guest_runtime_unavailable`”的死动词？候选仅两个：

- **A：CU optional feature 静态链接 qjswasm**；
- **B：在 versioned provider ABI 增加 guest execution entry**，由已有 runtime owner 执行。

本文件是事前法庭。两种最小接线和测量均获准实施；只有正式表面在结果出来前不预注册，避免产生死动词。

## 1. 冻结约束

- Windows release 的 `agenterm.exe` / `agenterm-cc.exe` 各有 4 MiB 法庭；以 exact release artifact 为准。
- 交接中的“静态链接约 +5.1 MiB”是历史测量，当前树必须复测，不能直接判 A。
- provider ABI 已有 version/cancel 先例，但新增入口仍是 ABI 变更，必须证明版本拒绝、取消、panic containment 与生命周期。
- 不允许两个 runtime 真相、后台 detached thread、借用跨 `'static`、或只在测试 feature 下可达的 public verb。

## 2. 事前判据

1. **发布可达（boolean）**：正式产物中的 public black-box 能执行 guest；不接受 unit-only 或
   `guest_runtime_unavailable`。
2. **发布体积（首个淘汰门）**：A 的 exact release artifact 任一受限 PE 超过 4 MiB即 A 判负；不得抬预算。
3. **ABI 完整性**：B 必须有版本协商、输入/output bounds、typed unsupported、cooperative cancel、panic containment、
   callback/thread/lifetime 证明；任一缺失即 B 判负。
4. **单一 runtime owner**：方案不得复制 qjswasm engine/catalog/host bridges。
5. **六格证据**：六格 compile；有 runner 的 cell 跑 runtime smoke，无 runner 诚实 BLOCKED。
6. **边际成本**：记录 CU、provider、根产品各自 release bytes 与新增生产 LOC，不跨 profile 比较。

## 3. 判决树

- A 过体积且 B 未过 ABI 完整性：选 A。
- A 超 4 MiB且 B 过 ABI 完整性：选 B。
- 两者都过：先选新增 TCB/bytes 更小者；仍平局时选 runtime owner 单一、公开 topology 更少的一方。
- 两者都不过：不发布 `guest-run`；native door 仍可作为 qjswasm 内部能力。
- 禁止先落 verb 再等待 runtime；可达性与 public surface 必须同一 coherent increment。

## 4. 最小实验

- 同一固定 guest、同一 input/output/cancel corpus，分别接 A/B。
- exact release build 后量受限 PE；provider 动态库单列，不把字节藏出总交付账。
- public black-box 覆盖 success、typed guest error、pre-cancel、in-flight cancel、oversized input/output、provider mismatch。
- 故意破坏 provider version 与取消 acknowledgement，证明门会红。

## 5. 时间盒与 kill criterion

时间盒：一个工作日完成两种最小接线和发布尺寸/ABI法庭；不实现完整 guest-run 产品语义。

- A 一旦越过 4 MiB立即停止 A，不做尺寸微调竞赛。
- B 若需要未版本化 ABI、非 `'static` 借用逃逸、detached thread 或不能证明回调归还，立即停止 B。
- 到时间盒仍无单一可发布方案：记录 blocked，不保留死 verb。

## 6. 后续

胜者由人类确认后，才设计 CU public envelope、grant/审计/预算与 PRD capability；失败者代码全部移除。
若产品不需要 CU 入口，则两者都不落地。
