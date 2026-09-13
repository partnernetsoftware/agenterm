# RESULTS — Script process witness

## 条件

- 目标/执行：macOS aarch64，真机执行。
- 边界：同一 qjswasm/tinyvm worker；每次 20 轮不缓存的整机进程枚举、当前 PID 查找和
  executable basename 读取。
- A：`process.command` 执行 `ps -axo pid=,comm=` 后在 guest 解析文本。
- B：`process.list` 后在 guest 解析 JSON。
- 构建：当前 debug `target/debug/agenterm`，两侧完全相同。
- 预算：120,000 ms / 1,000,000,000 operations，tool profile。

## 原始机器值

| 变体 | run | ok | steps | host bytes | host ops | duration ms | basename |
|---|---:|---|---:|---:|---:|---:|---|
| A `ps` | 1 | true | 281,066,194 | 1,301,962 | 23 | 15,882 | `agenterm` |
| A `ps` | 2 | true | 284,021,147 | 1,316,483 | 23 | 16,029 | `agenterm` |
| A `ps` | 3 | true | 303,339,479 | 1,331,715 | 23 | 17,224 | `agenterm` |
| B door | 1 | true | 74,517,230 | 1,004,259 | 23 | 4,663 | `agenterm` |
| B door | 2 | true | 76,069,497 | 1,024,724 | 23 | 4,722 | `agenterm` |
| B door | 3 | true | 76,330,797 | 1,028,164 | 23 | 4,771 | `agenterm` |

中位数 B/A：steps 26.78%，host bytes 77.84%，host ops 100%，wall time 29.46%。

## 判决 trace

G0 3/3 + 3/3 成功 → G1/G2 均通过 → G4 basename 等价 → G3 通过 → 迁移清单消费者。
外部 `ps` 进程及其 guest 文本 parser 同时退役。结果推翻了“完整 inventory 的 JSON
运输必然比 `ps` 文本更贵”的预期；bridge 字节没有消失，但结构化解析显著更便宜。

迁移后的 `product-court.qjs` 直接 import `script_smoke_helpers`，以当前 PID 和 basename
调用真实 `process_exists` / `process_ids`，并 spawn 一个真实子进程后通过迁移后的
`kill_pid` 终止、回收：PASS，8,770,249 steps / 103,425 host bytes / 7 host ops / 582 ms。

## 偏差与边界

- court 固定 20 轮以量轮询斜率，不模拟 `script-smoke` 每次恰好几轮。
- A court 从 `ps comm` 行取末 token，而旧 helper 会拼回 token 后再取 basename；`comm`
  在本次真实输入中是单 token，因此结果相同，但 court 不是旧 parser 的逐字复制。
- 在 macOS aarch64 真机运行；Windows owning gate 未由此 court 代替。
- A 第一次输出曾包含运行二进制路径；结果文件只保留 basename 与机器成本，没有把宿主
  绝对路径写入仓库。
- 两变体都额外调用 `arg`、`process_id` 和 `print`，所以 host ops 均为 23；共同截距相减。
- 产品迁移把 inventory/termination 的宿主失败从静默空表或忽略状态收紧为具名
  fail-closed；这不由性能 court 判定，必须由 owning gates 验证。
- 未改变事前判据。

## 复跑

从仓库根目录，分别执行三次：

```text
agenterm cli script run --json --profile tool --timeout-ms 120000 --max-operations 1000000000 research/script-process-witness/court.qjs -- ps
agenterm cli script run --json --profile tool --timeout-ms 120000 --max-operations 1000000000 research/script-process-witness/court.qjs -- door
agenterm cli script run --json --profile tool --project-root scripts/qjs --timeout-ms 120000 --max-operations 1000000000 research/script-process-witness/product-court.qjs
```
