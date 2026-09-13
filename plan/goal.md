# 目标：持续推进 AgenTerm，重点发展 dyn + qjswasm + CU

核心目标是持续推进 AgenTerm 的产品与架构，重点发展 `agenterm-dyn`、
`agenterm-qjswasm` 与 tinyvm 的集成，以及 `agenterm-cu`。不断发现并交付这些层之间
最有价值、具有证据支撑的能力与折叠，不把任何当前里程碑当作终点。

主要架构路线之一，是把 `dyn` 发展成原子般稳固的 native-import 基础：承载 ABI
调用、syscall、ioctl、host-op 以及未来的机制族。以此增强并折叠 qjswasm/tinyvm
和 CU 上层，集中所有权、提高代码的信息密度，把释放出的维护、构建与运行资源继续
转化为更强的产品能力。

持续从证据走向完整实现。优先解决真实消费者需求、具名正确性缺口和经过测量的折叠
机会，不为扩大表面能力而进行推测性扩张。保持类型边界与分层所有权，不把产品策略
下沉到机制层或 Script Runtime。详细的能力状态、依赖、证据与下一步折叠路线，应写入
对应 PRD 的 Markdown tree-DAG 与 Mermaid memory palace，而不是堆积在本目标中。

用户拥有并启动的 Pi 工作端，是从 `~/repos/piocgo` 启动的现有 tmux window
`pi-ocgo`。只允许通过以下命令向它派工：

```text
mux envelope 0:pi-ocgo "<标题>" "<正文>"
```

该窗口不是 Codex subagent。不得创建内部 agent 冒充或替代它；除非用户明确要求，
不得另行启动 Pi 进程。主代理负责审查它返回的证据、完成集成、运行最终门禁，并以
小而完整的增量提交；除非用户明确要求，不得 push。

这是持续性的开发目标。完成一次审计、一个叶或一个提交，都不代表目标完成；应继续
选择下一项有证据支撑的改进并推进落地。
