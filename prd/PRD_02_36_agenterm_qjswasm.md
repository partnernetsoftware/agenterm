# PRD 02.36 — `agenterm-qjswasm`（自研脚本引擎：`.qjs` 编译到 `.wasm`，tinyvm 当核）

Status: 引擎脊柱已落地并有实测证据（M0 + 上游 M1/M2 语言子集，`cargo test -p
agenterm-qjswasm` 86 passed / 1 ignored，2026-08-25）；两条归档门**均未全绿**，
两个被取代的 crate 原样保留。「authorized, not implemented」是 2026-08-24 下单时的
状态，已过期。本文件是产品真理；执行投影在
[`plan/design-agenterm-qjswasm.md`](../plan/design-agenterm-qjswasm.md)。

门的当前判定，一句话各一条（详见下文各节）：

| 门 | 判定 | 挡在哪 |
|----|------|--------|
| 归档 `agenterm-qjs` 门 1（`fleet.js` 等价物） | 远未绿 | 堆对象 + 函数值 + `try/catch` + JSON + 一条 `.qjs` 到门的路 |
| 门 2（两处生产调用点迁移） | 未动 | 依赖门 1 |
| 门 3（CLI 面） | **「声明」那一半已交付**，「有对应面」那一半未做 | 十三动词判决已定；前置 `Guest::CompiledQjs` 已补 |
| 归档 `agenterm-wasmcore` 门 1（能力清单） | **可判绿** | — |
| 门 2（`.wasm` 路由切换） | 不能绿 | `_start` 入口约定；缺同客人性能对比 |
| 门 3（现状实测） | 已复核 | — |

Owner: 政委定方向；主会话按独占文件域推进。

Upstream: [`partnernetsoftware/tinyvm`](https://github.com/partnernetsoftware/tinyvm)（本地 `../tinyvm`），
PRD 35 记录其迁出。依赖方向 **agenterm → tinyvm**，单向。本 crate 依赖上游两个 crate：
`tinyvm`（执行核）与 `tinyvm-qjs`（`.qjs → .wasm` 编译器），同一 rev 钉死。

Supersedes: `crates/agenterm-qjs`（rquickjs 外链）与 `crates/agenterm-wasmcore`（wasmtime + WASI p1）——**两者均待归档**（政委 2026-08-25 定），门见下。

---

## 产品句

**agenterm 自己的脚本引擎。`.qjs` 用纯 Rust 编译成 `.wasm`，`.wasm` 直接跑；核是
tinyvm——无 JIT、装载期校验、上限在核。不链 QuickJS C，不用 rquickjs。**

「编译一次到 `.wasm`」是 AOT/JIT 的概念落在**字节码**这一层，不是机器码：
`.qjs` 源码 → 自研编译器 → 标准 `.wasm` 字节 → tinyvm 解释执行。产物是普通 wasm，
过同一道装载期校验，吃同一套 `Limits`，跟手写的 `.wasm` 客人待遇完全一致。

## 定名

政委 2026-08-24 定 `.qjs` 为扩展名，两条理由：**一来尊重 QuickJS，二来 `.js` 容易和
Node.js / Bun 混淆。**

`.qjs` 是 agenterm 的 QuickJS 系 JavaScript：不是 Node（没有 `require` / `fs` /
`process`），不是浏览器 JS。看见 `.qjs` 就知道运行时是谁。

crate 名 `agenterm-qjswasm` = qjs + wasm，指的就是那条编译路径。

## 纪律

- **纯 Rust。** 编译器与运行时支持全部 Rust 实现。不链 QuickJS C 库，不用 `rquickjs`，
  不引入任何 C 依赖或构建期 C 工具链。
- **核只吃 wasm 字节。** `.qjs` 先降成 `.wasm` 再进核；核脸不接源码。
- **上限在核。** 预算用 tinyvm `Limits`，不在 agenterm 侧另造一套限流。
- **不做 JIT / AOT 到机器码，不碰可执行内存。** 「AOT」在本产品里只指到 wasm 码。
- **能力全在门。** 门名单是 `agenterm.*`，不得把 WASI `fd_*` 做成第二扇 OS 面。
- **不搬 tinyvm 源码。** git + rev 钉死，vendor 是违纪。
- **编译器在上游 `tinyvm-qjs`，本仓只留业务。** 见下 §编译器归属的撤销。
- **测试优先：先验收测再改脸。工人自报不算过。**

### 编译器归属的撤销（2026-08-24）

**曾经决定：** 编译器写刀在 agenterm 仓。原文：「tinyvm 只提供 `eval_wasm` + 校验 +
`Limits` + 门；语言由 agenterm 自己长，不受另一个仓的排期约束。上游 `tinyvm-qjs` 是
tinyvm 自己的演示皮，与本 crate 各长各的，不共用写刀。」

**该决定撤销。** 编译器已迁往上游 `tinyvm-qjs`，本 crate 建立 Cargo 依赖。

撤销理由两条，第二条是对原理由的直接否定：

1. **分层原则先于归属偏好。** 定下的分层是「通用动态引擎能力归 tinyvm，业务归
   agenterm」。按这条尺子量：迁走的 1113 行（lex / parse / ast / ir / emit / encode /
   diag）里**零个** agenterm 概念——它解决的是「JS 源码怎么变成 wasm 字节」，与谁在
   embed 无关。而留下的 `host.rs` 869 行里 `fleet` 出现 48 次、模块名写死
   `"agenterm"`，那才是业务。原决定把两半绑在一起，是按「谁先写的」而不是按「是什么」
   划线。
2. **排期绑架的风险被高估了。** 「不受另一个仓的排期约束」预设两个仓有各自的排期主体。
   实际上两仓同一个 owner、同一台机器、同一批工人；PRD 36 本身已授权「撞到 tinyvm 层
   的真实缺口就去 tinyvm 仓改，不绕」。在这个前提下，「上游排期」不是外部约束，是同一
   个人的排期——为躲一个不存在的依赖风险而各长一份编译器，代价是两份都长不快。

**边界没有变**：`agenterm.*` 门、槽、预算策略、`ScriptBackend` 接线仍在本仓，仍是本
crate 的写刀。上游给的是「编译 `.qjs`」这一件通用事。
`agenterm_qjswasm::compile_qjs` / `CompileError` 原样保留为再导出，调用点不动。

**顺带修掉的一处静默漂移**：`src/slot.rs` 的失败分类曾抄一份 tinyvm 的 trap 文案表
（`"step budget" | "call depth" | "call stack"`）。上游把 `"call stack"` 拆成四种条件
之后，抬 rev 会让「活动记录槽耗尽」从 `Budget` 悄悄变成 `Trap`——不报编译错，现有测试
也全绿（它们只断言活下来的那两条文案）。现改用上游的 `WasmError::class()` /
`ceiling()` 存取器，文案表删除，并补上那条本该存在的测试
（`exhausting_max_activation_slots_is_reported_as_that_budget`）。

## Clean-room 与来源

遵 [PRD 14 Research provenance](PRD_02_14_research_provenance.md)。

- **ECMA-262 是语义权威**。前端正确性以规范为准，任何实现（含 QuickJS）都不是判据。
- **QuickJS 是重点设计参考，不是次要参考。** 政委 2026-08-24：「quickjs 还有很多值得
  学习的……编译逻辑什么的也应该很有参考价值」。这条判断是对的，本 PRD 采纳。
- **边界仍是 clean-room**：可以吸收公开行为、数据结构设计、算法思路、取舍理由；
  **不得抄源码、注释、标识符、查找表、文档措辞**。写出来的必须是自己的 Rust。
- QuickJS 是 MIT 授权，与本仓 `MIT OR Apache-2.0` 兼容；但**直接复用**仍须按 PRD 14 走
  显式 provenance review，本 PRD 不预先授权任何直接复用。
- 测试向量独立构造，或取自规范条文示例并注明出处。

### 从 QuickJS 挖什么（按分期对照）

早前一版本文档写过「QuickJS 的价值集中在后端，本产品用不上」——**那句话是错的，已
撤销**。它把「后端 = 字节码解释器」误当成「除解析器外的一切」。QuickJS 最难的部分在
中间层，而中间层几乎全是**目标无关**的：解决的是「JS 语义如何紧凑表示与解析」，不是
「字节码如何派发」。

| 子系统 | 我们要定的决策 | 期 | 可移植性 |
|--------|----------------|----|----------|
| 作用域与变量解析（`var`/`let`/`const`、提升、TDZ、名字→索引） | wasm locals 本就按索引寻址，这一步必须做 | M1–M2 | **极高**（同一个问题） |
| 闭包变量装箱（捕获的局部变量提升为堆单元） | wasm 局部变量在帧上，内层函数无法按引用捕获，必须装箱进线性内存 | M5 | **极高** |
| 值表示（64 位标记联合 vs 32 位 NaN-boxing 的取舍理由） | **已判决**：V1 双字 `(tag:i32, payload:i64)`，由实测实验定，见 `plan/design-value-representation-experiment.md` 与 `research/value-representation/RESULTS.md` | 已落地 | **高** |
| 字符串表示（8 位 / 16 位双形态 + 驻留表） | M3 串表示与相等性 | M3 | 高 |
| shape / 隐藏类（属性查找） | M4 对象堆布局 | M4 | 高 |
| 引用计数 + 循环回收 | M4–M5 回收策略；小引擎选 RC 是强数据点 | M4–M5 | 高 |
| `try` / `catch` / `finally` 状态机 | tinyvm 核不吃异常提案，展开需自编码 | M5 | 中（设计层） |
| ASI 与解析器实务 | 规范是权威，QuickJS 是「实际怎么写才不痛」的样板 | M1 | 中 |
| 字节码指令集 + 解释派发循环 | 本产品后端是 wasm | — | **低**——真正不通用的只有这一行 |

### 唯一没有 QuickJS 对应物的部分

**降级本身**（JS 语义 → wasm 指令）是自己的活。QuickJS 编译到「有 C 运行时兜底的自家
字节码」——它每条 `add` 背后站着一个 C 函数；本产品编译到「除非自己发射、否则什么都
没有的 wasm」——那个函数也得编成 wasm 一起发射。这是两边工作量差异的真正来源，也是
下方成本表的由来。

### 源码获取

挖设计需要一份**只读**的 QuickJS 源码副本。规矩：放在仓外的参考位置，**不进本仓树、
不 vendor、不进构建图**，并按 PRD 14 记版本与来源。获取动作需政委单独确认（上游
tinyvm 研究期的纪律是「只吃设计，不搬代码、不 clone」，本 PRD 放宽为「可读、仍不搬」，
但取源这一步不默认执行）。

## JS 覆盖面：是排期，不是能力天花板

**撤销**：本文件 rev2 写过「完整 JS 是永久非目标」，并把它列为"被锁死的结论"。
那是错的，作废。政委 2026-08-24 质疑得对。

错在两处：

1. **搬错了论据。** 上游 `crates/tinyvm/research-qjs-wasm.md` 的「不现实」结论回答的是
   *另一个问题*——「tinyvm 要不要外挂一个 JS 引擎」。本产品问的是「我们自己写不写一个
   JS→wasm 编译器」。两个问题不同，结论不可搬运。
2. **「无运行时」是稻草人。** 「没有一个公开项目是把源码 AOT 成*无运行时*的 `.wasm`」
   这句本身为真，但没人需要无运行时的 wasm。把对象模型、GC、字符串、正则用 Rust 写好
   **一起编进那份 wasm**，产物仍是一份普通 `.wasm`，过同一道装载门。这正是公开项目
   **Porffor**（AOT JS→wasm，自带运行时，目标完整 ES）在做的事——它是这条路的存在性
   证明：缺的是工作量，不是可能性。

### 曾以为有一条原理排除，查证后**没有**

rev3 写过「`eval` / `new Function` 被 tinyvm 原理排除，因为核禁运行期热加载代码」。
**该判断错误，已撤销**（政委 2026-08-24 质疑「tinyvm 是我们自己的产品，设计不好就去
改造」，据此去读了源码——结论是不用改）。

错在把两件事混为一谈：

| | tinyvm 立场 | 该不该改 |
|---|---|---|
| 运行期生成**机器码**（JIT） | 禁 | **不改。** 这是 tinyvm 存在的全部理由（iOS 不许 JIT），是产品定义不是设计缺陷 |
| 运行期编译并装载一份**新 wasm 模块** | **已支持** | 不用改 |

`eval` 需要的是后者，不是前者。tinyvm 已有**跨实例函数链接**，且带 WABT 差分测试
（`crates/tinyvm/tests/wabt_imported_functions_oracle.rs`）：
`provider.exported_function_handle(name)` 取出导出句柄，
`consumer.bind_function_import(module, field, &f)` 绑进另一实例的 import，带精确类型
校验与 store 持有的函数引用。

所以 `eval` 的实现路径**今天就通**，不改 tinyvm 一行：

1. guest 调宿主门 `eval(src_ptr, src_len)`；
2. 宿主取出源码，跑本 crate 的 Rust 编译器 → 一份新 `.wasm`；
3. 宿主在**同一个 `Store`** 内实例化该模块；
4. 跨实例把新模块的导出绑回调用方。

唯一有额外难度的是**直接 `eval` 带词法作用域**（`eval("x+1")` 中 `x` 是调用方局部
变量）：被编译片段须看得见调用方的帧。解法与 M5 闭包装箱同源——被捕获变量提到堆上，
`eval` 复用同一机制。因此它也是**排期**，不是排除。

### 结论：本产品在 JS 覆盖面上没有原理性排除

全部是工作量与排期。任何一条「做不到」的说法都必须先给出读过代码的依据，不得凭印象
断言上限。

### 改造 tinyvm：已授权

政委 2026-08-24：「tinyvm 是我们自己的产品，如果它设计得不好那你就开始去改造它吧」。

**本 PRD 据此授权：当 `agenterm-qjswasm` 撞到 tinyvm 层的真实缺口时，去 tinyvm 仓改，
不绕。** 规矩三条：

1. **在 tinyvm 仓改**，带该仓自己的 PRD 条目与测试，遵它的「测试优先」纪律。改完在
   本 crate 抬 `rev` 钉子。依赖方向仍单向 agenterm → tinyvm。
2. **不许绕。** 上游缺一个能力，正路是去上游补，不是在本 crate 里长一份私有实现，
   也不是把需求扭曲成「现有 API 勉强能拼出来」的形状。
3. **唯一不动的是「不生成机器码」。** 那不是设计缺陷，是 tinyvm 存在的理由（iOS 不许
   JIT）。要动这一条就不是改 tinyvm，是换一个核，须政委单独下单。

改造前先读代码确认缺口真实存在——本 PRD 已有四次「凭印象断言上限、读码后发现是错的」
记录（见上文各撤销条），把读码当作提改造的前置条件。

### 断言纪律（因四次返工而立）

**任何「做不到 / 不支持 / 被限制」的说法，必须先给出读过代码的依据。** 不得凭印象、
不得搬运回答别的问题的结论、不得把「我没查」表述成「它不行」。撤销记录一律留在文档
里，不静默改写——下一个读者需要知道哪些判断被推翻过、为什么。

### 口径

对外一律说**当前支持到哪、下一步长什么**，不说「永远做不到」，也不说「支持 JavaScript」
——两种都不诚实。子集按**真实脚本需求**往上长（第一个锚点是 `fleet.js` 等价物，见
归档门），不按「离完整 JS 还差多少」排期。

## 机制

```text
.qjs 源码
   │  ① 词法 / 语法 / AST      （纯 Rust，规范为准）
   │  ② 降级到 wasm 指令       （含必要的 guest 侧运行时支持）
   ▼
标准 .wasm 字节
   │  ③ tinyvm 装载期校验 + Limits
   ▼
tinyvm 解释执行（无 JIT）
```

`.wasm` 输入跳过 ①②，从 ③ 进。两种输入在核这一层**完全同待遇**——这是「同一个引擎跑
两种东西」的确切含义，不是两条并行管线。

### 成本曲线在哪里（必须诚实）

①（前端）是常规编译器工作，成本可预期。真正的成本在 **② 的运行时支持**：

| 语言能力 | guest 侧需要什么 | 量级 |
|----------|------------------|------|
| binary64 算术 | 类型分派的运算符运行时（`__add` 等） | **已有** |
| 字符串 | 线性内存里的串表示 + 分配器 | **已有**（字面量池 + bump；三个 ECMA-262 转换未实现，撞上即 trap） |
| 数组 / 对象 | 堆布局 + 属性查找 | 大 |
| 闭包 | 环境捕获 + 间接调用表 | 大 |
| 异常（`try/catch`） | 展开策略（wasm 无异常提案时要自己编码） | 中 |
| `JSON` | 用 `.qjs` 自举，或编译器内建 | 中 |
| GC | 引用计数或标记清扫，随对象一起来 | 大 |

每加一层能力，产出的 `.wasm` 就多带一段运行时。**这段运行时也是 wasm，也吃 `Limits`，
也算进产物体积。** 排期时按这张表估，不按语法特性个数估。

## 归档门：`agenterm-qjs` 什么时候能下线

现状实测（2026-08-24）：

- `scripts/qjs/` 下**只有** `lib/fleet.js`（209 行 host binding 库），**没有任何实际
  任务脚本**——本仓 `plan/design-sql-execution-target.md` 已记录同一事实。
- `script-qjs` 是 `optional`、`default` 关。
- 生产路径真正调 `agenterm_qjs::` 的只有两处：`src/script_engine.rs` 的
  `QjsEngineBackend`、`src/bin/agenterm.rs` 的 `qjs` 子命令。其余是兄弟 crate 的
  **文档注释**在引用它的形状，不是代码依赖。

所以归档不是「砸掉在用的引擎」，是「换掉一个几乎无人使用的外链依赖」。

**归档门（可证伪，三条全绿才动手）：**

1. `agenterm-qjswasm` 能编译并跑通 `fleet.js` 的等价物——即支持对象字面量、函数表达
   式、字符串、`JSON.parse` / `JSON.stringify`、`try/catch`、带参宿主调用。
2. 上面两处生产调用点已迁到 qjswasm，且行为等价有测试锁住。
3. `qjs` CLI 子命令在新引擎上有对应面，或明确声明哪些不再提供、为什么。

**门 3 的措辞已改（2026-08-25）。** 原文点名 `check` / `pack` / `qualify` /
`check-many` 四个动词。实测 CLI 有**十三个**（`crates/agenterm-qjs/src/cli.rs`），
只答那四个等于对一个没人问的问题关门。逐动词判决见下。

### 门 3：逐动词判决（2026-08-25，全部跑过，不是读出来的）

判据表在 [`plan/design-qjs-archive-gate.md`](../plan/design-qjs-archive-gate.md)，
每格都有可复现的命令。三类判决：**必须提供**（新引擎上要有等价动词）、
**形状必然不同**（能力在，但产物或收据不同，须写清差异）、**可以不提供**（附理由）。

| # | 动词 | 判决 | 一句话 |
|---|------|------|--------|
| 1 | `check` | 必须提供 | backend 已实现（`src/script_engine.rs`），只差 CLI 壳 |
| 2 | `check-many` | 必须提供 | 共享 driver，约 60 行适配器，`kind` = `agenterm-qjswasm-check-manifest` |
| 3 | `pack build` | 形状必然不同 | 产物是一份自足 `.wasm`，不是 `.qjsc` + 源码目录 |
| 4 | `pack load` | 形状必然不同 | 前置已补，见下 |
| 5 | `pack build` 模块模式（`pack_module`） | 可以不提供 | 它绕的是 rquickjs 的约束，那约束在这里不存在；零生产调用者 |
| 6 | `qualify` | 形状必然不同 | 收据是超集：多 `steps` / `peak_call_depth`，qjs 造不出来 |
| 7 | `corpus-scan` | 必须提供 | 约 20 行，与 #2 同一套脚手架 |
| 8 | `eval` | 必须提供 | — |
| 9 | `run -- <args>` | 可以不提供（今天） | 门只有四件，没有 `args_len` / `arg`（`host.rs` 的 `SIGNATURES`） |
| 10 | `hash` | 形状必然不同 | 应当是产物哈希而不是源码哈希，见下 |
| 11 | `run-smoke` | 跟随 #4 | — |
| 12 | `task` | 必须提供 | stub 即可；退出码与文案被测试锁住 |
| 13 | `version` | 必须提供 | — |

**一处前置条件，已解除。** 一份 `.wasm` 文件不记得自己是从 `.qjs` 编来的：
`Convention` 是装载时记下的，`Guest::Wasm(&compile_qjs(src))` 会把 JsV1 丢掉，
返回值形状与直接跑源码不一致（实测 `[I32(1), I64(4631107791820423168)]` vs
`[Js(Number(42.0))]`）。所以**任何"编译落盘再加载"的动词在此之前都建在沙上**。
`Guest::CompiledQjs(&[u8])` 已于 2026-08-25 落地，验收测试是门自己点名的那条：
`tests/qjs_guest.rs::a_compiled_artifact_reloaded_gives_the_same_value_as_its_source`。
同一个变体另有一条独立理由要它（接缝的五条拒绝分支此前不可达），两条互不相干的路指向
同一个形状。

**附带记录一条假话，与 wasmcore README 那两句同等对待。**
`crates/agenterm-qjs/src/pack.rs` 的模块文档说 `bytecode_hash` 是
「a genuine reproducibility fingerprint」。**不是。** 同一份源码编到两个不同的
`--dir`，`source_hash` 相同而 `bytecode_hash` 不同（`1ab1e0b1…` / `1ab1e0b1…` 对
`bd9a9694…` / `7074b217…`），因为编译标签用的是绝对输出路径，`Module::write` 把它嵌进
了产物——`xxd pack.qjsc` 能直接看见。`compile.rs` 的三条单元测试全把 label 钉成
`"a.js"`，所以它们永远抓不到这条。**门绿前不改 `agenterm-qjs` 的代码**（纪律如此），
但这句话记在这里，免得下一个人照它判断；也说明门 3 的 `pack` 不是"移植"，移过去等于
移一份已经坏掉的契约。对照：`agenterm-lua` 的 `compile_lua(source)` 没有 label 参数，
干净。

### 第一条门的实测缺口清单（2026-08-24，rev `df8decd`）

把 `scripts/qjs/lib/fleet.js` 的每一种构造拿去**真编一次**，得到下表。这不是读源码估的，
是 209 行里逐条构造喂给编译器的结果。**没有归档任何东西**——这张表是路线图的下一份输入。

**已经能编（`fleet.js` 用到、今天就过）：**

| 构造 | 出处 | 证据 |
|------|------|------|
| 字符串字面量与拼接 | 全文的 `"tabs.list"` 等 operation id | `return "tab" + "s.list";` → `"tabs.list"` |
| `const` 声明、真作用域 | `const fleet = {}` 的声明部分 | 已测 |
| 带参函数声明 + `return` | `function call(opId, params) {…}` 的外壳 | `function call(a,b){return a;}` → 可调 |
| `===` / `!==` 与 `undefined` | `params === undefined` | `p === undefined` → `Bool` |
| 直接调用一个已知函数名 | 各 wrapper 里的 `call(...)` | 已测（含递归、互递归） |
| 嵌套函数读**脚本级**绑定 | wrapper 引用顶层的 `call` | 已测（`function g(){ return f()+1; }`） |

**还编不了（按在 `fleet.js` 里出现的重要性排序）：**

| 缺口 | `fleet.js` 里的形态 | 现在的诊断 | 属哪一期 |
|------|--------------------|-----------|----------|
| **对象字面量** | `const fleet = {};`、`{ tab_id: tabId, note: note }` | "does not support object literals yet" | M4 |
| **属性访问 / 属性赋值** | `fleet.tabs.list = …`、`__host.fleet_call`、`JSON.parse` | "does not support property access yet" | M4 |
| **把函数当值用** | `fleet.tabs.list = function () {…}`——右边是函数值 | "does not support using a function as a value yet" | M4/M5（需要函数值 + 间接调用表） |
| **条件表达式 `?:`** | `params === undefined ? "{}" : params` | "does not support conditional expressions yet" | 排期，纯前端 + 已有控制流，成本最低的一条 |
| **`try` / `catch`** | `call()` 里包住 `JSON.parse` | "does not support the `try` keyword yet" | M5 |
| **`JSON.parse` / `JSON.stringify`** | 每个带参 wrapper | 先撞属性访问 | M5（也可用 `.qjs` 自举） |
| **带参宿主调用** | `__host.fleet_call(opId, params)` | 自由名字先被拒 | 见下 |

**第七条要单独说，因为它不是「语法还没长到」：** `.qjs` 今天**根本够不着
`agenterm.*` 门**。编译器默认下自由名字一律拒（"this engine has no global bindings
yet"）；上游有一个 `Names::HostImport` 模式，但它发射的是模块名 `"js"`、按 JS 值传参的
导入，与本仓门的 `"agenterm"` + i32 两趟拷贝 ABI 不是同一扇门。所以第一条归档门里的
「带参宿主调用」需要的不只是编译器长一层语法，还需要**决定 `.qjs` 侧怎么落到这四个
import 上**——那是本仓的写刀，不是上游的。

**一句话结论**：`fleet.js` 等价物的距离 = 堆对象（对象字面量 + 属性）+ 函数值 +
`try/catch` + JSON + 一条 `.qjs` 到 `agenterm.*` 门的路。`?:` 是这堆里唯一可以立刻摘的
低垂果实。第一条门离绿还远，另两条门未动。

### 门 1 锚定的那个文件本身是破的（2026-08-25 实测）

门 1 说「跑通 `fleet.js` 的等价物」。查这个锚点的时候顺带把 `src/operations.rs` 声明的
面与两份 binding 库真的对了一遍——**link `OPERATION_CATALOG`，不扫文本**——发现的东西
改变了这条门的含义：**把 `fleet.js` 一比一搬过来，会把它的 bug 一起搬过来。**

先更正一个数：派单书与早前文档写的「46 个 `OperationSpec`」是旧数。实测
`OPERATION_CATALOG.len()` = **77**（44 条长写 + 33 条由 `nullary_ui_action()` 常量
构造器造，`src/operations.rs`），其中 **76 条** `script_surface` 以 `fleet.` 开头。

两条真实分歧：

1. **覆盖率 38%。** 76 条 `fleet.*` 里只有 **29** 条有 binding，**47 条两份 binding
   里都没有**——而且缺的是**同一批 47 条**。两份文件是互相抄出来的，不是从同一份源
   生成的。
2. **29 条里有 9 条（31%）发出的 params 宿主会拒。** 这是把
   `scripts/lua/lib/fleet.lua` 放进 `agenterm_lua::LuaEngine` 里、用一个会记录的
   `__host.fleet_call` **真跑出来**的载荷，不是正则匹配出来的：

   | surface | binding 发出 | spec 声明 |
   |---------|-------------|-----------|
   | `tabs.set-note` | `{"note":…,"tab_id":…}` | `tab`（必需，`stable_tab_id`），没有 `tab_id` |
   | `ui.tab.select` | `{"id":…}` | `tab` |
   | `ui.input.wheel` | `{"delta":…}` | `x` / `y` / `delta_y`，三个都必需 |
   | `terminal.paste` | `{"text":…}` | **NO_PARAMETERS** |
   | `ui.composer.send` | `{"text":…}` | 只有 `tab` |
   | `ui.hello` / `ui.deltas` / `events.read` | `{}` | 各有必需参数 |
   | `events.wait` | 只有 `timeout_ms` | 还要 `epoch` / `after` / `kind` |

   `validate_fleet_parameters`（`src/client/mod.rs`）拒未知键、也拒缺必需键，所以这九个
   binding 函数**今天不可能成功**。诚实标注：「所以宿主回
   `broker_invalid_arguments`」是读派发路径读出来的，不是端到端跑出来的——那需要一台活
   服务端，结算实验写在
   [`plan/design-fleet-catalog-binding.md`](../plan/design-fleet-catalog-binding.md)。

**本轮不修，理由写下来。** 47 条缺失的 binding 是**做功能**不是修 bug；那 9 条里有几条
（`terminal.paste` 收 `text` 而 spec 是无参、`events.wait` 少三个必需参数）不是打错字，
是要改脚本作者看得见的参数表，属产品决定。而且 `scripts/qjs/lib/fleet.js` 属于一个
**正在等归档**的引擎，现在改它要么白改，要么给门再添一条要交代的事实。
今天的状态被 `tests/fleet_catalog_conformance.rs` 用带注释的允许清单钉死——**新的漂移
会红**——所以拖着不修不会烂掉。**这是一张独立的单，不是本轮的收尾。**

**它对门 1 的含义**：门 1 的验收不能是「`fleet.js` 逐字编得过」，得是
「`fleet.*` 面的等价物在 qjswasm 上跑得通，且它发的 params 过
`validate_fleet_parameters`」。照抄一份编得过的破 binding 不算绿。

在三条全绿之前，`agenterm-qjs` **原样保留、不动、不腐化**；`.js` / `.mjs` 继续路由到
它。归档动作本身另行派单。

## 与其他执行面的关系

> **撤销（2026-08-25，政委定）。** 本节 rev1–rev4 写的是「本 crate 不替换任何一面，
> 也不改 `.wasm` 默认路由」，理由是能力集不同（wasmcore 给完整 POSIX，qjswasm 只给
> `agenterm.*` 门）。**该立场作废。** 政委 2026-08-25：「agenterm-qjs 和
> agenterm-wasmcore 要安排归档，原因是 agenterm-qjswasm 就是用来替代它们的」。
> 那条能力集差异**依然是真的**——它现在是**迁移工作量**，不再是拒绝替换的理由。

| 面 | crate | 引擎 | 去向 |
|----|-------|------|------|
| `.qjs` / `.wasm` | `agenterm-qjswasm` + `tinyvm-qjs` | tinyvm（**无 JIT**，自研纯 Rust 编译器） | **唯一长期主线** |
| `.js` / `.mjs` | `agenterm-qjs` | rquickjs → QuickJS C | **归档**，门见下 |
| `.wasm`（现默认路由） | `agenterm-wasmcore` | wasmtime + WASI p1（**JIT**） | **归档**，门见下 |
| `.sql` | `agenterm-sql` | — | **待观察**，地位未定；已 optional + default 关，维持不编进主程序 |

这一条同时结清了 [roadmap](PRD_02_18_roadmap.md) 末尾那个待派单任务「wasm/qjs 引擎重构为
依赖 tinyvm」：**两半都由本 crate 承接**，不再是未指派。

### 归档 `agenterm-wasmcore` 的门

`agenterm-qjs` 的门在下一节（锚在 `fleet.js`）。wasmcore 的门不同，因为它的用户是
**wasm 客人而不是脚本**。三条门与它们**今天的状态**：

| # | 门 | 状态（2026-08-25） |
|---|----|------|
| 1 | 能力差异有诚实清单，每条注明要补 / 有意不补 | **可判绿**——清单在下，交付物是 [`plan/design-wasmcore-archive-gate.md`](../plan/design-wasmcore-archive-gate.md) |
| 2 | `.wasm` 默认路由切到 qjswasm，拒绝形状有测试锁住 | **不能绿**，还差两件，见下 |
| 3 | 现状实测（零 `.wasm` 语料、optional + default 关） | **已复核**，数字与原文一致，但生产调用点是四处不是一处 |

#### 门 1：能力差异（实测，不是读规范）

**门自己的前提被证伪了。** 本节 rev4 写「wasmcore 提供完整 WASI p1」。
准确的说法是：**import 面完整，授出的能力几乎为零**。
`p1::add_to_linker_sync` 注册全部 46 个 witx 函数，所以每个 WASI import 都能绑；
但 `WasiCtx` 是 `WasiCtxBuilder::new().stdout(pipe).inherit_stderr().build_p1()`
——没有 `.args()`、没有 `.envs()`、没有 `.preopened_dir()`、没有 `.inherit_stdin()`。

拿一份真 `wasm32-wasip1` 探针客人跑 `WasmCoreHost::run_module` 实测：

```text
args_sizes_get errno=0 argc=0     | environ_sizes_get errno=0 count=0
clock_time_get(realtime/monotonic) errno=0，真值 | random_get errno=0，真熵
fd_fdstat_get(0/1/2) errno=0      | fd_fdstat_get(3..5) errno=8  ← 零 preopen
path_open(fd3) errno=8            | std::fs 读/写/列目录 → NotFound
fd_read(stdin) errno=0 n=0（立刻 EOF）
thread::spawn errno=58 | sock_shutdown errno=57 | proc_raise errno=58
```

所以「完整 POSIX vs 四件门」这个差距**不存在**。十六条逐条判决，**三条要补、
十三条有意不补**；而十三条里有**五条根本不是差异**——两边实测一样（stdin 都是立刻
EOF、文件系统都是全 BADF、argv/env 都是空、socket/信号/线程两边都 NOTSUP、
stdout 等价且 qjswasm 的超限形状更好：截断加标志 vs 整次丢弃）。

**真正只有 wasmcore 有的能力只剩两条，且两条都是要主动放弃的**：直通宿主 stderr
（与「坏槽只能弄死自己」冲突）、16 MiB 原生栈上的深递归（那正是要消灭的形状）。

三条要补，没有一条是「把 WASI 搬进门」：

| # | 要补什么 | 状态 | 说明 |
|---|---------|------|------|
| 3.5 | 时钟与熵 | **未下单**，不挡门 | 真实脚本会用；补成 `agenterm.*` 里的具名 import。**前置是先设计确定性开关**——它们会是本引擎唯一的不确定性来源，会破坏 `steps` / `peak_*` 的可重放性。今天可用一次 `fleet_call` 绕过，所以不是门的阻塞项 |
| 3.7a | `_start` 入口约定 | **未做，挡门 2** | `wasm32-wasip1` 客人导出 `_start`；qjswasm 产品路径固定调 `"main"`（`src/script_engine.rs`） |
| 3.7b | 装载期检查导入可绑定 | **已补 2026-08-25** | 见下 |

**3.7b 是这一轮抓到并修掉的真缺陷。** 一个 WASI 客人在 qjswasm 上曾经是：
`validate_wasm` 返回 `Ok(())`、`spawn` 成功、第一次调用才死在
`Trap("call to unbound imported function")`——**check 放行了 execute 跑不了的东西**，
违反本 PRD 自己的「装载期拒绝 vs 执行期 trap 要能分辨」，而且那条 trap 一个 import 名
都不带（tinyvm 是 `no_std`，文案是静态前缀），所以「运行期看出是哪个导入」这条路本来
就走不通。现在 `agenterm.*` 之外的任何 import 在装载期直接拒、点名，
`validate_wasm` 与 `spawn` 给同一个答案。分类落 `Door` 而不是 `Load`：模块本身是合法
wasm，缺的是**门**。锁在
`crates/agenterm-qjswasm/tests/host_door.rs::check_and_execute_agree_that_an_unbindable_import_is_refused_at_load`。
代价说明白：这**反转**了 qjswasm 侧一条既有决定（「别的模块名的 import 不关门的事」），
理由是那种 import 谁也绑不上，放它装载只是把答案推迟到一条不点名的 trap 上。门的另一半
宽容没动——四件门函数客人仍可只导入一部分或一个都不导入。

#### 门 2：还差什么，写成可判

三条判据，缺一不可，每条都能由一条会跑的命令回答：

1. 一个真 `wasm32-wasip1` 客人（导出 `_start`，只用 stdio/时钟/熵）在 qjswasm 上
   `check` 与 `execute` 都成功且输出与 wasmcore 一致——**或者**它在装载期被点名拒绝，
   且拒绝理由写进迁移说明。二选一，**不允许「运行期 trap」这第三种**。挡在 3.7a 上。
2. 一份同客人的 wasmcore-vs-qjswasm 计时。本仓今天**没有任何**这样的数，切路由之前
   「慢了多少」无人能答。
3. 既有六参数 `fleet_call` 客人在 qjswasm 上是**装载期签名拒绝**这一点有测试锁住，
   并附一份迁移到两趟拷贝 ABI 的等价 guest。

第 3 条同时是对门原文的措辞修正：原文写「既有 guest 的**行为变化**有测试锁住」，
字面上做不到——既有 wasmcore guest 的 `fleet_call` 是六参数，在 qjswasm 上是装载期
签名拒绝，那是**不加载**，不是行为变化。

#### 门 3：现状复核

`scripts/` 下零个 `.wasm` 语料、`script-wasmcore` optional + default 关：都属实。
一处要更正：**生产调用点是四处，不是一处**——`src/script_engine.rs` 的
`WasmcoreEngineBackend`、`src/script_worker.rs` 的 `execute_inner` 派发、
`src/script_backend.rs` 的环境变量与 `.wasm` 路由、`src/client/mod.rs` 的路径特判，
外加 `tests/wasmcore_framed_worker.rs` 这份产品级黑盒测试。只改 `script_engine.rs`
一处会把后三处留在原地。

> **附带修正（已完成 2026-08-25）**：`crates/agenterm-wasmcore/README.md` 曾写它
> 「not a member of the root workspace」「not wired into any product path」，**两条都是
> 假的**——根 `Cargo.toml` 的 `members` 列了它，它自己的 `Cargo.toml` 根本没有
> `[workspace]` 表；它也是 `script_engine.rs` 的一个 backend。同章另有两句假话一并
> 更正（「自带空 `[workspace]` 表」的前提、「把 wasmtime 挡在根 `Cargo.lock` 外面」
> ——根 `Cargo.lock` 里有 `wasmtime 47.0.3`）。活下来的一条被收窄保留：AOT 那对
> （`precompile_module` / `run_precompiled_module`）与 `run_module_from_bytes` 确实
> 在 `src/` 与 `tests/` 里零引用。
>
> 另记一条归档时要说实话的事实：该 crate 在**当前开发机**（macOS aarch64）上是
> **22/23 绿**，失败的那条是 `aot_cwasm_bytes_literally_embed_the_host_target_triple`
> ——它硬断言 `ARCH == "x86_64" && OS == "windows"`，从写下来就只在那台 Windows 机上
> 成立。归档说明里不要写「归档时全绿」。

## 隔离与预算

一份 `.wasm`（无论手写还是 `.qjs` 编出来的）= 一个槽 = 一份预算。槽只经宿主门看世界，
槽间互不见，一个坏槽只能弄死自己。

| 预算 | 归属 | 触发后 |
|------|------|--------|
| `max_steps`（每次顶层调用） | tinyvm `Limits` | 该次调用 trap；槽仍可回收；宿主活着 |
| `max_memory_pages` | tinyvm `Limits` | 装载期拒绝或 `memory.grow` 失败 |
| `max_table_elems` | tinyvm `Limits` | 装载期拒绝 |
| `max_call_depth` | tinyvm `Limits` | trap，不吃原生栈 |
| `max_activation_slots` | tinyvm `Limits` | trap |
| `max_stdout_bytes` | 宿主侧 | 截断并在结果上标记，不静默丢弃 |
| `max_bridge_result_bytes` | 宿主侧 | `Err`，不截断——截断会让 guest 读到半个 JSON |
| `max_result_string_bytes` | 宿主侧 | `Err`，不截断——理由同上（2026-08-25 补） |

`max_result_string_bytes` 是接缝把 `.qjs` 返回的字符串拷进宿主 `String` 的上限。
补它的理由是它原本**没有上限**：那块内存由宿主分配、由客人定大小，上面两个盖子都不管。
默认预算下唯一的天花板是偶然的（拼接是 O(n) 步，`max_steps` 先耗尽），一旦客人能便宜地
造出大字符串、或谁调高 `max_steps`，真实上限就变成 `max_memory_pages × 64 KiB`，
每次调用一份，持久槽上反复。检查顺序也是分类：**先越界、后盖子**——声明长度装不进客人
自己的内存是坏客人（`Door`），说成预算等于让人去调一个调了也没用的数。

**`max_memory_pages` 有一条运行期缺口，是已知的、上游的、今天补不了的。** 装载期超页是
`Load("memory page limit")`（有测试）；但运行期 `memory.grow` 被拒之后，上游
`tinyvm-qjs` 的 `__alloc` 把它降成一条裸 `unreachable`，到宿主这里与任何别的
`unreachable` 无法区分，所以报的是 `Trap` 而不是 `Budget("max_memory_pages")`
——调用者分不清「该调高预算」和「这脚本坏了」。
`src/slot.rs::ceiling_name` 的 `MemoryPages` 分支因此在运行期是死的。
**不在本仓补，也不用启发式猜**（靠「内存正好顶到上限」去猜会把真坏的脚本误判成预算
问题，正是 `classify` 的存取器写法要防的那种静默错分类）。按本 PRD「改造 tinyvm：已授权」
去上游补：分配失败必须可分辨——带 `WasmCeiling::MemoryPages` 的独立 fault，或走一扇门
报告。复现留在
`crates/agenterm-qjswasm/tests/seam_attack.rs::finding_4_running_out_of_pages_is_not_reported_as_a_budget`
（`#[ignore]`，`cargo test -p agenterm-qjswasm --test seam_attack -- --ignored` 可跑）。

失败必须**类型化**：编译期拒绝（不在子集内）、装载期拒绝、执行期 trap、预算耗尽、
门参数越界——五类要能分辨。**2026-08-25 增两类**，因为原来的五类把「调用者说错话」
算在了客人头上：`NoSuchExport`（这个槽没有那个导出，带名字）与 `Signature`
（参数个数或类型不合导出的声明，进客人之前就拒）。编译期拒绝的文案必须说清「这个语法本引擎还不支持」，
而不是含糊的"语法错误"，也不得暗示脚本本身写错了。

每次调用回报确定性执行统计（`steps` / `peak_call_depth` / `peak_activation_slots`），
使「这个脚本贵不贵」可度量。

## 宿主门 ABI（版本化产品契约）

模块名 `"agenterm"`。这是客人能看见的**全部**世界。

```text
print(ptr: i32, len: i32)                                              -> ()
fleet_call(op_ptr: i32, op_len: i32, params_ptr: i32, params_len: i32) -> i32   // status
fleet_result_len()                                                     -> i32
fleet_result(dst_ptr: i32, dst_len: i32)                               -> i32   // 写入字节数，负=目标太小
```

`status`：`0` = Ok · `1` = Err（应用级错误，正常结果，不是崩溃）· `2` = NoBridge。

背后是全仓共用的
`ScriptFleetBridgeFn = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>`。
本 crate 把**这一条既有能力**暴露给 wasm 客人，不发明第二条。`.qjs` 侧的
`fleet.call(op, params)` 最终降到这四个 import 上。

### 为什么是两趟拷贝

tinyvm 的宿主回调签名是 `Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError>`——回调
**持着线性内存的 `&mut`**；而回调进 guest 需要 `Instance::invoke_by_name(&mut self)`。
安全 Rust 里两者不能同时成立，即**宿主回调内部无法重入 guest**。这不是缺陷，是
「无 JIT + 显式调用栈 + 上限在核」的必然结果。

所以 `fleet_call` 只回 status，结果字节暂存在该槽宿主侧的 pending buffer；guest 自己
问长度、自己分配、再让宿主拷进来。代价是每次桥调用多两次跨界；换来零重入、宿主不
要求 guest 导出分配器，且与 tinyvm iOS 桥既有的「stable two-pass copy lengths」同一
手法。pending buffer **每槽一份**，不跨槽。

（`agenterm-wasmcore` 的六参数单次调用 ABI 依赖「宿主回调 guest 的 `wasmcore_alloc`」，
在 tinyvm 上因上述重入约束不可行。两套 ABI 的 `status` 语义保持一致，让 guest 作者只
学一套状态码。）

## Capability tree

Legend: `[x]` 已有可执行证据 · `[~]` 部分 · `[ ]` 规划 · `[–]` 有意排除

**M0（脊柱）+ 上游 M1/M2（语言）已落地并有实测证据，见下节。** 下表每个 `[x]` 都由
本仓 `tests/qjs_guest.rs` 或上游套件**编译并跑过**——不是读源码得出的。上游 rev 从
`f694733` 抬到 `df8decd`（2026-08-24）。

```text
agenterm-qjswasm                                        [~]
│
├── 上游零件（证据在 tinyvm 仓，本仓只消费）              [x]
│   ├── 标准 WASM decode / validate / instantiate        [x]
│   ├── 持久 Instance + 逐调用 fuel                       [x]
│   ├── Limits（steps/pages/table/depth/slots）           [x]
│   ├── 类型化宿主 import + 线性内存访问                   [x]
│   └── 确定性执行统计                                     [x]
│
├── .qjs 编译器（纯 Rust，写刀在上游 tinyvm-qjs）        [~]
│   ├── 前端                                              [~]
│   │   ├── 词法（识别超出子集的词素以便诊断）                [x]
│   │   ├── 表达式文法（优先级爬升 / 结合性）                 [x]
│   │   ├── 自动分号插入（ECMA-262 12.10）                   [x]
│   │   ├── 语句 / 块 / 控制流                              [x]
│   │   ├── 函数声明                                        [x]
│   │   ├── 函数表达式                                      [~] 只有立即调用可用；
│   │   │                                                      赋给绑定后调用 = 拒绝
│   │   └── 诚实的「尚不支持」诊断（指语法，不指用户）        [x]
│   ├── 值表示（V1 双字 tag:i32 + payload:i64）              [x] 由实测实验判定
│   │   ├── 数字 = ECMA-262 binary64（非 i32）              [x] 1/0=Infinity，无回绕
│   │   ├── 字符串 / 布尔 / null / undefined                [x]
│   │   └── 三个 ECMA-262 转换                              [ ] 未实现即 trap，不编造值
│   ├── 降级到 wasm                                        [~]
│   │   ├── 算术（binary64）                                [x]
│   │   ├── 局部变量 / 赋值（let/const/var + TDZ）           [x]
│   │   ├── 控制流（if / while / 三段式 for）                [x]
│   │   ├── 函数调用与返回（含递归、互递归）                  [x] 只有直接调用
│   │   ├── 字符串（字面量池 + 拼接 + 相等 + bump 分配器）     [x]
│   │   ├── 取余 `%` / `typeof`                              [ ] 解析后明确拒绝
│   │   ├── 数组 / 对象（堆布局 + 属性查找）                  [ ]
│   │   ├── 闭包（环境捕获 + 间接调用表）                     [ ] 捕获外层局部 = 拒绝；
│   │   │                                                      读脚本级绑定 = 可以
│   │   ├── try/catch（自编码展开）                          [ ]
│   │   ├── JSON                                           [ ]
│   │   └── GC                                             [ ] 现为 bump + 整体丢弃
│   ├── `.qjs` 调 agenterm.* 门                              [ ] 自由名字编译期即拒；
│   │                                                          门只有手写 .wasm 够得着
│   ├── 原型链 / getter / Proxy / 正则 / 标准库               [ ] 排期，非天花板
│   ├── eval / new Function（宿主重编 + 跨实例链接）           [ ] 排期，核已支持
│   └── 引擎插件逃生口（qjs.wasm）                           [–] 纪律排除 C 库
│
├── 槽与宿主门                                            [x]
│   ├── spawn / call / run_once / kill                     [x]
│   ├── SlotId 绑定到发它的 Engine（跨 Engine = NoSuchSlot） [x] 2026-08-25
│   ├── 调用前先核导出的声明签名                             [x] 2026-08-25
│   │   （缺导出 / 参数个数 / 参数类型 / 结果类型装不下）
│   ├── Guest::CompiledQjs（产物自报约定）                   [x] 2026-08-25
│   ├── agenterm.* 之外的 import 装载期即拒并点名             [x] 2026-08-25
│   ├── 持久 Instance，逐调用新鲜 fuel                       [x]
│   ├── trap 不回收槽（明确承诺，非意外）                     [x]
│   ├── 预算耗尽自成一类（非 Trap）                           [x]
│   ├── 槽间隔离（内存、trap、预算、bridge）                 [x]
│   ├── agenterm.print（有界捕获 + 截断可见）                 [x]
│   ├── agenterm.fleet_call（status 0/1/2）                 [x]
│   ├── 两趟取回（fleet_result_len / fleet_result）           [x]
│   ├── 越界指针 trap 该槽，不读宿主内存                      [x]
│   ├── 门声明装载期校验（错名/错签名 → Door）                [x]
│   ├── 缺席 import 不阻止装载                                [x]
│   ├── 两套调用约定同存（wasm 数值 / V1 pair），装载时定死    [x]
│   ├── JS 值投影成宿主数据（字符串在槽死前读出）              [x]
│   └── 约定不匹配 → UnsupportedValue，不按位重解释            [x]
│
├── 接线                                                  [x]
│   ├── ScriptBackend::Qjswasm + from_entry_path(.qjs)      [x]
│   ├── QjswasmEngineBackend : ScriptEngineBackend          [x]
│   ├── check 与 execute 走同一个编译入口                     [x]
│   ├── `.qjs` completion value → ScriptInvocationResult     [x]
│   ├── feature script-qjswasm，default 关                   [x]
│   └── 接管 .wasm 默认路由                                  [–]
│
├── 归档 agenterm-qjs                                     [~]
│   ├── fleet.js 等价物跑通                                 [ ] 缺口清单见上
│   ├── 两处生产调用点迁移 + 行为等价测试                     [ ]
│   └── CLI 面对应或明确声明缺口                              [~] 十三动词判决已交付；
│       └── Guest::CompiledQjs（pack 类动词的前置）           [x] 2026-08-25
│
└── 归档 agenterm-wasmcore                                [~]
    ├── 能力差异诚实清单（3 要补 / 13 有意不补）              [x] 2026-08-25
    │   ├── 3.7b 装载期导入可绑定检查                        [x] 2026-08-25
    │   ├── 3.7a `_start` 入口约定                          [ ] 挡门 2
    │   └── 3.5 时钟 / 熵（须先设计确定性开关）               [ ] 不挡门
    ├── .wasm 默认路由切换 + 拒绝形状锁住                     [ ]
    └── 现状实测复核                                        [x] 2026-08-25
```

## 证据门

不接受「读代码看着对」作为交付。

- **编译器每加一层能力，先有会失败的验收测。** 语法特性的证据是「这段 `.qjs` 编译出
  的 `.wasm` 跑出预期结果」，不是「parser 不报错」。
- **拒绝路径要有证据。** 每个尚不支持的语法要有一条测试断言它被拒绝、且诊断文案说清
  是引擎能力边界。这条是防止产品文案漂移的锁。
- **隔离与预算的证据必须对抗性**：死循环、深递归、越界指针、超额 `memory.grow` 各要
  一份真实客人，不能只测 happy path。
- **feature 关时根 crate 仍 build。**
- **归档门三条各自有证据**才动 `agenterm-qjs`。

### M0 实测（2026-08-24，aarch64-apple-darwin，Rust 1.97.0）

```sh
cargo test -p agenterm-qjswasm
```

**71 passed, 0 failed**，分布：

| 目标 | 数 | 覆盖 |
|------|----|------|
| `src/lib.rs` 单元 | 25 | 宿主门内部（23）+ LEB128 最小编码（2） |
| `tests/qjs_m0.rs` | 23 | `.qjs` 编译与执行、拒绝路径与诊断文案 |
| `tests/host_door.rs` | 9 | 门四件的产品面契约 |
| `tests/budget.rs` | 5 | 对抗性预算 |
| `tests/wasm_slot.rs` | 5 | 槽生命周期 |
| `tests/isolation.rs` | 4 | 对抗性隔离 |
| `tests/fixtures.rs` | 0 | 8 份对抗性客人素材（无自有测试） |
| doctests | 0 | — |

### 编译器迁出后的重测（2026-08-24，同机同工具链）

编译器迁往 `tinyvm-qjs` 之后，本 crate **53 passed, 0 failed**。少掉的 20 条不是删的，
是跟着被测代码走了：`tests/qjs_m0.rs` 的 23 条与 LEB128 编码器的 2 条单元测在上游
`crates/tinyvm-qjs/tests/qjs_subset.rs` 与 `src/encode.rs` 里原样跑（上游同批新增 5 条
`tests/compile_options.rs`，锁两种取名模式与诊断的窄化）。

| 目标 | 数 | 变化 |
|------|----|------|
| `src/lib.rs` 单元 | 23 | −2（LEB128 随编码器迁出） |
| `tests/qjs_guest.rs` | 6 | 新写：`.qjs` 端到端过槽、编译失败自成一类、产物过本 crate 装载门、扩展名路由。**替代**迁走的 `qjs_m0.rs`——语言子集归上游测，本 crate 只测自己的接缝 |
| `tests/host_door.rs` | 9 | — |
| `tests/budget.rs` | 6 | +1：活动记录槽耗尽必须报 `Budget`（补上那条会抓住静默漂移的测试） |
| `tests/wasm_slot.rs` | 5 | — |
| `tests/isolation.rs` | 4 | — |

其余门：

```sh
cargo clippy -p agenterm-qjswasm --all-targets -- -D warnings   # clean
cargo fmt -p agenterm-qjswasm --check                            # clean
cargo check -p agenterm --features script-qjswasm --lib          # clean, 18s
cargo check -p agenterm --lib                                    # clean, 15s；qjswasm 未参与编译
cargo check --workspace --all-targets --exclude agenterm-abi     # clean
```

`--exclude agenterm-abi` 是必需的：它有一条**故意**的 `compile_error!`，要求
`--profile abi-release` / `abi-dev`（工作区默认 `panic = "abort"` 会静默产出无
`catch_unwind` 围栏的库）。不是树坏了。

### 抬 rev 到 `df8decd` 后的重测（2026-08-24，同机同工具链）

上游从 `f694733` 抬到 `df8decd`（5 个提交），`.qjs` 从整数表达式长成真正的 M1/M2 子集。
本 crate **58 passed, 0 failed**；根 crate 的 `script_engine` 另加 2 条（feature 开时跑）。

| 目标 | 数 | 变化 |
|------|----|------|
| `src/lib.rs` 单元 | 23 | — |
| `tests/qjs_guest.rs` | 11 | +5：JS 值五种全过脸、字符串在槽死后仍可读、binary64 算术、约定不匹配被拒、本仓能力声明的文档锁 |
| `tests/host_door.rs` | 9 | — |
| `tests/budget.rs` | 6 | — |
| `tests/wasm_slot.rs` | 5 | —（手写 `.wasm` 路径**一行未改**仍全绿，这是「两套约定同存」的证据） |
| `tests/isolation.rs` | 4 | — |
| `src/script_engine.rs` 单元 | 2 | 新写：completion value 到得了调用者；`check` 与 `execute` 对子集口径一致 |

三条被抬掉的 M0 断言，逐条说明是**变强**还是**变弱**：

| 原断言 | 现在 | 强弱 |
|--------|------|------|
| `$0*2+2` 传 `Value::I32(20)` 得 `I32(42)` | 传 `JsValue::Number(20.0)` 得 `Number(42.0)`，另加约定不匹配被拒的测 | **变强**：多锁了「不按位重解释」这条 |
| `let x = 1` 必须被**拒绝** | 已支持，改为断言它跑出正确的值；拒绝测换成 `%` / `typeof` / 捕获外层局部的闭包（三条都是编出来确认的） | 中性：锁的对象从「M0 的极限」换成「今天真实的边界」 |
| `$0/0` 必须 **Trap** | `Infinity`。数字是 ECMA-262 binary64（6.1.6.1），旧断言锁的是 i32 除法的限制 | **变强**（是修正不是放宽）：另加 `0/0=NaN`、`2147483647+1` 不回绕、`-z` 保留零的符号 |

### 抬 rev 期间实测到的两处「源码看着支持、编出来不支持」

按本 PRD 的断言纪律，能力声明一律编译验证，不读源码断言。这一轮抓到两条：

| 声明 | 实测 | 结论 |
|------|------|------|
| 上游 README「函数：声明与表达式，具名或匿名」 | `let g = function (a) {...}; return g(21);` 被拒——"does not support using a function as a value"。`return f;`（f 是函数声明）同样被拒 | 函数表达式**只有立即调用**（IIFE）可用。文档口径已按此收窄 |
| 上游 README「`-0` 与 `0` 不同」 | 整体成立（`-(1-1)`、`1 / -z` 都给 `-Infinity`），但**字面量写法 `-0` 给的是 `+0`**（`1 / -0` = `Infinity`）。一元负号作用在数字字面量上时丢了零的符号 | 上游缺陷，已记录；不在本仓文件域，未改。上游 conformance 套件测了 `-(1-1)` 与 `0 * -1`，没测 `-0` 本身 |

**范围诚实声明**：`.qjs` 是一个真实但很小的子集，能力清单见
`crates/agenterm-qjswasm/README.md`（每条都有测试）；`.wasm` 侧是完整的。
`.qjs` **还够不着 `agenterm.*` 门**——自由名字在编译期就被拒，门今天只有手写 `.wasm`
客人能调。

### 接缝对抗审查后的重测（2026-08-25，同机同工具链）

一轮专门针对接缝（`Value::Js` 那张脸、每槽的 `Convention`、「字符串在槽死前被读成宿主
数据」这条承载性声明）的对抗性攻击，找到八条缺陷。**那条承载性声明扛住了每一次攻击**：
13 个 f64 边界值按位往返（NaN 载荷 `0xfff8deadbeefcafe`、信号 NaN、`-0` 的符号全保住）、
空串、内嵌 NUL、代理对拼出的星平面字符、从**长大过的**线性内存里取出的 1 MiB 字符串
（证明接缝读的是活视图而不是实例化时的快照）、第一次调用的串不被第二次调用和 `kill` 打扰。
没有任何一次 panic。

八条缺陷里**七条已修**（详见 §隔离与预算 与 §Capability tree 的对应条目），一条修不了：

| # | 缺陷 | 处置 |
|---|------|------|
| 1 | 参数**个数**不对 → `Trap("function")`，怪客人 | 修：`Signature`，报两个数 |
| 2 | 参数**类型**不对 → **不报**，还能返回签名禁止的类型 | 修：`Signature`，进客人之前 |
| 3 | 导出不存在 → `Trap`，且不带名字 | 修：`NoSuchExport`，带名字 |
| 4 | 运行期超页 → `Trap("unreachable")` 而非 `Budget` | **修不了，上游**，见 §隔离与预算 |
| 5 | `SlotId` 不绑 Engine：跨 Engine 静默跑错槽、静默杀错槽 | 修：进程级 engine tag |
| 6 | 脸拒收返回值时，客人已打印的输出被丢掉 | 修：改成进客人**之前**拒，没得丢；残留代价写进 `Slot::call` |
| 7 | 接缝拷出的字符串没有任何宿主侧盖子 | 修：`max_result_string_bytes` |
| 8 | 五条恶意指针防线从公开面**不可达**，无对抗覆盖 | 修：`Guest::CompiledQjs` 让它们可达，补五条攻击 |

第 6 条值得单说，因为**没按提出者建议的方式修**。建议是「让输出活过这次拒绝」，
实现上等于把它留到**下一次**调用的 `Outcome` 里——而 `slot.rs` 早就写明那比丢掉更糟
（张冠李戴）。改成查导出的**声明结果类型**，在客人还没跑的时候就拒，于是根本没有输出
被产生。这比原诉求更强：不是「输出活下来」，是「输出压根不用产生」。

第 8 条值得单说，因为它与 `agenterm-qjs` 门 3 的前置**是同一个变体**。接缝审查要它是
为了让 `read_guest_string` 的五条拒绝可测；CLI 判决要它是为了让「编译落盘再加载」不丢
约定。两条互不相干的路指向同一个 `Guest::CompiledQjs`——这是它形状对的强证据。
提出者同时建议在 `spawn` 加签名嗅探（「看着像 V1 就当 V1」），**这条明确不采纳**：
`(i32, i64, …) -> (i32, i64)` 是完全普通的手写 wasm 类型，嗅探就是猜，而
`Convention` 的纪律原文就是「记下来，绝不靠猜签名」。

```sh
cargo test -p agenterm-qjswasm    # 86 passed, 0 failed, 1 ignored
```

| 目标 | 数 | 变化 |
|------|----|------|
| `src/lib.rs` 单元 | 23 | —（其中 `a_foreign_import_is_left_alone_at_install` 改写成 `..._is_refused_at_install`，记下反转理由与实测前后） |
| `tests/qjs_guest.rs` | 12 | +1：产物重载与源码直跑值相等（门 3 第 0 步的验收测） |
| `tests/host_door.rs` | 10 | +1：`check` 与 `execute` 对绑不上的 import 给同一个答案 |
| `tests/budget.rs` | 6 | — |
| `tests/wasm_slot.rs` | 5 | —（两条改写：缺导出从 `Trap` 改成 `NoSuchExport` 并要求带名字） |
| `tests/isolation.rs` | 4 | — |
| `tests/seam_attack.rs` | 26 + 1 ignored | 新文件：13 条「攻不破」的正面锁 + 7 条已修缺陷的回归锁 + 5 条恶意指针攻击 + 1 条上游缺陷的复现（`#[ignore]`） |

`#[ignore]` 只用在第 4 条上，并且**没有被反转成断言错误行为**——那会把一份缺陷报告变成
一把锁住缺陷的锁。

### M0 期间在 tinyvm 上的实测发现（写下来免得再踩）

| 发现 | 依据 |
|------|------|
| `Module::instantiate()` **已经跑过 start 函数** | `wasm.rs` `Instance::new`；再调 `run_start()` 会跑第二遍 |
| `Limits` 随解码后的 module 进入 `Instance`，逐调用自动重置步数 | 所以槽的 `call` 不需要再传预算 |
| `memory.grow` 超 `max_memory_pages` **返回 `-1`，不 trap** | `wasm.rs` ~7580-7613，符合 wasm 规范 |
| 装载期 `initial_pages > max_memory_pages` 是**另一条**拒绝路径 | `wasm.rs` ~4821，属 `Load` 类而非 `Trap` |
| 客人活动记录走 `Vec<DefinedActivation>` 蹦床，**不吃原生栈** | `wasm.rs` ~6458-6730；深度测试因此可以设到默认值的 40 倍 |
| `WasmError` / `Limits` **都不实现 `Debug`** | 核是 `no_std` + fmt-free；下游要手写 `Debug` |
| `Trap("call stack")` 覆盖 ≥3 种不同条件，`Trap("memory size")` ≥2 种 | 24 / 14 处调用点；下游无法区分——**已修**：上游拆成 `activation slot limit` / `operand stack` / `call stack allocation` / `activation slot overflow`，并给出 `WasmError::class()` / `ceiling()`，下游不再匹配文案 |
| `Trap("no exported function named \`")` 文案被截断，尾随一个孤立反引号 | `wasm.rs` 5782 / 8442；fmt-free 核填不进名字——已派单 |
| 抄文案表的代价是**静默**的 | 上游拆分 `"call stack"` 时，本 crate 的分类表不报编译错、测试也全绿，只是分类悄悄错了。存取器不会这样漂——已按此改写 `src/slot.rs::classify` |

## Non-goals until 政委 orders otherwise

- JS 覆盖面**不设人为上限**，按真实脚本需求长。本 PRD 不再声明任何 JS 语义层面的原理
  排除；`eval` 亦为排期项。唯一不变的是执行核**不生成机器码**（tinyvm 产品定义）。
- 不链 QuickJS C 库，不用 `rquickjs`，不引入 C 依赖或构建期 C 工具链。
- 不做 JIT、AOT 到机器码、copy-and-patch，不碰可执行内存。
- 不用 tinyvm `wasi-p1` feature 当插件面。
- 归档 `agenterm-qjs` 与 `agenterm-wasmcore` 各自走完上文的门再动手；门未绿前两者原样保留、不腐化。
- 不做跨槽通信、共享内存、共享 table/global。
- 不 vendor tinyvm 源码（改 tinyvm 走上游仓，见上「改造 tinyvm：已授权」）。
- 不在归档门三条全绿前动 `agenterm-qjs`。
