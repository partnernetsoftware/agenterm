# 账单分成「算了多少」和「等了多久」（A1.12）

| 日期 | 2026-08-30 |
|---|---|
| 目的 | 让一次 `.qjs` 调用的账单说出宿主操作数与等待时长，并给宿主操作一个独立上限；步数留作失控 CPU 的护栏 |
| 来源 | grok 评审 `prd/review-qjswasm-2026-08-30-grok.md` §4；PRD_02_36 A1.12 |
| 范围 | 只在 agenterm 侧（`agenterm-qjswasm` 门 + `script_engine` 账单 + 协议默认值）；tinyvm 不动 |

## 0. 已定、不再讨论

1. 步数（`max_operations`）**保留**，仍是核心自己数的护栏；本页不改它的默认值。
2. 门是原始 `(ptr,len)`→JSON 的 42 个 `HostFn`，不改 ABI。
3. `wall_time_ms`（协议 2 000 ms）是 worker 的截止，不进 guest；本页不碰。

## 1. 三个计数器，都在门里

`ToolState` 已经为每次 `tool.*` 记一条 `calls: Vec<String>`（收据的行）。在同一处加：

| 计数 | 何时加 | 单位 |
|---|---|---|
| `host_ops` | 每次进入 `direct(...)`（13 个 `tool.*` 入口）与 `fleet_call`；**`tool_result` / `fleet_result` 取答复不算**（2026-08-30 深夜改：此前 `tool_result` 的两趟各算一次，旅程的数因此约翻倍） | 次 |
| `host_bytes` | 参数字节 + 结果字节（`state.result.len()`） | 字节 |
| `waited_ms` | `time.sleep_ms` 的请求值；`process.wait` / `process.command*` 从 `Instant::now()` 到返回的墙钟；`fleet_call` 等 broker 答复的墙钟 | 毫秒（墙钟，**非确定**） |

`Outcome` 三个字段同名带出；`ScriptCost` 加同名三项，`waited_ms` 的文档明说它是账单里
唯一非确定的一行——放进来正是为了把它从 `steps` 里剥出去。

## 2. 一个上限

`Budget.max_host_ops: usize`，门在第 N+1 次操作时不执行、返回
`QjswasmError::Budget("host operations")`（类别 `Limit`，与步数同类）。
协议 `ScriptBudgets.host_operations`：默认 **4 096**，硬顶 **1 000 000**，
`--max-host-operations` 走既有 `validate!` 通道。等待不设独立上限：`wait_time_ms`
（10 s）已经限制单次等待，`wall_time_ms` 限制总墙钟。

## 3. 判据

做完时三条旅程（server-smoke、wake-smoke、workbench-smoke）各自的账单能写成
`steps / host_ops / host_bytes / waited_ms` 四个数；当前默认 128M 步的「为什么这么大」
可以按这四个数回答（预期：步数大头在 `JSON.stringify` + 记账，等待在 25 ms 轮询）。

## 4. 不做

- 不按「每个宿主操作扣 N 步」把两种单位换算成一种：换算率没有依据，且会让步数护栏失真。
- 不改 25 ms 轮询脚本本身；先量，再决定是否给 `process.wait` 加事件等待。

## 5. 回填（2026-08-30 晚）

| 旅程 | steps | host_ops | host_bytes | waited_ms | heap_pages | 墙钟 ms |
|---|---|---|---|---|---|---|
| server-smoke | 31 557 629 | 1 127 | 509 876 | 267 | 45（第二次跑：31 560 640 步 / 266 ms） | 3 216 |
| wake-smoke | 48 867 366 | 1 149 | 359 051 | 245 | 63（第二次跑：48 804 793 步 / 253 ms） | 3 480 |

两次跑的步数相差 0.01%–0.13%（环境答复的字节数不同），`waited_ms` 相差 1–8 ms：账单在墙钟之外是稳定的。

命令：`agenterm cli script run scripts/qjs/<journey>.qjs --profile tool --timeout-ms 300000 --max-operations 1000000000 --json -- <repo> <server_exe> <cli_exe>`，
`target/debug/agenterm`（f336e626）。

**判决**：§3 的判据成立——四个数写得出来。**推翻预期的地方**：来源假设是「步数被 25 ms 轮询烧掉」，
账单说等待只占墙钟 8%，≈15M 步/秒的步数是 JSON 与记账的真计算；128M 默认对两条旅程是 2.5× 余量，
不是错的单位。`process.wait` 不加事件等待（§4 第二条按账单否决）。
**与规格不符（已在 §6 修正）**：§1 的 `fleet_call` 字节只记了成功答复；`tool.*` 的参数字节没记（各操作自己读参数，门看不到长度），
`host_bytes` 因此是下界。**已回答（同晚）**：失败调用的账单——槽留 `failed_cost`（与 `failed_stdout` 同一机制），`ScriptEngineError` / `ScriptFailure` 带 `cost`，信封里失败也有账。

## 6. 参数字节上账（2026-08-30 深夜）

门看得到长度：`tool::declarations()` 说了每个操作哪些参数是 `StrPtrLen`，`bind_metered` 按位置从原始 `i32` 里读出各个 `len`
（`tool::argument_length_slots`），与那一次操作一起 `charge`。`tool_result` 的落地缓冲不是发送，不在表里。

顺手发现 §5 的 `host_bytes` 两个方向都记错了：`answer()`（停放文本的 29 个操作）**从未**记停放的字节，而 `direct()`（答 `i32` 的操作）
每次都把**上一个**停放的答复再记一遍。现在 `answer()` 记停放的载荷（含被上限替换的拒绝句），`direct()` 只记它自己停放的诊断。
`tests/tool_door.rs::bytes_through_the_door_are_billed_in_both_directions` 钉：`fs_exists(p)` = `len(p)`；
`fs_read_to_string(p); tool_result(); fs_exists(p)` = `2·len(p) + 5`（`tool_result` 零字节；同夜起也零操作——见 §7.2）。

| 旅程 | steps | host_ops | host_bytes | waited_ms | heap_pages | 墙钟 ms |
|---|---|---|---|---|---|---|
| server-smoke | 31 739 529 | 1 123 | **886 437** | 232 | 45（第二次：31 893 163 / 1 127 / 886 444 / 291） | 3 246 |
| wake-smoke | 48 912 831 | 1 145 | **1 143 930** | 239 | 63（第二次：49 034 477 / 1 141 / 1 143 898 / 237） | 3 487 |

与 §5 比：server-smoke 510 KB → 886 KB，wake-smoke 359 KB → 1.14 MB——旧数既漏（答复）又重（陈旧答复），
新数两次跑只差 7–32 字节（环境答复的长度）。`host_bytes` 不再是下界。

## 7. 步数去了哪里（2026-08-30 深夜，server-smoke 的步数剖面）

没有 profiler；用三样东西拼：(a) 旅程自己的 `tool_calls` 收据（526 条有名操作 + `tool_result` 取答复 ≈601 次 = `host_ops` 1 127），
(b) 一份加了 `print(x.length)` 的旅程副本数出每次 `JSON.parse` / `JSON.stringify` / 拼接经手的字节，(c) 把**真实答复原文**（34 份，225 KB）
逐个塞进 tinyvm-qjs 进程内跑 `JSON.parse`、把 34 条真实记录逐个跑 `JSON.stringify`，用 `last_steps()` 差分定价（`tests/json_parse_cost.rs` / `json_stringify_cost.rs` 加了对应形状的钉）。
被剖的那次跑：**31 215 008 步 / 1 127 ops / 891 400 B / 258 ms**（加打印的副本 33.6M，打印不算宿主操作）。

| 家族 | 次数 | 经手字节 | 单价（进程内实测） | 步数 | 占比 |
|---|---|---|---|---|---|
| `JSON.parse` 广播答复（`command_json` 21 次 + `protocol-info` 1 次） | 22 | 225 454 | 58–64 步/字节（61 KB 3.94M、47 KB 2.71M、32 KB 1.88M、31 KB 1.85M；小答复 60–82） | **13 560 556** | **43.4%** |
| `JSON.parse` 折叠日志（`finalize_command_log`，34 行） | 34 | 15 870 | 81/字节 | 1 288 289 | 4.1% |
| `JSON.parse` 门信封（`process_command` 的 `{exit_code,…}`） | 34 | 2 582 | 7 499/次 | 254 966 | 0.8% |
| `JSON.stringify` 记录 ×2（journal 行 + `[record]`） | 68 | 31 672 | 每条 38.8k–97k（118–359/字节） | 4 773 377 | 15.3% |
| `JSON.stringify` 折叠日志（34 条一个数组） | 1 | 15 871 | — | 2 278 357 | 7.3% |
| `JSON.stringify` 命令 spec（`configured_cli_spec`，~770 B）+ 探针 | 35 | 26 775 | 51.7k/次（67/字节） | ≈1 800 000 | 5.8% |
| journal 拼接 `journal + rec + "\n"`（两次整段复制） | 34 | 265 074 | 2.4/字节 | ≈1 270 000 | 4.1% |
| 折叠日志的 `split` + `trim` | 1 | 15 870 | — | 794 296 | 2.5% |
| 其余（脚本自身：`"" + x`、`find_tab`、`count_tab_events`、`.length`、`slice`、harness 逻辑） | | | | ≈5 190 000 | 16.6% |

合计：**`JSON.parse` 15.10M（48.4%）**，`JSON.stringify` 8.86M（28.4%），拼接/切分 2.06M（6.6%），其余 16.6%。

**判决**：`JSON.parse` 一家过 40%，第 2 项打它。它的钱花在两处，都与答复是**美化打印**的有关（`agenterm cli --json` 两空格缩进；
bootstrap 答复 63% 的字节是缩进，protocol-info 38%）：

- **空白**：`__jp_ws` 每个空白字节 **39 步**（每字节一次 `__jp_at` 调用 + 四个比较）。旅程的答复里约 123 KB 空白 ⇒ ≈4.8M（旅程的 15%）。
- **每个字符串的固定价**：键与字符串值各付 ≈**760 步**固定（`__jb_new` 一个缓冲、`__jb_bytes` 一段、`__jb_take` 再拷成记录），每多一字节 +24。
  答复里约 7 900 个键/串 ⇒ ≈6.0M（19%）。
- 剩下的是逐节点解释常数（对象成员 5 键时 ≈1 600、20 键时 ≈2 600——`__obj_set` 线性扫已有键；`true` 386；`12345` 691）。

**顺手看见、不在第 2 项里的**：`JSON.stringify` 的记录为什么 108 字节要 38.8k 步——`recorded_at_ms: 1788101436756` 这种 13 位整数超出 i32，
离开 `num_to_string` 的位数循环走通用 double 路径，一个 **32 567 步**（`"" + 1788101436756` 同价 33 002）。旅程里每条记录序列化三次（journal、`[record]`、折叠），
102 个时间戳 ≈3.3M 步（10.7%）——这是 stringify 家族里最集中的一笔，也是第 2 项之后最便宜的一刀。`tests/json_stringify_cost.rs::a_journal_record_has_a_known_price` 钉着它。

wake-smoke 没有单独剖：同一个 harness、同一种答复，步数多出的 17M 与它多读的 `ui-bootstrap` 一致。

### 7.1 第 2 项落地后的账（同夜，tinyvm `77a804b`）

`__jp_ws` 空白按字（39 → 14 步/字节，缩进行 7）、`__json_pstr` 平凡串直接成记录（`"ab"` 758 → 591）：34 份真实答复进程内 **13.56M → 8.45M（−38%）**。

| 旅程 | steps 前 | steps 后 | host_ops | host_bytes | waited_ms | heap_pages 前 → 后 | 墙钟 ms |
|---|---|---|---|---|---|---|---|
| server-smoke | 31 215 008 | **26 275 039**（第二次 26 341 917） | 1 136 | 887 579 | 287 / 277 | 45 → **35** | 2 975 |
| wake-smoke | 48 912 831 | **41 220 613**（第二次 41 296 366） | 1 154 | 1 144 959 | 246 / 236 | 63 → **46** | 2 940 |

server-smoke −15.8%、wake-smoke −15.7%，与「parse 占 48% × 降 38% ≈ 18%」相符（差的 2 个点是 parse 里数字/结构那部分没动）。
堆页少了 10–17 页：每个串少一个 `__jb_new` 缓冲（64 B 起、按倍增长）。`host_ops` / `host_bytes` 不动——这一刀只在 guest 里。
下一刀按 §7 的表是 stringify 的 13 位时间戳（≈3.3M，10.7% → 现在 12.5%）。

### 7.2 `tool_result` 不是宿主操作（同夜）

§7 数出来的 1 127 个 `host_ops` 里只有 526 个是有名操作，其余 ≈601 是 `tool_result()` 的两趟（`result_len` + `result`）各记一次——
取一个已经在停放时上过账的答复，被记成两次操作、零字节。`fleet_result` 从来就是 `bind`（不计）；现在 `tool.result_len` / `tool.result` 同样走 `bind`，
`tests/tool_door.rs::bytes_through_the_door_are_billed_in_both_directions` 的 `read; tool_result(); exists` 从 4 次改钉 **2** 次，字节不变。

| 旅程 | steps | host_ops 前 → 后 | host_bytes | waited_ms | heap_pages | 墙钟 ms |
|---|---|---|---|---|---|---|
| server-smoke | 26 006 034 | 1 136 → **530** | 891 362 | 280 | 35 | 2 872 |
| wake-smoke | 41 075 927 | 1 154 → **512** | 1 145 998 | 248 | 46 | 2 890 |

`host_ops` 现在与收据 `tool_calls` 的行数同数量级（server-smoke 526 行 + 桥 0 次；差的 4 个是这次跑多的轮询）。协议默认 4 096 的余量因此从 3.6× 变成 7.7×，不改默认。

### 7.3 13 位时间戳（2026-08-31，tinyvm `129ea1d`）

§7 说的「stringify 家族里最集中的一笔」：`__num_to_string` 的位数循环只到 2^31，`recorded_at_ms` 那种 13 位整数走 Dragon4。
现在循环覆盖整个安全整数区（f64 里拆 `hi·1e9 + lo`，指令集没有 i64 除法）：`"" + 1788101436756` **32 786 → 797**、
`JSON.stringify({a: ts})` 比 `{a: 1}` 多 **32 567 → 541**、journal 记录一条 82k → 51k；`"" + 12345` 553 → 590（多一次除）。每个程序 +191 B。

| 旅程 | steps 前（§7.2） | steps 后 | host_ops | host_bytes | waited_ms | heap_pages 前 → 后 |
|---|---|---|---|---|---|---|
| server-smoke | 26 006 034 | **22 918 009**（第二次 22 919 067） | 527 | 890 201 | 284 / 281 | 35 → **32** |
| wake-smoke | 41 075 927 | **37 837 230**（第二次同数） | 509 | 1 144 943 | 256 / 244 | 46 → **43** |

server-smoke −3.09M（−11.9%），与 §7 估的 102 × 32k ≈ 3.3M 相符（少的那点是 §7 之后记录数略变）；wake-smoke −3.24M（−7.9%）。
堆页各少 3：Dragon4 每次 24 + 36 字节的中间记录没了。

### 7.4 workbench-smoke 也有账了（2026-08-31）

三条旅程里 workbench 停在 macOS 适配器的指针拒绝（`process.window_pointer: macOS background pointer delivery is unavailable: pointer events are not delivered to a non-frontmost child window`，
第 STEP「a single text click selects without entering edit mode」）。`--json` 失败也印 `cost`（§5），所以停点之前的账照记：

| 旅程 | 停点 | steps | host_ops | host_bytes | waited_ms | heap_pages | 墙钟 ms |
|---|---|---|---|---|---|---|---|
| workbench-smoke（pin `129ea1d`） | 指针拒绝 | 25 326 973（第二次 25 326 977） | 160 | 282 738 | 0 | 22 | 2 182 |

`waited_ms` 为 0：这条旅程到停点为止没有 `sleep`/`wait`，25M 步全是 JSON 与记账。与 server-smoke 的 22.9M 相比，操作只有它的 30%（160 对 527），
步数却相当——它每步读回的 `ui-*` 答复更大（282 KB / 160 次 ≈ 1.8 KB 一次，server-smoke 1.7 KB）且它的记录里塞了整份窗口树。这条线以后每次抬 pin 一起复测。

**`__jp_ws` 的裸 `\n`（第 3 项）不动**：现价 `'[1,\n2]'` 比 `'[1,2]'` 多 31 步、`'[1, 2]'` 多 18——裸换行比一个空格贵 13，不是 §7 写的 45（那是 `77a804b` 之前的数）。
这 13 是换行臂进缩进循环前的「还剩不到四字节」检查与两次跳转；要再省得在换行臂里先看下一字节是不是空格再进循环，对旅程里的答复（换行后总是缩进）没有收益。不是一臂之改，跳过。

### 7.5 `__obj_find` 的未命中（2026-08-31，tinyvm `5dc2288` → `b8319ff`）

§7 说 20 键对象每成员 ≈2 500 步、`__obj_set` 线性扫已有键。量了：同形键（`key000`…`key019`）一次未命中 ≈130 步（`__str_eq` 逐字节到第一个不同处），
但**真实记录的键长度各不相同**，那种未命中只要 28（`__str_eq` 比完长度就回）。第一稿（`5dc2288`：先比指针、再比长度、短键掩码首字、前奏 ≈25 步）把同形键砍了 3×，
却让键长各异的程序更慢——记录字面量 894 → 1 122、lease 答复 `JSON.parse` 18 083 → 18 823，两条旅程 **+0.8% / +1.4%**。第二稿（`b8319ff`）长度在前：
长度不同就是整个未命中（16 步），同长再比指针（字面量键直接命中）、首末字（≥ 4 字节）、最后 `__str_eq`。上游 `plan/design-obj-find-cheap-miss.md` 有三列价表。

| 旅程 | steps（§7.3，`129ea1d`） | `5dc2288`（第一稿） | **`b8319ff`** | host_ops | host_bytes | heap_pages |
|---|---|---|---|---|---|---|
| server-smoke | 22 918 009 | 23 097 099（+0.8%） | **22 521 339**（−1.7%；第二次 22 521 323） | 527 | 890 201 | 32 |
| wake-smoke | 37 837 230 | 38 380 600（+1.4%） | **37 221 769**（−1.6%；第二次 37 221 773） | 509 | 1 144 943 | 43 |
| workbench-smoke（到停点） | 25 326 973 | 25 534 011（+0.8%） | **25 138 481**（−0.7%；第二次 25 138 485） | 160 | 282 738 | 22 |

**判决**：键扫不是旅程的大头——§7 那个 2 500/成员是同形键的价，旅程里的对象是 5–12 个长度各异的键，扫的总价只有 2%。
一稿量错方向的代价是一次抬 pin；教训写在上游设计页：**先用旅程形状的对象量，再用极端形状**。

### 7.6 下一层量过、没动（2026-08-31，pin `b8319ff`）

第 5 项「还剩什么能一刀砍」：把 `JSON.parse` / `JSON.stringify` 逐节点定价（进程内 `last_steps()` 差分，扣掉同一程序不解析/不序列化的底）。

| `JSON.parse` | 步 | | `JSON.stringify` | 步 |
|---|---|---|---|---|
| `[]` / `{}` | 590 / 585 | | `{}` / `[]` / `1` / `"x"` | 1 183 / 1 191 / 1 225 / 1 304 |
| `[1]` − `[]`（一个数字元素） | 473 | | `{a:1}` − `{}`（一个属性） | 801 |
| `[true]` − `[]` | 243 | | `{a:true}` − `{}` | 513 |
| `[""]` / `["ab"]` / `["abcdefgh"]` − `[]` | 343 / 440 / 503 | | `{a:"x"}` − `{}` | 551 |
| `{"a":1}` − `{}`（一个成员） | 791 | | `{a:1,b:2}` − `{a:1}` | 564 |
| `{"a":1,"b":2}` − `{"a":1}` | 1 034 | | 键 10 字 / 20 字 − 1 字 | +107 / +594 |
| `[1,2]` − `[1]` | 373 | | `['ab','cd','ef']` − `[]` | 1 095 |

**判决**：没有单个热点。一个成员 ≈1 000 步里数字 ≈270、键串 ≈350、`__obj_set` + 分派 ≈400，都是「每节点几次调用、每次调用一次容量/边界检查」的解释常数；
`["ab"]` 比 `["abcd"]` 便宜（`__jp_run` 的整字测试 ≈45 步，短串走字节循环更划算）说明字扫描的门槛可以再调，但那是几十步不是几百。
要再降一个量级得改解析器的结构（首字节分派内联、成员路径不经 `__obj_find`、缓冲预留一次写引号和整段），是一项不是一刀；这里记下价，不动。
`__jp_ws` 裸换行（第 3 项）同理，见 §7.4。


## 8. 脚本侧退役（2026-08-31，pin `eeb0cbd`）

tinyvm A12 四批落地后，`scripts/qjs/**` 里为引擎缺口手写的工具改走引擎方法，导出名与签名不变、调用点未改：
`rh_compat.qjs` 的 `is_array` / `is_map` → `Array.isArray`，`array_has` / `list_has_value` → `includes`，`sort_strings` → `[].concat(v).sort()`（仍是副本），
`ascii_bytes` → 逐位 `charCodeAt`（95 项 `ASCII_PRINTABLE` 表删除），`stringify_pretty` → `JSON.stringify(v, null, 2)`（rh 当年就是 serde 的两空格；引擎的 `json_space.rs` 与 serde 每个 gap 逐字节相符，语料里没有逐字节比对这些文件的读者）；
`check.qjs::slowest_gates` 与 `prune_target_incremental.qjs` 的两份手写排序 → `sort`（稳定；比较器 `(a, b) => b.duration_ms - a.duration_ms` 复现「平局保持车道序」，默认序与 `<` 同一码元序）；
`cu-macos-smoke.qjs` 的 `invoke_arguments` / `path_text` → `concat` / `join`；`build-all.qjs` 的 `tail_of` 与前缀剥离 → `substring`（尾巴回到 rh 的「最后 N 个字符」）。

| 旅程 | steps 前 | steps 后 | Δ | STEP/EVIDENCE | host_ops | host_bytes | waited_ms | 墙钟 ms |
|---|---|---|---|---|---|---|---|---|
| server-smoke | 22 143 700（第二次 22 147 658） | **22 121 222** | −22k（−0.1%） | 7/1 → 7/1 | 527 | 890 201 | 274 | 2 665 |
| wake-smoke | 36 178 921（第二次 36 177 798） | **34 369 473** | −1.81M（−5.0%） | 3/2 → 3/2 | 511 | 1 144 950 | 265 | 2 479 |
| cu-macos-smoke | 74 121 887 | **74 029 205** | −93k（−0.13%） | 21/20 → 21/20 | 261 | 444 717 | 820 | 14 119 |

同一套二进制前后各跑（`target/debug/agenterm` 按 pin `eeb0cbd` 构建；cu 旅程先 `agenterm-abi --profile abi-dev` 拷 dylib 再 `agenterm-cu`），`exit_class` 均 `success`，`pgrep agenterm_ax_fixture` 空。
wake-smoke 的 1.8M 全在 `array_has(client_waited, client)`——它在轮询循环里，for-of 手写 `contains` 换成 `includes`（未命中 52 步/元素）；server-smoke 与 cu 只在握手/形状检查处碰到退役的工具，所以只降了零头。三条都没升。

**留下的一个：`only_chars`**（`preflight` / `qualification.valid_id` / `prune` 的字母表检查）。量了四种逐位拼法，都比「把每个允许字符 `replaceAll` 掉再看剩没剩」贵：

| 64 位 hex，200 次 | 步 | 10 位数字，200 次 | 步 |
|---|---|---|---|
| `replaceAll` 删字（现状） | **10.50M** | 现状 | **2.23M** |
| `charCodeAt` + 码值数组 `includes` | 18.59M | | 3.65M |
| `charCodeAt` + 128 项查表 | 19.32M | | 9.34M |
| `charAt` + 字母表数组 `includes` | 20.81M | | 2.46M |
| `s[i]` + 对象表 | 22.68M | | — |

每次字符访问是一次 prefab 调用（≈1.3k 步，含从串头走到位），十六趟 `replaceAll` 反而便宜。`ascii_bytes` 是反例：95 项 `startsWith` + `replace` 的剥皮对 40 字节的 workload 串 1.1M 步，`charCodeAt` 36k；300 字节的旧拼法撞宿主 deadline，新拼法 8.4M/20 次。
`xor16` / `int_div` / `floor_div` / `parse_int` / `pad_left` / `hex4` / `fnv1a64_hex` / `rfc3339_*` / `int_of` 等引擎的位运算 / `Math` / `parseInt` 批次落地再退。

## 9. 每个动作值多少步（2026-08-31，pin `028a914`，评审者量的）

方法：一个 100 次的 `while`，循环体里只多一条表达式，与只有 `n = n + 1` 的同形循环相减，再除以 100。
`./target/debug/agenterm cli script run <f>.qjs --profile tool --json`，读 `cost.steps`。
基线（`let s = "abcdefghij"; let o = { k: 1 }; let a = [1,2,3];` + 100 轮 `n = n + 1; i = i + 1;`）15 525 步 ≈ 155 步/轮。

| 多的那条 | 每轮多的步 | 说明 |
|---|---|---|
| `+ 1`（再加一次） | **31** | 一次动态 `+`：装箱对 → `__add` → 双 Number 臂。这是本引擎「一个动态运算」的底价 |
| `o.k` | **114** | 对象属性读：`__prop_get` → tag 派发 → `__obj_find`（一键记录） |
| `a[1]` | **71** | 数组下标：整数快路，比对象键便宜 |
| `a.length` | **192** | 走属性路 + 与 `"length"` 比键 |
| `s.length` | **311** | 同上，再加**每次都重走一遍**码元计数（10 字符；8 字节 ASCII 跳步之后仍是 O(n)） |
| `s.charCodeAt(3)` | **357** | 派发 + 从头走到第 3 个码元（O(i)） |
| `s.indexOf("c")` | **376** | 派发 + 窗口扫描 |

**读出来的两件事**：
1. **派发本身 ~110–190 步**，比它守着的工作还贵。短字符串上，`.length` 的 311 步里只有个位数是真的在数。
2. **字符串按位置的操作都是 O(n)**：`s.length` 每次重数，`charCodeAt(i)` 每次从头走。
   于是 `while (i < s.length) { … s.charCodeAt(i) … }` 是**二次**的——这正是 `only_chars` 用
   `replaceAll` 删字符比按位置走便宜 2–9× 的原因（§8）。

**实验判决（2026-09-04）**：缓存码元长度 + 全 ASCII 位确实把斜率压平：
`charCodeAt(999)` 5,560→82，`s[999]` 5,625→164，全 1,000 位遍历
3,009,139→228,053；但重复 `.length` 的常数成本是 166 steps/call，超过
事前冻结的 160 硬门。按决策树判 **reject**，实现已完整回滚，不能因为只差
6 steps 就事后改门。完整表在 tinyvm `plan/design-string-record-metadata-experiment.md`
（evidence commit `0a43271`）；本仓 engine pin 仍为 `028a914`。

**第二次实验判决（tinyvm evidence `a6ba2f9`）**：静态 `length` prefab
把 64 位和 6,000 位 ASCII 的 `.length` 都做到 **160 steps/call**，斜率为零，
说明最后 6 steps 的派发成本确实能够拿掉；但候选仍被 C2 判退。统一的构造后
扫描让已有负载产生真实回归：Array `join` **573.9 steps/element**（门 `<260`）、
1,000-byte String 的 `JSON.stringify` **73 steps/byte**（门 `<50`）、journal
stringify **68,490 steps**（门 `<60,000`）、flat Object **1,536
steps/property**（门 `<1,500`），并带坏 `includes` / `indexOf` / `split` courts。
因此不能以 `.length` 单点达标覆盖整机回归；实现已完整回滚，pin 仍为
`028a914`。

**下一刀**：只允许预注册“各字符串生产者直接发布元数据”的判决实验，避免
eager general post-construction scan。实验必须先列全生产者、定义未覆盖生产者的
正确 fallback，并冻结既有 join / JSON / journal / Object / search / split courts；
不能重开已判退候选，也不能放宽旧门。

**第三次实验判决（tinyvm evidence `69a3e3b`）**：各生产者直接发布元数据的
Variant D 避开了上一刀的统一构造后扫描，`join` 回到 **212.0
steps/element**（门 `<260`），`split` 回到 **34.5 steps/character**（门
`<35`）；但 `includes` 与 `indexOf` 都是 **10.5 steps/character**，没有满足
冻结的严格门 `<10`。按首个硬门失败即停止的规则，D5 JSON 与其后法院未运行，
实验实现全部回滚，生产 pin 仍为 `028a914`。这不是四舍五入问题：10.5 不能
解释成 `<10`。完整判决在 tinyvm
`plan/design-direct-string-metadata-publication-experiment.md`。

**事后尺子审计（tinyvm `cf70589`）**：上面的 REJECT 不改写，但 7.2 → 10.5
不是 search loop 变慢。旧 court 用 `search_steps - s.length_steps`；生产
`.length` 自身按 UTF-8 走约 **3.3 steps/character**，而 Variant D 把它变成
O(1)，所以只有旧基线被额外减掉了约 3.3。下一 court 必须同时量
`return 0` 的 build-only control 与 `return s.length` 的历史 control，并以
`absolute search - historical search = independent length slope` 闭合；再把绝对
search slope 分给 loop/read/compare/miss。历史判决有效，历史尺子不得继续用于
跨实现比较。

**新的性能 frontier**：暂停继续改 String record。先用不改变表示的诊断刀把
search court 的绝对成本、固定派发、循环控制、码元读取、比较和 miss-return 分项
计数；只有闭合后的某个线性 owner 足够大且能被正交修改，才为它另写冻结实验。
不得把 near miss 当作调低门槛的理由。
实验规格已在 tinyvm `plan/design-string-search-cost-attribution-experiment.md`
校正（`cf70589`）：双 control、四个以上长度点、两层斜率闭合、固定/线性成本分账；只出归因和下一
实验的 owner，不在同一轮偷做优化，也不改变当前 tinyvm pin。
