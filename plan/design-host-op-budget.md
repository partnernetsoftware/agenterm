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
| `host_ops` | 每次进入 `direct(...)`（13 个 `tool.*` 入口）与 `fleet_call` | 次 |
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
`fs_read_to_string(p); tool_result(); fs_exists(p)` = `2·len(p) + 5`（`tool_result` 加两次操作、零字节）。

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

### 7.1 第 2 项落地后的账（同夜，tinyvm `8d3f4c5`）

`__jp_ws` 空白按字（39 → 14 步/字节，缩进行 7）、`__json_pstr` 平凡串直接成记录（`"ab"` 758 → 591）：34 份真实答复进程内 **13.56M → 8.45M（−38%）**。

| 旅程 | steps 前 | steps 后 | host_ops | host_bytes | waited_ms | heap_pages 前 → 后 | 墙钟 ms |
|---|---|---|---|---|---|---|---|
| server-smoke | 31 215 008 | **26 275 039**（第二次 26 341 917） | 1 136 | 887 579 | 287 / 277 | 45 → **35** | 2 975 |
| wake-smoke | 48 912 831 | **41 220 613**（第二次 41 296 366） | 1 154 | 1 144 959 | 246 / 236 | 63 → **46** | 2 940 |

server-smoke −15.8%、wake-smoke −15.7%，与「parse 占 48% × 降 38% ≈ 18%」相符（差的 2 个点是 parse 里数字/结构那部分没动）。
堆页少了 10–17 页：每个串少一个 `__jb_new` 缓冲（64 B 起、按倍增长）。`host_ops` / `host_bytes` 不动——这一刀只在 guest 里。
下一刀按 §7 的表是 stringify 的 13 位时间戳（≈3.3M，10.7% → 现在 12.5%）。
