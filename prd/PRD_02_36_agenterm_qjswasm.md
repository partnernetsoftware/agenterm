# PRD 02.36 — `agenterm-qjswasm`（自研脚本引擎：`.qjs` 编译到 `.wasm`，tinyvm 当核）

Status: authorized, not implemented — 政委 2026-08-24 下单。
本文件是产品真理；执行投影在
[`plan/design-agenterm-qjswasm.md`](../plan/design-agenterm-qjswasm.md)。

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
3. `qjs` CLI 子命令的 `check` / `pack` / `qualify` / `check-many` 在新引擎上有对应面，
   或明确声明哪些不再提供、为什么。

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
**wasm 客人而不是脚本**：

1. **能力差异有诚实清单。** wasmcore 提供完整 WASI p1（`fd_*` / `_start` / `proc_exit`）；
   qjswasm 只提供 `agenterm.*` 四件。逐条列出「wasmcore 能而 qjswasm 不能」的事，
   每条注明是**要补**还是**有意不补**（把 WASI 做成第二扇 OS 面是纪律禁止的，
   所以多半是后者）。
2. **`.wasm` 默认路由切到 qjswasm**，且既有 guest 的行为变化有测试锁住。
3. 现状实测：`scripts/` 下**零个 `.wasm` 语料**，`script-wasmcore` 是 optional + default
   关，生产调用点只有 `src/script_engine.rs` 的 `WasmcoreEngineBackend`。所以这不是
   「砸掉在用的东西」——但**门仍要走完**，因为"没人用"是今天的事实，不是承诺。

> **附带修正**：`crates/agenterm-wasmcore/README.md` 现在写着它「not a member of the
> root workspace」「not wired into any product path」，**两条都已是假的**——它在
> workspace members 里，也是 `script_engine.rs` 的一个 backend。归档前先修这两句，
> 否则下一个人照 README 判断会判错。

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

失败必须**类型化**：编译期拒绝（不在子集内）、装载期拒绝、执行期 trap、预算耗尽、
门参数越界——五类要能分辨。编译期拒绝的文案必须说清「这个语法本引擎还不支持」，
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
└── 归档 agenterm-qjs                                     [ ]
    ├── fleet.js 等价物跑通                                 [ ]
    ├── 两处生产调用点迁移 + 行为等价测试                     [ ]
    └── CLI 面对应或明确声明缺口                              [ ]
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
