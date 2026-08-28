# `agenterm-wasmcore` 归档门（第一条）：能力差异的诚实清单

| 字段 | 值 |
|------|-----|
| **文档** | PRD 36「归档 `agenterm-wasmcore` 的门」第 1 条的交付物：逐条列出 wasmcore 能而 qjswasm 不能的事，每条标 **要补** / **有意不补** |
| 日期 | 2026-08-25 |
| 状态 | **已结案 2026-08-28**：crate 已归档。第 1 条门判绿；第 2 条**没有按原设想执行**——`.wasm` 默认路由**不切到 qjswasm**，改为落空 + 点名诊断，理由在 PRD 36 的路由表下方；第 3 条数字在 §5，已留档为 tinyvm「原生降级」轨的输入。本文件自此是**历史证据**，不再描述现状 |
| **产品真理** | [`prd/PRD_02_36_agenterm_qjswasm.md`](../prd/PRD_02_36_agenterm_qjswasm.md)。门的定义以该文件为准，本文件只填内容 |
| 关联 | [`plan/design-agenterm-qjswasm.md`](design-agenterm-qjswasm.md)、[`crates/agenterm-wasmcore/README.md`](../crates/agenterm-wasmcore/README.md)、`src/script_engine.rs`、`crates/agenterm-qjswasm/src/host.rs` |
| 方法 | 每条差异要么给 `file:line`，要么给**真跑出来的输出**。读源码得出的结论标「读码」，跑出来的标「实测」。§2 的表整张是实测 |

---

## 1. 先纠正门自己引用的一句话

PRD 36 写「wasmcore 提供完整 WASI p1（`fd_*` / `_start` / `proc_exit`）」。

这句话**对了一半，另一半会把归档决策带偏**：

- **导入面确实是完整的。** `crates/agenterm-wasmcore/src/lib.rs:317` 调
  `p1::add_to_linker_sync`，它由 `wiggle::from_witx!` 从 wasmtime-wasi 自带的
  `witx/p1/wasi_snapshot_preview1.witx` 生成，那份 witx 有 **46** 个 `@interface func`
  （实测：`grep -c "@interface func (export"` = 46）。所以任何 `wasm32-wasip1` 客人
  都能**链接成功**，一个导入都不缺。
- **拿到的能力远不是完整 POSIX。** 能力由 `WasiCtx` 决定，而本 crate 建的是一个刻意
  的空壳（`lib.rs:322-325`）：

  ```rust
  let wasi = WasiCtxBuilder::new()
      .stdout(stdout_pipe.clone())
      .inherit_stderr()
      .build_p1();
  ```

  没有 `.args()`、没有 `.envs()`、没有 `.preopened_dir()`、没有 `.inherit_stdin()`。

**所以真正的差异比门里假设的小得多。** 下面这张表不是读 spec 得来的，是把一个真的
`wasm32-wasip1` 探针客人喂给 `WasmCoreHost::run_module` 跑出来的。

## 2. wasmcore 客人实际拿到什么（实测）

探针：一个真编译的 `wasm32-wasip1` Rust 程序（`rustc --edition 2024 --target
wasm32-wasip1 -O`，2.1 MB，带 std），逐个调 WASI 原始导入并把 errno 打出来；宿主是一个
只依赖 `agenterm-wasmcore` 的临时驱动，走 `WasmCoreHost::run_module`。

跑的机器：macOS aarch64（Darwin 25.5.0），rustc 1.97.0，wasmtime 47.0.3。**与 README
「AOT precompilation」那组数字不是同一台机**（那组是 Windows x86_64），两处数字不要混用。

探针程序本身是临时件，**没有进仓**——这是本文件的一个已知弱点：数字可信（下面是原样
粘贴的输出），但复现要重写探针。执行归档门的人应当把它落成一个受跟踪的 fixture，
放在 `crates/agenterm-wasmcore/guests/` 旁边，形状照 `guests/fleet_guest.rs`
（同样是测试时真跑一次 `rustc --target wasm32-wasip1`，仓里不放 `.wasm` 二进制）。

原样输出：

```text
args_sizes_get = errno=0 argc=0 buf=0
environ_sizes_get = errno=0 count=0 buf=0
std::env::args() = []
std::env::vars() count = 0
clock_time_get(realtime) = errno=0 ns=1787592053220916000
clock_time_get(monotonic) = errno=0 ns=678042
clock_time_get(process_cputime) = errno=8 ns=0
clock_res_get(realtime) = errno=0 res=1000
SystemTime::now() = Ok(1787592053)
random_get = errno=0 bytes=[13, 82, 92, 237, 165, 182, 253, 66]
fd_fdstat_get(0) = errno=0 filetype=0 rights_base=0x2
fd_fdstat_get(1) = errno=0 filetype=0 rights_base=0x40
fd_fdstat_get(2) = errno=0 filetype=0 rights_base=0x40
fd_fdstat_get(3) = errno=8 filetype=0 rights_base=0x0
fd_prestat_get(3) = errno=8
fd_prestat_get(4) = errno=8
fd_prestat_get(5) = errno=8
fd_filestat_get(stdout) = errno=0
fd_write(stderr) = errno=0 n=18
fd_read(stdin) = errno=0 n=0
path_open(fd3, /etc/hosts) = errno=8 fd=0
std::fs::read(/etc/hosts) = Err(Os { code: 44, kind: NotFound, ... })
std::fs::write(/tmp/agenterm-probe) = Err(Os { code: 44, kind: NotFound, ... })
std::fs::read_dir(/) = Err(Os { code: 44, kind: NotFound, ... })
sched_yield = errno=0
sock_shutdown(1) = errno=57
proc_raise(2) = errno=58
thread::sleep(5ms) = 7
std::thread::spawn = Err(Os { code: 58, kind: Unsupported, ... })
fleet_call = status=0 payload={"op":"probe.op","params":{"a":1}}
recursion_depth_100k = 0
[host] guest exit(7)
```

（`stderr-from-guest` 那一行没出现在上面，因为它**没进捕获缓冲**——它直接打到了宿主
进程的真 stderr 上。这本身就是一条能力，见 §3 第 6 条。）

一句话读法：**wasmcore 客人拿到的是 stdio + 时钟 + 熵 + `sched_yield`/`poll_oneoff`，
再无其他。** 没有文件系统、没有 argv、没有环境变量、没有 socket、没有线程。

## 3. 逐条差异：wasmcore 能而 qjswasm 不能

qjswasm 的门是四件（`crates/agenterm-qjswasm/src/host.rs:70-75` 的 `SIGNATURES` 表就是
完整名单，模块名写死 `"agenterm"`，`host.rs:54`）：

```text
print(ptr, len)                                    -> ()
fleet_call(op_ptr, op_len, params_ptr, params_len) -> i32
fleet_result_len()                                 -> i32
fleet_result(dst_ptr, dst_len)                     -> i32
```

### 3.1 `fd_write(1)` / `fd_write(2)`：标准输出与标准错误

wasmcore：stdout 进 256 KiB 的 `MemoryOutputPipe`（`lib.rs:84`），stderr 走
`inherit_stderr()` 直通宿主进程。
qjswasm：只有 `agenterm.print`，进每槽的 pending 缓冲，受 `max_stdout_bytes` 约束。

**stdout：有意不补。** 语义已经等价，而且 qjswasm 那一侧更好：超限时 wasmcore 是
`println!` 失败 → Rust panic → abort → 整次运行返回 `Err`，**连已经攒下的输出一起丢**
（实测：一个打 ~300 KiB 的客人，最后拿到的是 `wasm trap: unreachable`，捕获内容为零）；
qjswasm 是留前缀 + 置 `truncated_stdout` 标志（`host.rs:95-103` 的 `write_stdout`）。把 `fd_write`
搬进门只会把这个更差的形状一起搬过来。

**stderr：有意不补，但要在迁移说明里写清。** 这是 wasmcore 真有而 qjswasm 真没有的一条
通道：客人能往宿主进程的 stderr 上写任意字节，不计入任何预算、不进 `Outcome`、宿主也
拦不住。它对客人作者是「调试很方便」，对产品是「一个槽能污染宿主的诊断流且不受限」。
「一个坏槽只能弄死自己」是 PRD 36 §隔离与预算的原话，直通 stderr 与那句话冲突。

### 3.2 `fd_read(0)`：标准输入

wasmcore：`fd_read(0)` 返回 `errno=0, n=0`——因为没 `inherit_stdin()`，拿到的是空
pipe，立刻 EOF。
qjswasm：门里没有输入通道。

**有意不补。** 差异是「一个永远返回 EOF 的读口」对「没有读口」。没有任何客人能靠它拿到
数据。要给客人喂输入，正路是 `fleet_call` 的 `params_json`，那条路已经通。

### 3.3 `path_open` / `fd_readdir` / `fd_prestat_*`：文件系统

wasmcore：导入齐全，**但 preopen 表是空的**，fd 3 起全是 `errno=8 (BADF)`，
`std::fs` 的读、写、列目录全部 `NotFound`（实测，见 §2）。
qjswasm：门里没有文件系统。

**有意不补。** 这是清单里最容易被误判成「大缺口」的一条，实测结果是**两边都没有文件
系统**。差的只是「有导入符号但全部 BADF」对「连符号都没有」。PRD 36 纪律「不得把 WASI
`fd_*` 做成第二扇 OS 面」在这里根本不需要动用——没有东西要放弃。

需要文件访问的脚本，正路是 `fleet_call` 打到宿主的 operation（名单由
`src/operations.rs` 的 `OperationSpec` 声明），由宿主决定给不给、给到哪。

### 3.4 `args_get` / `environ_get`：argv 与环境变量

wasmcore：调用成功，`argc=0`、`env count=0`（实测）。
qjswasm：门里没有。

**有意不补。** 同 3.3：wasmcore 这两个也是空的。而且 qjswasm 侧有一条更合适的路——
`Engine::call(slot, entry, args)` 直接给入口传参（`crates/agenterm-qjswasm/src/lib.rs:396`（`Engine::call`）），
比伪造一份 argv 干净。今天产品路径没用它（`src/script_engine.rs:622` / `:626` 固定传
`"main"` 与 `&[]`），那是接线没做，不是能力没有。

### 3.5 `clock_time_get` / `clock_res_get` / `random_get`

wasmcore：realtime 与 monotonic 都是真值，`res=1000`ns；`random_get` 给真熵；
`process_cputime` 是 `errno=8`（实测）。
qjswasm：门里没有时钟，没有随机源。

**这一条是「要补」，但补法不是开 WASI。**

理由：时钟与熵是**真实脚本会用**的东西（超时、重试退避、生成 id、给日志打时间戳），
且和「第二扇 OS 面」的顾虑没关系——它们不给客人任何对宿主状态的写权。但补的形状必须
是门里的具名 import，不是 `wasi_snapshot_preview1`：

```text
now_ms()                 -> i64     // 或 monotonic_ns() -> i64
random_bytes(ptr, len)   -> i32
```

两点要在下单时定死，否则会把确定性执行统计（`steps` / `peak_call_depth` /
`peak_activation_slots`，PRD 36 §隔离与预算要求的）变成不可重放：时钟与熵是这个引擎里
**唯二的非确定性来源**，必须能按槽关掉或钉住（注入固定时刻 / 固定种子）。在那个开关
设计出来之前，这条是「要补，未下单」，不是「今天欠着」。

另有一条**今天就能绕过去的**：`fleet_call` 打一个宿主 operation 拿时间，代价是一次跨界
往返。所以它不阻塞归档门，只是体验差。

### 3.6 `poll_oneoff` / `sched_yield`：让客人自己阻塞

wasmcore：`thread::sleep(5ms)` 真的睡了 7ms（实测）——客人能占住那条 worker 线程任意久。
qjswasm：门里没有。

**有意不补。** 这不是能力，是漏洞形状：`max_steps` 数的是指令，睡觉不花指令，所以
`poll_oneoff` 是一条**绕过步数预算**的路。PRD 36 的预算表里没有 wall-clock 一项，这条
补进来等于要求先长一个墙钟看门狗。真需要「等一会儿」的脚本，应当由宿主侧的 operation
来等，宿主那侧有超时。

### 3.7 `proc_exit` / `_start`：进程生命周期与入口约定

wasmcore：`_start` 是入口（`lib.rs:333-335`，`get_typed_func::<(), ()>(.., "_start")`）；
`proc_exit(code)` 由 wasmtime-wasi 化成携带 `I32Exit` 的 trap，被 `lib.rs:339-342` 降级
成 `GuestExit::Exited(code)`——正常生命周期信号，不是崩溃。
qjswasm：调具名导出（产品路径固定 `"main"`，`src/script_engine.rs:622`），没有
`proc_exit`，返回值走 `Outcome::values`。

**入口约定：要补——这是第 2 条门（切路由）真正的拦路石，不是本条清单里的小事。**
实测，把一个真 `wasm32-wasip1` 客人喂给 qjswasm：

```text
[qjswasm] validate_wasm: OK
[qjswasm] run_once ERR: guest trapped: no exported function named
[qjswasm] run_once(_start) ERR: guest trapped: call to unbound imported function
```

三件事一次暴露：
1. `main` 不存在——`wasm32-wasip1` 客人导出的是 `_start`；
2. 改叫 `_start` 也跑不动——WASI 导入没人绑，**而且是运行期 trap，不是装载期拒绝**；
3. **`check` 是绿的。** `validate_wasm` 只过 tinyvm 装载门，不检查导入能否绑定，所以
   `check` 放行、`execute` 才炸。这违反了 PRD 36「装载期拒绝 / 执行期 trap 要能分辨」的
   要求，也违反「check 通过的东西 run 不该拒」的一般期待。**这条要补，且应在切路由之前
   补**：装载时按 `module.imports()` 检一遍有没有绑不上的导入，报成 `Load` 类而不是
   `Trap` 类。

> **已补（2026-08-25）。** `agenterm-qjswasm` 的 `host::check_declarations` 现在对
> `agenterm.*` 以外的任何 import 在装载期直接拒，并把 `模块名.字段名` 报出来；
> `validate_wasm` 走同一条检查，所以 `check` 与 `execute` 给同一个答案。
> 分类落在 `Door` 而不是提议的 `Load`：`Load` 的含义是「这不是合法 wasm / 超了声明的
> 上限」，而这里模块本身完全合法，是它要的**门**不存在——`Door` 的定义就是这条。
> 同一次改动里顺带纠正了本节第 2 点的措辞：真实的运行期文案是
> `call to unbound imported function`，**它一个 import 名都不带**，因为 tinyvm 是
> `no_std`、文案是静态前缀；所以「运行期能看出是哪个导入」这条路本来就走不通，装载期
> 拒绝是唯一能点名的地方。实测前后对比：
>
> ```text
> 改前: validate_wasm = Ok(())   spawn = Ok   call = Err(Trap("call to unbound imported function"))
> 改后: validate_wasm = Err(Door("guest imports `wasi_snapshot_preview1.fd_write`; …"))
>       spawn         = Err(同一条)
> ```
>
> 锁：`crates/agenterm-qjswasm/tests/host_door.rs::
> check_and_execute_agree_that_an_unbindable_import_is_refused_at_load`。
> 代价说明白：这同时**反转**了 wasmcore 侧无关的一条既有决定——qjswasm 原先
> 「别的模块名的 import 不关门的事」，现在关。理由是那条 import 谁也绑不上，放它装载
> 只是把答案推迟到一条不点名的 trap 上。门的另一半宽容没有动：四件门函数客人仍可只导入
> 一部分或一个都不导入。

**`proc_exit` 语义：有意不补。** 一个 wasm 槽退出就是入口返回，`Outcome::values` 已经能
带回值；再造一个「进程退出码」概念只会让「槽死了还是宿主死了」变模糊。

### 3.8 `sock_*` / `proc_raise` / 线程

wasmcore：`sock_shutdown` → `errno=57 (NOTSOCK)`，`proc_raise` → `errno=58 (NOTSUP)`，
`std::thread::spawn` → `errno=58`（实测）。
qjswasm：没有。

**有意不补。** 三个在 wasmcore 上也都是不支持。零差异。

### 3.9 JIT vs 解释执行

wasmcore：wasmtime 默认 `Engine`，Cranelift JIT，真机器码、真 RW→RX（README「JIT,
deliberately」）。代价是这条纪律：本 crate 无法在禁 JIT 的地方跑。收益是速度，和
README 里实测的那条 AOT 路（`.cwasm`，端到端中位 71ms → 3ms）。
qjswasm：tinyvm 解释执行，不生成机器码。

**有意不补，而且这是替换的动机而不是代价。** PRD 36 纪律原文：「不做 JIT / AOT 到机器码，
不碰可执行内存」；tinyvm 存在的理由就是 iOS 不许 JIT。要补这一条等于换核，须单独下单。

**但性能差距必须诚实记账**，因为它今天没有测过：本仓没有任何 wasmcore-vs-qjswasm 的同
客人对比数。README 里那组 JIT/AOT 数字是 wasmcore **自己和自己**比。切路由之前应当补一
次同客人对比，否则「慢多少」这件事只有印象没有数字。这条列为**第 2 条门的前置测量**，
不是能力差异。

### 3.10 `wasmcore_alloc` 重入协议 vs 两趟拷贝

wasmcore：六参数一次调用，宿主在回调里**反过来调客人导出的 `wasmcore_alloc(len)`**
拿落地缓冲（`lib.rs:431-470`），再把 `(ptr, len)` 写回客人给的两个 out 参数。
qjswasm：`fleet_call` 只回 status，答案停在每槽的 pending 缓冲；客人自己问
`fleet_result_len()`、自己分配、再让宿主 `fleet_result(dst, len)` 拷进来
（`host.rs:9-28` 写了原因）。

**有意不补，因为在 tinyvm 上原理不可行。** tinyvm 的宿主回调签名是
`Fn(&[Val], &mut [u8]) -> Result<Vec<Val>, WasmError>`——回调持着线性内存的 `&mut`；
而重入客人需要 `Instance::invoke_by_name(&mut self)`。安全 Rust 里两者不能同时成立。
这不是 tinyvm 的缺陷，是「无 JIT + 显式调用栈 + 上限在核」的必然结果。

两边的收益差要说清，不要只说代价：
- wasmcore 少两次跨界，但**要求客人导出分配器**。README 的对抗测试里，
  `wasmcore_alloc` 缺失、签名错、返回越界指针、返回 0，各是一种要单独处理的失败面。
- qjswasm 多两次跨界，但客人不必有分配器（手写 `.wat` 客人也能用门），且宿主永远不写
  客人没给过的地址。

**对迁移的实际影响：现存 wasmcore 客人一个都不能直接跑。** ABI 不兼容是**签名级**的，
不是行为级的——实测，一个导入六参数 `agenterm.fleet_call` 的客人在 qjswasm 上直接被门的
声明检查拦下：

```text
[qjswasm] run_once ERR: host door: guest declares `agenterm.fleet_call` with
          the wrong signature: the door takes 4 i32 parameter(s) and returns 1
```

这是**好**的失败形状（装载期、点名、说清期望），但它意味着第 2 条门的「既有 guest 的行为
变化有测试锁住」在字面上无法满足：既有 guest 不是行为变了，是根本不加载。门的这一句应当
改成「既有 guest 的**拒绝形状**有测试锁住 + 提供一份迁移后的等价 guest」。

### 3.11 16 MiB worker 线程 vs 核内显式调用栈

wasmcore：每次 `run_module` 起一条 16 MiB 栈的 worker 线程跑客人（`lib.rs:78`,
`lib.rs:168-178`）。原因写在 README：Windows 主线程默认 1 MiB 栈装不下
Cranelift 编出来的客人代码。**客人的递归帧吃的是宿主的原生栈。**
qjswasm：客人活动记录在 VM 堆上，`max_call_depth` 在核里（tinyvm `Limits`）。

实测的两个数字，同一件事的两端：

```text
# wasmcore：客人递归 1,000,000 层
about to recurse 1000000 frames
returned 0
[host] guest returned normally

# qjswasm：同形状的递归客人，默认 Budget
depth       1: OK  values=[I32(1)] steps=21   peak_depth=3
depth     500: OK  values=[I32(500)] steps=5510 peak_depth=502
depth     510: OK  values=[I32(510)] steps=5620 peak_depth=512
depth     511: ERR budget exhausted: max_call_depth
depth  100000: ERR budget exhausted: max_call_depth
```

**有意不补，但默认值要重看一遍。** 「深递归」本身不是要保留的能力：wasmcore 那侧
1,000,000 层能过，只是因为没人管，栈爆了就是宿主线程崩，那正是 tinyvm 要消灭的形状；
qjswasm 那侧是一条**可分辨的 `Budget` 类失败**，槽还活着，宿主毫发无损——这是升级不是
退化。

要重看的是数字：默认 `max_call_depth = 512`（tinyvm `wasm.rs:1390-1397`）对一个手写
`.wat` 客人绰绰有余，对**编译出来的**客人（尤其将来 `.qjs` 长出闭包与递归下降的解析器
之后）可能偏紧。同样偏紧的还有 `max_memory_pages = 256`（16 MiB）——本次探针那个真 Rust
std 客人只是没走到堆压力大的路径。这两个是**调参**，属于第 2 条门的落地细节，不是能力
差异。

### 3.12 wasm 特性面（bulk memory / SIMD / 多内存 / 线程）

读码：tinyvm `df8decd` 的解码器识别 bulk memory、sign-extension、non-trapping
float→int、multi-value、reference types、多表、多内存、extended-const、tail-call
（`crates/tinyvm/src/wasm.rs:156-168` 的 `FeatureUsage`）。**SIMD 在 `simd` cargo feature
后面，`agenterm-qjswasm/Cargo.toml:29` 没开**；wasm 线程/原子没有实现。

实测：一个真的 2.1 MB `wasm32-wasip1` Rust std 客人，`validate_wasm` **通过**。所以
「rustc 今天发射的特性面」不是拦路石。

**SIMD：有意不补（默认关）**，需要时开 upstream 的 `simd` feature 即可，是开关不是工程。
**线程/原子：有意不补**，单槽单线程是隔离模型的一部分。

### 3.13 一条容易被漏掉的事实：核**有**一个 WASI 适配器，是我们不开它

上游 tinyvm（`df8decd`）自带 `crates/tinyvm/src/wasi_p1.rs`——一个可选的
`wasi_snapshot_preview1` 适配器，绑 16 个函数（`args_get` / `args_sizes_get` /
`environ_get` / `environ_sizes_get` / `clock_time_get` / `random_get` /
`fd_prestat_get` / `fd_prestat_dir_name` / `fd_close` / `fd_read` / `fd_write` /
`fd_seek` / `fd_filestat_get` / `path_open` / `path_unlink_file` / `proc_exit`），
文件头一句是「不默认启用，只绑显式实现的导入；未知导入或标准签名不符在实例化前失败」。

它躲在 tinyvm 的 `wasi-p1` cargo feature 后面，而 `crates/agenterm-qjswasm/Cargo.toml:29`
的依赖**没有开任何 feature**。

这条要写进清单，因为它把上面所有的「有意不补」从**能力缺失**改判成**产品选择**：
文件系统与 argv 不是 qjswasm 长不出来，是本仓一行 `features = ["wasi-p1"]` 就能长出来
而**决定不长**。PRD 36 纪律「能力全在门……不得把 WASI `fd_*` 做成第二扇 OS 面」在这里是
一条真的会被违反的纪律，不是一句空话——这一行永远不加，就是这道门在守的东西。

**判定：有意不补，且这是本清单里唯一一条「补起来只需一行、仍然不补」的。**

## 4. 汇总

| # | 能力 | 判定 | 一句话理由 |
|---|------|------|-----------|
| 3.1a | `fd_write(1)` stdout | 有意不补 | 已等价，且 qjswasm 的超限形状更好（截断+标志 vs 整次丢弃） |
| 3.1b | `fd_write(2)` 直通宿主 stderr | 有意不补 | 真差异，但与「坏槽只能弄死自己」冲突 |
| 3.2 | `fd_read(0)` stdin | 有意不补 | wasmcore 那侧也是立刻 EOF，零数据 |
| 3.3 | 文件系统 `path_open`/`fd_readdir` | 有意不补 | **两边都没有**：preopen 表为空，全 BADF（实测） |
| 3.4 | argv / 环境变量 | 有意不补 | wasmcore 那侧也是空的；传参走 `Engine::call` 的 `args` |
| 3.5 | 时钟 / 随机 | **要补**（未下单） | 真实脚本会用；补成门里的具名 import，且必须先设计可关/可钉的开关 |
| 3.6 | `poll_oneoff` 阻塞 | 有意不补 | 绕过 `max_steps` 的路；要补先补墙钟看门狗 |
| 3.7a | `_start` 入口约定 | **要补** | 切路由的实际拦路石 |
| 3.7b | 装载期检查导入可绑定 | **已补 2026-08-25** | 曾经 `check` 绿、`execute` 才炸；现在两条路同答案，且点名 |
| 3.7c | `proc_exit` 退出码 | 有意不补 | 入口返回值已经够，多一个概念只会混淆槽与宿主 |
| 3.8 | socket / 信号 / 线程 | 有意不补 | wasmcore 那侧同样不支持，零差异 |
| 3.9 | JIT | 有意不补 | 换核才谈得上；但**切路由前要补一次同客人性能对比** |
| 3.10 | `wasmcore_alloc` 重入协议 | 有意不补 | 在 tinyvm 上原理不可行；代价是既有 guest 全部不加载 |
| 3.11 | 16 MiB 原生栈上的深递归 | 有意不补 | 那是要消灭的形状；但默认 `max_call_depth`/`max_memory_pages` 要复审 |
| 3.12 | SIMD / wasm 线程 | 有意不补 | SIMD 是开关；线程与隔离模型冲突 |
| 3.13 | 上游 tinyvm 的 `wasi-p1` 适配器（16 个函数） | 有意不补 | 一行 feature 就能开；不开正是这道门守的东西 |

**三条要补，没有一条是「把 WASI 搬进门」**：3.5（时钟/熵，要先设计确定性开关）、
3.7a（入口约定）、3.7b（装载期导入检查）。**3.7b 已于 2026-08-25 落地**（见 §3.7 的
补记与实测前后对比），所以未结的只剩 3.5 与 3.7a，且只有 3.7a 挡着第 2 条门。3.9 与 3.11 的尾巴是测量与调参，不是能力。
其余十三条是有意不补，理由各自不同——其中五条（3.2 / 3.3 / 3.4 / 3.8，以及 3.1a 的
stdout 半边）根本不是「不补」，是实测下来**两边一样**，门里原本假设的差距不存在。

## 5. 影响面（实测数字）

| 问题 | 数字 | 怎么测的 |
|------|------|---------|
| `scripts/` 下 `.wasm` 语料 | **0** | `find scripts -name "*.wasm" \| wc -l` |
| 全仓被 git 跟踪的 `.wasm` / `.cwasm` | **0** | `git ls-files \| grep -E "\.(wasm\|cwasm)$" \| wc -l` |
| `script-wasmcore` 在默认特性里？ | **否**，`default = []` | `cargo metadata --no-deps` |
| 默认构建拉进多少 wasmtime 包 | **0**（开 `--features script-wasmcore` 时 **39**） | `cargo tree -p agenterm -e normal [--features script-wasmcore] \| grep -c wasmtime` |
| 是否 workspace member | **是**（18 个成员之一） | `cargo metadata --no-deps` |
| 产品调用点 | **4 处** | 见下 |
| CI / 构建脚本引用 `script-wasmcore` | **0 处** | 全仓 grep `*.sh/*.cmd/*.bat/*.yml/*.json` |
| crate 自身被跟踪文件 | 12 个（Rust 共 2296 行：`src` 536 + tests 1222 + examples 434 + guest 104） | `git ls-files`, `wc -l` |
| 仓根侧的产品级测试 | `tests/wasmcore_framed_worker.rs`，411 行，整文件 `#![cfg(feature = "script-wasmcore")]` | `wc -l` |
| 文档提及 | 11 个 md（含 PRD 36 与 PRD.md） | `grep -rl wasmcore docs plan prd *.md` |

四处产品调用点（全部在 `#[cfg(feature = "script-wasmcore")]` 后面）：

1. `src/script_engine.rs:474-538` — `WasmcoreEngineBackend`，`check` → `validate_binary`，
   `execute` → `run_module`。
2. `src/script_worker.rs:690-706` — `execute_inner` 的分发分支。
3. `src/script_backend.rs:62 / 79 / 109` — `AGENTERM_SCRIPT_BACKEND=wasmcore|wasm` 与
   `.wasm` 入口路径的路由。
4. `src/client/mod.rs:1640` — 唯一「按路径而不是按内容」传 source 的后端特判。

另有 `src/script_engine.rs:698 / 714 / 731 / …` 的 `ScriptEngine` 枚举分支，属同一处接线
的展开，不另计。

**crate 今天在这台机上不是全绿（实测）。** macOS aarch64 上跑
`cargo test --no-fail-fast`：23 个测试过 22 个，唯一失败的是
`tests/aot_precompile.rs:214` 的
`aot_cwasm_bytes_literally_embed_the_host_target_triple`——它在做字节搜索之前硬断言
`std::env::consts::ARCH == "x86_64"` 且 `OS == "windows"`：

```text
assertion `left == right` failed: sanity: this test's byte-search assumptions
are for this box's real arch
  left: "aarch64"
 right: "x86_64"
```

这不是回归，是这条测试从写下来就只在 Windows x86_64 上成立（README 的「AOT
precompilation」整节的数字也是那台机上的）。对归档决策的意义：**归档一个「在当前
开发机上不全绿」的 crate 是可以的，但不要在归档说明里写「归档时是绿的」**——写清是
22/23，失败的那条是平台锁死而不是坏了。

`WasmCoreHost` 的公开 API 里，产品只用三个：`new`、`validate_binary`、`run_module`。
`precompile_module`、`run_precompiled_module`、`run_module_from_bytes` 在 `src/` 与
`tests/` 里**零引用**（grep 实测）——归档时这三条连迁移都不用谈。

## 6. 这道门今天能不能关

**第 1 条（能力清单）：本文件即交付，可判绿。** 结论比门当初假设的宽松：所谓「完整
POSIX vs 四件门」的差距，实测之后只剩两条真差异（直通 stderr、原生栈深递归），而两条
都是**要主动放弃**的东西，不是要补的东西。

**第 2 条（切 `.wasm` 默认路由）：今天不能关。** 三个具体拦路石，全部有实测证据：

1. **入口约定不通**（3.7a）。`wasm32-wasip1` 客人导出 `_start`，qjswasm 产品路径固定调
   `"main"`。
2. ~~**`check` 与 `execute` 判断不一致**（3.7b）。~~ **已解除 2026-08-25**：两条路现在
   都在装载期拒，并点名那个绑不上的 import。见 §3.7 的补记。
3. **没有同客人性能对比数**（3.9）。切之前应当有一组数，否则「慢了多少」无人能答。

另外门的措辞要改一处：「既有 guest 的行为变化有测试锁住」在字面上做不到——既有 wasmcore
guest 的 `fleet_call` 是六参数，在 qjswasm 上是**装载期签名拒绝**，不是行为变化。应改成
「拒绝形状有测试锁住，并提供一份迁移后的等价 guest」。

**第 3 条（现状实测）：已复核，数字见 §5，与 PRD 36 所述一致**（零 `.wasm` 语料、
optional + default 关）。唯一要补一句：生产调用点不止 `script_engine.rs` 一处，是四处
（§5）；只改那一处会留下 `script_backend.rs` 的 `.wasm` 路由和 `client/mod.rs` 的路径
特判。

**建议的下单顺序**：~~先补 3.7b（装载期导入检查，最小、独立、本身就是 bug 修复），~~
（3.7b 已于 2026-08-25 落地，见 §3.7）先补 3.7a（入口约定），再做 3.9 的对比测量，
最后切路由。3.5（时钟/熵）与归档解耦，按真实脚本需求单独排期。

**门 2 剩下的判据，写成可判**（三条都能由一条会跑的命令回答，缺一不可）：

1. 一个真 `wasm32-wasip1` 客人（导出 `_start`、只用 stdio/时钟/熵）在 qjswasm 上
   `check` 与 `execute` 都成功，输出与 wasmcore 上一致——或者，它在**装载期**被点名
   拒绝，且拒绝理由写在迁移说明里。二选一，不允许「运行期 trap」这第三种。
2. 一份同客人的 wasmcore-vs-qjswasm 计时（§3.9），数字进 PRD。
3. 既有六参数 `fleet_call` 客人在 qjswasm 上是**装载期签名拒绝**这一点有测试锁住，
   并附一份迁移到两趟拷贝 ABI 的等价 guest（门原文「行为变化有测试锁住」在字面上做不到，
   见上）。
