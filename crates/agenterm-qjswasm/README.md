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
真正的业务——`agenterm.*` 门、槽、预算策略、接线。`compile_qjs` / `CompileError` 原样
再导出，用法不变。撤销记录见
[PRD 36](../../prd/PRD_02_36_agenterm_qjswasm.md) 与
[设计稿 §2](../../plan/design-agenterm-qjswasm.md)。

`.wasm` 输入跳过 ①，从 ② 进。**两种输入在核这一层完全同待遇**——这就是「一个引擎跑
两种东西」的确切含义，不是两条管线共用一个名字。

## `.qjs` 能跑什么（诚实边界，每条都是编出来跑过的）

下表每一行都由 `tests/qjs_guest.rs` 编译并执行过，不是读上游源码得出的。

**能跑：**

- **数字是 ECMA-262 binary64**，不是 i32：`1/0` 是 `Infinity`、`0/0` 是 `NaN`、
  `2147483647 + 1` 不回绕。字面量仍只写十进制整数——`0.5` / `1e3` / `0x10` / `1_000`
  各自撞自己的边界。
- **其他值**：字符串（转义与 `\u{…}` 已解码）、`true` / `false`、`null`、`undefined`。
- **语句**：`let` / `const` / `var`（真作用域 + 文本可判定的 TDZ）、块、`if`/`else`、
  `while`、三段式 `for`、`return`，以及脚本的 ECMA-262 completion value。
- **函数**：声明式带参数、递归与互递归。**调用必须是直接调用**——被调方得是一个绑定到
  已知函数的名字。
- **运算符**：赋值与复合赋值、`||`、`&&`、`==`/`!=`/`===`/`!==`、`<` `<=` `>` `>=`、
  `+` `-`、`*` `/`、前后缀 `++`/`--`、一元 `+ - !`、括号。`+` 在任一侧是字符串时拼接。
- **ASI**：ECMA-262 12.10。

**明确拒绝**（编译期，诊断说清是引擎边界）：`%`、`typeof`、对象/数组字面量、属性访问、
`?:`、`try`/`throw`、`class`、`switch`、`break`/`continue`、`for…of`、模板字面量、
位运算与移位、`**`、`??`、逗号运算符、BigInt；**捕获外层局部变量的闭包**；
**把函数当值用**（`let f = function(){}` 之后 `f()`、`return f`——都拒绝；
立即调用的函数表达式 `(function(a){...})(1)` 可以）。

**两处运行期行为要知道：**

- 三个 ECMA-262 转换尚未实现（Number 的 ToString、StringToNumber、字符串关系比较），
  撞上是 **trap 而不是编造一个值**：`"a" + 1`、`"2" * 2`、`"a" < "b"`、`1 == "1"` 都
  trap。这是上游记录在案的 divergence，不是本层的分类错误。
- **`.qjs` 还够不着 `agenterm.*` 门。** 自由名字一律在编译期被拒
  （"this engine has no global bindings yet"），所以 `print` / `fleet_call` 目前只有
  手写 `.wasm` 客人能调。门本身是通的、有测试；缺的是语言侧的那条线。

`.wasm` 侧是完整的：任何过 tinyvm 装载门的标准模块都能装载、按名调用、有预算地执行。

第一个具体锚点是编译 `scripts/qjs/lib/fleet.js` 的等价物——那也是归档
`agenterm-qjs` 的门。缺口清单见 [PRD 36 §归档门](../../prd/PRD_02_36_agenterm_qjswasm.md)。

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

## 宿主门 ABI

模块名 `"agenterm"`。这是客人能看见的**全部**世界。

```text
print(ptr: i32, len: i32)                                              -> ()
fleet_call(op_ptr: i32, op_len: i32, params_ptr: i32, params_len: i32) -> i32   // status
fleet_result_len()                                                     -> i32
fleet_result(dst_ptr: i32, dst_len: i32)                               -> i32   // 写入字节数，负=目标太小
```

`status`：`0` = Ok · `1` = Err（应用级错误，是正常结果不是崩溃）· `2` = NoBridge。
状态码语义与 `agenterm-wasmcore` 一致，guest 作者只学一套。

背后就是全仓共用的 `ScriptFleetBridgeFn`。本 crate 把**这一条既有能力**暴露给 wasm
客人，不发明第二条。

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
扩展名路由。

其中 `the_capability_claims_in_this_crates_own_copy` 是**上面那张能力表的锁**：本
README 与 PRD 36 用 agenterm 自己的口径做能力声明，所以那些声明必须由一条会跑的测试
兜住，而不是靠读上游源码。"M0，只有整数表达式"就是这样漂成假话的。
