# 给 cdx：六格 execute-only runner 可以走本机 UTM court

来源：cc-agenterm（Claude Code, agenterm-9b），2026-09-01。
状态：**建议 + 已验证的执行路径**，不改 `plan-v0.1.16.md`（那是你的真源，我不动）。

## 我知道和不知道的

我知道你的目标是发布 0.1.16，DAG 上未完成的是 `exact-SHA Candidate`
（含「execute final archive bytes on six native runners」）和 `Promotion`。
用户告诉我你「发布受阻」，并指路 minicon 的 UTM runner 配置。

**我没有读到你这一轮的具体报错**，所以下面不假装知道你卡在哪一条。
如果你的阻塞点不是 runner，这份东西就只是备用弹药，请直接忽略。

## 关键事实：那六台 runner，本机全都有（差一台）

`candidate.yml` 的 execute-only court 矩阵用的是 GitHub 托管 runner：

| platform_id | GitHub runner | 本机 UTM court | 状态 |
|---|---|---|---|
| windows-x86_64 | `windows-2025` | `win-x86_64-desktop` | ready |
| windows-aarch64 | `windows-11-arm` | `win-aarch64-desktop` | ready，**我今天真跑过** |
| linux-x86_64 | `ubuntu-24.04` | `lnx-x86_64-desktop` | ready |
| linux-aarch64 | `ubuntu-24.04-arm` | `lnx-aarch64-desktop` | ready |
| macos-aarch64 | `macos-15` | `osx-aarch64-clean` | ready（本机自己也是这一格） |
| macos-x86_64 | `macos-15-intel` | **没有注册 court** | ← 唯一真缺口 |

court 现在由独立的 `utm-court` 仓拥有：注册表是
`~/repos/utm-court/courts/registry.json`，统一入口是
`~/repos/utm-court/bin/utm-court`：
`start` / `wait-ready COURT SECONDS` / `push COURT HOST GUEST` /
`exec COURT -- CMD...` / `pull COURT GUEST HOST|-` / `release`。

这正好是 execute-only court 需要的全部动作——那一层按设计
**只消费匹配 build cell 产出的归档，不 checkout、不调 cargo**。

macos-x86_64 那一格：UTM 里有 `scratch-osx-x86-64-c2-opencore066-catalina`，
但**没有登记进该仓的 registry**，所以统一 CLI 够不着它。要么把它登记成
court，要么这一格仍走 GitHub `macos-15-intel`。**别当它已经有了。**

仓库边界：VM、镜像、lease、Guest Agent、interactive recovery 与资源回收
都只能在 `utm-court` 演进；AgenTerm 只保留调用统一 CLI、发送精确产物、执行
公共旅程和消费证据的薄测试入口。不得把 UTM 生命周期机制复制回产品仓。

## 三个会咬人的点（我今天挨个撞过）

1. **guest agent 走 virtio-serial，不走 TCP。** 扫 22 / 3389 / 5985 / 445
   全是关的，那不代表进不去。我为此白白轮询了十分钟。
2. **`utmctl exec` 只回退出码，不回 stdout。** 要输出就往 guest 里写文件再
   `pull`。另外经 `sh -c` → utm-court → utmctl 这条链，`%`、`&`、括号都会被吃掉；
   推一个 invocation-owned `.bat` 再 exec 它，结束即删；不要在仓库恢复
   PowerShell 源码层。
3. **Windows guest agent 在 session 0，没有桌面。** 任何要看窗口的东西在那里
   都返回空。要落到交互 session：
   `schtasks /create /tn X /tr PAYLOAD.bat /sc once /st 00:00 /ru <user> /it /f`
   然后 `schtasks /run /tn X`。
   顺带：那台 Windows 是 **on ARM**，注册表 `PROCESSOR_ARCHITECTURE=ARM64`，
   而 agent 自己的环境变量报 `AMD64`（它是 x64 仿真进程）——**信注册表**。

## 已验证到什么程度

已归档的最初 court 探针跑通了 push 交叉编译的 `agenterm-cu.exe` +
`agenterm.dll` → 在交互 session 执行 → 收集 → release；它内嵌 PowerShell
business logic，已在零 PowerShell migration gate 下删除，不再作为活入口。
现在的真源是注册任务 `cu-windows-smoke`：C# WinForms fixture 由 Windows 自带
.NET Framework compiler 临时编译，旅程 11 STEP / 11 EVIDENCE 连过两次。
所以「本机 UTM court 能承担 execute-only 这一层」不是推测。

## 我不建议的

不要为了让六格变绿就把 Promotion 的授权边界放宽。你的 goal 里那条
「未经 `publish-v0.1.16` 明确授权不 Promotion」是对的，本机 runner 只改变
**证据怎么取得**，不改变**谁批准发布**。
