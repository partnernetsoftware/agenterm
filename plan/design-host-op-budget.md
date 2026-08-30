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

| 旅程 | steps | host_ops | host_bytes | waited_ms | 墙钟 ms |
|---|---|---|---|---|---|
| server-smoke | 31 557 629 | 1 127 | 509 876 | 267 | 3 216 |
| wake-smoke | 48 867 366 | 1 149 | 359 051 | 245 | 3 480 |

命令：`agenterm cli script run scripts/qjs/<journey>.qjs --profile tool --timeout-ms 300000 --max-operations 1000000000 --json -- <repo> <server_exe> <cli_exe>`，
`target/debug/agenterm`（f336e626）。

**判决**：§3 的判据成立——四个数写得出来。**推翻预期的地方**：来源假设是「步数被 25 ms 轮询烧掉」，
账单说等待只占墙钟 8%，≈15M 步/秒的步数是 JSON 与记账的真计算；128M 默认对两条旅程是 2.5× 余量，
不是错的单位。`process.wait` 不加事件等待（§4 第二条按账单否决）。
**与规格不符**：§1 的 `fleet_call` 字节只记了成功答复；`tool.*` 的参数字节没记（各操作自己读参数，门看不到长度），
`host_bytes` 因此是下界。**未回答**：失败调用的账单（worker 错误路径不带 `cost`）——下一片。
