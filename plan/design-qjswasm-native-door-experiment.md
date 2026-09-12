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

## 8. 结论回填（2026-09-12，中间审计；实验仍 active）

### 8.1 当前判决

**尚不能沿 §4 宣判 B 获资格，也没有触发 B 判负。** 当前树已经实现 S1–S4；S4
的真实代码和测试；所以“native door 首片不存在”是错误描述。但主判据 2 要求把“前三个能力完成”与
“只加入第四能力”分成两个可比较 source state，而 `35098c28 qjswasm: open a contained native call door`
在同一提交中加入 door 生产实现、前三个能力和第四个 `additions/getpagesize.wat`，没有留下可复算的前态。
因此现有目录守卫证明第四能力**现在只是一份额外 WAT**，却不能替代事前要求的 Rust 生产代码
before/after diff。因此 `edc59a29` 以 `getegid.wat` 建立了新的可比边际点：完整生产 Rust 树哈希在新增前后均为
`44241fd41b550705a6de8d0c6bf24caf3300243d2071b4e3ffff45aaf68d0e51`
。这通过了斜率门，但必须把改用第五能力的偏差留在账上；S5 仍阻止资格宣判。

判决树 trace：判据 1 尚缺各 target 的运行证据 → 不判；判据 2 的新可比边际点生产 Rust diff 为零 →
主斜率通过；判据 3 的一次 `7 → 8` 已由表与测试锁住、未见第四扇门 → 通过当前树审计；
判据 4 有固定表和独立计数，但没有生成器产物账 → 部分通过；**判据 5 的 target 编译账已补齐**
（`x86_64-pc-windows-msvc`、`aarch64-pc-windows-msvc` 的 clippy 与 Linux `x86_64` 的 zigbuild，
三者均 rc=0 且属**仅编译**，见 §8.2 与 `RESULTS.md` §F7–§F11），但 Windows/Linux 的 **runtime
仍未取得**，判据 7 的 release 账本也仍缺 → **不得进入 §7 迁移**。

### 8.2 S1–S5 与事前判据对账

| 切片 | 当前状态 | 当前树的直接证据 | 未完成的事前要求 |
|------|----------|------------------|------------------|
| S1 schema + validator | **已实现** | `b7ff748e` 新增 `src/native.rs` 与 `tests/native_door_schema.rs`；上限、checked span、exact block、typed error code、GP/F64 分类和独立 381-pattern 重算均有测试 | 无实现缺口；最终结果账本仍须记录该 SHA/测试 |
| S2 单一 door + bounded caller | **已实现** | `35098c28` 将唯一 `agenterm.native_call(i32,i32,i32,i32)->i32` 接入 opt-in Engine；`native_door.rs` 覆盖默认关闭、显式开启、声明发现和 budget/cancel；非宿主目标编译归属已由 S5 记录 | 尚缺 Windows/Linux runtime 归属 |
| S3 三能力 + 负面矩阵 | **本机实现完成** | Unix `getpid/getppid/getuid` 真调用并分别与进程 ID、父进程 ID、real UID 的独立 host oracle 精确比较；另有 `abs(i32)` 与 `cos(f64)` 精确 ABI family；schema 测试覆盖 spec/block/argument span 和尺寸，door 测试区分 library/symbol/signature/OOB | 证据只归属于实际运行测试的 Unix host；逐 target runtime 资格仍由 S5 补齐 |
| S4 第四能力 | **斜率已由第五点补测通过** | 原第四点仍由 `getpagesize.wat` 证明；`edc59a29` 只新增 `getegid.wat` 与测试，独立 `id -g` oracle 精确相等，door 仍只有一个 native import | 原第四点没有独立基线，因此按 §8.4 记录规格偏差；第五点是替代的可比边际点，不改写原历史 |
| S5 qualification | **部分完成（编译面已补齐，运行面未齐）** | `research/qjswasm-native-door/RESULTS.md` 在 `93a46fcd` 记录本机 schema 9/9 与 runtime 10/10 及 49/381 复算；**追加段 §F7–§F11 在 `2158fffb` 记录**双 MSVC clippy（`-D warnings`，rc=0，qjswasm 零诊断）与 Linux `x86_64` `cargo zigbuild --all-targets`（rc=0，仅依赖 `agenterm-platform` 的 warning）**均已完成且均为仅编译**；`3afbdc1d` 另证 public artifact 监督 | **仍未取得 Windows/Linux runtime**（三格只到编译/静态检查，未在任何非宿主目标运行）；缺同口径 release qjswasm/根产品增量 |

### 8.3 判据账（数字均为当前树结构审计，不冒充目标运行）

| 判据 | 结果 | 数值/条件与执行状态 |
|------|------|---------------------|
| 1 正确性 | **目标资格未完成** | 3 个首组 Unix fixture + 1 个 addition 在本机 macOS 运行通过；`getpid/getppid/getuid` 均与独立 host oracle 精确相等。[实测·本机真机执行；其它 target 未运行] |
| 2 能力斜率（主） | **通过（第五点替代测量）** | 基线 `b6755b0f` 到 `edc59a29` 新增 `getegid`；`crates/agenterm-qjswasm/src/**/*.rs` 聚合 SHA-256 前后均为 `44241fd41b550705a6de8d0c6bf24caf3300243d2071b4e3ffff45aaf68d0e51`，生产 Rust diff = 0。[实测·本机真机执行] |
| 3 门面冻结 | **当前通过** | raw host `SIGNATURES` 为 8 项；compiler 默认声明为 5 项，native opt-in 的集合差恰好只追加 `native_call` 1 项，得到 6 项。8 与 6 是 inventory 与 compiler-visible 两层，不是漂移；后续 fixture 没有新增 import。[结构审计；本机专属测试通过] |
| 4 有限桩表 | **部分通过** | 参数类 2、返回类 3、arity `0..=6`，独立枚举为 `3 × Σ(2^0..2^6) = 381`；`22b79c0f` 让生产与测试共用唯一 exact-family admission，单测独立遍历 `7 × 7 = 49` 并钉住拒绝矩阵。实现以宏列出 0–6 arity，尚无规格所说的生成器结果文件。[结构审计；本机独立枚举测试通过] |
| 5 no-JIT / cross | **部分通过（编译面已补齐，运行面未齐）** | 代码使用固定 Rust `extern C` stubs + `libloading`，未见 executable allocation、机器码生成、汇编或 C build；**双 MSVC clippy 与 Linux x86_64 zigbuild 的仅编译账已在当前 follow-up source state 复验**（`RESULTS.md` §F7–§F11，测于 `2158fffb`：x64/aarch64 rc=0 且 qjswasm 零诊断，Linux rc=0 且仅依赖有 warning），本机 macOS 有 runtime；**Windows/Linux runtime 未取得**。[结构审计 + `RESULTS.md` 实测（仅编译）] |
| 6 资源边界 | **当前通过 schema 审计** | spec 1024 B、library 512 B、symbol 255 B、arity 6、block exact-size；span/addition/narrowing 有 typed checked paths 和测试。[结构审计；本机专属测试通过] |
| 7 体积账 | **未完成** | L1/L2/L3 均未测定；没有同 boundary/tool/build/target-execution 四元口径的 release before/after bytes。 |

### 8.4 与规格不符及不能外推之处

1. 时间盒写成“两个工作日”，而技能要求钉在具体判据；真正停止点仍是“第四能力斜率出数”。本审计不
   事后修改判据，只把缺失的可比两点列为 blocker。
2. S1 与 S2 分成了 `b7ff748e` / `35098c28`，但 S3 与原 S4 没有再分提交，破坏了原第四点的测量设计。实验没有伪造旧基线，而是用 `edc59a29` 的第五能力建立新边际点；这是明确的规格偏差。
3. 规格要求生成器穷举固定 arity；当前是固定宏桩 + 两个独立 cardinality 函数/测试，没有生成器产物。
4. `3afbdc1d script: supervise native wasm artifacts` 增加 public `.wasm` 的 explicit convention、bounded
   protocol 与 `WorkerSupervisor` crash/timeout 隔离。它是 Script Runtime 的投递/监督证据，不是 S1/S3
   的替代品，也不补齐 native door 的 target runtime、斜率或 release-size 判据。
5. `research/qjswasm-native-door/RESULTS.md` 已建立，但 release L1/L2/L3 仍未测定；按判决性实验纪律不得标“已判决”。

诚实条款：本次没有为了让 B 看起来胜出而改判据或把结构推断改写成真机实测。当前结果有且只有一种
合规读法：**实现已前进，资格实验仍 active；没有证据支持删除/弱化 dyn。**

### 8.5 可复跑证据与剩余 qualification

当前专属测试（从仓库根运行）：

```text
cargo test -p agenterm-qjswasm --test native_door_schema
cargo test -p agenterm-qjswasm --test native_door
cargo test --test script_native_artifact_supervisor
```

要结束实验，须在结果账本中补齐：

1. 对 `x86_64-pc-windows-msvc`、`aarch64-pc-windows-msvc`、一个 Linux target 与本机 target 记录精确
   source SHA 和 compile 命令；每个有 runner 的 cell 跑 native fixture，未运行的产物只标“仅编译”。Windows
   至少用 `windows_get_current_process_id.wat`，Unix 用 owning fixtures；不得用 public artifact 协议测试代跑。
   **当前进度**：`x86_64-pc-windows-msvc`、`aarch64-pc-windows-msvc` 与 Linux `x86_64` 的 **compile 面已记录**
   （`RESULTS.md` §F7–§F11：精确 SHA `2158fffb` + 工具链 + 命令 + rc/warning，均标仅编译），本机 Unix runtime 已记录；
   **仍未完成**：这三个非宿主 cell 的 **native fixture 运行**（runner 存在与否须逐 cell 说明），以及 release 体积账。
2. 补 release before/after 账：分别报告 L1 机制、L2 机制+OS 接缝、L3 整个投递足迹；每个数字附
   boundary/tool/build/target-execution 四元口径。缺可比 baseline 时写“未测定”，不得跨 profile 相减。
3. 写入独立 `research/.../RESULTS.md`，包含 exact SHA、工具链、门面计数、49/381 独立复算、runtime
   attribution、release bytes、偏差和复跑命令；完成前 §7 迁移门保持关闭。

### 8.6 判据 7 的 artifact boundary 表（可执行测量口径；**不改变判据权重，release 仍未测定**）

以下每一行都对应可由仓库构建入口重建的 artifact。字节工具沿用 `scripts/qjs/stage-build.qjs`
的 `rh.metadata(path).len` 与 package 链的 `rh.sha256_file`，不把工作区里来源不明的旧 `dist/` 文件当基线。

| 层 | artifact（repo-relative pattern） | profile / target | 构建入口 | 字节工具 | before/after source identity 要求 | 能说明什么 | **不能**说明什么 |
|---|---|---|---|---|---|---|---|
| **L1 机制** | `target/<lane>/release/deps/libagenterm_qjswasm-<cargo-hash>.rlib`（哈希文件名，按唯一 pattern 解析） | `release` / 同一 host target | `cargo build -p agenterm-qjswasm --release` | bytes + SHA-256 | before/after 各自在同 toolchain/profile/target 的空 lane 重建 | qjswasm crate archive 增量 | 最终链接贡献与交付足迹 |
| **L2 最终链接体** | 隔离 staging 下的 `dist/<lane>/agenterm`，其来源必须是同一次 build 的 release client artifact | build 所用的同一 release profile / target | `build`，同时声明 `CARGO_TARGET_DIR=target/<lane>` 与 `AGENTERM_BUILD_DIST_DIR=dist/<lane>` | bytes + SHA-256 | before/after 使用相同 build task、profile、target 与 stage 步骤 | dead-strip 后根产品净增量，包含 native door 与目标 OS 链接接缝 | 不能把差值中的某段字节单独归给 qjswasm；`abi-dev` 动态库不是本边界，禁止混入 |
| **L3 交付包** | 隔离 staging 下的 `agenterm-<version>-<target>-unsigned-preview.zip`、provenance、SHA-256 与 SBOM | Candidate 使用的同一 profile / target | `package-release-qualified` 的非发布资格路径 | archive bytes + SHA-256 + receipt | before/after 各留 exact source identity 与 artifact-manifest receipt | 实际交付足迹及其源码绑定 | 机制归因 |

**逐层缺口（不得伪造文件）**
1. L1 没有稳定文件名或独立预算，只能在空 lane 中要求 pattern **唯一命中**；零个或多个候选都使测量无效。
2. L2 的 OS 接缝不单独 materialize；只报告整个最终链接体的 before/after 差值，不制造接缝子文件。
3. L3 已有 provenance 字段可绑定 source commit 与 artifact manifest；仍须为 before/after 各自产生 receipt。
4. `aedfdf96` 提供 target 与 dist 的 repo-local 单层 lane，但尚未完成真实隔离 build；在该黑盒门通过前，三层继续标**未测定**。

**四元口径怎么填（每行都一样）**：`boundary` = 上表该层 artifact；`tool` = `rustc -V`/`cargo -V`（+`cargo xwin`/`zig` 版本）；`build` = 该行"构建入口"逐字命令；`target-execution` = 该 artifact 是否在目标上**运行过**（未运行一律标**仅编译**）。

**不得外推**：本表只命名**测量口径**。L1/L2/L3 的 release 数字**仍未测定**；任何"包/可执行大小"的数字在未按本表取得前**不得**写进任何判决。

## 9. 明确非目标

- 不删除 `invoke_unix_ioctl`；不把定参调用冒充 variadic。
- 不恢复 `job-spawn --expiry detach`。
- 不引入 JIT、libffi、C shim、通用 C parser、struct-by-value 或权限/路径 allowlist。
- 不在本实验中新增 `guest-run`、CU verb、provider ABI 或默认 feature。
- 不提前翻译完整 computer-use 面；少量 helper 与性能热点须另有独立证据。
