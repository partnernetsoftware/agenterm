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
