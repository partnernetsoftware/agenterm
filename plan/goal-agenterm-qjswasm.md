# /goal — `agenterm-qjswasm` M0 脊柱

> 用法：把 `--- GOAL ---` 之后的内容整段发给 agent（或 `/goal` 加载本文件）。
> 产品口径以 [PRD 36](../prd/PRD_02_36_agenterm_qjswasm.md) 为准，**不要重开辩论**
> 「要不要自研编译器」「完整 JS 行不行」——已定，见该文档各撤销条。
> 实现级设计：[`design-agenterm-qjswasm.md`](design-agenterm-qjswasm.md)。

--- GOAL ---

在仓库根执行 **`agenterm-qjswasm` M0「脊柱」**：新建 crate，依赖 tinyvm，让 agenterm
能装载并有预算地执行 `.wasm`，能把最小 `.qjs` 编译成 `.wasm` 走通同一条管线，宿主门四件
可用，槽间隔离与预算有**对抗性**证据。

**M0 不含**：完整编译器（M1–M5）、字符串/对象/闭包、归档 `agenterm-qjs`、改 `.wasm`
默认路由。

## 不变量（开工必须复述）

| 条 | 内容 |
|----|------|
| 核 | tinyvm，git + rev 钉死，**不 vendor**。不依赖 `tinyvm-qjs` |
| 纯度 | 纯 Rust。**无** rquickjs、无 QuickJS C、无 C 依赖、无构建期 C 工具链 |
| 编码 | 自己写 wasm 编码器，**不引** `wasm-encoder` 等 crate（产物要过 tinyvm 严格装载门） |
| 上限 | 预算用 tinyvm `Limits`，不另造限流 |
| 门 | 只有 `agenterm.*` 四件，**不**把 WASI `fd_*` 做成第二扇 OS 面 |
| 默认关 | `script-qjswasm` feature，`default = []` 不动 |
| 断言纪律 | 任何「做不到」必须先读代码给依据。不得凭印象断言上限 |
| 上游缺口 | 撞到 tinyvm 真实缺口 → **去 tinyvm 仓改**（已授权），不在本 crate 绕 |

## 任务树（DAG）

```text
S0 骨架（primary，串行前置，独占热文件）
│   Cargo.toml(root) · .cargo/config.toml · crates/agenterm-qjswasm/{Cargo.toml,src/lib.rs}
│   产出：公共脸定死 + crate 能 cargo check 通过（内部 todo!() 桩）
│
├── A 槽机制        并行 · 独占 src/slot.rs        + tests/wasm_slot.rs
├── B 宿主门        并行 · 独占 src/host.rs        + tests/host_door.rs
├── C 最小编译器    并行 · 独占 src/lower/**       + tests/qjs_m0.rs
├── D 对抗性素材    并行 · 独占 tests/fixtures.rs  + tests/{isolation,budget}.rs
├── E 接线          并行 · 独占 src/script_backend.rs · src/script_engine.rs（根 crate）
└── F crate 文档    并行 · 独占 crates/agenterm-qjswasm/README.md
                    │
                    ▼
        G 集成（primary，串行）：lib.rs 组装 A+B+C，解冲突
                    ▼
        H 验收（primary，串行）：fmt → clippy → test → doc-redact → 基线
```

**并行规则（AGENTS.md 并行纪律）**

- 每个 lane **独占**其文件域，活动期间不得碰他人文件。`src/lib.rs`、根 `Cargo.toml`
  是热文件，**只有 primary 能改**。
- 每个 lane 用**自己的** `CARGO_TARGET_DIR`（如 `/tmp/.../qjswasm-lane-a`），
  **禁止**多个 cargo 争同一 target 目录。
- lane 交回时留**未暂存**改动 + 报告：改了哪些文件、跑了什么命令、结果、遗留假设。
  primary 逐份审阅后才 stage。
- 最终 fmt / clippy / test 由 primary 在集成树上**串行**跑一遍。

## 各 lane 契约

### S0 骨架（primary，前置）

产出公共脸，其余 lane 全部对着它写：

```rust
pub struct Budget { pub limits: tinyvm::Limits,
                    pub max_stdout_bytes: usize,
                    pub max_bridge_result_bytes: usize }
pub enum Value { I32(i32), I64(i64), F32(f32), F64(f64) }
pub enum Guest<'a> { Wasm(&'a [u8]), Qjs(&'a str) }
pub struct SlotId(u64);
pub struct Outcome { pub values: Vec<Value>, pub stdout: String,
                     pub truncated_stdout: bool, pub steps: u64,
                     pub peak_call_depth: usize, pub peak_activation_slots: usize }
pub type FleetBridgeFn = Arc<dyn Fn(&str,&str) -> Result<String,String> + Send + Sync>;
pub enum QjswasmError { Compile(CompileError), Load(..), Trap(..),
                        Budget(..), Door(..), NoSuchSlot(..) }   // 五类可分辨
pub struct Engine { .. }
impl Engine { new / with_budget / spawn / call / run_once / kill / live_slots }
pub fn compile_qjs(source: &str) -> Result<Vec<u8>, CompileError>;
```

也负责：根 `Cargo.toml` 加 workspace member + `script-qjswasm` feature +
`unexpected_cfgs` 白名单补名；`.cargo/config.toml` 加 `[net] git-fetch-with-cli = true`
（**已实测必需**：私有仓 git dep 走 cargo 内置 libgit2 会 401）。

### A 槽机制 — `src/slot.rs`

`Module::from_bytes_with(bytes, limits)` → 绑门（调 B 的 installer）→ `instantiate()`
→ 持久 `Instance`。`call` 用 `invoke_by_name`，每次读回 `last_steps()` /
`last_peak_call_depth()` / `last_peak_activation_slots()`。`kill` 回收，之后 `call`
该槽给 `NoSuchSlot` 而**不是 panic**。`run_once` = spawn+call+kill。

验收：spawn/call 拿到返回值 · 同槽两次 call 各拿新鲜 fuel · kill 后 call 明确报错 ·
run_once 不留活槽。

### B 宿主门 — `src/host.rs`

四件，模块名 `"agenterm"`：

```text
print(ptr,len) -> ()
fleet_call(op_ptr,op_len,params_ptr,params_len) -> i32   // 0=Ok 1=Err 2=NoBridge
fleet_result_len() -> i32
fleet_result(dst_ptr,dst_len) -> i32                     // 写入字节数，负=目标太小
```

**两趟拷贝**，不照抄 wasmcore 的六参数单次调用——tinyvm 宿主回调签名是
`Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError>`，持着内存 `&mut`，**无法重入
guest**，所以宿主不能回调 guest 的分配器。pending buffer 每槽一份
（`Rc<RefCell<Pending>>`，四个闭包共享）。

验收：status 0/1/2 三条 · 两趟取回 · 目标太小回负数且不越界写 · 未调 fleet_call 就
fleet_result 长度 0 · op/params 指针越界 trap 该槽且不读宿主内存 · stdout 超上限截断
且 `truncated_stdout = true` · bridge 回程超上限报 `Err`（**不截断**）。

### C 最小编译器 — `src/lower/**`

`compile_qjs` 的 M0 版：整数字面量、`+ - * / %`、一元负号、括号、`$0`/`$1`… 取本次调用
参数。产出导出名 `main` 的标准 `.wasm`。**自己写编码器**（段、LEB128、指令）。

这是 M1–M5 的种子，所以结构要能长：`lex` / `parse` / `lower` 分开，别写成一个
递归下降直接吐字节的函数。诊断走 `diag`，文案必须是「本引擎尚不支持 X」而**不是**
含糊的"语法错误"。

验收：`1+2`→3 · `$0*2` args=[21]→42 · 运算优先级与结合性 · 除零/取模零的行为明确 ·
不支持的语法被拒且诊断说清能力边界 · **产物过 tinyvm 装载门**（不是只过自家 parser）。

### D 对抗性素材 — `tests/fixtures.rs` + `tests/{isolation,budget}.rs`

用 `wat` crate（**dev-dependency 限定**，tinyvm 自己的测试也这么用）写真实坏客人：
死循环、深递归、越界指针、`memory.grow` 炸弹。然后写隔离与预算测试。

验收：死循环 → `max_steps` 触发且宿主活着 · 深递归 → `max_call_depth` 触发且不吃原生栈 ·
`memory.grow` 超限被拒 · 两槽线性内存互不可见 · A 槽 trap 后 B 槽仍能 call ·
A 槽超预算后 B 槽预算未被消耗 · 槽 A 的 bridge 不被槽 B 调到。

### E 接线 — 根 crate 两文件

`ScriptBackend::Qjswasm`（cfg 门）+ `from_entry_path` 加 `.qjs` + `from_env` 加
`"qjswasm"`；`QjswasmEngineBackend` 实现 `ScriptEngineBackend`（`check` 只编不跑 /
只装载校验，`execute` 走 `run_once`，`ScriptFleetBridgeFn` 原样当 `FleetBridgeFn`）。

**不动** `.js` / `.mjs` 路由（仍走 `agenterm-qjs`），**不动** `.wasm` 默认路由
（仍先命中 `script-wasmcore`）。

验收：feature 关时根 crate 仍 build · 扩展名路由三态 · 与既有 backend 的契约测同形状。

### F crate 文档 — `README.md`

产品句、脸、宿主门 ABI（含两趟拷贝的理由）、M0 边界、怎么跑测试。指回 PRD 36。
**不得**暗示支持 JavaScript——说清 M0 只有整数表达式。

## 验收命令（primary 串行跑）

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all
cargo clippy -p agenterm-qjswasm --all-targets -- -D warnings
cargo test  -p agenterm-qjswasm
cargo check --workspace --all-targets --exclude agenterm-abi   # 基线，见下
./scripts/doc-redact-check.sh <每份改过的 md>
```

**`--exclude agenterm-abi` 是必需的**：它有一条故意的 `compile_error!`，要求
`--profile abi-release` / `abi-dev`（工作区默认 `panic = "abort"` 会静默产出无
`catch_unwind` 围栏的库）。这不是树坏了。

## 完成定义

M0 完成 = 上述命令全绿 + PRD 36 「证据门」一节替换为实测数字（测试数、命令、宿主），
格式对齐 PRD 34。**工人自报不算过**——没有命令输出就不算。

--- END GOAL ---
