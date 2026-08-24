# `agenterm-qjs` 归档门第三条：CLI 面逐动词判决

| 字段 | 值 |
|------|-----|
| **文档** | PRD 36 归档门第三条的交付物：`agenterm-qjs` 每一个 CLI 子命令与每一份 manifest schema，在 `agenterm-qjswasm` 上**有没有**对应面、**应该是什么形状**、**不提供的理由** |
| 日期 | 2026-08-25 |
| 状态 | 判决稿 rev1。门**未关**——本文件是关门需要的那份声明，不是关门动作本身 |
| **产品真理** | [`prd/PRD_02_36_agenterm_qjswasm.md`](../prd/PRD_02_36_agenterm_qjswasm.md)。归档门三条以该文件为准；本文件只回答第三条 |
| 关联 | [`plan/design-agenterm-qjswasm.md`](design-agenterm-qjswasm.md)、[`plan/design-qjs-module-imports.md`](design-qjs-module-imports.md)、[`crates/agenterm-qjswasm/README.md`](../crates/agenterm-qjswasm/README.md)、[`docs/agenterm-qjs-cheatsheet.md`](../docs/agenterm-qjs-cheatsheet.md) |
| 范围声明 | 只写判决与证据。不改任何 `.rs`，不改 PRD 36，不执行归档 |

PRD 36 §归档门第三条原文：

> `qjs` CLI 子命令的 `check` / `pack` / `qualify` / `check-many` 在新引擎上有对应面，
> 或明确声明哪些不再提供、为什么。

---

## 0. 一句话

门点名的四条：`check` **必须提供**，`check-many` **必须提供**，`pack`
**形状必然不同**（产物从「qjsc 字节码指纹 + 另存一份重解析用的源码」变成「一份可直接
执行的 `.wasm`」——是换东西，不是移植），`qualify` **形状必然不同**（随 `pack` 走，
回执字段是超集）。

门没点名但归档会一起带走的八条，判决见 §3 完整表。其中最值得单独说的是
`pack_module`（多文件 module pack）：**可以不提供**——它存在的那条约束在 qjswasm 上
不成立，而且今天的编译器连 `import` 关键字都在编译期就拒（E5）。

外加一条本次实测发现、必须写进判决的事实：**`agenterm-qjs` 的 `bytecode_hash` 根本
不是可复现指纹**——同一份源码编到两个不同的 `--dir`，`bytecode_hash` 不同（§4.1）。
所以"把 pack 移植过去"移植的是一个已经坏掉的契约。这一条加强了"形状必然不同"的判决，
不是削弱它。

---

## 1. 方法：所有判决都跑过

判决不是读源码估的。下面每条证据都是真跑出来的：base rev `696e1cf`，toolchain
1.97.0，`agenterm` 二进制以 `--features script-qjs,script-qjswasm` 编出。本文件引用到的
源文件（`crates/agenterm-qjs/`、`crates/agenterm-qjswasm/src/`、
`crates/agenterm-script-common/`、`crates/agenterm-{lua,sql}/`、`src/bin/`、
`src/operations.rs`、`tests/script_cli_verb_parity.rs`）在跑证据时**全部未被改动**，
行号可直接核对。唯一例外是 `src/script_engine.rs`——它正被另一条并行车道改（+7/-2），
所以对它的行号引用可能有小幅漂移，认构造名不认行号。

| 编号 | 命令 / 探针 | 结果 |
|------|------------|------|
| E1 | `agenterm qjswasm --help` | `AgenTerm GUI argument error: --help cannot be combined with other options`，exit 2。**qjswasm 今天没有任何 CLI 面** |
| E2 | `agenterm qjs pack build hello.js --dir packA` 与 `--dir packB`，同一份源码 | `source_hash` 相同；`bytecode_hash` **不同**（`bd9a9694…` vs `7074b217…`）；两份 `pack.qjsc` 各 274 字节，第 3 字节起就不同 |
| E3 | `agenterm_qjswasm::compile_qjs(src)` 连调两次 | 1627 字节，两次 **byte-identical**，`\0asm` 魔数正确，`validate_wasm` 通过 |
| E4 | `Engine::run_once(Guest::Qjs(src))` vs `Engine::run_once(Guest::Wasm(compile_qjs(src)))`，同一份 `function f(){return 42;} f();` | Qjs 路径 → `[Js(Number(42.0))]`；Wasm 路径 → `[I32(1), I64(4631107791820423168)]`（V1 pair 的原始两字，未解码） |
| E5 | `compile_qjs("import { v } from './x.qjs';\nv")` | `ERR this engine does not support the \`import\` keyword yet (at byte 0)` |
| E6 | 篡改 `compile_qjs` 产物中间一个字节后 `validate_wasm` | `Err(Load("validation: type mismatch"))` |
| E7 | `cargo test --features script-lua,script-qjs,script-sql --test script_cli_verb_parity` | 10 passed。**去掉 `script-qjs` 这 10 条全部编不过/失败** |
| E8 | `OPERATION_CATALOG.len()` 跑出来 | **77**（其中 76 条 `script_surface` 以 `fleet.` 开头）。派单书里写的 46 已过时 |

E2 / E3 / E4 / E5 / E6 是本文件三条主要判决的地基。

---

## 2. 现状：`agenterm-qjs` 的 CLI 面，与谁在用它

### 2.1 真实动词面（12 个，不是门里写的 4 个）

读自 `crates/agenterm-qjs/src/cli.rs:97-196` 的 `dispatch`，以 `agenterm qjs --help`
实跑核对：

```
check  eval  run  hash  pack build  pack load  qualify
check-many  corpus-scan  run-smoke  task  version
```

门只点了 `check` / `pack` / `qualify` / `check-many` 四个名字，但归档动作会同时带走
另外八个。本文件把十二个全部判决，否则"CLI 面有对应"这句话在归档那天会漏掉一半。

### 2.2 谁在调用（实测清单，非推测）

| 调用方 | 位置 | 依赖的是什么 | 归档后会怎样 |
|--------|------|-------------|-------------|
| 根二进制的 `qjs` 子命令 | `src/bin/agenterm.rs:16-24`（`ENGINE_SUBCOMMANDS`）、`:308-309`（`agenterm_qjs::cli::run`） | 整个 CLI | 子命令消失 |
| CLI 动词平价测试 | `tests/script_cli_verb_parity.rs:179-193`（`const QJS`）、`:209-211`（`engines()`）、`:636-671`（qjs `task` stub 专测） | `check` / `check-many` / `version` / 未知动词退出码 / `task` stub | **10 条测试全红**（E7） |
| lib 级平价测试 | `tests/script_engine_parity.rs:2`（`#![cfg(all(…script-qjs…))]`）、`:72-73`、`:102`、`:362-363` | `check_many::{read_manifest, run_check_many}` | 整个测试文件不再编译进构建（cfg 关掉），平价覆盖静默变窄 |
| 执行面平价测试 | `tests/script_engine_exec_parity.rs:7`（同样的 cfg 三连） | `AGENTERM_SCRIPT_BACKEND=qjs` 的执行路径 | 同上，38 处 qjs 引用整体失效 |
| Windows 转发测试 | `tests/agenterm_com_forwarding.rs:53`（`("qjs", "agenterm-qjs ")`） | `agenterm qjs version` 的 stdout 前缀 | **Windows 上直接红**。注意：该文件 `#![cfg(windows)]` 且**没有** `feature = "script-qjs"` 门，所以它今天在 default feature 的 Windows 构建上就已经是红的——归档只是让它红得更明显 |
| 速查手册 | `docs/agenterm-qjs-cheatsheet.md` §5/§6，`AGENTS.md:210` | 全部动词 + pack 格式 | 文档失真 |
| README | `README.md:64-65`、`:179-180` | 「`agenterm rh\|lua\|qjs\|sql` 子命令」这句话 | 文档失真 |

**没有**在用它的：`agenterm.tasks.json`（`qjs` 出现 0 次，实测）、`check.sh` /
`rh-check.sh` / `lint.sh` / `release.sh`（`qjs` 出现 0 次）、`.github/`（无匹配）、
`scripts/`（只有 `scripts/qjs/lib/fleet.js` 这一份库，没有任务脚本，与 PRD 36 §归档门
记录一致）。

**结论：没有任何门、任务、CI 依赖 qjs CLI。唯一会断的是测试与文档。**

### 2.3 qjswasm 今天的 CLI 面：没有

E1 实测：`agenterm qjswasm --help` 落进 GUI 参数解析器。`ENGINE_SUBCOMMANDS`
（`src/bin/agenterm.rs:16-24`）只有 `rh` / `lua` / `qjs` / `sql` 四个 token，没有
`qjswasm`。qjswasm 今天只有一个 `ScriptEngineBackend`
（`src/script_engine.rs:559-634`），只有 `check` 与 `execute` 两个方法，
经 `AGENTERM_SCRIPT_BACKEND=qjswasm` 到达。

顺带一条**文档已经跑在实现前面**：`plan/design-agenterm-qjswasm.md:206` 写着
「CLI `qjswasm build` 与 `check` 都走它」。那条 CLI 不存在。归档前应改成"计划中"
或按本文件 §9 落地。（该文件不在本轮独占域，只报不改。）

---

## 3. 逐动词判决表

判决口径：

- **必须提供** = 归档后缺了它，会有人（测试、门、文档承诺的用法）真的够不着；
- **可以不提供** = 缺了它没人断，且有理由不补；
- **形状必然不同** = 功能要有，但把 qjs 的实现移植过来是错的，对应物是另一样东西。

| # | 动词 | qjs 上做什么 | 谁在用 | 在 qjswasm 上意味着什么 | 判决 |
|---|------|-------------|--------|------------------------|------|
| 1 | `check` | `Module::declare` 解析 + 链接 module 图，不执行（`check.rs:66-81`）。带 `--project-root` 时走 `check_with_project_validation` 连整个 import 图一起验 | `tests/script_cli_verb_parity.rs`、`script_engine_parity.rs`；backend 面已有 | `compile_qjs(src)` 只编不跑（`lib.rs:87-89`）；`.wasm` 走 `validate_wasm`（`lib.rs:443-454`）。backend 侧**已经实现**（`src/script_engine.rs:575-593`），缺的只是 CLI 壳 | **必须提供** |
| 2 | `check-many` | 共享驱动 + qjs 的 `kind` 串 + 适配闭包（`check_many.rs:23-49`，60 行） | 两个平价测试文件；`fixtures/check-many.json` | 同一套 `agenterm_script_common::check_many`，`kind` 改 `agenterm-qjswasm-check-manifest`，闭包调 `compile_qjs`。sql 的适配器就是这个形状，60 行 | **必须提供** |
| 3 | `pack build` | qjsc 字节码指纹 + 存源码 + manifest；**执行时重解析源码，不加载字节码**（`pack.rs:1-21`） | 无生产调用方；`run-smoke` 与 `qualify` 内部调 | qjswasm 的产物**本身就是可执行的 `.wasm`**，没有"指纹 vs 执行体"这条裂缝。见 §4 | **形状必然不同** |
| 4 | `pack load` | 读 manifest → 校两个 hash → 重解析源码执行 | `run-smoke` 委托它（`cli.rs:168-177`） | 读 `.wasm` → `validate_wasm_with(budget)` → 进槽。**但今天缺一件**：`Guest` 只有 `Wasm` / `Qjs` 两个变体（`lib.rs:206-213`），没有"这份 wasm 是 `.qjs` 编出来的"这条路。见 §4.3 | **形状必然不同**（且需要一处 API 增补） |
| 5 | `pack build`（module 模式） | 复制整个静态 import 图进 pack 目录（`pack_module.rs`，486 行） | 无生产调用方 | 见 §5：约束不转移，`import` 今天还在编译期被拒（E5） | **可以不提供** |
| 6 | `qualify` | build + load + 跑 entry → receipt（`qualify.rs:36-52`） | 无生产调用方 | 语义完全成立且更强：编译 → 校验 → 跑一次 → 回执带**真正可复现**的 `wasm_hash`（E3）+ `steps` / `peak_call_depth` 这两个 qjs 根本没有的可测量数字 | **形状必然不同**（随 `pack` 走；回执字段必然不同） |
| 7 | `corpus-scan` | 递归扫 `.js`/`.mjs`，逐个 `check`（`corpus_scan.rs:14-20`，共享驱动） | 无 | 换成 `["qjs", "wasm"]` 两个扩展名，闭包换 `compile_qjs` / `validate_wasm`。20 行 | **必须提供**（顺手，成本近零；和 `check-many` 同一份 scaffolding） |
| 8 | `eval` | 跑一遍，打印 completion value | 无 | `Engine::run_once(Guest::Qjs(src), None, "main", &[])`。backend 已经这么做（`script_engine.rs:621-627`） | **必须提供** |
| 9 | `run` | 同 `eval`，外加把 `-- <args>` 接到 `__host.args_len` / `__host.arg`（`cli.rs:207-262`） | 无 | **门里没有这两件**。qjswasm 的门只有四件：`print` / `fleet_call` / `fleet_result_len` / `fleet_result`（`host.rs:71-76` 的 `SIGNATURES`）。要有 `run -- args` 就得往门上加，门是版本化产品契约，不能顺手加 | **可以不提供**（今天）。要提供必须先过 PRD 36 §宿主门 ABI 那一关 |
| 10 | `hash` | `sha256(source bytes)` | 无 | 成立，但在 qjswasm 上真正有意义的是 **`sha256(compile_qjs(source))`**——产物哈希（E3 证明它稳定），源码哈希只是文件哈希，`shasum` 就够 | **形状必然不同**（应是产物哈希，不是源码哈希） |
| 11 | `run-smoke` | 委托 `pack load`（`cli.rs:168-177`） | 无 | 随 `pack load` 走 | 随 #4 |
| 12 | `task` | 诚实 stub，exit 2 + 重定向文案（`cli.rs:178-195`） | `tests/script_cli_verb_parity.rs:636-671` 专测它 | 同样是 stub；真正的任务分发在 `src/script_worker.rs` | **必须提供**（就是一个 stub，但退出码与文案是被测试锁住的契约） |
| 13 | `version` | 打印 `agenterm-qjs <ver>` | `tests/agenterm_com_forwarding.rs:53` 校前缀 | 打印 `agenterm-qjswasm <ver>` | **必须提供** |

---

## 4. 专题一：`pack` —— 为什么不是移植，是换东西

### 4.1 先说一件坏消息：qjs 的 `bytecode_hash` 不可复现

`pack.rs:1-21` 的模块文档写着：

> `bytecode_hash` in the manifest is a genuine reproducibility fingerprint
> (see `compile.rs`) that a pack's *build* step can be verified against

**这句话是假的。** E2 实测：同一份 `hello.js`，编到 `packA` 与 `packB`：

```
source_hash   1ab1e0b1…   1ab1e0b1…    相同
bytecode_hash bd9a9694…   7074b217…    不同
pack.qjsc     274 bytes   274 bytes    第 3 字节起不同
```

原因在 `pack.rs:78-81`：`build_pack_dir` 把 `label` 取成
`dir.join("entry.js").display().to_string()`，也就是**输出目录的绝对路径**，再交给
`compile_qjs(source, label)`（`compile.rs:27-40`），而 `Module::write` 把这个 label
原样写进了字节码——`xxd pack.qjsc` 头部就能直接看见
`<绝对输出目录>/packA/entry.js` 这串路径。

`compile.rs` 的三个单元测试（`:51-71`）全部把 `label` 钉死成 `"a.js"`，所以它们永远
测不到这件事。同一份 `qualify` 与 `pack build` 对同一份源码给出不同 `bytecode_hash`，
也是这个原因（实测：`2b5b9656…` vs `7b91a156…`）。

这条不必修——`agenterm-qjs` 在门绿之前"原样保留、不腐化"，改它是腐化。但它决定了
判决：**没有一个值得移植的可复现契约在那里。**

对照组：`agenterm-lua` 没有这个毛病，`compile_lua(source)`（`compile.rs:6-15`）签名
里根本没有 label，`dump(true)` 还剥掉调试信息。这是 qjs 独有的缺陷，不是共享
scaffolding 的问题。

### 4.2 qjswasm 上诚实的对应物

qjs pack 的形状是被 rquickjs 逼出来的：`Module::write` 只能序列化 ES module，而
`entry()` 约定是全局脚本，所以"存的字节码"和"跑的东西"从第一天起就是两样东西，字节码
沦为一个（还坏了的）指纹。

qjswasm 没有这条裂缝：

- `compile_qjs(source) -> Vec<u8>` 产出的**就是**执行体（E3：1627 字节标准 wasm，
  `validate_wasm` 通过）；
- 它是**确定性**的：两次编译 byte-identical，签名里没有 label，没有路径可渗进去（E3）；
- 它自带完整性校验：改一个字节，`validate_wasm` 就报 `Load("validation: type mismatch")`
  （E6）。装载期校验本来就在核里，不需要 manifest 里再补一个 hash 才能发现篡改。

所以对应物应该是：

```
qjswasm build <file.qjs> --out <file.wasm>      # 只编不跑，产物即执行体
qjswasm run   <file.wasm>                        # 直接跑，装载期校验兜底
```

**不是** `pack build` / `pack load`。一个目录 + manifest + 两个 hash + 一份重复存放
的源码，在这里全是多余的：产物只有一份，它自己就是自己的凭证。

manifest 仍然值得留一份，但**理由完全不同**——不是为了校验完整性（核已经做了），
而是为了记两件核看不见的事：

1. **调用约定**（见 §4.3）；
2. **溯源**：`source_hash`、编译器 rev（`tinyvm-qjs` 的 git rev，今天钉在
   `df8decd`）、`Budget`。产物哈希本身可复现，所以"同样的源码 + 同样的编译器 rev
   → 同样的 wasm"是一句**可以真的验证**的话，而不是 qjs 那句写在文档里但跑一次就破的话。

### 4.3 一处必须先补的 API：`.wasm` 文件不记得自己是不是 `.qjs` 编的

E4 是本次最要紧的发现：同一个程序，两条路给出两种结果。

```
run_once(Guest::Qjs(src))                    -> [Js(Number(42.0))]
run_once(Guest::Wasm(compile_qjs(src)))      -> [I32(1), I64(4631107791820423168)]
```

（`4631107791820423168` = `42.0` 的 f64 位型；`I32(1)` 是 V1 pair 的 tag。）

这不是 bug，是**已写进代码的刻意设计**。`slot.rs:24-32`（`Convention` 的文档，枚举本体在 `:30-38`）：

> Not a property of the wasm bytes -- both conventions are ordinary wasm --
> but of where the bytes came from, so it is recorded at load time and can
> never be re-derived by guessing at a signature.

`Engine::spawn`（`lib.rs:351-371`）从 `Guest` 变体推 `Convention`：`Guest::Wasm` →
`Convention::Wasm`（原始 wasm 数值），`Guest::Qjs` → `Convention::JsV1`（解码成
`JsValue`）。

**后果**：任何"把 `.qjs` 编成 `.wasm` 落盘，之后再加载执行"的动作——也就是 `pack`
这件事的全部内容——今天都会把 JsV1 约定丢掉，返回值形状与 `eval` / `run` 直接跑源码
**不一致**。

所以 `pack`（无论叫什么名字）在 qjswasm 上要成立，前提是先有一条把约定带回来的路。
两个候选：

- **A（推荐）**：`Guest` 加一个变体，例如 `Guest::CompiledQjs(&[u8])`，显式声明
  "这份 wasm 说 JsV1"。改动最小，与 `slot.rs` 已写下的"约定随字节走、绝不靠猜签名"
  这条纪律一致。
- **B**：约定写进 manifest，由 CLI 读出来再选变体。等价，但把一条本该在类型里的
  信息挪进了 JSON——一份被手改过的 manifest 就能让解码错位。A 更硬。

**这是关门第三条的一个真实前置条件，不是实现细节。** 在它落地之前，qjswasm 上的
`pack` 只能是"编译后立刻在同一进程里跑"（也就是 `qualify` 的形状），不能是"落盘、
之后再加载"。

> **已落地（2026-08-25），选了方案 A。** `Guest::CompiledQjs(&'a [u8])` 已进
> `crates/agenterm-qjswasm/src/lib.rs`，`Engine::spawn` 从它推 `Convention::JsV1`。
> 判决理由与 §4.3 写的一致，另有一条独立证据支持同一个改动：接缝对抗审查发现
> `read_guest_string` 的五条拒绝分支（指针不是地址 / 头越界 / 体越界 / 非 UTF-8 /
> 没有线性内存）在此之前**没有任何可达的调用者**，因为 V1 约定只能由受信任的编译器
> 产物触发。两条独立的路要同一个变体，这是它该有的形状的强证据。
>
> 验收测试按 §9 第 0 步的原话写：
> `crates/agenterm-qjswasm/tests/qjs_guest.rs::
> a_compiled_artifact_reloaded_gives_the_same_value_as_its_source`——
> 五种返回值上 `run_once(Guest::Qjs(src))` 与
> `run_once(Guest::CompiledQjs(&compile_qjs(src)))` 逐一相等，并**另外**锁住
> `Guest::Wasm(&同一份字节)` 仍然交出原始 V1 pair，因为「约定是记下来的，不是猜出来的」
> 这条纪律要同时挡住两个方向的漂移。
>
> 所以 §9 的第 0 步已完成；§10 决定 1 已定：走。

---

## 5. 专题二：`pack_module` —— 约束不转移，功能也不跟着走

### 5.1 它为什么存在

`pack_module.rs:10-19` 写得很清楚，而且是**实测**过才写的：

> empirically confirmed before writing this file that rquickjs's
> `Module::write()` does **not** embed an imported module's bytecode/content
> into the entry module's serialized bytecode … So a single-blob "bytecode
> captures the whole graph" pack format isn't something rquickjs 0.12's
> public API can produce; copying source files is the correct shape

一句话：**rquickjs 序列化不了整张图，所以只好把源文件抄一遍。** 486 行里，
`discover_import_graph`（借 `RecordingLoader` 蹭 QuickJS 自己的链接过程）、
`QjsModulePackFile` / `QjsModulePackManifest`、`graph_hash`、逐文件校验——全部是这条
约束的产物。

### 5.2 约束在 qjswasm 上不存在

qjswasm 的编译产物是**一份**标准 `.wasm`。PRD 36 §编译面写得明确：运行时也一起编进
那份 wasm，产物仍是一份普通 `.wasm`，过同一道装载门。多模块在 wasm 这一层的答案是
**链接**（一份产物），不是**打包一堆源文件**。所以：

- 「多文件 pack 目录」这个形状在这里没有存在理由；
- `graph_hash` 没有对象——只有一份产物，产物哈希就是全部；
- 「pack 目录即 project root」这个巧思解决的是"load 时还要不要知道原 `--project-root`"，
  而一份自足的 `.wasm` 根本不问这个问题。

### 5.3 而且今天连入口都没有

E5 实测：

```
compile_qjs("import { v } from './x.qjs';\nv")
  -> ERR this engine does not support the `import` keyword yet (at byte 0)
```

编译器今天在词法/语法层就拒 `import`。**在 `.qjs` 长出多文件之前，讨论多文件 pack 的
形状是空转。**

### 5.4 判决

**`pack_module` 可以不提供。** 三条理由，逐条可证伪：

1. 它的成因（rquickjs 序列化不了图，`pack_module.rs:10-19` 实测记录）在 qjswasm 上不
   成立——产物是一份可自足执行的 wasm；
2. 今天 `.qjs` 没有 `import`（E5），没有图可打；
3. 它零生产调用方（§2.2 全仓搜索）。

将来 `.qjs` 长出模块时，正确的答案是**编译器把图链成一份 wasm**（工作在上游
`tinyvm-qjs`），不是在本仓重建一个抄源文件的打包器。这是 PRD 36 分层原则的直接推论，
不是本文件的新主张。

---

## 6. Manifest schema 逐条判决

| schema | 定义处 | 字段 | 在 qjswasm 上 | 判决 |
|--------|--------|------|--------------|------|
| `agenterm.qjs-pack-manifest/v1` | `manifest.rs:33-40` | `schema` `version` `source_hash` `bytecode_hash` `bytecode_file` `entry_file` | `bytecode_hash` 的对应物（产物哈希）**是**可复现的（E3），但已不再是"指纹"而是**产物本身的身份**；`entry_file`（重解析用的源码副本）没有对应物——产物就是执行体；要新增 `convention` 与 `compiler_rev` | **形状必然不同**。同名 `v2` 是误导，应另起 `agenterm.qjswasm-artifact/v1` |
| `agenterm.qjs-module-pack-manifest/v1` | `pack_module.rs:41`、`:134-152` | `schema` `version` `entry_file` `files[]` `graph_hash` | 无对象（§5） | **可以不提供** |
| `agenterm.qjs-qualification/v1` | `qualify.rs:13-23` | `schema` `version` `source_hash` `bytecode_hash` `entry_value` `stdout` | 语义成立且更强：可加 `steps` / `peak_call_depth` / `peak_activation_slots`（`Outcome` 已经带，E4 输出里可见），这三个数是 qjs 拿不出来的 | **形状必然不同**（字段是超集，schema 名必须换） |
| `agenterm.qjs-module-qualification/v1` | `pack_module.rs:320-328` | 同上 + `graph_hash` `file_count` | 随 `pack_module` 走 | **可以不提供** |
| `agenterm-qjs-check-manifest`（check-many 的 `kind`） | `check_many.rs:23` | `schema_version` `kind` `files[]`（形状在 `agenterm_script_common::check_many`） | **形状完全相同**，只换 `kind` 串。这是四引擎共享契约，rh/lua/sql 都用同一个 | **必须提供**，`kind` = `agenterm-qjswasm-check-manifest` |

一条口径：**只有 check-many 的 manifest 是跨引擎契约，其余三份是 qjs 私有的。**
私有的那三份没有"兼容"义务，换 schema 名是对的，硬套旧名字才是错的。

---

## 7. 共享 scaffolding：拿掉 qjs 会不会碰坏 lua / sql / rh

**不会。实测：`agenterm-script-common` 对 qjs 有零处代码级耦合。**

```
grep -rc "qjs" crates/agenterm-script-common/
  cli.rs 8   pack_support.rs 8   check_many.rs 7   lib.rs 5
  hex.rs 2   test_support.rs 2   corpus_scan.rs 1   Cargo.toml 1
```

34 处全部是**文档注释与 description 字符串**，没有一处是 `use` / 类型 / 常量 /
`cfg`。逐条核过。

依赖方向也是干净的（`grep agenterm-script-common crates/*/Cargo.toml`）：

```
agenterm-lua  → agenterm-script-common
agenterm-qjs  → agenterm-script-common
agenterm-rh   → agenterm-script-common
agenterm-sql  → agenterm-script-common
```

四条平行的边，没有 qjs → 别人、别人 → qjs 的边。删掉 `agenterm-qjs` 这一个顶点，
另外三条边纹丝不动。

三处**会失真但不会断**的地方（都是注释，都在别人的域里，只报不改）：

- `crates/agenterm-script-common/src/lib.rs:3-13`：「rh、lua、qjs 各自独立收敛到同一
  形状」这段历史叙述——归档后应改成 rh / lua / sql / qjswasm；
- `crates/agenterm-script-common/src/pack_support.rs:7-11`、`:22-40`：几处 error
  prefix 参数化的理由，明确写着"因为 qjs 的某某测试逐字断言了这段文案"。qjs 走了以后
  这些参数**仍然被 lua 用着**，不能顺手合并——注释要改，签名不能动；
- `crates/agenterm-script-common/src/test_support.rs:19-32`：「qjs 用 `.mjs`」作为
  `good_b` 第二扩展名存在的举例。qjswasm 接手后 `good_b` 变成 `.wasm`，例子照样成立。

**唯一需要真动的一处**：`agenterm-sql` 的模块文档大量以 qjs 为参照系
（`src/cli.rs:5`、`:19`、`:26-27`、`:137`、`:170-171`、`src/error.rs:1-2`、`:13`、
`src/check_many.rs:8`、`:61`）。这些是 sql 的文件，不在本轮域内，但归档 PR 必须把参照
系换掉，否则 sql 的退出码约定会指向一个已归档的 crate。

---

## 8. 归档那天会红的东西（完整清单）

| 会红的 | 为什么 | 修法 |
|--------|--------|------|
| `tests/script_cli_verb_parity.rs` 10 条（E7） | `const QJS`（:179）在 `engines()`（:209）里；`qjs_task_stub_exits_nonzero_with_redirect_message`（:645）直接跑 `agenterm qjs task` | 把 `QJS` 换成 `QJSWASM`，`engines()` 保持 4 元组；`task` stub 专测跟着换引擎名 |
| `tests/script_engine_parity.rs` 整个文件 | `#![cfg(all(…script-qjs…))]`（:2）；`:72-73` 直接调 `agenterm_qjs::check_many` | cfg 换 `script-qjswasm`；调用换新 crate。**前提是 §9 步骤 2 先落地**，否则无 `check_many` 可调 |
| `tests/script_engine_exec_parity.rs` 整个文件 | 同样的 cfg 三连（:7），38 处 qjs 引用 | 同上 |
| `tests/agenterm_com_forwarding.rs:53` | 校 `agenterm qjs version` stdout 前缀 `"agenterm-qjs "` | 换 `("qjswasm", "agenterm-qjswasm ")`。**注意它今天在 default-feature 的 Windows 构建上就已经红**（`#![cfg(windows)]` 但没有 feature 门）——归档 PR 顺手把这个既有洞一起补上 |
| `src/bin/agenterm.rs:20-21`、`:308-309` | `ENGINE_SUBCOMMANDS` 与 dispatch 里的 `qjs` 臂 | 换成 `qjswasm` |
| `Cargo.toml:2`（members）、`:30`、`:35`、`:45` | workspace 成员、`script-qjs` feature、`check-cfg` 名单、optional dep | 四处一起摘 |
| `src/script_engine.rs:148`、`:154`、`:328-331` 等 | `#[cfg(feature = "script-qjs")]` 的 backend | 摘 backend |
| `src/script_worker.rs:660` | `#[cfg(all(not(test), feature = "script-qjs"))]` | 同上 |
| `docs/agenterm-qjs-cheatsheet.md` 全篇、`AGENTS.md:210` | 手册整篇讲 qjs | 换成 qjswasm 手册，或移进 `docs/archive/` |
| `README.md:64-65`、`:179-180` | 「`agenterm rh\|lua\|qjs\|sql` 子命令」 | 改成 `rh\|lua\|qjswasm\|sql` |

外加一条**与归档无关但顺手该修的假话**：`docs/agenterm-qjs-cheatsheet.md:143` 写着
「`agenterm-qjs` also exists as a standalone binary with the same verbs」。根
`Cargo.toml:14-27` 只声明三个 `[[bin]]`（`agenterm`、`agenterm-com`、`agenterm-cc`），
`README.md:64-65` 自己也说独立二进制已在 2026-08-09 退役。这一句今天就是错的。

---

## 9. 关掉第三条门的最短诚实路径

第三条门的原文允许两条出路：**有对应面**，或**明确声明缺口**。本文件已经把"声明"
写完了——§3 表格给了十三条判决，§5 给了 `pack_module` 的三条不提供理由，§6 给了
schema 判决。**门的"声明"那一半，本文件即为交付物。**

"有对应面"那一半的最短路径，按依赖排序：

**第 0 步（前置，不可跳）—— 已完成 2026-08-25。** `Guest::CompiledQjs`（§4.3 方案 A）。
没有它，任何"编译落盘再加载"的动词返回值形状都和直接跑不一致，`pack` / `qualify`
全部建在沙上。验收测试已写并绿：
`tests/qjs_guest.rs::a_compiled_artifact_reloaded_gives_the_same_value_as_its_source`。
（写下本步时这条测试是红的——那正是它可证伪的意思。）

**第 1 步**：`crates/agenterm-qjswasm` 加 `agenterm-script-common` 依赖 + `check.rs`
/ `check_many.rs` / `corpus_scan.rs` 三个薄适配器。照 `agenterm-sql` 抄形状：
`check_many.rs` 约 60 行（`crates/agenterm-sql/src/check_many.rs:28-58`），
`corpus_scan.rs` 约 20 行（同 crate `:9-21`）。`kind` =
`agenterm-qjswasm-check-manifest`，扩展名 `["qjs", "wasm"]`。
纪律核对：script-common 的依赖是 serde / serde_json / sha2 / walkdir / tempfile，
**全纯 Rust，无 C**，不违反 PRD 36 §纪律第一条。
验收：`tests/script_engine_parity.rs` 的 cfg 换成 `script-qjswasm` 后整个文件绿。

**第 2 步**：`crates/agenterm-qjswasm/src/cli.rs` + `src/bin/agenterm.rs` 的
`ENGINE_SUBCOMMANDS` 加 `qjswasm`。动词按 §3 判决落：
`version` `check` `eval` `run`（不带 `-- args`）`build` `check-many` `corpus-scan`
`qualify` `task`(stub)。退出码照 qjs 已经对齐好的 `Parse`/`Check` → 1、`Usage` → 2
（`crates/agenterm-qjs/src/cli.rs:14-33` 那段文档就是现成的规格）。
验收：`tests/script_cli_verb_parity.rs` 的 `const QJS` 换成 `QJSWASM` 后 10 条绿。

**第 3 步**：写下"不提供"的三条到 crate README，让下一个人不必读本文件也知道
`pack` / `pack load` / `run -- args` 为什么不在那儿。

**第 4 步**：才是归档动作本身（另行派单，且要三条门全绿——本文件只管第三条）。

估算：第 0 步是唯一有设计成分的（一个枚举变体 + 一条 `spawn` 分支 + 一条测试）；
第 1、2 步是抄形状，sql 已经证明了这条路走得通。

---

## 10. 留给整合者的四个决定

本文件不替这四件事下结论，它们跨了独占文件域：

1. ~~**`Guest::CompiledQjs` 走不走。**~~ **已定：走**（2026-08-25，见 §4.3 的补记）。
   变体已落地并有验收测试。选它而不是"干脆不提供落盘再加载的动词"，是因为第二条独立
   的理由指向同一个变体：没有它，接缝那五条拒绝分支不可达、不可测。
   §3 表格第 4 行 `pack load` 的判决维持「形状必然不同」。

2. **`agenterm-qjs` 的 `bytecode_hash` 缺陷（§4.1）要不要留痕。** 门绿前不改代码是
   对的，但 `pack.rs:1-21` 那句「genuine reproducibility fingerprint」是假话，下一个
   照它判断的人会判错。建议在 PRD 36 §归档门里记一句，与 wasmcore README 那两句假话
   同等对待（PRD 36 已有那条附带修正的先例）。

3. **派单书里的「46 个 `OperationSpec`」已过时**：实测 77（E8，
   `OPERATION_CATALOG.len()` 跑出来的，`src/operations.rs:525` 起，44 条字面量 +
   33 条经 `nullary_ui_action` 构造），其中 76 条 `script_surface` 以 `fleet.` 开头。
   与本门无直接关系，但既然核过就记下来。

4. ~~**`plan/design-agenterm-qjswasm.md:206` 说 CLI `qjswasm build` 存在**，实测不存在
   （E1）。~~ **已改成"计划中"**（2026-08-25），并在那句里指回本文件 §9 的落地顺序。
   复核：`src/bin/agenterm.rs:16` 的 `ENGINE_SUBCOMMANDS` 是 `rh` / `lua`(feature) /
   `qjs`(feature) / `sql`(feature)——没有 `qjswasm`。
