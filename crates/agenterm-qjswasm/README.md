# agenterm-qjswasm

AgenTerm 自己的脚本引擎。`.qjs` 用**纯 Rust** 编译成 `.wasm`，`.wasm` 直接跑；
核是 [tinyvm](https://github.com/partnernetsoftware/tinyvm)——无 JIT、装载期校验、
上限在核。

**不是** `rquickjs`，**不链** QuickJS C 库，**不是**（目前还不是）一个 JavaScript 引擎。

产品真理：[`prd/PRD_02_36_agenterm_qjswasm.md`](../../prd/PRD_02_36_agenterm_qjswasm.md)。
实现设计：[`plan/design-agenterm-qjswasm.md`](../../plan/design-agenterm-qjswasm.md)。

## 管线

```text
.qjs 源码
   │  ① 词法 / 语法 / 降级      tinyvm-qjs，纯 Rust，ECMA-262 为语义权威
   ▼
标准 .wasm 字节
   │  ② decode / validate / Limits      tinyvm
   ▼
解释执行，不生成机器码
```

编译器（①）2026-08-24 从本 crate 迁往上游 `tinyvm-qjs`：它一行 agenterm 概念都没有，
按「通用引擎能力归 tinyvm、业务归 agenterm」这条分层线，它属于上游。本 crate 留下的是
真正的业务——`agenterm.*` 门、槽、预算策略、接线。`CompileError` 原样再导出；
`compile_qjs` 是本 crate 的函数，签名不变，但它现在**带着门的声明表**进上游编译器
（`compile_qjs_m1_with`），因为「有哪些宿主能力」正是业务那一半。撤销记录见
[PRD 36](../../prd/PRD_02_36_agenterm_qjswasm.md) 与
[设计稿 §2](../../plan/design-agenterm-qjswasm.md)。

`.wasm` 输入跳过 ①，从 ② 进。**两种输入在核这一层完全同待遇**——这就是「一个引擎跑
两种东西」的确切含义，不是两条管线共用一个名字。

## `.qjs` 能跑什么（诚实边界，2026-08-25 在 rev `6920c60` 上逐条实测）

**这张表是跑出来的。** 能跑的每一条都编译并执行过，拿到的就是这里写的值；拒绝的每一条
都编译过，拿到的就是这里引的诊断。读上游源码、或转述别人的报告，都不算证据——这两种口径
在本仓各骗过一次人：`%` 与 `typeof` 早已支持却还挂在拒绝表上，以及本文件曾说这个 crate
在工作区外而它在里面。

**能跑：**

- **数字是 ECMA-262 binary64**，不是 i32：`1/0` 是 `Infinity`、`0/0` 是 `NaN`、
  `2147483647 + 1` 是 `2147483648` 不回绕。字面量仍只写十进制整数——`0.5`
  （fractional numbers）、`1e3`（exponent）、`0x10`（hexadecimal）、`1_000`
  （numeric separators）各自撞自己的那条诊断。
- **其他值**：字符串（转义与 `\u{…}` 已解码：`"a\tb\u{1F600}"` 解出来是 `a`、一个制表符、
  `b`、`😀`）、`true` / `false`、`null`、`undefined`。
- **语句**：`let` / `const` / `var`（真作用域 + 文本可判定的 TDZ）、块、`if`/`else`、
  `while`、三段式 `for`、`return`，以及脚本的 ECMA-262 completion value（`1 + 2;` → `3`）。
- **函数**：声明式带参数、递归（斐波那契 `f(10)` = `55`）与互递归、嵌套的函数声明、
  函数体里读模块顶层的绑定（`var` 与 `let` 都可以——那不算「捕获」）。
  **调用必须是直接调用**——被调方得是一个绑定到已知函数的名字。
- **运算符**：赋值与复合赋值、`||`、`&&`、`==`/`!=`/`===`/`!==`、`<` `<=` `>` `>=`、
  `+` `-`、`*` `/`、`%`、`typeof`、前后缀 `++`/`--`、一元 `+ - !`、括号。`+` 在任一侧是
  字符串时拼接；`typeof` 照规范来（`typeof null` 是 `"object"`）。
- **ASI**：ECMA-262 12.10。
- **`agenterm.*` 门**：`print` / `fleet_call` / `fleet_result`——见下一节。

**明确拒绝**（全在编译期）。分三类，因为拒绝的**理由**不是一个：

1. **语法认得，能力还没有**——诊断形如「this engine does not support X yet」：
   对象/数组字面量、属性访问与索引、`?:`、`try`/`throw`、`class`、`switch`、
   `break`/`continue`、`for…of` / `for…in`（报的是 `of` / `in` 关键字）、`do`/`while`、
   模板字面量、位运算与移位、`**`、`??`、逗号运算符、BigInt、箭头函数、`new` / `delete` /
   `void` / `in` / `instanceof`、展开与 rest、解构、默认参数、可选链、`async`/`await`、
   `import`、带标签的语句（这一条的诊断说错了，见下）；
   **捕获外层局部变量的闭包**；**把函数当值用**（`let f = function(){}` 之后 `f()`、
   `return f`——都拒绝；立即调用的函数表达式 `(function(a){...})(1)` 可以）。
2. **根本没有全局对象。** `JSON`、`Math`、`String`、`Number` 今天不是名字，写它们撞的是
   门的诊断：``this engine has no host function named `JSON`; this embedder declares
   `print`, `fleet_call` and `fleet_result` ``。所以 `JSON.stringify(x)` 不是「JSON 还没
   实现」，是两层都不在：没有那个名字，也没有属性访问。
3. **解析就没过**——正则字面量 `/a/`（「needs an operand here, and found a `/`」）、
   生成器 `function*`（「needs a name for the function declared here」）。

**诊断的诚实度有两处实测缺口**，在上游，本仓改不了，写在这里免得被当成不存在：
**带标签的语句**（`outer: while (true) {}`、`lbl: for (;;) {}`）报的是「does not support
conditional expressions yet」——把 `:` 当成三目的冒号了；**`for (const x of y)`** 报的是
「needs a value for the `const` binding `x`」而不是 `of`（同一句写成 `let` 或 `var` 就正确
报 `of` / `in` 关键字）。两条都确实拒绝了，只是说错了撞的是哪条能力边界。

**运行期要知道的一条：** 三个 ECMA-262 转换尚未实现（Number 的 ToString、
StringToNumber、字符串关系比较），撞上是 **trap 而不是编造一个值**——`"" + 1`、`"a" + 1`、
`"2" * 2`、`"a" < "b"`、`1 == "1"` 都 trap。字符串之间的拼接与 `==` / `!=` / `===` 是好的。
这是上游记录在案的 divergence，不是本层的分类错误。

### `.qjs` 已经够得着门（2026-08-25，rev `6920c60`）

曾经这里写的是「自由名字一律在编译期被拒，所以 `print` / `fleet_call` 只有手写 `.wasm`
客人能调」——**那一条已作废**。脚本直接写三个名字：

```js
print("hello");
let status = fleet_call("tabs.list", "{}");   // 0=Ok 1=Err 2=NoBridge
if (status === 0) { return fleet_result(); }
return status;
```

这段是**原样跑过的**：`stdout` 是 `"hello"`，桥收到 `("tabs.list", "{}")`，返回值是桥的
答案；同一段不给桥再跑一遍，返回 `2`，而 `"hello"` 照样进 `stdout`。

门**一个字没改**：客人仍然导入那四个原始 i32 函数（下面「宿主门 ABI」那张表），与手写
`.wasm` 客人同一张 import 表。拆包是**编译器**的活——JS 字符串拆成 `(ptr, len)`，两趟取回
的字节（`fleet_result_len` → bump 分配 → `fleet_result`）组装回 JS 字符串。门不认识 JS
值，这个方向就是设计本身：让门说 V1 双字会弄坏每一个手写客人，也会把一门语言的值表示
泄进一个本该服务任意客人的边界（`plan/design-agenterm-qjswasm.md` §6.5）。

声明表在 `src/host.rs::declarations()`，公开面是 `door_declarations()`。
**脚本可见名 = field 名**，不改名：读上面那张门表写下来的，就是能用的名字。

动手之前值得知道的，每条都是实测：

| 事实 | 实测 |
|------|------|
| 三条声明发射四个 import——`fleet_result` 是两趟 | `print("x"); fleet_call("o","p"); return fleet_result();` → `agenterm.print` · `fleet_call` · `fleet_result_len` · `fleet_result` |
| `fleet_result_len` **不是脚本能写的名字**，长度那趟归编译器 | 写它 → ``this engine has no host function named `fleet_result_len` `` |
| 只有脚本**语法上提到**的声明才变成 import | `return 1 + 1;` → 零个 import。按「提到」不按「可达」：`if (false) { print("x"); }` 仍然 emit `agenterm.print` |
| 参数**类型**是编译期查的，门只收字符串 | `print(42)` → ``cannot pass a Number to argument 1 of the host function `print`, which is declared to take a String`` |
| 参数**个数**也是编译期查的 | `print("a","b")` → ``given the host function `print` with 1 argument(s), and this call passes 2`` |
| 传进去的字符串不必是字面量 | `let op = "tabs" + ".list"; fleet_call(op, "{}")` → 桥收到 `"tabs.list"` |
| `print(...)` 求值为 `undefined` | `typeof print("x")` → `"undefined"` |
| 没调 `fleet_call` 就 `fleet_result()` → 空串，不是错 | `"[" + fleet_result() + "]"` → `"[]"` |
| 没有桥 → status `2`，脚本继续跑，不 trap | `return fleet_call("a","b")` → `2` |
| 门名字**不是保留字**，脚本自己的绑定盖得住 | `function print(m) { return 1; } return print("x");` → `1`，宿主门没被调到；声明只是自由名字的兜底 |

**一个会咬人的坑：status 是数字，而 Number 的 ToString 还没实现，所以 `"status:" + s`
会 trap。** 状态码用 `===` 分支，别拼进字符串。这不是门的毛病（是上面那三个未实现转换
之一），但它正好落在门最常见的写法上。

证据在 `tests/qjs_door.rs`（13 条：状态 0/1/2 三条路、`print` 进 `Outcome::stdout`、
`print` 求值为 `undefined`、两个宿主侧上限、门外的名字被拒且诊断把声明了哪些列出来、
以及把 emit 出来的 import 表解码出来逐字对 `src/host.rs::SIGNATURES`）。

想要一份**够不着门**的产物（import 表按构造为空）用 `compile_qjs_without_door`——实测它
对 `print("x")` 报 ``this engine finds no declaration of `print` ``，对 `return 1 + 1;`
发射零个 import。`check` 与 `execute` 都走 `compile_qjs`，两边看见的是同一门语言。

`.wasm` 侧是完整的：任何过 tinyvm 装载门的标准模块都能装载、按名调用、有预算地执行。

第一个具体锚点仍是编译 `scripts/qjs/lib/fleet.js` 的等价物——那也是归档 `agenterm-qjs`
的门。**门本身已经不在缺口清单里了**，挡着它的是语言：对象/数组、属性访问、`try`/`catch`、
`JSON`。清单见 [PRD 36 §归档门](../../prd/PRD_02_36_agenterm_qjswasm.md)。

## 脸：两套调用约定，一张脸

手写 `.wasm` 客人说的是 wasm 数值；`.qjs` 客人说的是编译器的 **V1 表示**——一个
JavaScript 值是一对 `(tag: i32, payload: i64)`，所以入口每个参数占两个 wasm 参数、
返回两个结果。`Value` 同时承载两者，槽在装载时记下自己是哪一套，两边的调用者都不必
学对方的 ABI。

```rust
use agenterm_qjswasm::{Engine, Guest, JsValue, Value};

let mut engine = Engine::new();

// `.qjs`：一个 JavaScript 值进，一个 JavaScript 值出。
let out = engine.run_once(
    Guest::Qjs("$0 * 2"),
    None,
    "main",
    &[Value::Js(JsValue::Number(21.0))],
)?;
assert_eq!(out.values, vec![Value::Js(JsValue::Number(42.0))]);

// 手写 `.wasm`：wasm 数值，这一路一行没变。
let out = engine.run_once(Guest::Wasm(&bytes), None, "add", &[Value::I32(40), Value::I32(2)])?;
assert_eq!(out.values, vec![Value::I32(42)]);
```

`JsValue` 是**已解析成宿主数据**的投影，不是转发的原始 pair。理由是机制性的：字符串的
payload 是指向**该槽线性内存**的指针，而 `run_once` 在返回前就把槽杀了——转发指针等于
在最常见的路径上交出一个悬垂引用。所以接缝在实例还活着的时候把它读出来。

这个形状挺得过 M4：固定下来的不是变体清单，而是**解析点**——「客人表示变成宿主数据」
只有一处。数组与对象到来时是在同一处多几个变体，不是让调用者再学一套机制。真的投影不
出来的（函数值、循环对象）走 `QjswasmError::UnsupportedValue`，那个类的含义本来就是
「客人没错，是这张脸装不下」。

约定不匹配也走同一类：把裸 wasm 数值递给 `.qjs` 槽、或把 `JsValue` 递给手写模块，
是 `UnsupportedValue` 而不是默默按位重解释。字符串**作为参数**同样被拒——那需要在客人
堆里分配，而这张脸还没有通往那个分配器的门。

**约定是在装载时记下的，从来不靠签名去猜**，所以「已经编好的 `.qjs` 产物」要有自己的
入口：`Guest::CompiledQjs(&[u8])`。一份 `.wasm` 文件不记得自己是从 `.qjs` 来的，用
`Guest::Wasm` 装回去，V1 pair 就原样过脸——字符串变成一个 tag 加一个指向马上要被丢掉的
线性内存的指针。任何「先编译到盘、以后再跑」的形状（`pack` 产物、缓存、网上取来的客人）
都需要这个变体。它不多给任何权力：同一道装载校验、同一套 `Limits`、同一扇门，只是槽记
下的约定不同。

顺带，它也是**接缝那五条防线第一次可测**的原因：`read_guest_string` 的五种拒绝（指针
不是地址、头越界、体越界、非 UTF-8、根本没有线性内存）在此之前只可能被信任的编译器产物
触发，即没有任何可达的调用者。`tests/seam_attack.rs` 的 `a_hostile_*` 五条现在真的打得到。

`spawn` / `call` 分开，因为 tinyvm 的 `Instance` 是**持久**的：装一次、调多次，
每次顶层调用拿一份新鲜的 `max_steps` 预算。一次性客人用 `run_once`。

每次调用回报确定性成本（`steps` / `peak_call_depth` / `peak_activation_slots`），
所以「这个脚本贵不贵」是可度量的，不是靠猜。

## 隔离与预算

一份 `.wasm`（手写的或 `.qjs` 编出来的）= 一个槽 = 一份预算。槽间互不可见，
只经宿主门看世界，**一个坏槽只能弄死自己**。

| 预算 | 归属 | 触发后 |
|------|------|--------|
| `max_steps`（每次顶层调用） | tinyvm | 该次调用 trap，槽可回收，宿主活着 |
| `max_memory_pages` / `max_table_elems` | tinyvm | 装载期拒绝或 `grow` 失败 |
| `max_call_depth` / `max_activation_slots` | tinyvm | trap，不吃原生栈 |
| `max_stdout_bytes` | 本 crate | 截断并置 `truncated_stdout`，不静默丢 |
| `max_bridge_result_bytes` | 本 crate | 报错，**不截断**——半个 JSON 比拒绝更糟 |
| `max_result_string_bytes` | 本 crate | 报错，**不截断**——理由同上 |

`max_result_string_bytes` 2026-08-25 补上，因为宿主侧原本只有两个盖子，而接缝把
`.qjs` 返回的字符串**拷进宿主 String** 是第三块宿主分配、由客人定大小、两个盖子都不管。
在它之前唯一的上限是偶然的：默认预算下 `max_steps` 先耗尽（拼接是 O(n) 步），一旦客人
能便宜地造出大字符串、或谁调高 `max_steps`，真实上限就变成
`max_memory_pages × 64 KiB`，每次调用一份，持久槽上反复。
顺序也是分类：**先做越界检查，再看盖子**——声明长度装不进客人自己的内存是坏客人
（`Door`），把它说成预算等于让人去调一个调了也没用的数。

`max_memory_pages` 有一条**运行期**缺口，写在这里免得被当成已解决：装载期超页是
`Load`，但运行期 `memory.grow` 被拒之后，上游 `tinyvm-qjs` 的 `__alloc` 把它降成一条
裸 `unreachable`，到宿主这里与任何别的 `unreachable` 无法区分，所以报的是
`Trap` 而不是 `Budget("max_memory_pages")`。这不是本仓能修的——信息在上游就被丢掉了，
宿主侧靠"内存正好顶到上限"去猜会把真坏的脚本误判成预算问题。要补在上游：分配失败必须
可分辨（带 `WasmCeiling::MemoryPages` 的独立 fault，或走一扇门报告）。
复现见 `tests/seam_attack.rs::finding_4_running_out_of_pages_is_not_reported_as_a_budget`。

## 宿主门 ABI

模块名 `"agenterm"`。这是客人能看见的**全部**世界。**两种客人共用这一张表**：手写
`.wasm` 直接导入它，`.qjs` 通过三个自由函数名落到它上面（见上面「`.qjs` 已经够得着门」，
以及 `plan/design-agenterm-qjswasm.md` §6.5）。

```text
print(ptr: i32, len: i32)                                              -> ()
fleet_call(op_ptr: i32, op_len: i32, params_ptr: i32, params_len: i32) -> i32   // status
fleet_result_len()                                                     -> i32
fleet_result(dst_ptr: i32, dst_len: i32)                               -> i32   // 写入字节数，负=目标太小
```

`status`：`0` = Ok · `1` = Err（应用级错误，是正常结果不是崩溃）· `2` = NoBridge。
状态码语义与 `agenterm-wasmcore` 一致，guest 作者只学一套。

**`agenterm.*` 之外的 import 装载期即拒，并把名字说出来。** 四件门函数客人可以只导入
一部分、或一个都不导入；但导入**别的模块名**（最典型的是
`wasi_snapshot_preview1.fd_write`）是另一回事——没人能绑它。2026-08-25 之前这种模块
`validate_wasm` 返回 `Ok(())`、`spawn` 成功、第一次调用才死在
`Trap("call to unbound imported function")`：check 放行了 execute 跑不了的东西，而且
那条 trap 一个 import 名都不报（tinyvm 是 `no_std`，文案是静态前缀）。现在两条路
给同一个答案，都在 `Door` 类里，都带名字。锁在
`tests/host_door.rs::check_and_execute_agree_that_an_unbindable_import_is_refused_at_load`。

背后就是全仓共用的 `ScriptFleetBridgeFn`。本 crate 把**这一条既有能力**暴露给 wasm
客人，不发明第二条。

### 调用不合导出的签名，是调用者的错，不是客人的错

`call` 在进客人**之前**先问一次导出的声明类型
（`WasmInstance::exported_function_handle`），三种误报因此各归各位：

| 情形 | 2026-08-25 之前 | 现在 |
|------|----------------|------|
| 导出名不存在 | `Trap("no exported function named")`，不带名字 | `NoSuchExport`，带名字 |
| 参数**个数**不对 | `Trap("function")` | `Signature`，两个数都报；`.qjs` 槽按 JavaScript 参数个数报，不按 wasm 字数 |
| 参数**类型**不对 | **不报**——`(param i32)` 收 `I64` 返回 `Ok([I64(..)])`，与导出自己的 `(result i32)` 矛盾；只有客人真去用那个值才变成 trap | `Signature`，报参数序号和两个类型 |
| 结果类型这张脸装不下 | 客人跑完之后才 `UnsupportedValue`，它一路上打印的输出被一起丢掉 | 进客人之前就拒，什么都没产生，也就没得丢 |

最后一行顺带修掉了一个丢输出的洞。**残留的代价现在是明说的**：客人已经跑起来之后才失败
的调用（trap、预算、V1 pair 畸形），它打印的东西仍然会丢——这条写在 `Slot::call` 上，
是有意的，因为把它留到**下一次**调用的 `Outcome` 里比丢掉更糟（张冠李戴）。

### 为什么是两趟拷贝

`agenterm-wasmcore` 的六参数单次调用里，宿主拿到结果后回调 guest 导出的
`wasmcore_alloc` 要一块 buffer。**那条路在 tinyvm 上走不通**，理由是机制性的：

tinyvm 的宿主回调签名是 `Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError>`——
回调**持着线性内存的 `&mut`**，而回调进 guest 需要 `Instance::invoke_by_name(&mut self)`。
安全 Rust 里这两者不能同时成立，即**宿主回调内部无法重入 guest**。这不是 tinyvm 的
缺陷，是它「无 JIT + 显式调用栈 + 上限在核」的必然结果。

所以 `fleet_call` 只回 status，字节暂存在该槽宿主侧的 pending buffer；guest 自己问
长度、自己分配、再让宿主拷进来。多两次跨界，换来零重入、宿主不要求 guest 导出分配器，
且与 tinyvm iOS 桥既有的 two-pass 手法同源。

## 与相邻 crate 的关系

| 面 | crate | 引擎 | 信任模型 |
|----|-------|------|----------|
| `.qjs` / `.wasm`（本 crate） | `agenterm-qjswasm` + `tinyvm-qjs` | tinyvm，**无 JIT**，自研编译器 | 不信任字节 |
| `.js` / `.mjs` | `agenterm-qjs` | rquickjs → QuickJS C | 信任脚本，**待归档** |
| `.wasm`（默认路由） | `agenterm-wasmcore` | wasmtime + WASI p1，**JIT** | 本机工具链产物 |

本 crate **不改** `.js` / `.mjs` / `.wasm` 的默认路由。要让它接管 `.wasm`，显式设
`AGENTERM_SCRIPT_BACKEND=qjswasm`。

## 开发

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo test  -p agenterm-qjswasm
cargo clippy -p agenterm-qjswasm --all-targets -- -D warnings
```

依赖 `tinyvm` 与 `tinyvm-qjs` 都是**同一个私有仓的 git 依赖**，钉同一个 rev。仓根 `.cargo/config.toml` 里的
`[net] git-fetch-with-cli = true` 是**必需**的：cargo 内置的 libgit2 客户端拿不到
GitHub 私有仓凭据，实测报 `failed to receive HTTP 200 response: got 401`。

`wat` 只是 **dev-dependency**，用来把对抗性客人写成可读的 `.wat` 文本。产品自己的
wasm 编码器在上游 `tinyvm-qjs/src/encode.rs`——刻意不引 `wasm-encoder`，因为产物必须过
tinyvm 的严格装载门（canonical function expression、strict memarg alignment、strict
i64 signed-LEB range…），那份正确性要自己负责。

语言子集的验收测（能编什么、拒什么、诊断怎么说）跟编译器一起在上游
`crates/tinyvm-qjs/tests/`。本 crate 的 `tests/qjs_guest.rs` 测接缝：`.qjs` 端到端
过槽、两套调用约定、JS 值投影与字符串解析、编译失败自成一类、产物过本 crate 的装载门、
扩展名路由；`tests/qjs_door.rs` 测 `.qjs` 那一侧的门。

其中 `the_capability_claims_in_this_crates_own_copy` 是**上面那张能力表的锁**：本
README 与 PRD 36 用 agenterm 自己的口径做能力声明，所以那些声明必须由一条会跑的测试
兜住，而不是靠读上游源码。"M0，只有整数表达式"就是这样漂成假话的。

**这把锁今天还没盖满整张表**，说清楚免得读的人高估它。有锁的：「能跑」那一栏的绝大部分，
以及门那张表里的骨干（四个 import 的字节、三条状态路、`print` 求值为 `undefined`、门外
名字被拒）——都在 `qjs_guest.rs` / `qjs_door.rs` 里。**没锁的**：「明确拒绝」那一栏只有
四条源码进了 `a_source_outside_the_subset_is_a_compile_error_not_a_load_error`
（`?:`、对象字面量、模板字面量、捕获闭包），其余二十来条、两条诊断缺口、以及门那张表里
的编译期参数检查 / 遮蔽 / 死分支仍发射 import 这几行，都是 2026-08-25 逐条编译执行记录
下来的：跑过，但上游一旦放宽，只有那四条会自己喊出来。要么把它们补进那条测试，要么下次
抬 rev 时把这张表整个复测一遍。
