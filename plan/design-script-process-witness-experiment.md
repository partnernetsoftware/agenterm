# Script process witness 判决实验

不进 must-ship，不改变 PRD 能力状态；本实验只判定任务脚本是否应把
`ps -axo pid=,comm=` 清单替换为既有 `process.list` door。

| 项 | 值 |
|---|---|
| 日期 | 2026-09-14 |
| 目的 | 比较同一“枚举进程并按 PID/名称查询”能力的两条真实宿主路径 |
| 实现 | `research/script-process-witness/` |
| 前置 | `plan/goal.md`、`prd/PRD_02_36_agenterm_qjswasm.md` |
| 来源纪律 | 只用仓内实现与本机真实进程表；不合成性能数字 |

## §0 已确定事项

1. `process.kill_pid(pid)` 与 `process.observe(pid)` 是已有的精确键 door；替换任务脚本的
   `kill -9` / `kill -0` 不需要本实验裁决。
2. `process.list` 与 `ps` 都枚举整机进程，能力集合可比；本实验不讨论新增过滤参数。
3. `ps comm` 与 `process.list.executable_name` 的名称语义不同；即使性能胜出，产品迁移仍须
   单独证明 worker 名称完全等价，否则只交付精确键部分；不得以宽松前缀匹配掩盖差异。

## §1 硬约束

- A/B 使用同一个 qjswasm/tinyvm pin、tool profile、预算、重复次数和 PID/名称读取序列。
- A 是一次 `process.command(ps ...)` 后解析；B 是一次 `process.list` 后解析；均不得缓存。
- 每侧至少三次成功运行，记录 steps、host bytes、host ops 与 wall time 的中位数。
- 任何为了让 B 获胜而给 `process.list` 增加过滤参数或新 host op 的冲动，是本实验要检测的病，
  不是本实验需求。

## §2 最小实验

同一 court 重复 20 轮：枚举进程，确认当前 PID 存在，并读取当前可执行名称。变体只改变
清单来源。20 轮暴露轮询斜率，同时避免启动真实 `script-smoke` 的 Windows-only 产品依赖。

## §3 事前判据

| 编号 | 性质 | 判据 |
|---|---|---|
| G0 | 布尔 | 两侧 3/3 成功，且都找到当前 PID；否则实验无效 |
| G1 | 斜率 | B 的 steps 中位数不得高于 A 的 110% |
| G2 | 斜率 | B 的 host bytes 中位数不得高于 A 的 110% |
| G3 | 截距 | B 的 wall-time 中位数低于 A；只作平局裁决，不覆盖 G1/G2 |
| G4 | 安全 | worker 名称逐值等价；不等价则禁止迁移名称消费者 |

## §4 判决树、kill criterion、时间盒

1. G0 不过：实验无效，停止，不迁移清单消费者。
2. G1 或 G2 不过：判 `process.list` 清单迁移为 NO；只交付精确键替换。
3. G1/G2 均过：看 G4；G4 不过则只迁移 PID-existence 消费者，不迁移名称消费者。
4. G1/G2/G4 均过：G3 胜则迁移，G3 不胜则维持现状。

Kill criterion：任一侧需要新产品 API、清单缓存或放宽预算才能成功，立即判 NO。
时间盒：三次 A/B 机器值与 G0–G4 判决齐备即停，不做后续优化。

## §5 目录

`research/script-process-witness/` 保存 court、原始运行信封与 `RESULTS.md`。

## §6 已排除选项

| 选项 | 原因 |
|---|---|
| 给 `process.list` 加 PID/name 参数 | 改变公开契约并混入新能力 |
| 新增另一份进程清单 op | 制造第二份真相 |
| 仅以 host-op 次数判断 | 忽略 bridge bytes 与 guest JSON.parse 成本 |

## §7 不回答

- Windows `script-smoke` 的完整行为与 staged artifact 资格。
- `process.kill_pid` / `process.observe` 精确键替换是否正确。
- 新增平台进程能力。

## §8 结论回填

判决：G0 通过；B 的 steps、host bytes 均低于 A，因此 G1/G2 通过；两侧对当前
worker 的 basename 都是 `agenterm`，G4 通过；B 的 wall time 亦更低，G3 通过。
按 §4 路径，迁移 `script_smoke_helpers` 的清单消费者。

三次中位数：A=`ps` 为 284,021,147 steps / 1,316,483 host bytes / 23 host ops /
16,029 ms；B=`process.list` 为 76,069,497 steps / 1,024,724 host bytes / 23 host ops /
4,722 ms。B/A 分别为 26.78% / 77.84% / 100% / 29.46%。

与预期不同：全量 inventory 的 bridge 字节仍大，但 qjs 解析 `ps` 文本的成本更大；此前
“全量 door 必然更贵”的推断被真实 A/B 推翻。未为结论改变判据。macOS court 不能替代
Windows-only `script-smoke` owning gate，后者仍必须在集成阶段单独验证。

完整复跑命令、逐次数字与偏差见 `research/script-process-witness/RESULTS.md`。
