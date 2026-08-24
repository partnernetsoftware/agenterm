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

## M0 能跑什么（诚实边界）

当前是 **M0**，`.qjs` 只支持：

- 十进制整数字面量
- `+` `-` `*` `/` `%`、一元负号、括号（正确的优先级与结合性）
- `$0` `$1` … 取本次调用的参数

`g()*2+$0` 这类能跑。`function f(){}`、字符串、`let`、对象——**编译期明确拒绝**，
诊断文案会说清是引擎能力边界，不会含糊地说"语法错误"让你以为脚本写错了。

`.wasm` 侧是完整的：任何过 tinyvm 装载门的标准模块都能装载、按名调用、有预算地执行。

增长路线（M1 前端 → M2 整数世界 → M3 字符串 → M4 堆对象 → M5 闭包/try/JSON）见设计稿。
第一个具体锚点是编译 `scripts/qjs/lib/fleet.js` 的等价物——那也是归档
`agenterm-qjs` 的门。

## 脸

```rust
use agenterm_qjswasm::{Engine, Guest, Value};

let mut engine = Engine::new();
let out = engine.run_once(Guest::Qjs("$0 * 2"), None, "main", &[Value::I32(21)])?;
assert_eq!(out.values, vec![Value::I32(42)]);
# Ok::<(), agenterm_qjswasm::QjswasmError>(())
```

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
`crates/tinyvm-qjs/tests/`。本 crate 的 `tests/qjs_guest.rs` 只测接缝：`.qjs` 端到端
过槽、编译失败自成一类、产物过本 crate 的装载门、扩展名路由。
