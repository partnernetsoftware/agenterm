# PRD 02.36 — `agenterm-qjswasm`（自研脚本引擎：`.qjs` 编译到 `.wasm`，tinyvm 当核）

Status: authorized, not implemented — 政委 2026-08-24 下单。
本文件是产品真理；执行投影在
[`plan/design-agenterm-qjswasm.md`](../plan/design-agenterm-qjswasm.md)。

Owner: 政委定方向；主会话按独占文件域推进。

Upstream: [`partnernetsoftware/tinyvm`](https://github.com/partnernetsoftware/tinyvm)（本地 `../tinyvm`），
PRD 35 记录其迁出。依赖方向 **agenterm → tinyvm**，单向，本 crate 不持有 tinyvm 写刀。

Supersedes: `crates/agenterm-qjs`（rquickjs 外链引擎）——**待归档**，门见下 §归档门。

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
- **编译器写刀在本仓。** tinyvm 只提供 `eval_wasm` + 校验 + `Limits` + 门；语言由
  agenterm 自己长，不受另一个仓的排期约束。上游 `tinyvm-qjs` 是 tinyvm 自己的演示皮，
  与本 crate 各长各的，不共用写刀。
- **测试优先：先验收测再改脸。工人自报不算过。**

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
| 值表示（64 位标记联合 vs 32 位 NaN-boxing 的取舍理由） | M3 统一值表示 | M3 | **高** |
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
| 整数算术 | 无 | 已有（447 行原型） |
| 字符串 | 线性内存里的串表示 + 分配器 | 中 |
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

在三条全绿之前，`agenterm-qjs` **原样保留、不动、不腐化**；`.js` / `.mjs` 继续路由到
它。归档动作本身另行派单。

## 与其他执行面的关系

| 面 | crate | 引擎 | 归属 |
|----|-------|------|------|
| `.qjs` / `.wasm`（本 PRD） | `agenterm-qjswasm` | tinyvm（**无 JIT**，纯 Rust 编译器） | 自研，长期主线 |
| `.js` / `.mjs`（现状） | `agenterm-qjs` | rquickjs → QuickJS C | **待归档**，见上 |
| `.wasm`（现状默认） | `agenterm-wasmcore` | wasmtime + WASI p1（**JIT**） | 不动 |

**本 crate 不改 `.wasm` 的默认路由。** `from_entry_path` 里 `.wasm` 仍先命中
`script-wasmcore`；qjswasm 接管 `.wasm` 需要显式 `AGENTERM_SCRIPT_BACKEND=qjswasm`。
理由是能力集不同：wasmcore 给的是完整 POSIX（`fd_*`、`_start`、`proc_exit`），
qjswasm 只给 `agenterm.*` 门，静默换引擎会让现有 WASI guest 突然找不到 `fd_write`。

> 「用 tinyvm 取代 wasmcore」是 [roadmap](PRD_02_18_roadmap.md) 末条那个**待派单**
> 任务，不是本 PRD 的范围。

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

**M0（脊柱）已落地并有实测证据，见上节。** 编译器仍只到整数表达式；M1–M5 未开工。

```text
agenterm-qjswasm                                        [ ]
│
├── 上游零件（证据在 tinyvm 仓，本仓只消费）              [x]
│   ├── 标准 WASM decode / validate / instantiate        [x]
│   ├── 持久 Instance + 逐调用 fuel                       [x]
│   ├── Limits（steps/pages/table/depth/slots）           [x]
│   ├── 类型化宿主 import + 线性内存访问                   [x]
│   └── 确定性执行统计                                     [x]
│
├── .qjs 编译器（纯 Rust，写刀在本仓）                    [ ]
│   ├── 前端                                              [~]
│   │   ├── 词法（识别超出子集的词素以便诊断）                [x]
│   │   ├── 表达式文法（优先级爬升 / 结合性）                 [x]
│   │   ├── 自动分号插入                                    [ ]
│   │   ├── 语句 / 块 / 控制流                              [ ]
│   │   ├── 函数声明与函数表达式                            [ ]
│   │   └── 诚实的「尚不支持」诊断（指语法，不指用户）        [x]
│   ├── 降级到 wasm                                        [~]
│   │   ├── 整数算术                                       [x] 自研，字节级对齐参考汇编器
│   │   ├── 局部变量 / 赋值                                 [ ]
│   │   ├── 控制流（if / while / for）                      [ ]
│   │   ├── 函数调用与返回                                  [ ]
│   │   ├── 字符串（表示 + 分配器）                          [ ]
│   │   ├── 数组 / 对象（堆布局 + 属性查找）                  [ ]
│   │   ├── 闭包（环境捕获 + 间接调用表）                     [ ]
│   │   ├── try/catch（自编码展开）                          [ ]
│   │   ├── JSON                                           [ ]
│   │   └── GC                                             [ ]
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
│   └── 缺席 import 不阻止装载                                [x]
│
├── 接线                                                  [x]
│   ├── ScriptBackend::Qjswasm + from_entry_path(.qjs)      [x]
│   ├── QjswasmEngineBackend : ScriptEngineBackend          [x]
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

**范围诚实声明**：以上是 M0。`.qjs` 目前只有整数表达式；`.wasm` 侧是完整的。

### M0 期间在 tinyvm 上的实测发现（写下来免得再踩）

| 发现 | 依据 |
|------|------|
| `Module::instantiate()` **已经跑过 start 函数** | `wasm.rs` `Instance::new`；再调 `run_start()` 会跑第二遍 |
| `Limits` 随解码后的 module 进入 `Instance`，逐调用自动重置步数 | 所以槽的 `call` 不需要再传预算 |
| `memory.grow` 超 `max_memory_pages` **返回 `-1`，不 trap** | `wasm.rs` ~7580-7613，符合 wasm 规范 |
| 装载期 `initial_pages > max_memory_pages` 是**另一条**拒绝路径 | `wasm.rs` ~4821，属 `Load` 类而非 `Trap` |
| 客人活动记录走 `Vec<DefinedActivation>` 蹦床，**不吃原生栈** | `wasm.rs` ~6458-6730；深度测试因此可以设到默认值的 40 倍 |
| `WasmError` / `Limits` **都不实现 `Debug`** | 核是 `no_std` + fmt-free；下游要手写 `Debug` |
| `Trap("call stack")` 覆盖 ≥3 种不同条件，`Trap("memory size")` ≥2 种 | 24 / 14 处调用点；下游无法区分——已派单去 tinyvm 仓修 |
| `Trap("no exported function named \`")` 文案被截断，尾随一个孤立反引号 | `wasm.rs` 5782 / 8442；fmt-free 核填不进名字——同上，已派单 |

## Non-goals until 政委 orders otherwise

- JS 覆盖面**不设人为上限**，按真实脚本需求长。本 PRD 不再声明任何 JS 语义层面的原理
  排除；`eval` 亦为排期项。唯一不变的是执行核**不生成机器码**（tinyvm 产品定义）。
- 不链 QuickJS C 库，不用 `rquickjs`，不引入 C 依赖或构建期 C 工具链。
- 不做 JIT、AOT 到机器码、copy-and-patch，不碰可执行内存。
- 不用 tinyvm `wasi-p1` feature 当插件面。
- 不替换 `agenterm-wasmcore`，不改 `.wasm` 默认路由。
- 不做跨槽通信、共享内存、共享 table/global。
- 不在本仓改 tinyvm，不 vendor 其源码。
- 不在归档门三条全绿前动 `agenterm-qjs`。
