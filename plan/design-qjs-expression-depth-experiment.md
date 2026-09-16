# QJS runtime expression-depth 判决实验

| 项 | 值 |
|---|---|
| 日期 | 2026-09-16 |
| 目的 | 判定 tinyvm-qjs 能否精确执行 PRD 36 的 invocation-scoped `expression_depth`，且不把它偷换成调用深度、activation slots 或静态源码嵌套 |
| 实现位置 | 上游 tinyvm 的 `crates/tinyvm-qjs/`；本文件只冻结实验规格 |
| 前置阅读 | `prd/PRD_02_36_agenterm_qjswasm.md` 的 remaining public budget truth；上游 `emit.rs` 的 expression lowering、call、throw/catch/finally 接缝 |
| 来源纪律 | 只使用本仓 PRD 与上游源码；不引入 AgenTerm 产品词汇或策略到 tinyvm |

本实验属于已具名的产品正确性叶，不是 must-ship 状态本身。只有 §4 的判决树
给出可实施结论并由产品黑盒闭环后，PRD 能力状态才可改变。

## §0 已确定的事实

1. `expression_depth` 表示一次 invocation 中，当前函数里同时活动、尚未完成的
   JavaScript 表达式求值链。“当前函数里”是对 PRD“function call frames are not
   charged”的操作化定义，不是另行缩小能力范围。
2. 它不累计函数调用帧。深调用配浅表达式不能因为调用者仍等待返回值而耗尽本预算；
   调用深度与 activation slots 已有独立预算。
3. 未执行的条件分支、短路右操作数和 catch 之外的死路径不收费。
4. reusable packed artifact 必须在每次装载时接受不同 ceiling；值不得烘进 artifact。
5. 上游只拥有通用 runtime limit 与 distinct exhausted fault；AgenTerm 拥有数值来源、
   audit 与 `Budget("expression_depth")` 投影。
6. 固定内存地址与新增 guest export 已在 collection-items 叶中被排除；本实验不重开。

待判定的结构选择只有两条：

- **A：函数内计数器。** artifact 声明一个 generic limit import；mutable guest global
  记录当前函数的活动表达式深度。JS 调用点在进入 callee 前保存并清零，返回后恢复；
  catch/finally 的 abrupt-completion 接缝恢复当前函数基线。
- **B：隐藏 depth 参数。** 编译器给 guest 函数与间接调用 ABI 增加内部参数，以参数传递
  当前函数的表达式状态；调用点显式建立 callee 的零基线。

## §1 硬约束

任一违反即实验无效：

1. exact-limit 成功、limit-plus-one 只返回 distinct expression-depth fault。
2. 深表达式、深调用配浅表达式、短路死分支、caught throw 后继续执行四类反例必须
   同时通过；不得用 call depth、activation slots、fuel 或编译期最大嵌套代替。
3. 同一 packed artifact 在 ceiling `N` 与 `N-1` 下分别成功和拒绝，不得重编译。
4. runtime-limit opt-in 关闭时，旧 compile API 的 import/export 与行为不变。
5. 不新增 guest export，不占用固定 memory word，不让 tinyvm-qjs 认识 AgenTerm 名词。
6. 未声明 limit import 的 artifact 继续可装载；声明但宿主未绑定则响亮拒绝。
7. throw、catch、finally、return 与 short-circuit 不得留下计数债务，也不得把未执行表达式计入。
8. 病灶探测器：任何为了让计数方便而把新的通用异常栈、产品预算表或第二套调用 ABI
   塞进 tinyvm 核心的冲动，都是实验要记录的失败信号，不是需要满足的需求。

## §2 最小实验内容

| 维度 | 固定内容 | 原因 |
|---|---|---|
| 语义集合 | 二元/一元/conditional/logical/call/throw-catch-finally | 覆盖正常、分支与 abrupt completion |
| 调用隔离 | 递归 32 层，每层只有一个浅表达式 | 直接反证把 call frames 算入 expression depth |
| 深表达式 | 8 层与 64 层同形表达式 | 至少两个点，能检查插桩增长斜率 |
| 死分支 | `false && DEEP`、`true ? shallow : DEEP` | 静态嵌套近似会被判负 |
| 当前函数 catch | 深表达式中 throw，被当前函数 catch；catch 后再执行浅表达式并核对结果 | 检出遗漏 decrement 或错误清零 |
| 跨函数 throw | callee 深处 throw、caller catch；catch 后在 caller 再执行 exact-limit 表达式 | 同时压住调用隔离与异常恢复 |
| finally 求值 | normal/throw 两条路径都进入含 exact-limit 表达式的 finally | 证明 finally 使用当前函数的新基线而非已中止链 |
| return + finally | `try` 内 return，finally 求值后恢复 return；另测 finally 的 return 覆盖 pending return | 覆盖 return 与 finally 的两个 abrupt-completion 出口 |
| artifact 复用 | 一份 bytes、两个运行期 ceiling | 冻结每 invocation 语义 |
| 变体顺序 | 先 A；只有 A 被布尔门证伪才实现 B | 避免两份大原型引入投入偏差 |

不把性能优化、产品 audit 接线或三平台法院放进本实验。它们在机制语义成立后属于产品叶。

## §3 事前判据与度量纪律

| 编号 | 判据 | 性质 | 通过条件 |
|---|---|---|---|
| G1 | §1 的语义矩阵 | 布尔 / 安全 | 全部通过；任一近似、漏计或错误债务即失败 |
| G2 | reusable artifact 两 ceiling | 布尔 | 同 bytes 给出 exact 成功与 plus-one distinct refusal |
| G3 | guest ABI 漂移清单 | 清单 / 安全 | A 不改变已有 guest 函数签名、table 形状和 export；B 必须逐项报告变化 |
| G4 | emitted mechanism 增长 | 斜率 | 对 8/64 层分别记录 wasm bytes 与新增动态检查次数，拆分共享 prologue 与逐表达式成本 |
| G5 | abrupt-completion 接缝数 | 清单 | 列出必须恢复状态的 call/throw/catch/finally/return 接缝；没有未解释的旁路 |

度量命令、toolchain SHA、upstream SHA 与测试源码必须写进上游实验结果。字节只比较同一
编译器、同一选项生成的 wasm artifact；不得把 Rust 测试二进制大小与 emitted wasm 相除。
G4 是工程成本，不可覆盖 G1/G2 的语义判负。

## §4 判决树、kill criterion 与时间盒

本实验不是把 A、B 同时实现后横向计分的对照实验：G1-G5 是方案内判据；A/B 的分叉只
由第 3 步的失败归因与 G3 的结构代价决定。

1. 先静态审计 A 的状态恢复能否覆盖 direct/indirect/callback call 与
   throw/catch/finally/return 的全部 abrupt-completion 边。发现无法落点的边，直接把该
   最小反例带入第 3 步；静态未证伪才实现 A 的最小插桩并跑 G1。
2. 若 A 的 G1 通过，再跑 G2、G3、G4、G5。G1/G2/G5 全过且 G3 无已有 ABI 漂移，判
   **A**；G4 只决定是否另立优化叶，不推翻语义判决。否则进入第 5 步。
3. 只有 A 因“无法在不改函数 ABI 的情况下正确恢复异常或隔离调用”而失败，才实现 B。
4. B 必须重跑同一 G1/G2/G4/G5；若通过，且 G3 的 ABI 漂移被完整限定为编译器内部、
   旧 opt-out artifact 不变，判 **B**。
5. A 判负且 B 未触发，或 B 也失败，则保持 `expression_depth` 为 unenforced，回填最小
   反例，不加近似实现。任一核心反例经过两轮定向审计仍无法形成可观测的 limit-boundary
   判定时，记为“未决”并暂停，不得算通过；观察只用 exact/plus-one 行为，不导出计数器。

kill criterion：

- 任一方案需要把 limit 烘进 artifact、复用 call depth/activation slots、增加 guest export、
  固定 memory word 或 AgenTerm 专有 import，立即判负。
- caught throw 后只能靠“结束整个 invocation”掩盖计数债务，立即判负。
- 只跑静态深源码、不跑 dead branch 与深调用反例，实验无结论。
- 为过 G1 放宽 PRD 语义或删除反例，实验无效。

时间盒：先完成一次静态接缝清单；随后最多两轮定向实现/审计，使 A 的 G1 核心反例
（深表达式、深调用、死分支、当前/跨函数 throw、finally、return）各自得到明确结果。
仍不可判定的格记为“未决”并暂停。此前不做产品接线、性能优化或 B；A 被上述特定原因
证伪后，时间盒转为 B 的同一 G1 结果。

判据对账：G1、G2 是首要布尔门；G5 是安全门；G3 决定 A/B 的结构代价是否可接受；
G4 最后报告且不能凌驾于语义。五个判据都在判决树中有位置。

## §5 建议目录

```text
tinyvm/crates/tinyvm-qjs/tests/expression_depth.rs
tinyvm/crates/tinyvm-qjs/src/opts.rs
tinyvm/crates/tinyvm-qjs/src/emit.rs
tinyvm/crates/tinyvm-qjs/src/runtime.rs
```

若 A 只需现有 lowering 与 runtime-limit 接缝，不新建 research crate。结果随上游测试提交，
并在本文件 §8 回填判决 trace 与复跑命令。

本实验必须等待 collection-items 的上游提交与本仓 pin 正式落地后再开工；两叶共用
`emit.rs`、`lib.rs`、`opts.rs` 与 `runtime.rs`，不得在同一热文件集上并行实现。

## §6 已排除选项

| 选项 | 排除原因 |
|---|---|
| `Limits::max_call_depth` | 深调用与浅表达式是独立事实，PRD 明确要求反例 |
| activation slots | 局部变量与调用资源，不表示活动表达式链 |
| 编译期 AST 最大深度 | 会向未执行短路/conditional 分支收费，且不能表达每 invocation ceiling |
| fuel / instruction count | 累计工作量，不是同时活动深度 |
| 固定 memory word | 与运行时 fault/detail 区域存在所有权冲突，且形成隐式 ABI |
| hidden guest setter export | 扩大 guest 可调用面，忘记调用时会静默无界 |
| 每个 expression 都跨 host 查询并维护计数 | 把 guest 内部状态变成高频 host 往返，且 abrupt completion 仍需恢复协议 |

## §7 本实验不回答什么

- 预算默认值或 public override UX；
- 三平台运行时证据与 Candidate receipt；
- 表达式插桩的后续性能优化；
- object/property/collection budgets；
- tinyvm 对完整 ECMAScript expression taxonomy 的未来扩展。

## §8 结论回填

### 最终判决（2026-09-16）

上游 tinyvm `9805985` 已完成并判定 **A 胜出**。G1/G2/G3/G5 全过：4 层 exact-limit
成功、3 层 ceiling 返回 distinct expression-depth fault；32 层递归浅表达式不计调用帧；
dead/short-circuit 分支不收费；direct、indirect 与 runtime callback 均有上下界证据；catch、
跨函数 throw、normal/throw finally 与 return+finally 保持精确结果；普通 opt-out 编译 bytes
逐字相等。G4 同工具链测得 8 层 10,884 B / 17 checks，64 层 15,028 B / 129 checks。

完整判决 trace、safe failure、适用边界与复跑命令位于上游
`research/expression-depth/RESULTS.md`。B 未触发，C 保持判负。本仓随后以远端 pin
`9805985` 接入 generic import 与 fault 映射；产品 evidence 由 PRD 36 和 owning tests 管理，
不复制上游 RESULTS。

### 静态预审（非实验判决）

已按上游 `6cff7d4` 完成第 1 步静态接缝审计，尚未实现插桩、未运行 G1-G5：

- expression lowering 当前没有 expression-depth 计数器；控制块深度、解析器帧、call depth
  与 activation slots 都不是本预算的可复用事实；
- short-circuit 与 conditional 的运行期分支天然不会执行死支，支持 A 的运行期计数方向；
- throw/catch/finally 使用 unwind globals、handler labels 与 completion slots，而非 wasm EH；
  `bind_caught`、`leave_with_throw` 与 invocation-entry reset 提供了 A 所需的状态复位落点；
- A 因而没有被“无法在不改变函数 ABI 的情况下恢复异常或隔离调用”静态证伪；B 不触发，
  C 则因累计挂起 caller 资源且 limit 绑定在 module/instance 构造期，不满足调用隔离与
  per-invocation ceiling；
- finally 与 return/finally 的交互仍只能由 §2 的动态反例裁决，不能从静态落点存在推导通过。

这份预审只把实施顺序收敛为“A + 显式异常/入口复位”；它不是 A 的通过证据，也不改变
PRD 状态。正式回填仍必须包含 G1-G5 表、A 实际走过的判决路径、与规格不符之处、复跑命令
以及是否出现推翻预期的结果。在结果落库前，本实验状态为“已立项、静态预审完成、实现未开工”，
不得被引用为 `expression_depth` 已实现或已判决。
