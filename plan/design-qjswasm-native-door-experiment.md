# qjswasm native door：动态能力斜率判决实验

Status: **active strategic experiment; bounded prototype implementation authorized**  
Owner: `prd/PRD_02_34_agenterm_dyn.md`  
Product posture: important bottom-layer work; evidence decides its product landing

## 0. 要裁决的问题

`agenterm-dyn` 当前用一门有界 S-expression 语言承载 native `dlcall`。新增系统能力是否必然继续扩充 Rust
特殊形式，还是可以由 qjswasm guest 的线性内存加一扇固定 native door 承载，使“新增一个能力”只增加 guest
程序或 host-fact 数据、Rust 生产代码增量为零？

本实验先裁决这条增长曲线。它不是搁置理由：S1–S5 的原型、测试和六格证据现在可以主动推进；
删除现有语言层与新增 CU verb 分别在斜率证据和 runtime-placement 法庭后紧接实施，避免先落一个不可达的表面。

## 1. 已知事实与证据等级

- **仓库内已证**：qjswasm/tinyvm 已提供 guest 执行与线性内存；`agenterm-dyn/src/hosts.rs` 持有六格
  OS×ISA host facts；`agenterm-dyn/src/exec.rs` 按已批准决定保留给未来 JIT 工具，核心产品仍 no-JIT。
- **仓库内已证**：现有 dyn 语言层约由 `eval.rs`、`parse.rs`、`sym.rs`、`value.rs` 组成，并被
  `Dyn` public API、native trampoline、集成测试和示例消费；它不是当前可直接删除的孤立代码。
- **真机实验转述，待本实验复验**：Apple arm64 上同一 `ioctl` request 经 variadic 声明成功、定参声明失败。
  因此 `invoke_unix_ioctl` 保留；本实验禁止用宽度修正冒充调用约定等价。
- **已丢原型的交接证据，不能当仓库绿证据**：曾有
  `agenterm.native_call(spec_ptr, spec_len, block_ptr, block_len) -> i32` 原型，并以新增 `.wat`、零 Rust
  改动运行 `getpid`。原型与 worktree 已不存在，所有结论必须从当前树复验。

## 2. 候选方案

### A — 保持现有 dyn 语言扩展

每个新能力继续经 parser/evaluator/native dispatch 扩展。优点是沿用已发布 API；风险是生产 Rust 与能力数一起增长，
且 caller-owned struct buffer 会继续推动语言内存原语。

### B — 固定 qjswasm native door

只新增一项 host import：

```text
agenterm.native_call(spec_ptr, spec_len, block_ptr, block_len) -> i32
```

guest 提供 `library|symbol|ret(arg,...)` 描述和位于自身线性内存的有界参数块；host 只做边界校验、符号解析、
签名分类和调用。无 C/libffi、无 runtime codegen、无汇编、无新跨平台语义封装。

首轮只接受能被六格明确证明的固定 ABI 子集。variadic、struct-by-value、未证明的 `f32` 形状一律 typed refusal；
Unix `ioctl` 继续走已有 variadic 特例，不纳入“通用门已覆盖”的宣传。

## 3. 事前判据

判据按顺序裁决，实验后不得改权重：

1. **正确性**：四个只读 guest 夹具在拥有 runtime runner 的 target 上与独立 oracle 一致；错误 library、symbol、
   signature、越界指针/长度、参数块尺寸均 typed refusal，且拒绝早于 native call。
2. **能力斜率（主判据）**：前三个能力完成后，再加入第四个形状已覆盖的只读能力；第四个能力的 Rust 生产代码
   diff 必须为 **0 行**，只允许新增 guest fixture/data/test registration。
3. **门面冻结**：qjswasm host import 数只允许本实验这一次 `7 → 8`；第四能力不得再次增门。
4. **有限桩表**：签名 grammar 先归约为 ABI register classes，再由生成器穷举固定 arity 上限。表规模必须由生成器
   输出和测试独立重算；不得直接复述丢失原型的“`3^arity`”而不列出三类分别是什么。
5. **no-JIT / 可交叉验证**：无 executable allocation、机器码生成、汇编或 C 构建依赖；两 MSVC targets、Linux target
   和本机 target 均能编译，native runtime 证据只归属于真正运行过的 host。
6. **资源边界**：spec、参数数、参数块、guest memory span、library/symbol 长度均有明确上限；所有加法与窄化 checked。
7. **体积账**：按 release/candidate 同口径记录 qjswasm/根产品增量；debug 或静态链接估算不能代替发布产物。

## 4. 判决树与 kill criterion

- 判据 1 任一失败且不能通过收紧支持集合修复：**B 判负**，维持 A，不扩充新语言内存原语。
- 判据 2 非零：**“固定门止住能力斜率”命题判负**；停止继续加能力，不用更多夹具掩盖。
- 判据 3 失败：B 判负。
- 判据 4 的桩/分发表随能力数增长，或 fixed ABI 子集仍需要 runtime codegen/libffi：B 判负。
- 判据 1–6 全过：B 成为替代底层，立即进入 §7 的消费者迁移与语言层收缩；`guest-run` 同时进入独立落点法庭。
- 任一 target 只能靠 `allow`、伪消费者或未运行产物宣称支持：实验无效。

## 5. 时间盒与实施切片

时间盒：两个工作日；到第四能力斜率结论即停。

```text
S1  签名 grammar + 参数块 schema + independent validator
 └─ S2  单一 native_call door + bounded caller
     ├─ S3  三个只读能力与负面边界矩阵
     └─ S4  第四能力（只加 guest/data）→ 主判据
         └─ S5  六格 compile + 可用 host runtime + release size ledger
```

建议首组只读能力从 `getpid`、`gettimeofday`、`uname` 等选择，但必须覆盖至少两种参数形状；选择本身不构成
预先定义 computer-use 能力目录。第四能力须在 S1 grammar 已覆盖，禁止为了让它通过而改 Rust。

## 6. 交付与验证

- 实验实现落在 qjswasm 的单一 native-door 模块、host import 注册和专属 tests/fixtures；不接 CU。
- 结果文件记录：source SHA、工具链、每 target 的 compile/runtime 证据、门面计数、生成桩数、release bytes、
  第四能力生产 Rust diff。
- 变异验证至少覆盖：边界 span、签名 kind、符号名、第四能力 Rust 零增量守卫。变异必须原地可逆并证明前后哈希一致。
- 会失败的进程/资源夹具必须由 `Drop` 自清理，并以一次故意红跑检查残留。

## 7. 后续迁移门（本实验不实施）

B 获资格后立即开迁移叶：

只读消费者审计已确认：仓内没有其它 crate、binary 或 task 链接 `agenterm-dyn`，但它的自身证据面很大，
所以迁移风险可控而绝非“四文件删除”。

1. 为旧面建立 keep / port / archive 表：`Dyn`/`Value`/`Symbol` public API、`native.rs`、约 190 个
   integration tests、22 个 module tests，以及 84 个含 S-expression/`dlcall` 的 examples；新门必须先接住仍成立的
   ABI refusal、Darwin probe、catalog/document、resource ownership 证据。
2. `hosts.rs` 当前完全独立，继续作为 OS×ISA 数据 owner；`exec.rs` 只依赖 `DynError::Exec`，按已定方向保留。
   迁移前明确是暂留 `DynError::Exec` 还是拆出 `ExecError`，不得顺手破坏 `CodeBuffer` API。
3. 保留 Unix `ioctl` variadic ABI、`mach_host_self` ownership refusal、exec W^X 与六格 host facts；语言测试减少必须由
   新门等价证据解释，不能靠 pass-count 下降冒充简化成功。
4. 删除旧门时同步处理 `libloading` 依赖归属、`crates/agenterm-dyn/README.md`、根 README、parked CI、
   `plan/goal-agenterm-dyn-macos.md` 与 reviews 的历史状态。
5. `plan/chassis-l1-surface.json` 当前点名 `crates/agenterm-dyn/src/native.rs`；先给边界测试增加 exact-path-exists
   断言，再迁到新门真实路径，防止清单假绿。
6. 同批更新 `prd/PRD_02_34_agenterm_dyn.md` 与 `plan/ARCHITECTURE.md`，不得留下两个 living native doors。

## 8. 明确非目标

- 不删除 `invoke_unix_ioctl`；不把定参调用冒充 variadic。
- 不恢复 `job-spawn --expiry detach`。
- 不引入 JIT、libffi、C shim、通用 C parser、struct-by-value 或权限/路径 allowlist。
- 不在本实验中新增 `guest-run`、CU verb、provider ABI 或默认 feature。
- 不提前翻译完整 computer-use 面；少量 helper 与性能热点须另有独立证据。
