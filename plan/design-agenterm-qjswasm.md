# `agenterm-qjswasm` 实现设计：`.qjs` → `.wasm` 纯 Rust 编译器 + tinyvm 执行面

| 字段 | 值 |
|------|-----|
| **文档** | 新 crate `crates/agenterm-qjswasm` 的实现级设计：分期、编译器结构、宿主门 ABI、机制取舍、验收树 |
| 日期 | 2026-08-24 |
| 状态 | 设计稿 rev3（M0 已落地；编译器已迁往上游 `tinyvm-qjs`） |
| **产品真理** | [`prd/PRD_02_36_agenterm_qjswasm.md`](../prd/PRD_02_36_agenterm_qjswasm.md)（PRD 36）。产品句、纪律、clean-room、JS 覆盖面口径、归档门**以该文件为准**；本文件只是执行投影 |
| 关联 | tinyvm 仓 `prd/PRD.md`、`crates/tinyvm/research-qjs-wasm.md`；本仓 `src/script_engine.rs`、`crates/agenterm-wasmcore/README.md`、[PRD 14 provenance](../prd/PRD_02_14_research_provenance.md) |
| 派单 | 2026-08-24 政委：在 tinyvm 上自研脚本引擎，跑 `.wasm` 与 `.qjs`；`.qjs` 先编译到 wasm 码（不是机器码）；**不用 rquickjs、不用 QuickJS C 库**，纯 Rust 实现，可参考 QuickJS 源码的设计；编译器写刀原定在 agenterm 仓，2026-08-24 撤销，迁往上游 `tinyvm-qjs`（见 §2） |
| 范围声明 | 本文档只写设计与分期，不代替实现 |

---

## 1. 一句话

`.qjs` → 自研 Rust 编译器 → 标准 `.wasm` → tinyvm 解释执行。`.wasm` 输入跳过前两步。
两种输入在核这一层完全同待遇。

## 2. 依赖方向与仓位

```
agenterm (embedder)
  └── crates/agenterm-qjswasm          ← 本刀：agenterm 的门与策略
        ├── tinyvm-qjs  (git rev)      ← .qjs → .wasm 编译器，写刀在 ../tinyvm
        └── tinyvm      (git rev)      ← 执行核，写刀在 ../tinyvm
```

三层，一条分界线：**通用动态引擎能力在 tinyvm 侧，业务在 agenterm 侧。**

| 层 | 内容 | 写刀 |
|----|------|------|
| `tinyvm` | wasm 核 + embedder 面（`guest_memory`、`PendingResult`、故障分类） | ../tinyvm |
| `tinyvm-qjs` | `.qjs → .wasm` 编译器（lex / parse / ast / ir / emit / encode / diag） | ../tinyvm |
| `agenterm-qjswasm` | agenterm 的门（`agenterm.*`、`fleet`）、槽与预算策略、`ScriptBackend` 接线 | 本仓 |

- 依赖方向 **agenterm → tinyvm**，单向。不 vendor tinyvm 源码。
- 用 **git + rev 钉死**，对齐下游 `minicon` 依赖 agenterm 的既有形状。
  两个 crate 钉**同一个 rev**——它们出自同一个仓，钉不同 rev 会让编译器和核对不上。

### 撤销：「不依赖 `tinyvm-qjs`」（2026-08-24）

**本节 rev2 写过：**

> **不依赖 `tinyvm-qjs`。** 那是 tinyvm 自己的演示皮（447 行表达式子集），归上游长。
> 本 crate 的编译器独立实现，可以读它取经，但不建立 Cargo 依赖——否则 agenterm 的语言
> 路线图被另一个仓的排期卡住，与「自己的语言」这条派单相悖。

**该决定撤销。** 编译器已迁往 `tinyvm-qjs`，本 crate 建立 Cargo 依赖。两条理由：

1. **编译器里没有业务。** 迁走的 1113 行（`src/lower/**`）中 agenterm 概念数为**零**：
   它回答「JS 源码怎么变成 wasm 字节」，与谁在 embed 无关。同期留下的 `src/host.rs`
   869 行里 `fleet` 出现 48 次、模块名写死 `"agenterm"`。按分层原则量，这两半本就该分
   在两层。原决定按「谁先写的」划线，不是按「是什么」。
2. **排期绑架的风险被高估。** 「被另一个仓的排期卡住」预设两仓有各自的排期主体；实际
   同一 owner、同一批工人，且 PRD 36 已授权「撞到 tinyvm 层的真实缺口就去上游改，
   不绕」。为躲一个不存在的外部依赖而各长一份编译器，代价是两份都长不快。

**没有变的**：`agenterm.*` 门、槽、预算、接线仍是本仓的写刀；
`agenterm_qjswasm::compile_qjs` / `CompileError` 原样再导出，调用点一行未动。

**上游为容纳它做的一处让步**：一个裸名字在两边含义不同——语言侧还没有绑定，
`eval_wasm` 皮侧它是 `js.<name>` 零参导入。这不是宽严之别，是两个真实的世界，所以做成
`Options { names }` 一个字段，而不是让某一边将就。
- **feature 门默认关**：根 `Cargo.toml` 加 `script-qjswasm = ["agenterm-qjswasm"]`，
  `default = []` 不动。对齐 `script-wasmcore` 的既有形状。

### 已实测：私有仓 git dep 必须走系统 git

2026-08-24 在本机用一个抛弃式 probe crate 实测（跑完即删，未进树）：

- `cargo fetch` **失败**：`failed to receive HTTP 200 response: got 401; class=Net (12)`。
  cargo 内置 libgit2 客户端拿不到 GitHub 私有仓凭据。
- `CARGO_NET_GIT_FETCH_WITH_CLI=true cargo fetch` **成功**：改用系统 `git`，复用本机
  已有凭据。

所以实现刀必须同时在 `.cargo/config.toml` 加：

```toml
[net]
git-fetch-with-cli = true
```

不能只靠开发者记住设环境变量——那是「在我机器上能编」的经典形状。

## 3. 分期（每期都是可交付、可证伪的一刀）

| 期 | 交付 | 归档门进度 |
|----|------|-----------|
| **M0 脊柱** | crate 骨架 + tinyvm 依赖 + `.wasm` 直跑 + 宿主门四件 + 槽 + 预算 + 对抗性隔离测试。**编译器暂缺**，`.qjs` 走一个最小整数表达式降级证明管线通 | — |
| **M1 前端** | 词法（含 ASI）+ 语法 + AST，覆盖整数 / 变量 / 语句 / 控制流 / 函数。诊断诚实（指语法能力边界，不指用户） | — |
| **M2 整数世界** | 降级：locals、赋值、`if`/`while`/`for`、函数声明与调用、返回。**不带堆** | — |
| **M3 字符串** | guest 侧运行时第一块：线性内存串表示 + bump 分配器。字符串字面量、拼接、比较、长度 | — |
| **M4 堆对象** | 数组 + 对象字面量 + 属性读写。堆布局 + 属性查找 | — |
| **M5 parity** | 闭包 + `try`/`catch` + `JSON`。**跑通 `fleet.js` 等价物 → 触发归档门评估** | ✅ |

M0 先落地是刻意的：它把「tinyvm 能不能真的当 agenterm 的执行核」这个风险最大的问题
用真实证据答掉，而且此后每一期编译器工作都有一条已验证的执行管线可以立刻验收，不必
等编译器完整才第一次看到脚本跑起来。

## 4. crate 结构

```
crates/agenterm-qjswasm/            ← 本仓：业务
├── Cargo.toml
├── README.md
├── src/
│   ├── lib.rs          — 门面：Engine / Budget / Guest / Outcome / Error
│   │                     + 再导出 tinyvm_qjs::{compile_qjs, CompileError}
│   ├── slot.rs         — 槽：Module→Instance 生命周期、每槽 pending buffer、故障分类
│   └── host.rs         — 宿主门四件：print / fleet_call / fleet_result_len / fleet_result
└── tests/
    ├── wasm_slot.rs · isolation.rs · budget.rs · host_door.rs
    └── qjs_guest.rs    — `.qjs` 端到端过槽、编译失败自成一类、扩展名路由

../tinyvm/crates/tinyvm-qjs/        ← 上游：语言
└── src/
    ├── lib.rs          — compile_qjs / compile_qjs_with / Options / eval_qjs
    ├── lex.rs · ast.rs · parse.rs · ir.rs · emit.rs · encode.rs
    ├── diag.rs         — CompileError + Boundary：位置 + 「本引擎尚不支持 X」文案
    └── qjs2wasm.rs     — eval_wasm 皮的编译入口（Names::HostImport + 诊断窄化）
```

`encode.rs` 自己写 wasm 编码器，不引 `wasm-encoder` 之类 crate：产物要过 tinyvm
的严格装载门（canonical function expression、strict memarg alignment、strict i64
signed-LEB range…），自己编码才能对这些约束负责，也少一个依赖。这条纪律随编译器一起
迁到上游。

### 诊断与 fmt-free 核的张力（迁移时必须解掉的一处）

`CompileError` 带 `String`（文案 + 字节偏移），`tinyvm::WasmError` 只带
`&'static str`——核是 `no_std` + fmt-free，那是产品属性不是疏忽。上游的 `eval_qjs` 必须
回 `WasmError`，于是两者在 `qjs2wasm` 处相撞。

**没有把诊断降级成 `&'static str`。** 一个按需求生长的子集会频繁拒绝好脚本，
「hexadecimal number literals，第 4 字节」与「语法错误」的差别就是这个产品本身。解法是
每条诊断在**产生时**声明一个 `Boundary`（`FullJs` / `ThirdBinding` / `Subset`），
`qjs2wasm` 按这个已声明的类别窄化——不是回头去匹配句子。后者正是
`WasmError::class()` 存在的理由所要杀掉的习惯。

## 5. 脸（public face）

```rust
pub struct Budget {
    pub limits: tinyvm::Limits,         // steps / pages / table elems / call depth / activation slots
    pub max_stdout_bytes: usize,
    pub max_bridge_result_bytes: usize,
}

pub type FleetBridgeFn = Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>;

pub enum Guest<'a> { Wasm(&'a [u8]), Qjs(&'a str) }

pub struct SlotId(u64);

pub struct Outcome {
    pub values: Vec<Value>,
    pub stdout: String,
    pub truncated_stdout: bool,
    pub steps: u64,
    pub peak_call_depth: usize,
    pub peak_activation_slots: usize,
}

pub struct Engine { /* budget + slots */ }

impl Engine {
    pub fn new() -> Self;
    pub fn with_budget(budget: Budget) -> Self;

    /// 装载 + 校验 + 绑门 + instantiate + run_start。不跑入口。
    pub fn spawn(&mut self, guest: Guest<'_>, bridge: Option<FleetBridgeFn>)
        -> Result<SlotId, QjswasmError>;

    /// 在已有槽上调一个导出。每次调用拿一份新的 max_steps 预算。
    pub fn call(&mut self, slot: SlotId, entry: &str, args: &[Value])
        -> Result<Outcome, QjswasmError>;

    pub fn run_once(&mut self, guest: Guest<'_>, bridge: Option<FleetBridgeFn>, args: &[Value])
        -> Result<Outcome, QjswasmError>;

    pub fn kill(&mut self, slot: SlotId);
    pub fn live_slots(&self) -> usize;
}

/// 只编不跑：`.qjs` → `.wasm` 字节。CLI `qjswasm build` 与 `check` 都走它。
pub fn compile_qjs(source: &str) -> Result<Vec<u8>, CompileError>;
```

`spawn` / `call` 分开，是因为 tinyvm 的 `Instance` 是**持久**的
（`invoke_by_name(&mut self)`，`last_steps()` 逐调用重置）：装一次、调多次、每次一份新
fuel。`eval_wasm` 那个一次性糖面做不到，所以走
`Module::from_bytes_with` + `bind_import_typed` + `instantiate`，不走 `eval_wasm`。

`compile_qjs` 单独暴露是刻意的：编译与执行分离，才能有「只编不跑」的 CI 门，也才能把
编译产物当普通 `.wasm` 交给任何一个 wasm 宿主验证（差分 oracle 的前提）。

## 6. 宿主门 ABI（模块名 `"agenterm"`）

```text
print(ptr: i32, len: i32)                                              -> ()
fleet_call(op_ptr: i32, op_len: i32, params_ptr: i32, params_len: i32) -> i32   // status
fleet_result_len()                                                     -> i32
fleet_result(dst_ptr: i32, dst_len: i32)                               -> i32   // 写入字节数，负=目标太小
```

`status`：`0` = Ok · `1` = Err（应用级错误，正常结果，不是崩溃）· `2` = NoBridge。
与 `agenterm-wasmcore` 状态码语义一致，guest 作者只学一套。

### 为什么两趟拷贝，而不是照抄 wasmcore 的六参数单次调用

`agenterm-wasmcore` 的 ABI 里，宿主拿到结果后**回调 guest 导出的 `wasmcore_alloc`** 要
一块 buffer 再写进去。那条路在 tinyvm 上走不通，理由是机制性的：

tinyvm 的宿主回调签名是

```rust
F: Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError> + 'static
```

回调**持着线性内存的 `&mut [u8]`**，而回调进 guest 需要
`Instance::invoke_by_name(&mut self)`。安全 Rust 里这两者不能同时成立——即宿主回调
内部**无法重入 guest**。这不是 tinyvm 的缺陷，是它「无 JIT、显式调用栈、上限在核」
那套设计的必然结果。

所以改成两趟：`fleet_call` 只回 status，结果字节暂存在槽的宿主侧 pending buffer；guest
自己问长度、自己分配、再让宿主拷进来。代价是每次桥调用多两次跨界；换来零重入、宿主
不要求 guest 导出分配器、与 tinyvm iOS 桥既有的「stable two-pass copy lengths」同一手法。

`(status, bytes)` 存在 `Rc<RefCell<Pending>>` 里，由该槽的四个闭包共享。**每槽一份**，
不跨槽。`fleet_call` 覆写它，`fleet_result` 读它。guest 不取回就丢弃，不是错误。

M3 之后 `.qjs` 侧的 `fleet.call(op, params)` 降级到这四个 import 上——在字符串能力
到位之前，`.qjs` 调不了带参宿主调用，这是 M0–M2 的已知边界，不是缺陷。

## 7. 编译器取舍

### 参考来源（遵 [PRD 14](../prd/PRD_02_14_research_provenance.md)）

- **ECMA-262 是语义权威**，前端正确性以规范为准。
- **QuickJS 是重点设计参考。** 逐子系统的挖掘清单见
  [PRD 36 §从 QuickJS 挖什么](../prd/PRD_02_36_agenterm_qjswasm.md#从-quickjs-挖什么按分期对照)——
  作用域/变量解析与闭包装箱是**极高**可移植（wasm locals 本就按索引寻址、帧上变量本就
  无法被捕获，是同一个问题）；值表示、串表示、shape、回收策略是高可移植；真正不通用的
  只有它的字节码指令集与派发循环。
  > 本文档 rev1 曾写「QuickJS 的价值集中在后端，用不上」——**该判断错误，已撤销**。
- **边界仍是 clean-room**：吸收设计与取舍理由，**不抄源码、注释、标识符、查找表、
  文档措辞**。写出来的必须是自己的 Rust。
- 测试向量独立构造，或取自规范条文示例并注明出处。

### 值表示

M2 之前只有 i32/i64，直接用 wasm 原生数值。M3 起需要统一值表示，**决定点在 M3 开工前，
不在本文档拍死**——先做一份对照实验，输入包括：

- QuickJS 的两套选择及其理由（64 位标记联合 vs 32 位 NaN-boxing），它为什么按位宽分岔；
- 本目标是 wasm32：指针天然是 i32，没有 64 位指针压力，也没有 GC 提案可用（tinyvm 核
  不吃），所以驱动 QuickJS 选 NaN-boxing 的那个约束在这里不成立；
- 候选：`(tag: i32, payload: i32/f64)` 双字 vs 单 f64 NaN-boxing。前者好调试、不与 f64
  语义纠缠；后者省内存、省拷贝。

rev1 在这里直接倾向了双字，**属于未做功课的手挥**，撤销该倾向，改为待实验。

### guest 侧运行时

M3 起每份产物都带一段编译器生成的运行时（分配器、串操作、后续的堆与属性查找）。
它也是 wasm，也吃 `Limits`，也算产物体积。**排期按 PRD 36 的成本表估，不按语法特性
个数估。**

内存回收：M3–M4 先只做 bump 分配 + 整体丢弃（脚本级生命周期，跑完即回收整块内存）。
这是**分期**不是天花板；到 M4 对象落地时按 QuickJS 的「引用计数 + 循环回收」做对照，
再定本产品的回收策略。

## 8. 接进 agenterm

- `src/script_backend.rs`：加 `ScriptBackend::Qjswasm`（`#[cfg(feature = "script-qjswasm")]`）；
  `from_entry_path` 增 `.qjs` → `Qjswasm`；`from_env` 增 `"qjswasm"`。
- `src/script_engine.rs`：加 `QjswasmEngineBackend` 实现 `ScriptEngineBackend`。
  `check` = `compile_qjs` 只编不跑（`.qjs`）或 `Module::from_bytes_with` 只装载校验
  （`.wasm`）；`execute` = `run_once`，`ScriptFleetBridgeFn` 原样当 `FleetBridgeFn`
  （同形状，零成本）。
- **`.js` / `.mjs` 路由不动**，继续走 `agenterm-qjs`，直到 PRD 36 的归档门三条全绿。
- **`.wasm` 默认路由不动**，仍先命中 `script-wasmcore`。

## 9. 验收树

「测试优先：先验收测再改脸。工人自报不算过。」

```text
M0 脊柱
├── 槽机制                                            [ ]
│   ├── spawn 一份 .wasm，call 导出，拿到返回值           [ ]
│   ├── 同一槽 call 两次，第二次拿到新鲜 fuel              [ ]
│   ├── kill 后 call 该槽 → 明确错误，不 panic            [ ]
│   └── run_once 不留活槽                               [ ]
├── 隔离（对抗性）                                     [ ]
│   ├── 两槽线性内存互不可见                             [ ]
│   ├── A 槽 trap 后 B 槽仍能正常 call                   [ ]
│   ├── A 槽超 max_steps 后 B 槽预算未被消耗              [ ]
│   └── 槽 A 的 bridge 不被槽 B 调到                      [ ]
├── 预算（对抗性）                                     [ ]
│   ├── 死循环 guest → max_steps 触发，宿主活着            [ ]
│   ├── memory.grow 超 max_memory_pages → 拒绝            [ ]
│   ├── 深递归 guest → max_call_depth 触发                [ ]
│   ├── print 超 max_stdout_bytes → 截断且标记             [ ]
│   └── bridge 回程超上限 → Err，不是截断                  [ ]
├── 宿主门                                            [ ]
│   ├── fleet_call → status 0 → 两趟取回结果               [ ]
│   ├── bridge 返回 Err → status 1 + 错误文本              [ ]
│   ├── bridge = None → status 2 + 固定诊断                [ ]
│   ├── fleet_result 目标太小 → 负数，不越界写              [ ]
│   ├── 未调 fleet_call 就 fleet_result → 长度 0           [ ]
│   └── op/params 指针越界 → trap 该槽，不读宿主内存         [ ]
├── 装载门                                            [ ]
│   ├── 非 `\0asm` 字节 → 拒绝                            [ ]
│   ├── 坏 wasm → 装载期拒绝，不进执行                      [ ]
│   └── check 不执行 start（副作用不发生）                  [ ]
└── 接线                                              [ ]
    ├── feature 关时根 crate 仍 build                     [ ]
    └── 扩展名路由 .wasm / .qjs / 其余                     [ ]

M1–M5 语言（每期同形状，此处只列门）
├── 每个新语法：一条「编译产物跑出预期结果」的测           [ ]
├── 每个未支持语法：一条「被拒绝且诊断说清能力边界」的测   [ ]
├── 编译产物过 tinyvm 严格装载门（不是只过自家 parser）    [ ]
└── M5：fleet.js 等价物端到端跑通                        [ ]
```

## 10. 工具链

2026-08-24：本机（darwin/aarch64）原先没有 Rust 工具链，已按 `rust-toolchain.toml`
装上 **rustup + 1.97.0-aarch64-apple-darwin**（minimal + clippy / rustfmt / rust-src）。
本机自此是开发主力机。

基线：`cargo check --workspace --all-targets --exclude agenterm-abi` 干净（15s，0 error）。
**必须 `--exclude agenterm-abi`**：它有一条故意的 `compile_error!`，要求
`--profile abi-release` / `abi-dev`，因为工作区默认 profile 是 `panic = "abort"`，
会静默产出没有 `catch_unwind` 围栏的库。这不是树坏了。

## 11. 非目标（本设计显式排除）

- JS 覆盖面不设人为上限（PRD 36：无原理排除，全是排期）。`eval` 走「宿主重编 + 同
  `Store` 新实例 + 跨实例链接」，tinyvm 已支持，属排期项不属排除项。
- 执行核**不生成机器码**——tinyvm 产品定义，不改。
- 任何 QuickJS C 库、`rquickjs`、C 依赖或构建期 C 工具链。
- JIT / AOT 到机器码 / copy-and-patch / 可执行内存。
- `wasm-encoder` 之类第三方 wasm 编码依赖（自己写，对严格装载门负责）——该纪律随
  编译器迁到 `tinyvm-qjs`，仍然成立。
- tinyvm `wasi-p1` 当插件面。
- 替换 `agenterm-wasmcore` 或改 `.wasm` 默认路由。
- 跨槽通信 / 共享内存 / 共享 table / global。
- 真 GC（M3–M4 只做 bump + 整体丢弃）。
- 在归档门三条全绿前动 `agenterm-qjs`。
