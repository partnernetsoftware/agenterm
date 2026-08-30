# PRD 02.36 归档：2026-08-24 → 08-29 的过程记录

> 从 `PRD_02_36_agenterm_qjswasm.md` 移出的历史段落（2026-08-30 整理）。这些段落记录的是**已经判决、已经落地**的过程：
> 两个旧引擎的归档门（都已关）、M0 到数组落地的逐次复测、「接下来」的决策日志、记忆宫殿里的十一课。
> 结论都还在主文件里（各一段摘要 + 指回这里的锚）；这里只放证据与推理过程，不再更新。

## 归档门：`agenterm-qjs` 什么时候能下线

现状实测（2026-08-24）：

- `scripts/qjs/` 下**只有** `lib/fleet.js`（209 行 host binding 库），**没有任何实际
  任务脚本**——本仓 `plan/design-sql-execution-target.md` 已记录同一事实。
- `script-qjs` 是 `optional`、`default` 关。
- 生产路径真正写 `agenterm_qjs::` 的只有两处：`src/script_engine.rs` 的
  `QjsEngineBackend`、`src/bin/agenterm.rs` 的 `qjs` 子命令。其余是兄弟 crate 的
  **文档注释**在引用它的形状，不是代码依赖。

  **但「写 `agenterm_qjs::` 的地方」不等于「要动的地方」（2026-08-26 复核）。**
  第三处是 `src/script_worker.rs:683`：它按 `QjsEngineBackend.enabled()` 分发，
  **真 `task run` 走的就是这条**。它不写 `agenterm_qjs::`（写的是那个适配器），
  所以按前一个口径数不出来。要迁的是三处：**后端本体、worker 分发、CLI 别名**。
  这正是「先数无聊的那个数」那条纪律——数之前先说清楚数的是什么。

所以归档不是「砸掉在用的引擎」，是「换掉一个几乎无人使用的外链依赖」。

**归档门（可证伪，三条全绿才动手）：**

1. `agenterm-qjswasm` 能编译并跑通 `fleet.js` 的等价物——即支持对象字面量、函数表达
   式、字符串、`JSON.parse` / `JSON.stringify`、`try/catch`、带参宿主调用；
   **并且它发出的 params 过得了 `validate_fleet_parameters`**（这一句是
   2026-08-25 加的，理由见「门 1 锚定的那个文件本身是破的」）。
2. 上面两处生产调用点已迁到 qjswasm，且行为等价有测试锁住。
3. `qjs` CLI 子命令在新引擎上有对应面，或明确声明哪些不再提供、为什么。

### 门 1：**已全绿**（2026-08-25 复测，上游 rev `f21f0f2`）

门 1 点名六件语言能力、外加 2026-08-25 加的那句「params 过得了
`validate_fleet_parameters`」限定。**七格今天全部为「有」，且证据不是「能编得过」而是
「真文件真跑通」。**

| 门 1 要的 | 今天 | 证据 |
|-----------|------|------|
| 字符串（字面量、拼接、相等） | **有** | `tests/qjs_guest.rs`；100 KB 串过门字节不变（`door_attack.rs`） |
| 带参宿主调用 | **有** | 13 条 `tests/qjs_door.rs` + 31 条 `tests/door_attack.rs` |
| 对象字面量 | **有** | `f203858`；`JSON.stringify({tab:t,note:n})` 实测出 `{"tab":"@1","note":"hi"}` |
| 属性访问 / 属性赋值 | **有** | `fleet.ui.tabs.show = function () {...}` 三层嵌套赋值 + `fleet.ui.tabs.show()` 调用，实测通 |
| 函数表达式当值用 | **有** | `ba143c5`；整份绑定就是「函数存进属性再取出来调」 |
| `JSON.parse` / `JSON.stringify` | **有** | `90b8aca`/`4fd101d`；数组仍 throw，见下 |
| `try` / `catch` | **有** | `5bdb557`；绑定的 `call()` 用它兜 `JSON.parse` |
| 限定句：params 过校验 | **有，且是全目录** | `tests/qjs_produces_a_fleet_operation.rs` 拿 `OPERATION_CATALOG` 逐字段校验 |

**验收物是真文件，不是缩略版。** `tests/qjs_produces_a_fleet_operation.rs` 现在读
`scripts/qjs/lib/fleet.qjs` 本身再接一段 driver，形状照抄 `agenterm-qjs` 的
`eval_fleet_module`（它同样读真 `fleet.js`）。三条：

- 字符串 payload：`fleet.tabs.set_note("@1", ...)` → 门收到目录接受的 params，
  返回值是 `reply.ok` 而不是一段文本——也就是说 `JSON.parse` 真在宿主给的字符串上跑过，
  属性访问真在它产出的对象上跑过。
- 数字 payload：`fleet.ui.tabs.set_width(320)` → `{"width":320}`，是 JSON 数字不是
  `"320"`。
- 拒绝可 catch：桥返回 `Err` → 绑定 `throw`，脚本 `catch` 得到带 operation id 与
  broker 原文的字符串。

**`fleet.qjs` 本轮从 8/29 补成 29/29 完整移植**，并加进三方互锁
（`tests/script_fleet_facade_parity.rs::qjs_and_qjswasm_fleet_facades_are_the_same_binding`），
所以「等价于 `fleet.js`」从此是一条测试而不是一次目视 diff。行为也对齐：拒绝**抛**，
不再返回 `"ERR " + text`——旧形状会让一个从 `.js` 港过来的脚本保留 `try`/`catch`
却什么都catch不到，错误当数据往下流。

#### 作废：上一版对门 1 的两处误判

**（一）「`Value` 没有 Object 变体，这是本层的、也是门 1 唯一剩下的拦路石」——错两处。**

- **归属错。** 没有 Object 变体的是**上游** `tinyvm-qjs`：`repr::host_decode` 的
  `TAG_OBJECT` 臂直接 `Err`，本 crate 只是转述那句话。按本文件自己的分层判据
  （「随 agenterm 业务变 → 业务层；随 JS 语言 / wasm 规范 / tinyvm 变 → 上游」），
  客人堆里的对象怎么表示只随 V1 表示变，是上游的事。
- **因果错。** 它根本不挡门。`fleet.qjs` 以 `fleet;` 结尾是**本仓自己加的一行**，
  `fleet.js` 没有——因为它是**库**不是程序。用 `script run` 跑一个库文件是范畴错误：
  去掉那行之后，完成值变成最后一句赋值求出来的**函数**，同样出不来，也同样不重要。
  库的正确检查是 `script check`（答 `OK`），正确用法是被脚本引用，而那正是上面三条
  测试做的事。

  这条错误值得留档：它把一个上游的表示层限制，误报成本层的产品级拦路石；而真正挡着门的
  是**绑定只港了 29 个操作里的 8 个**——一件谁都能数出来、却没人去数的事。「读源码之前
  别断言」这条纪律，这次是栽在**没读自己那半边**上。

**（二）「数字上的边界：10 条操作因必需数值参数而发不出去」——已作废。**

那张表（无参 50 / 全字符串 16 / 含数值 11 / 其中必需数值 10）成立于 Number→String
尚未实现的时候。`ba143c5` 之后三个 ECMA-262 转换全到，`JSON.stringify` 也直接吃数字，
`fleet.ui.tabs.set_width` / `ui.input.pointer` / `ui.input.wheel` / `events.wait` 等
十条今天全部发得出去。`tests/qjs_produces_a_fleet_operation.rs::the_reachable_share_of_
the_catalog_is_known` 按它自己文档里预写的规则，从「量比例并断言差额存在」改成了
「全目录无一条因参数类型而写不出来」的等式。

### 门 2 迁完之后，两条分歧**翻转**了（2026-08-26 实测）

`tests/script_engine_exec_parity.rs` 是「四引擎执行层平价」的记录，它的价值在于
**把分歧写下来**而不是逼出假统一。qjs 的位置换成 qjswasm 之后，它记录的两条分歧
不是失效了，是**换了边**：

**一、「缺少入口点」这件事只剩 rh 还 fail-closed。** 原文记的是「rh 和 qjs 都
fail-closed，lua 没有这个契约」——三个里两个。现在 lua 的 chunk 就是程序、qjswasm 的
script 就是程序，两者都**不存在**「没有入口点」这个状态：lua 答 `0`，qjswasm 答完成值
（实测 `40 + 2;` → `Some(42)`）。**按老的多数派写的调用者，正是这次会坏的那个。**

**二、未捕获的 throw 不携带抛出的值。** rquickjs 上 `throw new Error('boom')` 的错误
文本里有 `boom`；qjswasm 答的是 `the script threw a value and nothing caught it`，
**里面没有 `boom`**——编译后的模块不导出持有那一对的 global，上游
`GuestFault::UncaughtThrow` 明说这是宿主边界的决定而不是抛出的问题。测试对这条写了
**双向断言**：既断言它被具名为一次 throw，也断言 `boom` **不在**里面，并注明「哪天在了，
说明上游长出了读它的办法，这条分歧该退休」。

（顺带：这里写的是 `throw "boom"` 不是 `throw new Error(...)`——这个引擎没有 `Error`
全局，`new` 也不在子集里。）

### 门 2 的等价现在锁在**迁移真正发生的那一层**（2026-08-26）

`tests/script_engine_equivalence.rs` 比的是两个 **crate**——直接驱动
`agenterm_qjs::eval_entry_with_host` 与 `agenterm_qjswasm::Engine`，走各自的真绑定。
那是回答「两个引擎会不会发出同一个 Fleet 操作」的正确测试，**而它不是调用点用的那一层**：
`script_worker.rs` 与 CLI 走的是 `ScriptEngineBackend::check` / `::execute`，上面还有
启用闸门、宿主接线、结果投影。迁移动的是**那一层**，所以许可它的等价必须在那一层断言。

`src/script_engine.rs::tests::gate_two_trait_equivalence` 四条：

| 断言 | 内容 |
|------|------|
| stdout 与值一致 | 三段程序，各按引擎自己的入口约定写，`print` 输出与返回值逐条相同 |
| `check` 一致 | 子集内的都收，坏语法的都拒 |
| 未被选中时都拒绝 | worker 先问 `enabled()` 再调，一个照跑的后端会顶着别人的名字执行 |
| **子集更窄，这是会坏的东西** | 默认参数 / `Math` / 带标签的模板——rquickjs 跑得动、qjswasm 拒绝（**捕获闭包 `68afb35`、模板 `653cebe`、箭头 `ee3842b` 已先后离开这一行**） |

**第四条是故意留着的**，因为迁移的真实风险就在那里：两个引擎在 Fleet 面等价，在**语言**上
不等价。可以接受的两条理由都不是假设：`scripts/` 里**没有任何 `.js` 任务脚本**（只有
`fleet.js` 绑定库），而每一条拒绝都是**具名能力诊断**——编译期大声失败，不是运行期悄悄
给错答案。

写这条测试时第一版把捕获写在了脚本顶层，引擎**跑通了**返回 1——那里的 `a` 是脚本级绑定
不是外层局部，上游原话就是「捕获外层局部 = 拒绝；读脚本级绑定 = 可以」。测试错了，不是
引擎错了；已改成嵌在函数里。

### 门 2 的证据先行（2026-08-25）

门 2 要的是「两处生产调用点已迁到 qjswasm，且行为等价**有测试锁住**」。按本仓
「先验收测再改脸」的纪律，测先写：`tests/script_engine_equivalence.rs`。

它检的东西比 `script_fleet_facade_parity` 低一层。后者比的是**绑定文件**——函数路径与
operation id 一致，能抓改名和漏港，抓不到引擎拿它们做了什么。两份在 `tabs.set-note`
上完全一致的文件，仍然可能发出不同 params、对「拒绝算不算异常」有分歧、或给调用者
不同形状的答案。这个文件比的是那些。

「同一段脚本」在这里不能是同一串字节：`agenterm-qjs` 调顶层 `entry()`、经
`__host.fleet_call` 出门；`agenterm-qjswasm` 跑整个文件取完成值、门是自由函数。所以每条
用例是一段 **body**，各自套上本引擎的写法、接在**各自的真绑定文件**后面（两份都从磁盘读，
测的是随包的绑定不是副本）。

| 用例 | 判定 |
|------|------|
| 字符串 payload（`tabs.set_note`） | 两边同 op 同 params，返回值同为 `reply.ok` |
| 数字 payload（`ui.tabs.set_width(320)`） | 两边同为 `{"width":320}`，是 JSON 数字 |
| 无参操作（`ui.snapshot`） | 两边同送 `{}`，同取回 `snap.width` |
| 拒绝可 catch（`ui.hello`） | 两边都抛、都被 `catch` 接住 |
| 数组答案（`tabs.list`） | **一致**（`048bcf2` 起）：两边都解析成列表，`tabs.length + "/" + tabs[0].id` 同为 `2/tab1/tab2` |
| 非 JSON 答案 | **一致**：两边都走 `catch` 交回原文——数组能解析之后，这条兜底还在，且两边同样走 |

**第一次跑就抓到一条，而且是测试自己错。** `1920` vs `1920.0`——`agenterm-qjs` 经
`JSON.stringify` 出来的整值 double 不带小数，测试的投影带了。两个**产品脸**其实是一致的
（`src/script_engine.rs::number_as_json` 对 tinyvm 侧施的正是 ECMA-262 那条规则），所以
断言 `serde_json::Number` 的**拼写**是在断言两个引擎都没承诺、两个调用者也都看不见的东西。
已改成按值比，并把这段写进文件注释：这个文件管的是线上的字节与值，JSON 数字怎么拼是
上面一层的事。

数组那条曾经是**故意留红字的具名分歧**：上游 `tinyvm-qjs` README 在
`JSON.parse("[1]")` 那条边界上自己写了这个下游后果，而那条用例写明「数组到了这条会失败，
正确的修法是把它挪进上面四条的行列，**不是**把断言放宽」。**2026-08-26 它如期失败，
也照这条修了。** 一条写来会在成功时失败的测试，是把「上游落地了」从一句话变成一次
别人能复跑的测量——这一轮它兑现了。

**运行条件（诚实记一笔）**：本文件要 `script-qjs` 与 `script-qjswasm` **同时**打开，
两个 feature 都不是 default，`.github/workflows/ci-agenterm.yml` 现在是 `.disabled`。
所以它不会被 `cargo test --workspace` 带到。命令是
`cargo test --features "script-qjs,script-qjswasm" --test script_engine_equivalence`。

### 这个盲区已经咬过一次（2026-08-26）

上面那句写的时候还是个隐患，现在不是了。`src/script_engine.rs` 里的
`qjswasm_check_refuses_what_execute_would_refuse` **从 2026-08-25 抬 rev 起就是红的**，
一整天没人看见：它钉的是「`1 ? 2 : 3` 在子集外」，而 `?:` 正是那次 rev（`5bdb557`）
带来的。

**为什么没看见**：它在 root crate 的 lib 里、藏在 `script-qjswasm` 后面。
`cargo test -p agenterm-qjswasm` 跑的是那个 crate 自己的测试，够不着 root crate 的 lib；
`cargo test --workspace` 用默认 feature（`default = []`），把它整个编译掉。**两条都跑过，
两条都绿。**

并且它 panic 时持着共享的 `ENV_LOCK`，把锁毒了——**一条真失败显示成六条**。
（这正是 `baseline-must-predate-your-change` 那条记忆写的形状：一个模块整片红，先找
有没有一条测试 panic 后污染了共享夹具，再怪模块。）

**第一条命令必须带 `--no-fail-fast`，否则它只跑了 lib 一个 target。** 2026-08-28
在同一天踩了两次「读了一部分就报总数」：先是一条挂死的测试让运行停在 13 行结果
（`-- --skip live_resolution_…` 绕过），后是 cargo 在第一个失败 target 就停——
lib 单独报 **793 passed / 4 failed**，整个 workspace 是 **2569 passed / 52 failed**。
提交 `7f580b55` 的信息里写的「four failures」就是后一种，**那个数字是错的，
真实是 52 条，全部既有**。在此更正留档。

**这条仓库的既有失败基线是 ~52 条，不是 0。** 所以「干净」的判据不是失败数小，
而是**失败集合逐条不变**：跑一次改动前的提交，`comm` 双向比对名字。

**所以本产品的验证口径是两条命令，不是一条**（2026-08-28 前是三条）：

```sh
cargo test --workspace --exclude agenterm-abi \
  -- --skip live_resolution_removes_subshell_response_files   # 见下面那条，非跳不可
cargo test --features "script-qjswasm,script-lua,script-sql" \
  --lib -- --test-threads=1                            # root crate lib 里的各引擎适配层
```

第三条曾是 `cargo test --features "script-qjs,script-qjswasm" --test
script_engine_equivalence`，那份集成测试要**两个引擎同时在**才有意义；`agenterm-qjs`
归档时它跟着走了。第二条的 feature 列表同时掉了 `script-qjs` 与 `script-wasmcore`。
**口径缩短不等于覆盖变松**：掉的两条测的都是「两个引擎是否等价」，只剩一个引擎时那个
问题不存在了。真正变少的覆盖是**另一件事**——没有第二个引擎能对照，本引擎的答案就只能
与它自己的历史比。这条写在这里，不藏在命令块的行数变化里。

`--test-threads=1` 不是装饰：并行跑时锁一被毒，真正的那条失败会淹在五条
`PoisonError` 里。

##### 第一条命令**不加 `--skip` 就跑不完**（2026-08-28 发现）

`tests/cursor_agent_chat.rs::live_resolution_removes_subshell_response_files`
起一个 mock server 线程，**写死 `for _ in 0..2` 收两个连接**，然后 `server.join()`。
被测脚本少发一个请求，`listener.accept()` 就永远阻塞——**没有任何超时**，
整条 workspace 套件卡死在那里。

这不是理论问题，它咬过：本轮我**四次**报告「第一条命令干净，813 passed / 4 failed」，
四次读的都是**停在第 13 条 `test result` 行**的部分输出。而这个 workspace
（root crate 54 个集成测试文件 + 各 crate 约 52 个）应当打印**一百多条**。
也就是说那四次各自只覆盖了约 12%，而我把它当成了全绿。

**教训不是「那条测试有 bug」，是「一条不会终止的命令，其部分输出和成功长得一模一样」。**
跑完的证据不是「没看到失败」，是**结果行的条数**和末尾的退出码。

本文件不改那条测试：`docs/agenterm-rust-cheatsheet.md` 正在被另一个 session 编辑，
而它新加的那段讲的正是「跨机测试夹具不该拿 `env!("CARGO_MANIFEST_DIR")`
当运行期仓库定位器」——同一片地。绕开，不动。（确认过是**孤例**：套件里其它用
`TcpListener` 的测试都设了超时。）

##### 加上 `--skip` 之后的真实数字，以及第一条命令**本身就是个不好的门**

跑完一次：**154 条结果行，2658 passed / 70 failed**，exit 101。
对照我之前报的「813 / 4」——那是 31% 的测试和 70 条失败里的 4 条。

70 条里**落在 qjs 地界的只有 1 条**：
`script_cli_verb_parity::qjs_alias_is_retired_and_names_where_its_verbs_went`。
它的失败信息是 ``agenterm: unknown engine subcommand `qjs` ``——
把引擎 feature 打开重跑，`script_cli_verb_parity` **10 passed / 0 failed**。
旁边几条 `sql_*` 同理，同一句话。

也就是说**它们不是回归，是第一条命令自己的构建配置造出来的**：
`cargo test --workspace` 用**默认 feature** 编 `target/debug/agenterm`，
而这些集成测试要 shell 出去调引擎子命令。这正是记忆
`cargo-test-workspace-overwrites-the-binary` 记的那个机制，只是这次它不是
「把我要量的二进制覆盖了」，而是「让一批测试在这条命令下必然红」。

**结论：第一条命令目前不是一个能判绿的门**，它稳定产出几十条与被测改动无关的失败。
剩下约 69 条落在 rh / sql / executor / stdlib / release-candidate 各处，
**本轮没有逐条归属**——它们不在 qjs 地界，而本轮 agenterm 侧的整个 diff 里
root crate 只动了测试字符串与注释，没有一行默认构建会编进去的生产代码。
如实记为「未归属」，不写成「既有失败」，两者不是一回事。

#### 门 1 之外，本轮顺带关掉的一条

**未捕获的 `throw` 不再报成裸 trap。** `QjswasmError::UncaughtThrow` 自成一类，
`slot.rs::explain()` 读 `tinyvm_qjs::guest_fault()`，把堆耗尽与未捕获抛出一起从
`Trap` 里分出来；`GuestFault` 是 `#[non_exhaustive]`，认不出的第四种原因照旧落回
`classify`，不猜。两条测试锁住：一条钉分类，一条钉「上一次调用留下的 fault word
不许污染下一次」。**抛出的值本身仍拿不到**——编译后的模块不导出持有它的 global，
上游明说这是宿主边界的决定而不是抛出的问题。

**门 3 的措辞已改（2026-08-25）。** 原文点名 `check` / `pack` / `qualify` /
`check-many` 四个动词。实测 CLI 有**十三个**（`crates/agenterm-qjs/src/cli.rs`），
只答那四个等于对一个没人问的问题关门。逐动词判决见下。

### 门 3 沿途挖出的两条活缺陷（2026-08-26 实测）

**一、任何 `.wasm` 文件的路径会被当源码交给选中的引擎。** `cli script` 里有一条分支把
「路径」而不是「文件内容」当 source 传，因为 wasmcore 的 `check` 真的是
`std::fs::read(source)`。那条分支**按扩展名判**，注释还写着「rh/lua/qjs/sql 不受影响：
`from_entry_path` 只对 `.wasm` 返回 wasmcore」。前半句真，结论不成立——**分支不决定谁来跑，
路由只看 `AGENTERM_SCRIPT_BACKEND`**。拿一份真的 9 785 字节模块实测：

```
rh        rh parse error: Unexpected '/' (line 1, position 1)
lua       lua_parse: syntax error …
qjswasm   compiling .qjs: needs an operand here, found a `/` at byte 0
```

三个引擎都在拿**路径**当程序解析。这正是 `QjswasmEngineBackend::execute` 注释里记着
「本层已修掉」的那个 `source`-当路径缺陷，被上面一层重新引入了。

已改成问引擎而不是问扩展名，并把这个属性做成 `ScriptEngineBackend::source_is_a_path()`
——**没有默认实现**，一个引擎不能靠沉默获得这个行为（那正是它会回来的方式）。一条测试断言
**有且只有一个** `true`。

**二、二进制文件的拒绝理由是「不是 UTF-8」，读起来像「你的文件坏了」。** 真正的答案是
「这扇门载的是文本」。已在那条错误后面追加一句说清楚，且只对这一种错误追加——真的读不到
文件的人还是要看见原因。

### 门 3：逐动词判决（2026-08-25，全部跑过，不是读出来的）

判据表在 [`plan/design-qjs-archive-gate.md`](../plan/design-qjs-archive-gate.md)，
每格都有可复现的命令。三类判决：**必须提供**（新引擎上要有等价动词）、
**形状必然不同**（能力在，但产物或收据不同，须写清差异）、**可以不提供**（附理由）。

| # | 动词 | 判决 | 一句话 |
|---|------|------|--------|
| 1 | `check` | **已有面**（2026-08-26 实测） | 不需要新 CLI 壳：`agenterm cli script check FILE` 本来就按 `AGENTERM_SCRIPT_BACKEND` 路由。`.qjs` 过则答 `OK`，不过则给引擎自己的能力诊断 |
| 2 | `check-many` | **判决改了**：不提供，按名字拒绝（2026-08-26） | 原估「约 60 行适配器」低估了：这个动词是**把本可执行文件按 `__agenterm-internal-engine rh` 重新拉起**跑的，manifest schema、`kind`、收据全是 rh 的。产品早就写了 `check_many_requires_rh_error()`——**定义了，从没被调用过**，所以选别的后端时它静默跑 rh，报的是「rh parse error: unknown field …」，把「引擎选错了」说成「manifest 写错了」。已接线并具名 |
| 3 | `pack build` | **已交付**（2026-08-26） | 产物是**一份自足 `.wasm`**，不是 `.qjsc` + 源码目录 + manifest——这个方向的「形状不同」是变简单不是变差。挡路的曾是「`agenterm cli script` 结构上只载文本」，那张吃字节的脸已经补上：`pack_artifact` + `execute_artifact`，四个动词是**一张脸**不是四个适配器 |
| 4 | `pack load` | **已交付** | 走 `Guest::CompiledQjs` 而不是 `Guest::Wasm`——从 `.qjs` 编出来的模块说 V1 约定，当匿名 wasm 客人装载会把它丢掉，调用者拿到裸 `(i32,i64)` |
| 5 | `pack build` 模块模式（`pack_module`） | 可以不提供 | 它绕的是 rquickjs 的约束，那约束在这里不存在；零生产调用者 |
| 6 | `qualify` | **已交付**（2026-08-26） | 收据确实是超集，而且这句话现在有东西支撑：`ScriptInvocationResult` 多了 `cost: Option<ScriptCost>`（`steps` / `peak_call_depth` / `peak_activation_slots`）。**`None` 是「这个引擎不计数」不是「免费」**——六个里五个不计，序列化成零会是一次没人做过的测量。实测 `return 40 + 2;` → `steps 65, peak_call_depth 2, peak_activation_slots 15` |
| 7 | `corpus-scan` | **已交付**（2026-08-26） | 估得准：引擎侧四行（`crates/agenterm-qjswasm/src/corpus_scan.rs`），渲染/退出码/`--dir` 全走 `agenterm_script_common::cli` 的共享 driver——和 `agenterm qjs corpus-scan` 同一条，不会漂成两份报告。测试直接用共享的 `CorpusScanContract`。CLI 面是 `agenterm cli script corpus-scan [--dir DIR]`，按后端路由；rh 与 wasmcore **各自**给出自己的不提供理由（rh 的在它自己的 dev CLI；wasmcore 的语料是字节不是源码） |
| 8 | `eval` | **已交付**（2026-08-26） | 曾经**每个引擎都收到 rh 源码**；wrapper 移到 `ScriptEngineBackend::eval_entry_source`，六个引擎各答各的方言，wasmcore 按名字拒绝 |
| 9 | `run -- <args>` | 可以不提供（今天） | 门只有四件，没有 `args_len` / `arg`（`host.rs` 的 `SIGNATURES`） |
| 10 | `hash` | **已交付**（2026-08-26），且差异是**改进**不是妥协 | `agenterm cli script hash FILE` 打三列：`<hex>  <哈希的是什么>  <路径>`。qjswasm 哈希的是**编译出来的 `.wasm`**，其余引擎哈希源码，标签跟着数字走——两者都是 `hash` 的正确答案，但互相不可比，看不见标签就比不了。实测的决定性性质：只差一条注释和空白的两份源码，qjswasm 同哈希（`1c8388e0…`），qjs 源码哈希不同（`b77e2112…` / `b9db333b…`）——「这是不是同一个程序」正是这个动词存在的问题，源码摘要答不了。编不过的源码没有产物也就没有哈希，给的是编译器自己的诊断而不是退回哈希文本。wasmcore 按名字拒绝：它的输入本身就是产物 |
| 11 | `run-smoke` | **已交付**，且**就是** `pack load` 的同一条代码路径 | 不是复制一份：冒烟测试若跑的是另一条路，测的就是没人部署的那条。`agenterm-qjs` 的 `run-smoke` 委托给 `pack load` 同理。实测 `script run 源码` 与 `pack load 产物` 输出**逐字节相同**——第一版不同（`tabs.list` vs `"tabs.list"`），因为新动词自己渲染了一遍值；已抽成一个函数 |
| 12 | `task` | **已有面**（2026-08-26 实测） | `agenterm cli script task list` 与引擎无关（读的是任务清单不是脚本），选 qjswasm 时照常列出 |
| 13 | `version` | **已交付**（2026-08-26） | `agenterm cli script version`，六个引擎各答各的 identity。qjswasm 的多一样别人没有的：**上游 pin**——`agenterm-qjswasm 0.1.16 (tinyvm 577af37)`。一周里这个 pin 动了五次，每次都改变「`[1,2,3]` 编不编得过」的答案，拿着二进制的人此前无从分辨。`UPSTREAM_TINYVM_REV` 由本 crate 一条测试钉死在自己的 `Cargo.toml` 上（两个 pin 必须相等），所以打印出来的是事实不是声明 |

**一处前置条件，已解除。** 一份 `.wasm` 文件不记得自己是从 `.qjs` 编来的：
`Convention` 是装载时记下的，`Guest::Wasm(&compile_qjs(src))` 会把 JsV1 丢掉，
返回值形状与直接跑源码不一致（实测 `[I32(1), I64(4631107791820423168)]` vs
`[Js(Number(42.0))]`）。所以**任何"编译落盘再加载"的动词在此之前都建在沙上**。
`Guest::CompiledQjs(&[u8])` 已于 2026-08-25 落地，验收测试是门自己点名的那条：
`tests/qjs_guest.rs::a_compiled_artifact_reloaded_gives_the_same_value_as_its_source`。
同一个变体另有一条独立理由要它（接缝的五条拒绝分支此前不可达），两条互不相干的路指向
同一个形状。

**附带记录一条假话，与 wasmcore README 那两句同等对待。**
`crates/agenterm-qjs/src/pack.rs` 的模块文档说 `bytecode_hash` 是
「a genuine reproducibility fingerprint」。**不是。** 同一份源码编到两个不同的
`--dir`，`source_hash` 相同而 `bytecode_hash` 不同（`1ab1e0b1…` / `1ab1e0b1…` 对
`bd9a9694…` / `7074b217…`），因为编译标签用的是绝对输出路径，`Module::write` 把它嵌进
了产物——`xxd pack.qjsc` 能直接看见。`compile.rs` 的三条单元测试全把 label 钉成
`"a.js"`，所以它们永远抓不到这条。**门绿前不改 `agenterm-qjs` 的代码**（纪律如此），
但这句话记在这里，免得下一个人照它判断；也说明门 3 的 `pack` 不是"移植"，移过去等于
移一份已经坏掉的契约。对照：`agenterm-lua` 的 `compile_lua(source)` 没有 label 参数，
干净。

### 第一条门的实测缺口清单（2026-08-24，rev `df8decd`；2026-08-25 在 rev `f8adef8` 上复测）

把 `scripts/qjs/lib/fleet.js` 的每一种构造拿去**真编一次**，得到下表。这不是读源码估的，
是 209 行里逐条构造喂给编译器的结果。**没有归档任何东西**——这张表是路线图的下一份输入。

**已经能编（`fleet.js` 用到、今天就过）：**

| 构造 | 出处 | 证据 |
|------|------|------|
| 字符串字面量与拼接 | 全文的 `"tabs.list"` 等 operation id | `return "tab" + "s.list";` → `"tabs.list"` |
| `const` 声明、真作用域 | `const fleet = {}` 的声明部分 | 已测 |
| 带参函数声明 + `return` | `function call(opId, params) {…}` 的外壳 | `function call(a,b){return a;}` → 可调 |
| `===` / `!==` 与 `undefined` | `params === undefined` | `p === undefined` → `Bool` |
| 直接调用一个已知函数名 | 各 wrapper 里的 `call(...)` | 已测（含递归、互递归） |
| 嵌套函数读**脚本级**绑定 | wrapper 引用顶层的 `call` | 已测（`function g(){ return f()+1; }`） |

**还编不了（按在 `fleet.js` 里出现的重要性排序）：**

| 缺口 | `fleet.js` 里的形态 | 现在的诊断 | 属哪一期 |
|------|--------------------|-----------|----------|
| **对象字面量** | `const fleet = {};`、`{ tab_id: tabId, note: note }` | "does not support object literals yet" | M4 |
| **属性访问 / 属性赋值** | `fleet.tabs.list = …`、`__host.fleet_call`、`JSON.parse` | "does not support property access yet" | M4 |
| **把函数当值用** | `fleet.tabs.list = function () {…}`——右边是函数值 | "does not support using a function as a value yet" | M4/M5（需要函数值 + 间接调用表） |
| **条件表达式 `?:`** | `params === undefined ? "{}" : params` | "does not support conditional expressions yet" | 排期，纯前端 + 已有控制流，成本最低的一条 |
| **`try` / `catch`** | `call()` 里包住 `JSON.parse` | "does not support the `try` keyword yet" | M5 |
| **`JSON.parse` / `JSON.stringify`** | 每个带参 wrapper | 先撞属性访问 | M5（也可用 `.qjs` 自举） |
| ~~**带参宿主调用**~~ | `__host.fleet_call(opId, params)` | ~~自由名字先被拒~~ | **已交付 2026-08-25** |

**2026-08-25 复测的三处变化**（上游 rev 抬到 `f8adef8`，逐条编译验证）：

| 缺口 | 2026-08-24 | 2026-08-25 |
|------|-----------|-----------|
| 带参宿主调用 | 够不着门 | **能**：`fleet_call("tabs.list", "{}")` 真的到达 bridge，`fleet_result()` 真的取回答案 |
| `%` / `typeof` | 「解析后明确拒绝」 | **能**：`7 % 3` → `1`，`typeof "x"` → `"string"`（上游 `dd35c44` / `c707558`） |
| `-0` 字面量丢符号 | 上游缺陷，`1 / -0` 给 `Infinity` | **已修**（上游 `1cba206`）：`let z = -0; 1 / z` → `-Infinity` |

其余各行原样成立：对象字面量、属性访问、函数当值、`?:`、`try/catch`、JSON 逐条复测仍被拒，
诊断文案与上表一致。

**第七条当时要单独说，因为它不是「语法还没长到」。** 原文：`.qjs` 根本够不着
`agenterm.*` 门——编译器默认下自由名字一律拒；上游的 `Names::HostImport` 发射的是模块名
`"js"`、按 JS 值传参的导入，与本仓门的 `"agenterm"` + i32 两趟拷贝 ABI 不是同一扇门；
所以它需要的不只是编译器长一层语法，还需要**决定 `.qjs` 侧怎么落到这四个 import 上**
——那是本仓的写刀。

**那把刀已经落下（2026-08-25）。** 上游改出 `Names::Declared(Vec<HostFn>)`：embedder
交一张「脚本能写的名字 → 模块/字段/每个 JS 参数怎么拆成 raw i32 / 结果怎么包回来」的
声明表，机制里没有一个 agenterm 词。本仓交三条声明（`src/host.rs::declarations()`），
`fleet_result` 是 `HostResult::Bytes` 双趟，所以三条声明发四个 import。方向是整个设计：
**门不学 JavaScript 的值表示，编译器往下拆**——否则每个手写 `.wasm` 客人都得跟着改，
而那扇门是给任何客人用的，不是给一种语言用的。脚本只有真写了那个名字，import 才会出现；
`return 1 + 1;` 编出来的模块 import 表是空的。

**一句话结论（2026-08-25 版）**：`fleet.js` 等价物的距离 = 堆对象（对象字面量 + 属性）+
函数值 + `try/catch` + JSON + `?:` + **Number→String**。这些**全在上游语言层**；
「一条 `.qjs` 到 `agenterm.*` 门的路」已经不在这张单子上了。第一条门仍未绿，
另两条门未动。

### 门 1 锚定的那个文件本身是破的（2026-08-25 实测）

门 1 说「跑通 `fleet.js` 的等价物」。查这个锚点的时候顺带把 `src/operations.rs` 声明的
面与两份 binding 库真的对了一遍——**link `OPERATION_CATALOG`，不扫文本**——发现的东西
改变了这条门的含义：**把 `fleet.js` 一比一搬过来，会把它的 bug 一起搬过来。**

先更正一个数：派单书与早前文档写的「46 个 `OperationSpec`」是旧数。实测
`OPERATION_CATALOG.len()` = **77**（44 条长写 + 33 条由 `nullary_ui_action()` 常量
构造器造，`src/operations.rs`），其中 **76 条** `script_surface` 以 `fleet.` 开头。

两条真实分歧：

1. **覆盖率 38%。** 76 条 `fleet.*` 里只有 **29** 条有 binding，**47 条两份 binding
   里都没有**——而且缺的是**同一批 47 条**。两份文件是互相抄出来的，不是从同一份源
   生成的。
2. **29 条里有 9 条（31%）发出的 params 宿主会拒。** 这是把
   `scripts/lua/lib/fleet.lua` 放进 `agenterm_lua::LuaEngine` 里、用一个会记录的
   `__host.fleet_call` **真跑出来**的载荷，不是正则匹配出来的：

   | surface | binding 发出 | spec 声明 |
   |---------|-------------|-----------|
   | `tabs.set-note` | `{"note":…,"tab_id":…}` | `tab`（必需，`stable_tab_id`），没有 `tab_id` |
   | `ui.tab.select` | `{"id":…}` | `tab` |
   | `ui.input.wheel` | `{"delta":…}` | `x` / `y` / `delta_y`，三个都必需 |
   | `terminal.paste` | `{"text":…}` | **NO_PARAMETERS** |
   | `ui.composer.send` | `{"text":…}` | 只有 `tab` |
   | `ui.hello` / `ui.deltas` / `events.read` | `{}` | 各有必需参数 |
   | `events.wait` | 只有 `timeout_ms` | 还要 `epoch` / `after` / `kind` |

   `validate_fleet_parameters`（`src/client/mod.rs`）拒未知键、也拒缺必需键，所以这九个
   binding 函数**今天不可能成功**。诚实标注：「所以宿主回
   `broker_invalid_arguments`」是读派发路径读出来的，不是端到端跑出来的——那需要一台活
   服务端，结算实验写在
   [`plan/design-fleet-catalog-binding.md`](../plan/design-fleet-catalog-binding.md)。

**本轮不修，理由写下来。** 47 条缺失的 binding 是**做功能**不是修 bug；那 9 条里有几条
（`terminal.paste` 收 `text` 而 spec 是无参、`events.wait` 少三个必需参数）不是打错字，
是要改脚本作者看得见的参数表，属产品决定。而且 `scripts/qjs/lib/fleet.js` 属于一个
**正在等归档**的引擎，现在改它要么白改，要么给门再添一条要交代的事实。
今天的状态被 `tests/fleet_catalog_conformance.rs` 用带注释的允许清单钉死——**新的漂移
会红**——所以拖着不修不会烂掉。**这是一张独立的单，不是本轮的收尾。**

**它对门 1 的含义**：门 1 的验收不能是「`fleet.js` 逐字编得过」，得是
「`fleet.*` 面的等价物在 qjswasm 上跑得通，且它发的 params 过
`validate_fleet_parameters`」。照抄一份编得过的破 binding 不算绿。

在三条全绿之前，`agenterm-qjs` **原样保留、不动、不腐化**；`.js` / `.mjs` 继续路由到
它。归档动作本身另行派单。


### 归档 `agenterm-wasmcore` 的门

`agenterm-qjs` 的门在下一节（锚在 `fleet.js`）。wasmcore 的门不同，因为它的用户是
**wasm 客人而不是脚本**。三条门与它们**今天的状态**：

| # | 门 | 状态（2026-08-25） |
|---|----|------|
| 1 | 能力差异有诚实清单，每条注明要补 / 有意不补 | **可判绿**——清单在下，交付物是 [`plan/design-wasmcore-archive-gate.md`](../plan/design-wasmcore-archive-gate.md) |
| 2 | `.wasm` 默认路由切到 qjswasm，拒绝形状有测试锁住 | **不能绿**，还差两件，见下 |
| 3 | 现状实测（零 `.wasm` 语料、optional + default 关） | **已复核**，数字与原文一致，但生产调用点是四处不是一处 |

#### 门 1：能力差异（实测，不是读规范）

**门自己的前提被证伪了。** 本节 rev4 写「wasmcore 提供完整 WASI p1」。
准确的说法是：**import 面完整，授出的能力几乎为零**。
`p1::add_to_linker_sync` 注册全部 46 个 witx 函数，所以每个 WASI import 都能绑；
但 `WasiCtx` 是 `WasiCtxBuilder::new().stdout(pipe).inherit_stderr().build_p1()`
——没有 `.args()`、没有 `.envs()`、没有 `.preopened_dir()`、没有 `.inherit_stdin()`。

拿一份真 `wasm32-wasip1` 探针客人跑 `WasmCoreHost::run_module` 实测：

```text
args_sizes_get errno=0 argc=0     | environ_sizes_get errno=0 count=0
clock_time_get(realtime/monotonic) errno=0，真值 | random_get errno=0，真熵
fd_fdstat_get(0/1/2) errno=0      | fd_fdstat_get(3..5) errno=8  ← 零 preopen
path_open(fd3) errno=8            | std::fs 读/写/列目录 → NotFound
fd_read(stdin) errno=0 n=0（立刻 EOF）
thread::spawn errno=58 | sock_shutdown errno=57 | proc_raise errno=58
```

所以「完整 POSIX vs 四件门」这个差距**不存在**。十六条逐条判决，**三条要补、
十三条有意不补**；而十三条里有**五条根本不是差异**——两边实测一样（stdin 都是立刻
EOF、文件系统都是全 BADF、argv/env 都是空、socket/信号/线程两边都 NOTSUP、
stdout 等价且 qjswasm 的超限形状更好：截断加标志 vs 整次丢弃）。

**真正只有 wasmcore 有的能力只剩两条，且两条都是要主动放弃的**：直通宿主 stderr
（与「坏槽只能弄死自己」冲突）、16 MiB 原生栈上的深递归（那正是要消灭的形状）。

三条要补，没有一条是「把 WASI 搬进门」：

| # | 要补什么 | 状态 | 说明 |
|---|---------|------|------|
| 3.5 | 时钟与熵 | **未下单**，不挡门 | 真实脚本会用；补成 `agenterm.*` 里的具名 import。**前置是先设计确定性开关**——它们会是本引擎唯一的不确定性来源，会破坏 `steps` / `peak_*` 的可重放性。今天可用一次 `fleet_call` 绕过，所以不是门的阻塞项 |
| 3.7a | `_start` 入口约定 | **未做，挡门 2** | `wasm32-wasip1` 客人导出 `_start`；qjswasm 产品路径固定调 `"main"`（`src/script_engine.rs`） |
| 3.7b | 装载期检查导入可绑定 | **已补 2026-08-25** | 见下 |

**3.7b 是这一轮抓到并修掉的真缺陷。** 一个 WASI 客人在 qjswasm 上曾经是：
`validate_wasm` 返回 `Ok(())`、`spawn` 成功、第一次调用才死在
`Trap("call to unbound imported function")`——**check 放行了 execute 跑不了的东西**，
违反本 PRD 自己的「装载期拒绝 vs 执行期 trap 要能分辨」，而且那条 trap 一个 import 名
都不带（tinyvm 是 `no_std`，文案是静态前缀），所以「运行期看出是哪个导入」这条路本来
就走不通。现在 `agenterm.*` 之外的任何 import 在装载期直接拒、点名，
`validate_wasm` 与 `spawn` 给同一个答案。分类落 `Door` 而不是 `Load`：模块本身是合法
wasm，缺的是**门**。锁在
`crates/agenterm-qjswasm/tests/host_door.rs::check_and_execute_agree_that_an_unbindable_import_is_refused_at_load`。
代价说明白：这**反转**了 qjswasm 侧一条既有决定（「别的模块名的 import 不关门的事」），
理由是那种 import 谁也绑不上，放它装载只是把答案推迟到一条不点名的 trap 上。门的另一半
宽容没动——四件门函数客人仍可只导入一部分或一个都不导入。

#### 门 2：还差什么，写成可判

三条判据，缺一不可，每条都能由一条会跑的命令回答：

1. 一个真 `wasm32-wasip1` 客人（导出 `_start`，只用 stdio/时钟/熵）在 qjswasm 上
   `check` 与 `execute` 都成功且输出与 wasmcore 一致——**或者**它在装载期被点名拒绝，
   且拒绝理由写进迁移说明。二选一，**不允许「运行期 trap」这第三种**。挡在 3.7a 上。
2. 一份同客人的 wasmcore-vs-qjswasm 计时。本仓今天**没有任何**这样的数，切路由之前
   「慢了多少」无人能答。
3. 既有六参数 `fleet_call` 客人在 qjswasm 上是**装载期签名拒绝**这一点有测试锁住，
   并附一份迁移到两趟拷贝 ABI 的等价 guest。

第 3 条同时是对门原文的措辞修正：原文写「既有 guest 的**行为变化**有测试锁住」，
字面上做不到——既有 wasmcore guest 的 `fleet_call` 是六参数，在 qjswasm 上是装载期
签名拒绝，那是**不加载**，不是行为变化。

#### 门 3：现状复核

`scripts/` 下零个 `.wasm` 语料、`script-wasmcore` optional + default 关：都属实。
一处要更正：**生产调用点是四处，不是一处**——`src/script_engine.rs` 的
`WasmcoreEngineBackend`、`src/script_worker.rs` 的 `execute_inner` 派发、
`src/script_backend.rs` 的环境变量与 `.wasm` 路由、`src/client/mod.rs` 的路径特判，
外加 `tests/wasmcore_framed_worker.rs` 这份产品级黑盒测试。只改 `script_engine.rs`
一处会把后三处留在原地。

> **附带修正（已完成 2026-08-25）**：`crates/agenterm-wasmcore/README.md` 曾写它
> 「not a member of the root workspace」「not wired into any product path」，**两条都是
> 假的**——根 `Cargo.toml` 的 `members` 列了它，它自己的 `Cargo.toml` 根本没有
> `[workspace]` 表；它也是 `script_engine.rs` 的一个 backend。同章另有两句假话一并
> 更正（「自带空 `[workspace]` 表」的前提、「把 wasmtime 挡在根 `Cargo.lock` 外面」
> ——根 `Cargo.lock` 里有 `wasmtime 47.0.3`）。活下来的一条被收窄保留：AOT 那对
> （`precompile_module` / `run_precompiled_module`）与 `run_module_from_bytes` 确实
> 在 `src/` 与 `tests/` 里零引用。
>
> 另记一条归档时要说实话的事实：该 crate 在**当前开发机**（macOS aarch64）上是
> **22/23 绿**，失败的那条是 `aot_cwasm_bytes_literally_embed_the_host_target_triple`
> ——它硬断言 `ARCH == "x86_64" && OS == "windows"`，从写下来就只在那台 Windows 机上
> 成立。归档说明里不要写「归档时全绿」。


## 记忆宫殿的十一课

**为什么值得画。** 2026-08-26 这一天抓到的三条真缺陷，**全是同一种病：东西放错房间**。
2026-08-29 又加了一条反过来的用法：`for…of` 需要一个运行期检查，看起来该给第 ③ 间
加预制件——实测发现它**能用第 ① 间已有的词汇原样说出来**（四道 `if`/`throw`），
于是第 ③ 间一个字节没动，也不需要门。**先问「能不能用现有房间的话说出来」，
再决定要不要新开一格。**

**同一天还发现这张图漏画了一间房——而它一直有人在住。**

`scripts/qjs/lib/fleet.qjs` 是第 ⑦ 间的**门的客户侧**：29 个操作的绑定库。
它零消费者，原因不在这七间房里的任何一间，而在**没有一条走廊通向它**：
唯一的用法是下游测试里那行 `format!("{lib}\n{driver}")`，在 Rust 里把库文本
拼到脚本前面。**一个库要能被 import，才算住进了这张图。**

**同一天还学到一条关于「量什么」的：换一把尺，路线图就换一张。**
第一次需求普查数的是**语法**构造，`for-of` 排第一；第二次数**标准库面**，
同一个 82 脚本语料，第一名变成 `.contains`（71% 的脚本、721 次）。
两张表毫无重叠。所以「用哪把尺」本身就是一个要被记录和辩护的决定——
默认拿一把尺量下去，得到的不是需求，是那把尺的形状。

**同一条路上还有第二课，来自 `toLowerCase`：一张判据表也有形状。**
那份决定文档的判据是照着「一张映射表」写出来的，所以它想得到
**哪些映射缺失**（`İ` 一对多，事前点名了），想不到
**哪些映射是有条件的**（词尾 `Σ` → `ς` 依赖前后文，是测试抓到的）。
成本估算同样错了 2.6 倍，而错法有两种：一种是**没去量**（delta 极值），
一种是**没去想**（把「压得更小」和「查得更快」当成能叠加）。
**判据表防得住「忘了做」，防不住「没想到要判」——所以事前判据之外，还必须有事后回填。**

**第三课，来自并行推进本身：并行的边界是「共享了什么」，不是「相关不相关」。**
2026-08-29 把 `break`/`continue`（循环降级）与 `replace`（字符串 prefab）并行做，
代码上零冲突，一次验证过。但在后台跑 workspace 测试的**同时**去 CLI 探一个问题，
探出来的全是 rh 的错误——那次测试用默认 feature 把 `target/debug/agenterm`
重建了。两件事在**代码**上无关，在 **target 目录**上是同一个资源。
改到上游 `tinyvm` 去探就对了，那才是另一个 target。
**并行拆分要按共享资源拆，不是按模块拆。**

**第四课：一个 grep 模式会不会数到散文，是那个模式自己的性质。**
第二次普查的尾部两行原写 spread「14 个脚本 20 次」、`switch`「2 个脚本 6 次」——
剥掉字符串、模板与注释后**两者都是 0**。那些 `...` 在 `<args...>`、
`` `require(...)` `` 里，`switch` 在英文句子里。
头部行复查后**一个没变**（`.contains(` 58/721 等），**所以照表建的东西没建错**。
区别很具体：`.contains(` 带前导 `.` 与尾随 `(`，进不了人话；`...` 是标点，
`switch` 是裸词，都会。**裸词与标点的模式必须先剥离字符串与注释，
带定界符的方法调用模式不必——这条可以事前判断，不必事后发现。**

**第十一课：`kill` 杀的是进程不是树，而测试里的 `sh -c` 让我以为 reap 坏了。**
子进程句柄的「无孤儿」测试先红：`process_kill` 之后 `pgrep -P` 仍看到子进程。
不是 `Drop` 没跑（加一行 trace 证明它跑了），是 `sh -c "sleep 30"` 里被杀的是 `sh`，
`sleep` 成了孤儿——而 rh 的 `kill` 本来就是这个语义（它另有 `kill_tree`）。
把测试改成直接 spawn `sleep`，绿。**一个测试红了，先分清是「机制坏了」还是
「测试里的间接层把语义换了」——这次两个候选各一半，trace 一行就定了。**

**第十课：第一个真脚本撞上的两堵墙，一堵是引擎面的，一堵是我自己砌的。**
迁 `validate-artifact-manifest` 时：（1）引擎面**不能把字符串参数送进 guest**
（「no door onto the guest allocator」），`$0` 拿路径不可能——解法不是开那扇门，
是让参数走**已经存在的两趟机制**：`arg_count()` / `arg(n)` + `tool_result()`。
（2）`isArtifactName` 里 `role.split("")` 撞上 **08-29 我自己定的 `split("")` trap**
（孤立代理不可表示）。库写错了，不是决定错了——「每个字符都是 a–z」改写成
「逐个删掉 26 个字母后是否为空」，用现有工具问同一个问题。
**一个决定的代价，要等第一个真用户来付时才看得见；付的人是我自己，正好。**
顺带一条给上游：那个 trap 出来还是裸 `unreachable`，**该带第四类 fault code**。

**第八课：一把锁被毒，十一条无辜测试陪葬——第一条 panic 才是病，其余是症状。**
合入 rh-out 后 `--features script-lua,script-sql --lib` 红了 16 条。按名字读：
**只有一条**是真的（`lua_engine_execute_errors_when_not_enabled`，它断言的正是 08-28
删掉的那道 `enabled()` 门），其余全是 `ENV_LOCK` 的 `PoisonError`。
它藏了一天，因为它在 `script-lua` feature 后面，而整套 workspace 用默认 feature 跑。
**两条规矩**：验证口径里的第二条命令（带三个 feature 的 `--lib`）不是可选的；
读失败清单时先找**非 PoisonError 的第一条**，别数总数。

**第九课：测试 spawn 的二进制不是测试自己编的那个。**
`worker_supervisor` 两条红——真正原因是 `target/debug/agenterm` 是上一次
默认 feature 构建留下的，没有 qjswasm，worker 答 `None`。指向带 feature 的
二进制后 3/3 过。这是「`cargo test --workspace` 覆盖二进制」那条的镜像：
**一个 spawn `target/debug/agenterm` 的测试，必须先自己保证那个文件是谁编的。**

**第七课：并行的正确形状是「四条独立 worktree + 一次合并」，不是「四个人改同一棵树」。**
2026-08-29 把 rh 移出、`tool.*` 门、`path.qjs`、双向金丝雀交给一个 workflow 四路并行。
每路先只读普查、再在**自己的 worktree 分支**上建、再被一个对抗代理复跑试图推翻。
三路 agenterm 分支合到 main 时**零冲突**——因为拆分是按**共享文件**拆的
（只有 `Cargo.lock` 重叠），不是按「相关不相关」的感觉拆的。
合并后整套 1978/31，31 条全在基线里。**并行省下的不是时间，是「四个改动互相踩」那种
只有串行才能避免的错——它被 worktree 隔离顶替掉了。**

**第六课：同一份数据，三种前提，三个结论——前提是什么，要问出来，不要猜。**
两次需求普查拿 82 个 `.rh` 脚本当语料。我先默认「那是本产品的脚本」（按它排了八个里程碑），
政委说「rh 是另一个仓的」时我改判「那是别人的语料，排序依据不成立」，
政委再说「rh 归档、体系转 `.qjs`」时它变成**「那是迁移语料，排序恰好是对的」**。
数据一个字没变，结论翻了三次。**每次翻转都来自一句我问不出来、只能等的产品判断**——
所以量之前要问「这是谁的语料、要拿它做什么」，问不到就把**两种读法都写下来**，
而不是选一种当唯一结论。

**第五课：一条注释预言了自己的未来，而那正是它该做的事。**
`slot.rs` 里那条兜底臂写着「`GuestFault` 是 `#[non_exhaustive]`，
将来的上游可能在同一个字上记第四种原因」——2026-08-29 它真的来了，
而下游**一行都不用改结构**，只要把预留的位置填上。
**一个「我不认识的将来」写进代码里，比一个 `match` 穷举更耐久**：
穷举会在上游加变体时**编译失败**，兜底臂会在那之前**继续给出真话**
（「引擎报了个我不懂的原因，这是核看到的 trap」），
然后由人决定要不要把它升级成一句更好的话。

**一条待办，决定是对的而诊断是空的。** String 上读 `length` 以外的属性会 trap，
理由写在 `runtime.rs` 里且成立：`"ab".toUpperCase` 在 ECMA-262 里**是个真函数**，
返回 `undefined` 是「错答案穿着对答案的衣服」。但 trap 出来只有
`unreachable executed`，**说不出是哪一类事**。fault word 现在三个码
（预算 / 脚本抛了 / 引擎坏了），这是**第四类**：运行期撞上引擎能力边界。
按 fault word 自己的立论——「分不清『你的脚本抛了』和『你的脚本坏了』的宿主，
会告诉作者错误的事」——这一类该有自己的码。

走廊在 2026-08-29 当天接通到了 CLI：规格串解析成什么由**本产品**说了算——
编译器上游一个文件都不读——policy 是「project root 下的一条路径，扩展名省略」，
而那个 root **默认就是入口文件自己的目录**，所以旁边有 `lib/` 的脚本写
`import * as lib from "lib/fleet"` 什么都不用配。这在实践上就是 ECMA-262
「相对导入者」的形状，而 `--project-root` 是需要伸远一点时的那个旋钮。
逃出 root 的规格串被拒，判据落在**规范化之后**的路径上——`../`、符号链接、
绝对路径因此是同一件事，而不是三件各自要写对的文本判断。

模块（rev `8bbdf2d`）加的正是那条走廊，而且它落在第 ① 间：`import` 是**编译期取入**，
不是装载期链接——第 ④/⑤/⑥ 间完全没变，仍是一个 `.wasm`、一道装载门、一套 `Limits`。
「做对」的部分只有一个词：**命名空间**。`format!` 把库的顶层名字倒进脚本作用域，
`import` 把它们收进一个对象——两者的差别就是这张图上「房间之间有没有墙」。

实现时那堵墙**先建反了两次**，两次都值得记：
把模块作用域挂在脚本作用域下 → 模块能读到导入者的名字（等于没墙）；
命名空间的引用带了导入者的偏移量 → 落在模块自己的 `const` 之前，被 TDZ 正确拒绝。
**「隔离」不是一个布尔值，它有方向，两个方向都要测。**


## 三波迁移的过程

最后一条是这套图迄今最强的一次自证：`map` 的循环之所以待在第 ① 间，是因为大家都以为
「第 ③ 间的家具够不到第 ⑥ 间的函数表」——**那条前提是错的**。把循环搬回第 ③ 间，
代价降到 1/3.4，而整场判决的结论跟着反了过来。
**「放错房间」不只是代价问题，它会让你把结构问题误读成方案优劣。**

第一波迁脚本（2026-08-29，6 组并行，每组由另一个核验者**照着报告里的证明命令重跑**）
教的不是哪个函数缺，是**证明本身怎么失效**——六种，每种都在这张图上有房间：
① **证明命令照抄跑不出来**：build.qjs 的命令原样跑是 `host_hard_timeout`，默认 wall 是
2 000 ms（`script_protocol.rs:104`），报告写成「~60 s」——那是门里 `process.command` 的
子进程超时，另一个房间的数字。② **空证**：lint 自测说「夹具被 `cli script check-many`
拒了」，可那条动词是对一切输入 exit 2 的桩，worker 换成 `/usr/bin/false` 照样 PASS——
只看退出码非零的断言什么都没证。③ **归因错的「未证」**：build-all 说缺 cargo-zigbuild /
cargo-xwin，机器上全装着；真因是它逐字复刻了已退役的 `rh task run client-build`，任务表里
根本没有 client-build——搬家把死门牌一起搬了。④ **静默收窄**：`rh_compat.absolute`
不规范化 `.`、不认 `C:\`，rh 接受的 `./target` 被拒、Windows 车道的每个 REPO 参数都会拼到
cwd 底下，而文件里还留着 rh 的注释说「两种写法都规范化」——注释比代码活得久，就成了
反证据。⑤ **夹具过了、实输入过不了**：四个脚本在 mini repo 上全绿，对真仓一律
`budget exhausted: max_steps`——证明的单位是「输入」不是「脚本」。⑥ **合并不等于合对**：
两支各自给 `rh_compat` 追加了同一个 `sha256_file`，git 判无冲突，模块却导出了两次；自测钉着
CLI 的旧句子「the script threw a value」，而 main 在分支底下改了句子（`2cde8b63`），分支各自
全绿、合到一起先红。六条里没有一条是「脚本写错了」，全是**证据放错了房间**：把门的超时当
CLI 的、把桩的退出码当判决、把工具链当死因、把注释当规格、把夹具当输入、把「无冲突」当
「已合并」。核验者只做一件事——照着写的命令再跑一遍——六组里四组因此翻案；核验的成本是
原报告的零头，而它抓到的每一条，下一波都会变成别人的基线。

第二波（2026-08-29，7 组 19 个句柄门脚本，仍是「一组写、另一人照命令重跑」）把上一段的
结论推进了一格：**证明命令全部复现，判决仍可以不成立**。七组 51 条证明 51 条照抄跑出了
同一行，而两组被判「不成立」——翻案的不是证明，是核验者自己另起的输入：给
control-center-linux 一个真服务器，第一条 `protocol-info` 答 61 528 字节，`bounded_record_text`
的 `slice` 在引擎里是 exit_class=configuration、try/catch 接不住，于是脚本跳过的恰是它存在的
目的（bundle + 孤儿合同）；给 target-report 一个绝对路径，`rh_compat.absolute(".")` 答
`<cwd>/.`，`repo_local` / `cleanup_allowed` 答反。两条都在脚本的**书面用法**之内，报告的证明
只是没走到那里——证明的单位仍是输入，而「照抄能复现」只证明了报告没撒谎。第二课在库的
合并上：五组各自给 `test_harness.qjs` 追加了一段（52 / 68 / 195 / 118 / 94 行），git 在文件尾
把第三段的追加切成 **5 个冲突区**与前两段交错，又在第二段把两边共有的收尾 `}` 拉到冲突区
外——两处都是「自动合并成功」的形状，都被那条一行的 importer 抓住（``needs a `}` to close
the function body opened at byte 13832``）；最后的合并规则是「ours + 分支相对 merge-base 的
纯追加」，先用 `cmp` 证明追加是纯的再拼。跨组唯一重名 `append_command_record_bounded`
（同签名、两种截断），按先合者留：workbench 的 `ui-snapshot` 记录从 `<…omitted:13108 chars>`
变成 548 字符加 rh 的截断标记，manifest 照写——重名不是错，但**保留哪个是行为**，得测。
第三课是数字：同一堵墙被写了五遍——五组各自写了一个不用 `slice` 的截断
（`_words` / `_safe` / `_lines` / `_cheap` 加 control-center-b 的本地版），而 main 在分支切出之后
已经用 `5adcc238` 把 `slice` 落了地；「门的 `process.wait` 只答一次」也被写成五种拼法
（`wait_collected` / `child_wait` / `wait_once` / `forget_child` / `drain`）。一条门事实在库里
有五个名字，比缺一个名字更难维护。最后是步数：三组独立量出同一条曲线——61 KB 的
`protocol-info` 一次 JSON.parse 约 10–16M 步、384 KiB 的 `script api --json` 单独超 50M、
一张 822 行的 `ps` 表 30–40M——在 macOS 上真能跑的旅程（native-ipc 把答复过滤后打出
PASS）卡住的不是门缺口，是 100M 的天花板；这条曲线现在有三个互不相识的测量，可以当
基线用。工作区 118 条 `test result`，1 987 过 31 败，败集与基线逐名相同。

第三波（2026-08-29，7 组：一个驱动 `check.qjs` 加 `qualification` 库、一个 `prune-target-incremental`、五组只做「证」不做「迁」，仍是一组写、另一人照命令重跑；7/7 判成立，七次 `--no-ff` 零冲突，每次合并后那行 importer 都答 `functionfunction`）教的是**迁与证隔着什么**：迁完的标志是 `script check` 答 OK、corpus-scan 83/83，证完的标志是 PASS 那一行真的打出来——两者之间的距离可以数。十条 GUI/服务器旅程在 macOS 上走到 PASS 的只有两条：server-smoke（7 STEP，`PASS: headless server owns PTY, parser, workspace, events, and no HWND`）和 wake-smoke（3 STEP，`PASS: coalesced wake delivery preserved IPC, PTY, and mutation correctness`）；两条都不是门补了口，而是脚本自己把 Windows 形状（`cmd.exe`、`for /L`、powershell TcpClient）改成宿主形状、把 `pgrep` 见证换成 `process_pid(handle)`、再从 100M 步里省出预算——server-smoke 从「80–100M 才够、最后一步撞死」降到 60M 内打出 PASS、100M 内收尾；wake-smoke 65M 撞死、80M 过。省的办法只有一种：不让 61 KB 的 `protocol-info` 走门的 JSON 信封（信封解析 8–12M 步，`sh -c 'exec … >FILE'` 后从文件读 4–6M），把记录预切到 513 字符再交给 `append_command_record`。其余八条各停在一处，停点**没有一处在脚本里**：startup 与 unix-frontend 停在第 1 步要 `process_platform_facts(handle)`，workbench 停在第一次点击要 `process.window_pointer`；native-ipc 停在第 2 步是产品回归——`connect()` 先建父目录再查 103 字节，EROFS 抢在 `InvalidEndpoint` 前（`6043a7df` 引入，rh 的断言原样保留，把它换成 true 则 16 步全过）；working-context 停在第一个 STEP 之前，unix 前端没有 `open-proxy-editor`；fleet 停在第 2 步要 Windows 的 `pipe:`；cli-smoke 与 theme-smoke 停在 100M 天花板——23 条与 28 条 CLI 记录后 `budget exhausted: max_steps`，而整段旅程要 ~150 条与 ~75 条，且 cli-smoke 即便预算无限也到不了第 4 步：macOS 的 GUI 自带 IPC 服务器（`start_ipc_server`），附到预先起的 server 上答 `EndpointInUse`，「GUI 作客户端附到无头 server」这个形状只在 Windows 存在。第三波量出的价格表：步数按字符计——`String.length` 约 30 步/字符且每次读都重算、拷贝/拼接 50–67 步/字符、JSON.parse 110–130 步/字符；`fs_symlink_metadata` 一次 10–20k 步，224 项目录的 `fs_read_dir` 超 1M，所以 prune 在 14 代 incremental 上过、在 1 306 代上单是枚举就超 100M；harness 的日志「整读 + 拼 + 整写」是 O(n²)，80 条记录 ≤100M、150 条超。所以「证」比「迁」多给出的是三张带命令和输出行的清单：门缺的口（`platform_facts`、`window_*`、`fs.append`、`stdout_path`、TCP、tree digest、advisory lock）、产品的坑（GUI 不能只做客户端、autostart 只在 Windows、connect 的检查顺序、unix 前端少一条 ui-action）、引擎的价格表——一条旅程停在哪一张清单上，比它「已迁移」有用得多。


## 历次复测

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

### 编译器迁出后的重测（2026-08-24，同机同工具链）

编译器迁往 `tinyvm-qjs` 之后，本 crate **53 passed, 0 failed**。少掉的 20 条不是删的，
是跟着被测代码走了：`tests/qjs_m0.rs` 的 23 条与 LEB128 编码器的 2 条单元测在上游
`crates/tinyvm-qjs/tests/qjs_subset.rs` 与 `src/encode.rs` 里原样跑（上游同批新增 5 条
`tests/compile_options.rs`，锁两种取名模式与诊断的窄化）。

| 目标 | 数 | 变化 |
|------|----|------|
| `src/lib.rs` 单元 | 23 | −2（LEB128 随编码器迁出） |
| `tests/qjs_guest.rs` | 6 | 新写：`.qjs` 端到端过槽、编译失败自成一类、产物过本 crate 装载门、扩展名路由。**替代**迁走的 `qjs_m0.rs`——语言子集归上游测，本 crate 只测自己的接缝 |
| `tests/host_door.rs` | 9 | — |
| `tests/budget.rs` | 6 | +1：活动记录槽耗尽必须报 `Budget`（补上那条会抓住静默漂移的测试） |
| `tests/wasm_slot.rs` | 5 | — |
| `tests/isolation.rs` | 4 | — |

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

### 抬 rev 到 `df8decd` 后的重测（2026-08-24，同机同工具链）

上游从 `f694733` 抬到 `df8decd`（5 个提交），`.qjs` 从整数表达式长成真正的 M1/M2 子集。
本 crate **58 passed, 0 failed**；根 crate 的 `script_engine` 另加 2 条（feature 开时跑）。

| 目标 | 数 | 变化 |
|------|----|------|
| `src/lib.rs` 单元 | 23 | — |
| `tests/qjs_guest.rs` | 11 | +5：JS 值五种全过脸、字符串在槽死后仍可读、binary64 算术、约定不匹配被拒、本仓能力声明的文档锁 |
| `tests/host_door.rs` | 9 | — |
| `tests/budget.rs` | 6 | — |
| `tests/wasm_slot.rs` | 5 | —（手写 `.wasm` 路径**一行未改**仍全绿，这是「两套约定同存」的证据） |
| `tests/isolation.rs` | 4 | — |
| `src/script_engine.rs` 单元 | 2 | 新写：completion value 到得了调用者；`check` 与 `execute` 对子集口径一致 |

三条被抬掉的 M0 断言，逐条说明是**变强**还是**变弱**：

| 原断言 | 现在 | 强弱 |
|--------|------|------|
| `$0*2+2` 传 `Value::I32(20)` 得 `I32(42)` | 传 `JsValue::Number(20.0)` 得 `Number(42.0)`，另加约定不匹配被拒的测 | **变强**：多锁了「不按位重解释」这条 |
| `let x = 1` 必须被**拒绝** | 已支持，改为断言它跑出正确的值；拒绝测换成 `%` / `typeof` / 捕获外层局部的闭包（三条都是编出来确认的） | 中性：锁的对象从「M0 的极限」换成「今天真实的边界」 |
| `$0/0` 必须 **Trap** | `Infinity`。数字是 ECMA-262 binary64（6.1.6.1），旧断言锁的是 i32 除法的限制 | **变强**（是修正不是放宽）：另加 `0/0=NaN`、`2147483647+1` 不回绕、`-z` 保留零的符号 |

### 抬 rev 期间实测到的两处「源码看着支持、编出来不支持」

按本 PRD 的断言纪律，能力声明一律编译验证，不读源码断言。这一轮抓到两条：

| 声明 | 实测 | 结论 |
|------|------|------|
| 上游 README「函数：声明与表达式，具名或匿名」 | `let g = function (a) {...}; return g(21);` 被拒——"does not support using a function as a value"。`return f;`（f 是函数声明）同样被拒 | 函数表达式**只有立即调用**（IIFE）可用。文档口径已按此收窄 |
| 上游 README「`-0` 与 `0` 不同」 | 整体成立（`-(1-1)`、`1 / -z` 都给 `-Infinity`），但**字面量写法 `-0` 给的是 `+0`**（`1 / -0` = `Infinity`）。一元负号作用在数字字面量上时丢了零的符号 | 上游缺陷，已记录；不在本仓文件域，未改。上游 conformance 套件测了 `-(1-1)` 与 `0 * -1`，没测 `-0` 本身 |

**范围诚实声明（2026-08-24 当时）**：`.qjs` 是一个真实但很小的子集，能力清单见
`crates/agenterm-qjswasm/README.md`（每条都有测试）；`.wasm` 侧是完整的。
`.qjs` **还够不着 `agenterm.*` 门**——自由名字在编译期就被拒，门今天只有手写 `.wasm`
客人能调。**最后这句已于 2026-08-25 作废**，见下文「门落地 + 门被攻击后的重测」；
同表的 `-0` 字面量缺陷也已由上游 `1cba206` 修好（`let z = -0; 1 / z` → `-Infinity`）。

### 接缝对抗审查后的重测（2026-08-25，同机同工具链）

一轮专门针对接缝（`Value::Js` 那张脸、每槽的 `Convention`、「字符串在槽死前被读成宿主
数据」这条承载性声明）的对抗性攻击，找到八条缺陷。**那条承载性声明扛住了每一次攻击**：
13 个 f64 边界值按位往返（NaN 载荷 `0xfff8deadbeefcafe`、信号 NaN、`-0` 的符号全保住）、
空串、内嵌 NUL、代理对拼出的星平面字符、从**长大过的**线性内存里取出的 1 MiB 字符串
（证明接缝读的是活视图而不是实例化时的快照）、第一次调用的串不被第二次调用和 `kill` 打扰。
没有任何一次 panic。

八条缺陷里**七条已修**（详见 §隔离与预算 与 §Capability tree 的对应条目），一条修不了：

| # | 缺陷 | 处置 |
|---|------|------|
| 1 | 参数**个数**不对 → `Trap("function")`，怪客人 | 修：`Signature`，报两个数 |
| 2 | 参数**类型**不对 → **不报**，还能返回签名禁止的类型 | 修：`Signature`，进客人之前 |
| 3 | 导出不存在 → `Trap`，且不带名字 | 修：`NoSuchExport`，带名字 |
| 4 | 运行期超页 → `Trap("unreachable")` 而非 `Budget` | **修不了，上游**，见 §隔离与预算 |
| 5 | `SlotId` 不绑 Engine：跨 Engine 静默跑错槽、静默杀错槽 | 修：进程级 engine tag |
| 6 | 脸拒收返回值时，客人已打印的输出被丢掉 | 修：改成进客人**之前**拒，没得丢；残留代价写进 `Slot::call` |
| 7 | 接缝拷出的字符串没有任何宿主侧盖子 | 修：`max_result_string_bytes` |
| 8 | 五条恶意指针防线从公开面**不可达**，无对抗覆盖 | 修：`Guest::CompiledQjs` 让它们可达，补五条攻击 |

第 6 条值得单说，因为**没按提出者建议的方式修**。建议是「让输出活过这次拒绝」，
实现上等于把它留到**下一次**调用的 `Outcome` 里——而 `slot.rs` 早就写明那比丢掉更糟
（张冠李戴）。改成查导出的**声明结果类型**，在客人还没跑的时候就拒，于是根本没有输出
被产生。这比原诉求更强：不是「输出活下来」，是「输出压根不用产生」。

第 8 条值得单说，因为它与 `agenterm-qjs` 门 3 的前置**是同一个变体**。接缝审查要它是
为了让 `read_guest_string` 的五条拒绝可测；CLI 判决要它是为了让「编译落盘再加载」不丢
约定。两条互不相干的路指向同一个 `Guest::CompiledQjs`——这是它形状对的强证据。
提出者同时建议在 `spawn` 加签名嗅探（「看着像 V1 就当 V1」），**这条明确不采纳**：
`(i32, i64, …) -> (i32, i64)` 是完全普通的手写 wasm 类型，嗅探就是猜，而
`Convention` 的纪律原文就是「记下来，绝不靠猜签名」。

```sh
cargo test -p agenterm-qjswasm    # 86 passed, 0 failed, 1 ignored
```

| 目标 | 数 | 变化 |
|------|----|------|
| `src/lib.rs` 单元 | 23 | —（其中 `a_foreign_import_is_left_alone_at_install` 改写成 `..._is_refused_at_install`，记下反转理由与实测前后） |
| `tests/qjs_guest.rs` | 12 | +1：产物重载与源码直跑值相等（门 3 第 0 步的验收测） |
| `tests/host_door.rs` | 10 | +1：`check` 与 `execute` 对绑不上的 import 给同一个答案 |
| `tests/budget.rs` | 6 | — |
| `tests/wasm_slot.rs` | 5 | —（两条改写：缺导出从 `Trap` 改成 `NoSuchExport` 并要求带名字） |
| `tests/isolation.rs` | 4 | — |
| `tests/seam_attack.rs` | 26 + 1 ignored | 新文件：13 条「攻不破」的正面锁 + 7 条已修缺陷的回归锁 + 5 条恶意指针攻击 + 1 条上游缺陷的复现（`#[ignore]`） |

`#[ignore]` 只用在第 4 条上，并且**没有被反转成断言错误行为**——那会把一份缺陷报告变成
一把锁住缺陷的锁。（那条 `#[ignore]` 已于 2026-08-25 撤销：上游把信息补出来了，测试
现在断言 `Budget("max_memory_pages")`。全仓 `#[ignore]` 归零。）

### 门落地 + 门被攻击后的重测（2026-08-25，同机同工具链，上游 rev `f8adef8`）

三件事按顺序发生：`.qjs` 接上门（rev `6920c60` 的 `Names::Declared`）、门被专门攻了一轮
（26 条攻击测，五条缺陷）、缺陷里能在本仓修的三条修掉（rev 抬到 `f8adef8`）。

```sh
cargo test -p agenterm-qjswasm            # 133 passed, 0 failed, 0 ignored
cargo test --test fleet_catalog_conformance   # 12 passed, 0 failed
cargo clippy -p agenterm-qjswasm --all-targets -- -D warnings   # clean
cargo fmt -p agenterm-qjswasm --check                            # clean
```

### 数组落地后的重测（2026-08-26，上游 rev `577af37`）

```sh
cargo test -p agenterm-qjswasm                                    # 138 passed, 0 failed
cargo test --features "script-qjs,script-qjswasm" \
  --test script_engine_equivalence                                # 6 passed（零具名分歧）
cargo test --features script-qjswasm --test qjs_produces_a_fleet_operation  # 5 passed
cargo test --test script_fleet_facade_parity                      # 5 passed
cargo test --workspace --exclude agenterm-abi                     # 781 passed / 4 failed（全为既有）
```

那 4 条既有失败：2 条 `platform::boundary_tests`（产品名泄漏，`472ff12d` 引入）+
2 条 `script_process` 收孙进程（timing flake，单跑同模块失败成员会换）。与本轮改动无关，
按「diff 失败集合不是失败计数」核对过。

新增的 3 条（135 → 138）是**本仓 README 数组文案的锁**：
`the_array_claims_in_this_crates_own_copy`（22 条断言）、`the_array_claims_that_are_traps`、
`an_array_does_not_cross_this_crates_face`。写第三条时发现上游加了 `TAG_ARRAY` 却没加
`host_decode` 里给它具名的那一臂——本层收到的是 `V1: unknown tag 7`，读起来像引擎有毛病。
上游 `577af37` 修掉，本层断言改成整句相等而不是 `contains`（后者在错答案上会通过）。

| 目标 | 数 | 变化 |
|------|----|------|
| `src/lib.rs` 单元 | 25 | +2 |
| `tests/qjs_door.rs` | 13 | 新文件：`.qjs` 真的到达 bridge、真的取回答案、import 表逐字节解码出来与门的 `SIGNATURES` 一致 |
| `tests/door_attack.rs` | 31 | 新文件：对门的敌意脚本 / 敌意桥 / 预算边界；含五条缺陷的复现，其中三条现已是修复后的回归锁 |
| `tests/seam_attack.rs` | 27 | +1 且 **`#[ignore]` 归零**：FINDING 4（运行期超页）从「上游挡着」变成断言 `Budget("max_memory_pages")` |
| `tests/qjs_guest.rs` | 12 | — |
| `tests/host_door.rs` | 10 | — |
| `tests/budget.rs` | 6 | — |
| `tests/wasm_slot.rs` | 5 | —（手写 `.wasm` 路径一行未改仍全绿：门加了声明层，ABI 一个字节没动） |
| `tests/isolation.rs` | 4 | — |

**攻击找到的五条，逐条处置：**

| # | 缺陷 | 处置 |
|---|------|------|
| 1 | 桥的答案撑爆客人堆 → `Trap("unreachable executed")`，且槽从此报废 | **已修**：抬 rev 到 `f8adef8`，客人自报堆耗尽 → `Budget("max_memory_pages")`；不自愈这件事写进 `Engine::call` 文档并有测 |
| 2 | 桥 panic 直接穿出 `Engine::call`，`run_once` 漏掉它承诺回收的槽 | **已修**：门接住 → `Door`（带 panic 原话与当时的 op）；`run_once` 另加回收-再抛的 finally |
| 3 | 光提一个零参数门函数就等于调用它（`typeof fleet_result` 给 `"string"`） | **上游**：`tinyvm-qjs` `emit.rs` 的「裸宿主名 = 零参调用」规则，`Names::Declared` 之后不再无害；测试留着，头注说明修好后改成什么 |
| 4 | `check` 收下 `execute` 装不进去的脚本 | **已修**：`check_qjs` = 编译 + 过装载闸门 |
| 5 | 运行期类型不符在门上是一条裸 `unreachable`，说不清是哪个参数 | **上游**：`repr.rs` 的 `unbox_string` 需要一个核能携带的 trap 码 |

**攻不破的部分同样记下来**（每条都是正面锁，不是没测）：空串 / 内嵌 NUL / 100 KB 参数
字节不变；堆上拼出来的串与字面量池里的串在门上一模一样；参数从左到右求值恰好一次
（`Bytes` 取回会把 raw 参数压两遍，「先求值后拆包」是唯一不重复副作用的顺序）；
`max_steps` 卡在两趟取回的任意一刀上，要么给出完整答案要么报 `Budget("max_steps")`，
从不给半截字符串；pending buffer 每槽一份；脚本自己定义的 `print` 遮住门而不是反过来。

`.qjs` **能力口径本轮的变化**：够得着门了（见 §归档门），`%` / `typeof` 可用，
`-0` 字面量符号已由上游修好。仍然撞墙的最显眼一条是 **Number→String**：
`"status:" + s` 与 `"{\"x\":" + x + "}"` 都 trap，这是今天写 fleet 调用最先遇到的墙。

### M0 期间在 tinyvm 上的实测发现（写下来免得再踩）

| 发现 | 依据 |
|------|------|
| `Module::instantiate()` **已经跑过 start 函数** | `wasm.rs` `Instance::new`；再调 `run_start()` 会跑第二遍 |
| `Limits` 随解码后的 module 进入 `Instance`，逐调用自动重置步数 | 所以槽的 `call` 不需要再传预算 |
| `memory.grow` 超 `max_memory_pages` **返回 `-1`，不 trap** | `wasm.rs` ~7580-7613，符合 wasm 规范 |
| 装载期 `initial_pages > max_memory_pages` 是**另一条**拒绝路径 | `wasm.rs` ~4821，属 `Load` 类而非 `Trap` |
| 客人活动记录走 `Vec<DefinedActivation>` 蹦床，**不吃原生栈** | `wasm.rs` ~6458-6730；深度测试因此可以设到默认值的 40 倍 |
| `WasmError` / `Limits` **都不实现 `Debug`** | 核是 `no_std` + fmt-free；下游要手写 `Debug` |
| `Trap("call stack")` 覆盖 ≥3 种不同条件，`Trap("memory size")` ≥2 种 | 24 / 14 处调用点；下游无法区分——**已修**：上游拆成 `activation slot limit` / `operand stack` / `call stack allocation` / `activation slot overflow`，并给出 `WasmError::class()` / `ceiling()`，下游不再匹配文案 |
| `Trap("no exported function named \`")` 文案被截断，尾随一个孤立反引号 | `wasm.rs` 5782 / 8442；fmt-free 核填不进名字——已派单 |
| 抄文案表的代价是**静默**的 | 上游拆分 `"call stack"` 时，本 crate 的分类表不报编译错、测试也全绿，只是分类悄悄错了。存取器不会这样漂——已按此改写 `src/slot.rs::classify` |


## 接下来：按「解锁什么」排，不按工作量排

现状一句话：**引擎可用**，边界从「没有闭包、没有标准库」缩到了「**没有标准库**」——
闭包（`68afb35`）、数组（`048bcf2`）、整个 DecimalLiteral（`ab29522`）、模板字面量
（`653cebe`）、箭头函数（`ee3842b`）都到了。写 fleet 绑定和自动化脚本够用且顺手；
缺的是**方法**，不是语法。

**01 · ~~迁移两处生产调用点 / 归档 `agenterm-qjs`~~ —— 2026-08-28 已完成，本条留档**

`agenterm-qjs` 的三条归档门全绿：`qjs` 现在是 `qjswasm` 的一个弃用拼写，worker 里的
qjs 分发已删，`agenterm qjs` 别名退休成一条重定向（exit 2），
`the_old_adapter_is_unreachable_from_the_environment` 断言生产里没有任何路由能回到旧引擎。

**那一步已经做了。** 2026-08-28 摘除 `crates/agenterm-qjs`（216K）、`script-qjs`
feature 与可选依赖、workspace 成员、`QjsEngineBackend` 适配层、注册表里的那一支、
以及 37 处 `#[cfg(feature = "script-qjs")]`。

**收益是不可逆的那一条**：`rquickjs` 从依赖树里消失了——`Cargo.lock` 零条目，
`cargo tree -i rquickjs` 报「找不到包」。这正是当初把它排第一的理由：
**唯一一条能删掉一整个外链 C 依赖的路**。

**两处兼容面**故意留着，而且都有断言：
- `AGENTERM_SCRIPT_BACKEND=qjs` 仍然可用，解析到 `qjswasm`
  （`script_backend::qjs_backend_from_env`）；
- `agenterm qjs <verb>` 仍然打那条重定向并退 2
  （`script_cli_verb_parity::qjs_alias_is_retired_and_names_where_its_verbs_went`）。
  这条**不再挂在任何 feature 上**——它指向的引擎已经不在了，重定向反而更该在。

**跟着走掉的证据**：`tests/script_engine_equivalence.rs`（六条一致、零分歧）与
`script_engine.rs` 里的 `gate_two_trait_equivalence` 是门 1 的证据，它们比较的
另一半没了，所以也一起摘除。**结论留在本文件里**——这是「证据可以退休、结论必须留档」
的一个实例，两处代码里都写了指回这里的注释。

~~`agenterm-wasmcore` **不动**：它的门 2 未绿（见 03），没有授权。~~
**2026-08-28 作废并执行。** 政委重申「qjs 和 wasmcore 这两个 crate 都归档」，授权到位。
门 2 到归档时也已不是阻塞——03 记的三次改写把它推到了「两扇门同名同形、同一份客人
字节两边都跑」。

**02 · ~~方法：接收者要怎么绑上去~~ —— 2026-08-28 已判决并落地，本条留档**

判决轨 `research/method-binding/` Q1 **已判决：调用点特化**。
三种做法（`this` 走调用约定 / 属性读时装进闭包 / 调用点特化）全部实现、
全部通过一份**在任何实现之前写好**的语料，再按边际成本比。
输的两种连同它们的 feature 一起删了；赢的那种转正，
`trim` / `indexOf` / `push` / `pop` / `map` 现在是本引擎的真能力（rev `21d8d9a`）。

**判决路径**：加第五个方法的边际三方全 0 → 两条斜率各赢一条、互不支配 →
边际行数打平（各 1 行）→ **泄漏清单最短者胜**。截距同样指向它。

**最值钱的一条**（已写进 `.claude/skills/decisive-experiment` §2.6.1）：
赢家一度读成输家的 **5.4 倍**，眼看要判负——它之所以那么贵，是因为绕开了一条
**共同前提**（「这一层做不到 `call_indirect`」），而实现另一个变体的过程
**证明了那条前提是错的**。给它换成同样的做法后那个数降到 1/3.4，**判决因此反转**。
所以「可修性检查」不是对自己那个变体做一次：**任何变体推翻一条共同前提时，
所有变体都要重测**，否则是「谁先被实现」在决定判决。

**转正前补掉一个研究期可以容忍、上线不能容忍的缺口**：`trim` 原本只认十个具名空白，
不认 `Zs` 其余。研究期三个变体同价、不影响比较；上线则是**错答案**——
`"\u{2003}a".trim()` 会留着那个空格且无任何诊断。

**还缺的方法**：其它字符串方法与数字方法（读即 trap）、数组的 `filter`/`join`
（读为 `undefined`，调用才 trap）。加一个方法今天是**一行注册 + 一个方法体**，
且**不调它的程序付 0 字节**——这正是判决要挑的那个性质。

**但「继续加方法」不是下一步，因为没有需求。** 数过了：

```sh
cat scripts/qjs/lib/*.js scripts/qjs/lib/*.qjs | grep -oE '\.[a-zA-Z_][a-zA-Z0-9_]*\(' | sort | uniq -c
#   16 .stringify(   2 .parse(   2 .fleet_call(   2 .set_note(
```

本仓自己的脚本一个字符串/数组方法都没用；`research/value-representation/corpus/`
的十几个 `.qjs` 也一个都没用。**唯一在用的是 `JSON.stringify` / `JSON.parse`，
早就有了。** 文档里那个 `.join(` 是散文里的 Rust `server.join()`，不是 JS。

所以再加方法是**替想象中的脚本做的**。按 `count-the-boring-thing-first`：
先数，再说「只差 X」。**等有脚本真的写了 `.split()` 再加它**——那时的成本是
一行注册加一个方法体，不会因为等了而变贵。这条判决恰恰把「再加一个」变成了
**可以安全推迟**的事。

**03 · wasmcore 门 2：阻塞点第三次改写（2026-08-28 实测）**

这条被改写过三次，每次都因为**量了一下**：

1. 「等一个 `_start` 入口约定」→ **伪问题**（2026-08-27）：本引擎调任何导出名。
2. 「wasmcore 的客人是 std 编到 `wasm32-wasip1`，import 了 WASI；把它改成
   `no_std` 只吃 `agenterm.*` 就行」→ **不充分**（2026-08-28）。
3. 真正的阻塞点：**两扇 `agenterm.*` 门不是同一扇门。**

实测——把一个「只 import `agenterm.*`、导出 `memory`/`_start`/`wasmcore_alloc`」
的客人（也就是第 2 条要求的那个成品）喂给 qjswasm：

```
host door: guest declares `agenterm.fleet_call` with the wrong signature:
the door takes 4 i32 parameter(s) and returns 1
```

**不是 WASI 的抱怨，是签名的抱怨。** wasmcore 的 `fleet_call` 是**六个参数**
（宿主经 out-param 写回答案），qjswasm 的是**四个**（状态码 + 第二趟取回）。

**而且不可移植的方向是单向的**，理由是结构性的、`src/host.rs` 头部早就写着：
wasmcore 那套要宿主**回调进客人**的 `wasmcore_alloc`，而 tinyvm 的类型化宿主回调
在整个调用期间持有客人内存的 `&mut`——**它做不到重入**。
所以 qjswasm **长不出**六参数那套；而 wasmtime 两套都能做，它可以改成两趟。

**门 2 的迁移成本因此是「改 wasmcore 的门」，不是「改客人」。** 这是一件
明确、可估、不需要谁拍板的工程，而且方向只有一个。

### 那件工程做完了大半，**门只剩一条差距**（2026-08-28）

**wasmcore 的门现在与 qjswasm 的门同名同形。** `agenterm.fleet_call` 在两边都是
同一个四参数第一趟，后面同样跟着 `fleet_result_len` / `fleet_result`；
原来那套六参数的**行为一字未改**，只是换了个说明用途的名字 `fleet_call_into`
（宿主写**进**客人）。

**为什么是改名而不是退休**：`agenterm.fleet_call` 这个名字应当在两个引擎那里
是同一件事，而一个 `(module, name)` 只能绑一个函数——所以端口名必须让给可移植的
那套。但一次调用比三次少两趟过界，在不要求可移植的地方仍然值得有，所以它留着。
**改名代价事先数过**：一个客人加五个测试/示例文件，全在仓内，随包 `.wasm` 语料为零。

**两条实测的锁**：
- `agenterm-wasmcore/tests/portable_door.rs` 三条，客人手写 `wat` 且**自己校验自己**
  （WASI 把退出码限死在 `[0,126)`，装不下字节数；会校验的客人本来也是更好的证人）：
  答案原样跨界、目的地太小是**拒绝不是截断且答案仍留着**、无桥是状态 2 加诊断。
- `agenterm-qjswasm/tests/host_door.rs::the_other_engines_portable_door_imports_load_here_unchanged`
  ——把上面那个文件的 import 块**原样**拿过来，qjswasm 收下。
  **一个 import 块，两个引擎。**

### 报告约定也通了：**同一份字节，两个引擎，同一个答案**

原来两边的入口/结果约定不同——wasmcore 只调 `_start` 且不取返回值，所以它的客人
为了报告去用 WASI 的 `proc_exit`，而 qjswasm 拒 WASI。**wasmcore 长出了
`run_export`**：调具名导出、取回一个 `i32`，与 qjswasm 同形。

于是一个**只 import `agenterm.*`、用具名导出返回数字**的客人两边都能跑。
三处锁：

- `agenterm-wasmcore/tests/portable_door.rs::a_guest_that_imports_only_the_door_reports_through_a_named_export`
- `agenterm-qjswasm/tests/host_door.rs::the_same_guest_bytes_run_here_and_at_wasmcore`
- `tests/portable_guest_two_engines.rs::the_same_guest_gives_the_same_answer_at_both_engines`
  ——根 crate 同时看得见两个引擎，**同一份 `wat` 源、同一个桥、比对两边的返回值**。

客人源在两个 crate 里各存一份（两个 crate 互不依赖——**要靠链接对方才能共享的门
不是门，是耦合**），
`the_portable_guest_source_matches_wasmcores_character_for_character` 逐字符对账。

### 门 2 要的「同客人性能对比」——现在能做了，做了

200 轮，每轮含各自的装载 + 实例化 + 一次过桥：

| 引擎 | 200 轮 | 每轮 |
|---|---|---|
| `agenterm-wasmcore`（wasmtime + WASI，JIT） | 414.8 ms | **2.07 ms** |
| `agenterm-qjswasm`（tinyvm，无 JIT） | 30.5 ms | **0.152 ms** |

**qjswasm 快 13.6 倍**——但**必须连着读它量的是什么**：这个数由**启动**主导，
不是执行吞吐。wasmcore 每轮起一个 worker 线程并建一份 WASI 上下文；wasmtime 的
JIT 在这么小的客人身上没有时间把编译成本挣回来。**换一个计算密集的客人，
结论很可能反过来**，这里没有任何数据说不会。

它支持的是更窄、但对本产品恰好相关的那句话：
**对一个只过一次门的短客人，解释器是更便宜的引擎。**

**门 2 的两半——「同一个客人两边都跑」与「同客人性能对比」——都有了。**

### 门 1 判定（2026-08-28，正式判一次）

这条门挂着「可判绿」很久，从来没真判过。判之前先**数**，不是先列。

**产品到底用了 wasmcore 的什么？**（`grep` 全仓，排除 wasmcore 自己）

```
agenterm_wasmcore::WasmCoreHost        6 处
agenterm_wasmcore::WasmFleetBridgeFn   5 处
方法：run_module / run_module_from_bytes / validate_binary
适配层 execute 的返回：stdout 有，value: None，cost: None
随包 .wasm 文件：0 个
```

**逐条对表：**

| 能力 | wasmcore | qjswasm | 产品用了吗 |
|---|---|---|---|
| 跑一个 `.wasm` | ✓ `_start` | ✓ 具名导出 | **是** |
| 抓 stdout | ✓ WASI `fd_write` | ✓ `agenterm.print` | **是** |
| `fleet_call` | ✓（两套约定） | ✓ | **是** |
| 不执行就校验 | ✓ `validate_binary` | ✓ 装载门 | **是** |
| AOT 预编译（cwasm） | ✓ | ✗ | **否** |
| JIT | ✓ | ✗（产品定义排除） | **否** |
| WASI（fs / clocks / args / env） | ✓ | ✗（**纪律排除**） | **否** |
| 每调用预算（steps/depth/pages/slots） | ✗ | ✓ | — |
| trap 分类 / fault word | ✗ | ✓ | — |
| 槽隔离、逐调用新鲜 fuel | ✗ | ✓ | — |

**判定：这条门一半绿一半不绿，而两半必须分开说。**

- **绿的那一半**：产品今天从 wasmcore 用到的每一样，qjswasm 都有替代，
  而且 qjswasm 还多出 wasmcore 没有的三样（预算、trap 分类、槽隔离）。
- **不绿的那一半**：AOT 预编译、JIT、WASI 三样 qjswasm 没有。
  其中 **WASI 是纪律排除**——本文件的纪律那条写着「不得把 WASI `fd_*`
  做成第二扇 OS 面」——所以它不是缺口，是**故意不做**。
  **AOT 与 JIT 是真缺口**，qjswasm 没有也不打算有（`tinyvm` 的产品定义就是
  执行核不生成机器码）。

**所以「归档 wasmcore」这句话要拆开问**：

> 产品今天需要 AOT / JIT 跑 `.wasm` 吗？

**今天不需要**——随包 `.wasm` 是 0 个，适配层连返回值和 cost 都没在用。
但这是**产品决定**，不是技术判定：删掉 wasmcore 就是宣布 agenterm 不再提供
JIT 执行 `.wasm` 的能力。这一条**没有人授权过**（政委 2026-08-25 那句是
「两者均待归档」，前提是被取代——而 AOT/JIT 这两样并没有被取代，是被**放弃**）。

**因此：门 1 判定完毕，归档 wasmcore 仍不授权**，理由从「门 1 没判过」
换成了「门 1 判出一个产品决定，而那个决定没人做过」。

### 那个决定做了：**不归档**（政委 2026-08-28）

> 「除了 iOS 不走 JIT/AOT，其它桌面系统未来还是会考虑 JIT/AOT 优化的。」

**这句话被实测支持了，而且此前那个 13.6× 的数不构成反证**——那个数当时就标了
「由启动主导，换计算密集的客人结论很可能反过来」。反过来了，而且幅度很大：

**同一份字节、同一个答案（`1233513906`）、纯计算 2000 万轮：**

| 引擎 | 用时 |
|---|---|
| `agenterm-wasmcore`（wasmtime，JIT） | **30.1 ms** |
| `agenterm-qjswasm`（tinyvm，无 JIT） | **16.08 s** |

**535 倍。**

**交叉点也量了**（每轮含各自的装载+实例化）：

| 循环轮数 | wasmcore | qjswasm | 赢家 |
|---|---|---|---|
| 0 | 1.45 ms | **0.16 ms** | qjswasm |
| 1 000 | 1.25 ms | **0.95 ms** | qjswasm |
| **2 000** | **1.29 ms** | 1.74 ms | **wasmcore** |
| 20 000 | **1.28 ms** | 16.1 ms | wasmcore |
| 100 000 | **1.41 ms** | 79.7 ms | wasmcore |

wasmcore 几乎**平**（全是启动，与轮数无关），qjswasm **线性**。
**分界线在约 1 500 轮真实计算。**

### 「默认最优」为什么不能自动，以及默认该按什么选

**不能按速度自动选**：一个客人要跑多少轮，在跑之前不知道；猜错的代价是 535 倍。

**但有两条不用猜的：**

1. **平台是硬约束。** iOS 上 JIT 不可用——那不是选择，是没得选，tinyvm 是唯一解。
2. **速度不是唯一的轴，而另一条轴指向相反方向。** 门 1 那张表里：
   **每调用预算（steps/depth/pages/slots）、trap 分类、槽隔离——qjswasm 有，
   wasmcore 一个都没有。** 所以跑**不受信任**的客人时，tinyvm 是**正确**的引擎，
   哪怕慢 500 倍——因为 wasmtime 那条路**根本圈不住它**。

**所以默认应当按「信任」而不是按「速度」选**：

> **默认 tinyvm**（可圈、可判、短脚本还更快——那正是 agenterm 实际跑的形状）；
> **wasmtime 是显式 opt-in**，给桌面端**受信任的、计算密集的**载荷。

这也正是今天的形状：两个都是 default-off 的 Cargo feature，
运行期由 `AGENTERM_SCRIPT_BACKEND` 选。**开关已经在了**，
这次补的是「默认按什么选」的理由，以及那条 1 500 轮的分界线。

### 留着花多少钱：默认构建**零**

```
cargo tree -e normal                        | grep -c wasmtime   # 0
cargo tree -e normal --features script-wasmcore | grep -c wasmtime   # 39
```

**这就是「数需求」那条纪律在这里不适用的原因**，值得写下来免得日后被误用：
那条纪律反对的是**造没人用的东西**，不是**删已经造好、而且默认不编译的东西**。
删是不可逆的，代价是丢掉 535 倍那条路；留着的代价是一个默认关闭的 feature。
**两边不对称，所以「今天没需求」不足以支持删。**

锁在 `host_door::a_wasmcore_shaped_guest_is_refused_for_its_door_signature_not_for_wasi`
——它同时断言那句诊断里**不出现 WASI**，因为出现了就会把读者引去改错的东西
（正是第 2 条那次的教训）。

**03b · ~~`_start` 入口约定~~ —— 这是个伪问题，2026-08-27 已作废**

我把它当成「等人拍板的产品决定」挂了好几轮。**它不是决定，因为两个答案都够不到分歧的所在。**
两条实测把它退了休：

**一、这个引擎调你要的任何名字。** 没有「约定」可选：`Engine::call` 收的是导出名，
`"main"` 只是 `.qjs` 编译器碰巧发射的那个，而 `_start` 今天就是个能用的普通导出。
一个两边都零成本满足的「约定」，不构成任何人要做的决定。

**二、WASI 客人根本到不了入口点。** 它在**门**那里就被按 import 拒了，任何导出名都没被查过：

```
host door: guest imports `wasi_snapshot_preview1.fd_write`;
`agenterm.*` is the only host module this engine offers, so nothing can bind it
```

这正是本文件 §纪律 写的那条在起作用：「能力全在门。门名单是 `agenterm.*`，
**不得把 WASI `fd_*` 做成第二扇 OS 面**。」

**所以分开两个引擎的是 import 面，不是入口名。** 而 wasmcore 的客人为什么带 WASI，
也量出来了：它自己的测试客人是 **Rust `std` 编到 `wasm32-wasip1`** 的程序（产物里能读到
`std::os::wasi::fs`、`std::rt::lang_start`、`wasip1`）。它的 `_start` 是
`std::rt::lang_start`，它 import WASI **是因为 std 要，不是因为产品要**。

**判定（我定的，不再等）：agenterm 的 `.wasm` 客人是 `agenterm.*` 门上的
reactor，不是 WASI command。** 依据不是偏好：
WASI 的 command 是「跑完就结束、走 stdin/stdout、导出 `_start`」，reactor 是
「实例存活、宿主随时调具名导出」——而本引擎的槽**按设计就是持久的**
（装一次、调多次、每次调用一份新鲜 `max_steps`），客人**返回一个值**而不是退出码。
它本来就是 reactor，`_start` 是别的模块种类的约定。

**门 2 的阻塞点因此换了个说法，也换了性质**：不是「等一个约定」，是
「一个 std 客人要改成 `no_std`，只 import `agenterm.*`」——那是**客人侧的改写成本**，
可以量，不需要谁拍板。零 `.wasm` 语料随包，所以今天没有客人要付这笔钱。

两条实测锁在 `qjs_guest.rs::the_wasm_entry_point_is_a_name_and_the_wasi_surface_is_the_real_boundary`，
其中一条断言那句拒绝里**不出现 `_start`**——出现了就会把读者引去改入口名，而那不是问题所在。

**04 · ~~扩展名路由从来没有接上~~ —— 2026-08-28 同日发现并修复，本条留档**

归档做完之后跑一次真实 CLI 验收，第一条命令就露了：

```
$ agenterm cli script run t.qjs          # 内容是模板 + 箭头 + map + .length
{"code":"rh_backend","message":"rh parse error: Expecting ',' … function call 'map'"}
$ AGENTERM_SCRIPT_BACKEND=qjswasm agenterm cli script run t.qjs
sum=3
```

`.qjs` 落到了 **rh**。不是 qjswasm 坏了——加上环境变量它跑得好好的，一行里同时验过
模板、箭头、`map`、`.length` 四个里程碑。坏的是**路由**。

**范围：所有语言，不只 `.qjs`。** 同一轮实测 `.lua` 也落到 rh
（`rh transpile error: cdylib pack requires fn entry()`）。

**根因，一句话：`ScriptBackend::from_entry_path` 在生产代码里有零个调用者。**
全仓 grep，它只出现在测试与文档注释里。`entry_extensions()` 同样。
真正决定引擎的是 `ScriptEngineBackend::enabled()`，而它的全部内容是
`ScriptBackend::from_env() == self.backend_id()`——`src/client/mod.rs` 里那条
2026-08-26 的注释其实已经把话说白了：**「routing is `AGENTERM_SCRIPT_BACKEND`
and nothing else」**。那句话是对的，只是没人把它和「有一个叫 `from_entry_path`
的函数、还有一份叫 `lua_task_entry_backend_selection` 的测试」这两件事对上。

**这条为什么该记在记忆宫殿那一节而不是普通 bug 列表**：它是同一种病的第八例。
`lua_task_entry_backend_selection` 的注释写着 `// Verify path-based backend
selection.`，而它验的是**纯函数**，不是**任何产品路径会调用它**。绿的测试
+ 未接线的功能 = 本文件 §接下来 01 那条 worker 分发臂漏掉时**一模一样**的形状
（「`.qjs` 注册了、编译了、测试绿了，而产品根本跑不了一个」）。
**同一个形状在不同层复发了一次，说明防它的不该是记性。**

**修成了什么样**：不是把 `from_entry_path` 塞进九个 `from_env()` 里的某一个，
而是收成**一个**入口 `ScriptBackend::resolve(label)`——优先级钉死为
**显式 `AGENTERM_SCRIPT_BACKEND` 胜过扩展名，扩展名胜过 rh 默认**。
显式必须赢，否则一个设了环境变量的人会被文件名悄悄改判，那是把这条缺陷换个方向再犯一次。

**验收判据**（事前钉死，未事后改动），四条全绿：

| # | 判据 | 结果 |
|---|------|------|
| 1 | `script run t.qjs` 不设环境变量 | `sum=3` ✅ |
| 2 | `AGENTERM_SCRIPT_BACKEND=rh script run t.qjs` | 仍是 rh 的解析错误 ✅ |
| 3 | `.lua` 按扩展名走通 | `3` ✅ |
| 4 | 测试断言**产品路径**而非纯函数 | `tests/script_entry_extension_routing.rs`，spawn 真二进制 ✅ |

**修的时候发现根因是两处，而且都不是「少了一次调用」**——这一条比缺陷本身值钱：

1. **`worker_supervisor::script_backend_environment` 在父进程未设时把 `"rh"`
   实体化进 worker 的环境。** 一个**被急切物化的默认值，和用户的显式选择长得一模一样**，
   所以「显式赢」这条规则每次都命中，扩展名永远轮不到。默认值本身没错，错在它住的位置：
   现在它住在**决策点**（`resolve` 的最后一档），而不是提前盖进环境里。
2. **每个引擎的 `check`/`execute` 开头还各有一次 `if !self.enabled()`**，
   在分发器已经选完之后**重新读一遍环境**。选择多了第二个输入以后，两道门立刻打架：
   `script run t.qjs` 被分发到 qjswasm，然后被 qjswasm 自己以
   「qjswasm backend not enabled」拒绝。这道门在此之前就是**冗余**的——
   分发器从不调用它没选中的引擎。**冗余不等于无害：一个决定的第二份副本，
   就是它出错的第二个地方。** 这与记忆宫殿记的 `.wasm` 路由是同一条教训。

**验证**：`--no-fail-fast` 跑完整套，2569 passed / 52 failed，
与**改动前同一命令**的失败集**逐条相同**（`comm` 双向为空）——52 条都是既有的，
零新增零修复。基线取的是本改动**之前**的提交，不是 stash 之后的当前树。

**05 · GC，以及 `eval`**（长期）

堆今天是 bump 分配 + 整堆丢弃：一次调用内只涨不落，调用结束整个实例扔掉。

**「对当前脚本形态够用」这句话 2026-08-28 量过了**，结论比原来的说法精确得多：

**默认预算下，四种分配密集的形状全部死在 `max_steps`，一次都没碰到
`max_memory_pages`：**

| 形状 | 默认预算下撞的墙 |
|---|---|
| 循环里字符串拼接 | `Budget(max_steps)`（1000 过，5000 不过） |
| 每轮造一个对象 | `Budget(max_steps)`（10000 过，50000 不过） |
| 数组 push 到大再 map | `Budget(max_steps)`（10000 过，50000 不过） |
| 每轮一个模板 | `Budget(max_steps)`（1000 过，5000 不过） |

**把 `max_steps` 抬到 200 亿之后，墙才换成 `max_memory_pages`**（默认 256 页 = 16 MiB）：
字符串拼接约 **5781 轮**、每轮造对象约 40–80 万轮、每轮一个模板约 1 万轮。

**GC 值多少钱，也能算出来了。** 拼接 5781 次单字符，累计分配约 **15 MiB**，
而结束时**存活只有 5781 字节**——**2700 倍**。这些中间字符串在下一次拼接的那一刻
就是垃圾。有 GC 的话，这个形状的天花板从「累计分配 16 MiB」变成「**存活** 16 MiB」。

**所以 GC 的触发条件是可以写出来的一句话**：

> **默认预算下 GC 一分钱都不值**——步数先耗尽，堆根本到不了。
> **一旦 embedder 把 `max_steps` 抬上去**，分配密集的脚本立刻变成堆受限，
> 而拼接类形状的浪费是 2700 倍。

这跟「再加方法」那条是同一个判断：**先数，再说要不要做**。
今天没有 embedder 抬 `max_steps`，所以 GC 排在这里而不是排在前面；
而它该被提前的信号也明确了——**有人抬 `max_steps` 的那一天**。

**`eval` 也数过了：需求为零。** 全仓 `scripts/` 与 `research/` 里，
客人侧 `eval(` 出现 **0 次**，`new Function` **0 次**。
（产品的 `script eval` 动词是**宿主侧**的——把表达式包成一个入口函数再编译，
跟客人侧的 `eval()` 不是一回事，别混。）
前提（跨实例函数链接）核里已有，缺宿主侧重编译进新实例那段——**等有人真的写
`eval(` 再做**。

### 三项待办数完之后的一句实话

再加方法、GC、`eval`——**三项的实测需求都是零**。这不是三次巧合，是一句
应当写进本文件的状态：**引擎已经跑在需求前面了。**

所以本轮最后做的不是再加特性，而是**回头验收产品面**（见下节）。
往后判断「下一步做什么」，先问的应该是「哪个脚本今天写不出来」，
而不是「引擎还缺什么语言特性」。

### 产品面复验（2026-08-28，本轮大改之后）

本轮动了很多：归档一个引擎、上线五个方法、改两扇门的 ABI。所以把本文件
**最吃重的几条主张**重新跑了一遍，不是读代码得出的：

| 主张 | 复验 |
|---|---|
| `fleet.qjs` 29 个操作 | `grep -oE 'call\("[a-z.-]+"'` 去重 = **29** |
| `.qjs` 语料干净 | `script corpus-scan --dir scripts/qjs` → `1 scripts ok` |
| `script run` 走通门 | 一个用了 `map`+箭头、`trim`、`push`、模板、`.length`、`JSON.stringify`、`for` 的脚本跑出 `alpha:5 beta:4` / `ok 2 {"n":2}` |
| `qualify` 出自足 `.wasm` + 收据 | **17 041 字节**，收据带 `steps 23745` / `peak_call_depth 10` / `peak_activation_slots 117`，engine 栏写着 `agenterm-qjswasm 0.1.16 (tinyvm 21d8d9a)`（该次测量时的 pin） |
| `pack load` 复现同样的 stdout 与值 | **一字不差** |
| 失败路径的退出码 | 目录当产物、文件不存在、不是 wasm——**三条都退 1**（顺手查的：一次 `exit=0` 是 `head` 的退出码，不是命令的） |

**这条流水线用到的正是本轮加的东西**，所以它同时是那些特性的产品级验收。

### 两件小的，但会一直咬人

- ~~**扩展名不选引擎。**~~ **已选（2026-08-29）**：`ScriptBackend::resolve(label)`——显式环境
  变量 > 扩展名 > 具名拒绝；`.qjs` 不用告诉就走 qjswasm，`tests/script_entry_extension_routing.rs`
  11 条。留下的坑见下节：环境变量能压过扩展名，测试里谁 `set_var` 谁就改了别人的路由。
- **验证口径是三条命令不是一条。** 见上文 §这个盲区已经咬过一次。

