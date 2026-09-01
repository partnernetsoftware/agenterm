# goal-local-six-cell

状态：active（2026-08-25 立项；**P0 / P1 / P3 零缺口；P2 运行时三格已过、三格阻塞待人工；装置已在 minicon 上二次证明**）
角色：本机（Apple Silicon mac mini）作为六格构建与四格运行时验证的单一宿主
编排拍板：2026-08-25
关联：`AGENTS.md`（Cross-platform build and test contract）、`plan/plan-v0.1.18.md`（轨 D `agenterm-cu`）、`prd/PRD_02_30_cu_targets_transports.md`（`vnc` 目标族）、`crates/agenterm-vnc`
**脱敏**：仓内一律仓库相对路径，家目录统一 `~/...`；不写宿主绝对 home、不写凭据。

> 用法：把下面 `--- GOAL ---` 之间的内容整段发给 agent（或 `/goal` 加载本文件）即可。
> 可执行到底；**技术选型已拍板（§2 D1–D9），禁止重开辩论**，尤其不要再提 VirtualBox、
> 不要再问「要不要用 CI」。缺口只允许**书面证伪**，不允许换方案绕过。

--- GOAL ---

在 agenterm 仓库根执行 **本机六格构建与测试宿主化**：把「一台 Apple Silicon mac mini
一次产出 `{x86_64,aarch64} × {win,lnx,osx}` 六格产物，并在本机 VM 内验证其中四格运行时」
做成可复现、可验收的装置。**自主执行到底**，不要中途问「是不是该用 GitHub CI」——
CI 不删，但它从「唯一构建路径」降级为兜底与公证，这一条已拍板。

## 开工必须先读（不得跳过）

1. 本文件 §2（已拍板 D1–D9）、§3（边界）、§6（已知坑）
2. `AGENTS.md` → *Cross-platform build and test contract*（现行六格表，宿主假设待本 goal 改写）
3. `crates/agenterm-vnc/src/session.rs` → `SessionHandle` 的 `send_key` / `send_mouse` /
   `request_full_refresh` 与 `Frame.rgba`（P1 的既有能力，**不要重写**）

## 边界一句话（开工必须复述）

| 层 | 归属 | 装什么 |
|----|------|--------|
| 构建驱动 | `scripts/bootstrap.sh` 的 `client-build-all` task | 六格并发、按格 `-j`、失败显式红 |
| 带外 KVM | `crates/agenterm-vnc`（既有） | RFB 按键/指针注入 + 整帧 RGBA 取图 |
| 带内 smoke | 既有 `scripts/rh/*-smoke.rh` | 不动语义，只补目标格 |
| 证据 | `~/.local/share/agenterm/evidence/six-cell-<UTC>` | 每条标注 `native` 或 `emulated=<prism\|rosetta\|tcg>` |

**不改产品代码语义**；只增构建/测试装置与证据面。
**不新增第五个 `scripts/build-*.sh`**——现有已有四个，收口进 `client-build-all`。

## 执行顺序（不得乱序）

**P0 六格构建** → **P1 带外 KVM 回路（先于装 VM，在本机拿现成 VNC server 验通）** →
**P2 VM 落地** → **P3 一键化与证据**。

P1 早于 P2 是刻意的：**两个不确定性不叠加 debug**。VM 装好当天就要有能用的 harness。

## 每完成一格必须产出

- 命令原文 + 退出码 + 产物路径 + SHA-256
- 该格是 `native` 还是 `emulated=<...>`
- 失败时：**定因**，不是「换个方案试试」

## 三条硬纪律

1. **诚实性优先于绿**：Prism / Rosetta / QEMU TCG 下的结果一律不得记成「真机 PASS」，
   无 `emulated` 标注的模拟结果视为无效证据。
2. **不得静默跳过任一格**：六格里任何一格构建失败必须显式红并定因，
   不允许「先做通四格再说」。
3. **§6 的坑逐条兑现**：`winresource` 的 resource compiler、`usb-tablet` 绝对坐标、
   QMP `sendkey` 组合键、整帧刷新、UTM 不开 VNC、Eval 90 天快照、`script-*` feature 的 C 编译边界。

--- END GOAL ---

---

## 0. 目标一句话

在**这一台** mac mini 上完成 `{x86_64,aarch64} × {win,lnx,osx}` 六格的**构建**，
并在本机 VM 内完成其中四格（win×2、lnx×2）的**运行时验证**，
使 GitHub CI 从「唯一构建路径」降级为「兜底与公证」。

不是替代 CI，是**把反馈回路从分钟级远程拉回到本机**。

---

## 1. 宿主事实（实测，2026-08-25）

| 项 | 值 |
|----|-----|
| 机器 | Apple M4 Pro / 14 核 / 24 GB |
| 系统 | macOS 26.5.1（arm64），`kern.hv_support=1` |
| 盘 | 363 GB 可用；`target/` 现 2.5 GB |
| 已装 | Xcode SDK 26.5（含 x86_64 slice）、rust 1.97.0、clang、make、homebrew |
| 未装 | zig、cargo-zigbuild、cargo-xwin、llvm、qemu、UTM、**Rosetta**、docker/podman/colima |
| 已装 rust target | 仅 `aarch64-apple-darwin` |

> `.cargo/config.toml` 的 `jobs = 6` 注释写明是为「8 核 / 8 GB 开发机」定的。
> 本机 14 核 / 24 GB，六格并发时该上限反成瓶颈——**不改默认值**，在批量脚本里按格传 `-j`。

---

## 2. 已拍板（直接执行，禁止重开辩论）

| ID | 决策 | 根因 |
|----|------|------|
| **D1** | **VirtualBox 出局** | brew cask 在本机解析到 arm64 Developer Preview（描述即 "Virtualiser for arm64 hardware"）。ARM 版**无 x86 二进制翻译**，只能跑 ARM guest：lnx×x86_64 与 win×x86_64 直接起不来，win×aarch64 亦不稳。四格废两格半 |
| **D2** | 虚拟化统一到 **UTM + 裸 QEMU** | UTM 管装机与人工使用；自动化 harness 走 `brew` 的 qemu 直连同一块 qcow2。两个前端一块盘 |
| **D3** | lnx×2 构建用 **cargo-zigbuild** | 一个工具同时供两个 `linux-gnu` 目标当 linker + libc；附带可钉 glibc 下限（`--target x86_64-unknown-linux-gnu.2.17`），兼容面比 ubuntu-latest 产物更宽。`.cargo/config.toml` 里 `aarch64-linux-gnu-gcc` 那条 linker 随之退休 |
| **D4** | win×2 构建用 **cargo-xwin** | 宿主无关（不是 Linux 专属），且默认 feature 图是纯 Rust——`windows-sys` 只是绑定，mlua / rusqlite / rquickjs / wasmtime 全在 optional feature 后面，不碰 C 编译 |
| **D5** | osx×2 **原生构建** | Xcode SDK 本身双架构，`rustup target add x86_64-apple-darwin` 即可 |
| **D6** | Windows guest 镜像 = **Evaluation Center · Windows 11 Enterprise 25H2 Arm64 ISO** | 90 天全功能、**不需要产品密钥**、官方明确提供 Arm64。旧的 Windows 11 Development Environment 预制 VM（Hyper-V/VMware/VirtualBox/Parallels 四格式）已于 **2024-10-23 全部到期**并下线，老地址 301 跳到 `learn.microsoft.com/windows/dev-environment/`，该页已不提 VM 镜像；且它当年只有 x64 |
| **D7** | win×x86_64 运行时靠 guest 内 **Prism** 模拟 | Win11 24H2+ 自带，25H2 满足。**一台 ARM guest 覆盖两个 win 格** |
| **D8** | 带外 KVM = **QEMU VNC server + 本仓 `agenterm-vnc`** | `SessionHandle::send_key` / `send_mouse` / `request_full_refresh` + `Frame.rgba` 已就位，K/V/M 三样齐；`agenterm-cu` 的 `vnc` 目标族本就在 PRD 30 路线上。**用被测程序自己的截图 API 验它自己的渲染是循环论证**，故带外为真值 |
| **D9** | 带外 KVM 与 Rosetta 冲突时**优先 KVM** | 带外 KVM 需 QEMU 后端，Rosetta 只在 Apple Virtualization 后端有。交互/渲染验证两台 VM 走 QEMU（arm64 有 hvf 加速，接近原生）；Rosetta 那台只留给 lnx×x86_64 的**无头 CLI smoke** |

---

## 3. 边界

- 本 goal **不改产品代码语义**，只增构建/测试装置与证据面。
- **不删** `.github/workflows/`。CI 保留为兜底与发布公证。
- **不得**把 Prism / Rosetta / QEMU TCG 下的结果记成「真机 PASS」。
  凡经模拟或翻译的证据，回执必须带 `emulated=<prism|rosetta|tcg>` 标注。
- Wine 路线不再扩展：`AGENTS.md` 自陈 Wine 撑不住 interactive ConPTY（tab 起来即 `dead`），
  真 Windows guest 落地后，Linux+Wine 降为可选的快速 lint/unit 回路。

---

## 4. 任务（按序，P0 → P3）

### P0 — 六格构建打通

| 格 | 驱动 | 完成判据 |
|----|------|----------|
| osx × aarch64 | 原生 | 已有；纳入统一入口 |
| osx × x86_64 | 原生 + `rustup target add` | 四个产品 bin 全出，**无 `--bin` 过滤** |
| lnx × aarch64 | cargo-zigbuild | 同上 |
| lnx × x86_64 | cargo-zigbuild | 同上，且钉住 glibc 下限 |
| win × aarch64 | cargo-xwin | 同上 |
| win × x86_64 | cargo-xwin | 同上 |

收口物：`scripts/bootstrap.sh` 增 **`client-build-all`** task，内部按格并发。
**不再新增第五个 `build-*.sh`**——现有已有四个，再加就散了。

### P1 — 带外 KVM 回路（**先于装 VM**）

先在本机拿一个现成 VNC server 把 `agenterm-vnc` 的 K/V/M 回路验通，
再去接 VM。**两个不确定性不叠加 debug。**

1. `send_key` / `send_mouse` 能改变 server 端画面；
2. `request_full_refresh()` 拿到整帧，`Frame.rgba` 落 PNG；
3. 同一操作序列两次截图可比（视觉回归的前提）。

### P2 — 运行时落地

**先做宿主两格，不需要任何 VM：**

- osx × aarch64：宿主自身，原生
- osx × x86_64：`softwareupdate --install-rosetta` 后**直接在宿主跑** x86_64 产物。
  GUI 与 PTY 均真实可用，是六格里最省事的一格

**再做 VM 四格：**

- **VM1 Windows 11 ARM64**（QEMU + hvf，Enterprise Eval Arm64）→ win×aarch64 原生 + win×x86_64 经 Prism
- **VM2 Linux arm64**（QEMU + hvf）→ lnx×aarch64 原生 + GUI 交互验证
- **VM3 Linux x86_64**：Apple Virtualization + Rosetta 的**无头 CLI smoke**（不承诺 GUI）；
  需要**真 x86_64 内核**时改用 QEMU TCG 全模拟，只在发布前跑

产物入 VM 走 ssh/scp 或 `utmctl file push` / `exec`（需 guest tools）。
**禁止共享文件夹拖拽**——进不了脚本。

### P3 — 一键化与证据

- 六格构建 + 四格 smoke 收敛为单一入口，产出结构化回执
- 回执落 `~/.local/share/agenterm/evidence/six-cell-<UTC>`，与现有 evidence 惯例一致

---

## 5. 完成判据

### 5.1 六格运行时覆盖矩阵（**这是本 goal 的真实承诺，不要读成「六格真机」**）

| 格 | 执行载体 | CLI smoke | 交互/渲染闭环 | 证据等级 |
|----|----------|-----------|---------------|----------|
| osx × aarch64 | **宿主自身** | ✅ | ✅ | `native` |
| osx × x86_64 | **宿主 + Rosetta 2**（无需 VM） | ✅ | ✅ | `emulated=rosetta2`（真 macOS 内核与框架，仅指令翻译） |
| lnx × aarch64 | VM2（QEMU + hvf） | ✅ | ✅ | `native`（虚拟化非模拟） |
| lnx × x86_64 | VM3（Rosetta-in-Linux；发布前改 QEMU TCG） | ✅ | ❌ 不承诺 | `emulated=rosetta` / `emulated=tcg` |
| win × aarch64 | VM1（QEMU + hvf） | ✅ | ✅ | `native`（虚拟化非模拟） |
| win × x86_64 | VM1 内 Prism | ✅ | ✅ | `emulated=prism` |

**六格构建 + 六格 CLI smoke + 五格交互闭环。** 唯一不承诺交互闭环的是 lnx×x86_64——
它是四个非宿主格里唯一既非原生架构、又要额外起一台 VM 才有 GUI 的，性价比最低。

> **osx×x86_64 不需要 VM。** Rosetta 2 直接在宿主上跑 x86_64 macOS 产物，
> GUI 与 PTY 都真实可用。这是六格里最省事的一格，早前的 P2 规划漏了它。

### 5.2 判据

| 层 | 判据 |
|----|------|
| 构建 | 一条命令产出六格全部产品 bin，含 SHA-256 清单；任一格失败必须显式红，不得静默跳过 |
| 运行时 · CLI | **六格**各自 `agenterm cli --help` 通过，并跑该平台**对应的**既有 smoke（见 §5.16——原文只写 `cli-smoke` 是错的，那是 Windows 专属任务） |
| 运行时 · 交互 | 除 lnx×x86_64 外的**五格**完成按键→截图闭环；两个 VM 内的格走**带外 VNC** |
| 诚实性 | 每条证据标注 §5.1 的等级；无标注的模拟结果视为无效证据 |

### 5.3 本 goal **不**给你什么（验收时不得含糊）

1. **没有任何真实 x86_64 硅片。** 三个 x86_64 格全部是翻译或模拟。
   指令级正确性基本可信（Rosetta 2 与 Prism 都正确保持 x86 的 TSO 内存序），
   但**性能数字一律无效**，不得用于任何 perf 判断。
2. **硬件探针类能力在 x86_64 格上读到的是虚拟值。** 本仓
   `agenterm-platform` 的 `cache-hierarchy` / `processor-topology` /
   `virtualization-probe` / `host-memory` 这几个 feature，在 Rosetta / Prism / VM 下
   拿到的是宿主或虚拟拓扑，**不是**目标平台真实拓扑。这几个面的验收仍需真机或 CI。
3. **不覆盖 Windows 真机独有的签名/驱动路径**：Authenticode 信任链、
   AppContainer 隔离在 eval guest 里的行为可能与终端用户机不同。
4. **`script-*` feature 图未纳入 P0 判据**，且**已实测证明它确实做不到**（§5.11）：
   `script-lua` 在两个 driver 上都挂，`script-sql` 在 windows 挂。
   带 scripting 引擎的产物仍归 CI。

> 阻塞格并非零覆盖：`six-cell-qualify` 对六格一律先做静态验证（§5.9），
> 所以「runtime BLOCKED」的格仍然保证是**正确架构、正确格式**的产物。
> 静态验证不能替代运行时，但它把「完全不知道」降为「知道产物本身是对的」。

CI 因此不能删——它是上述四条的兜底与公证面，这与 §0「降级为兜底」是一致的，不是矛盾。

---

## 5.4 执行回执 · P0（2026-08-25）

### 已达成：六格构建全绿，单命令可复现

```
AGENTERM_BOOTSTRAP_TASK=client-build-all ./scripts/bootstrap.sh dev
```

| 格 | driver | 产物数 | 校验 |
|----|--------|--------|------|
| aarch64-apple-darwin | cargo | 4 | Mach-O arm64 |
| x86_64-apple-darwin | cargo | 4 | Mach-O x86_64 |
| aarch64-unknown-linux-gnu | cargo-zigbuild | 4 | ELF aarch64 PIE，interp `/lib/ld-linux-aarch64.so.1` |
| x86_64-unknown-linux-gnu | cargo-zigbuild | 4 | ELF x86-64 PIE |
| aarch64-pc-windows-msvc | cargo-xwin | 4 | PE32+ Aarch64 |
| x86_64-pc-windows-msvc | cargo-xwin | 4 | PE32+ x86-64 |

每格 4 件（3 个 PE/ELF/Mach-O + 1 个 `libagenterm` 动态库），24 件全部带 SHA-256，
落 `target/qualification/six-cell/summary.json`（`kind: agenterm-six-cell-build`）。

**改动面**（仅装置，无产品语义）：

- `scripts/rh/build.rh`：`--driver` 增第三值 `cargo-zigbuild`；主 args 与 `abi-*` profile 两处分支都接
- `scripts/rh/build-all.rh`：新增，六格驱动
- `agenterm.tasks.json`：新增 `client-build-all` task 与同名 contract

### 三条与立项预判不符的实测（**修正 §6，不要再按原文执行**）

| 原预判 | 实测 |
|--------|------|
| §6.1「`winresource` 是 P0 最可能的唯一硬阻力，需 `brew install llvm` 取 `llvm-rc`」 | **不成立**。两个 windows 格在**没有 llvm 在 PATH** 的情况下直接编过，`.exe` / `.com` / `.dll` 齐出。llvm 装了但没用上 |
| §4「`client-build-all` 内部按格并发」 | **改为串行**。cargo 对 build directory 持整场排他锁，共享一个 `target/` 的六个并发 `client-build` 只会在锁上排队。真并发需每格一个 `CARGO_TARGET_DIR`，该 trade 未取 |
| 未预见 | task schema 把 contract `budget.timeout_ms` 上限钉在 **3_600_000 ms**，六格共享一小时天花板；单格进程超时因此收到 1800 s |

### 尚未做（P0 范围内的诚实缺口）

- ~~六格只验了 `dev` profile~~ **已清**：`dev` / `release-fast` / `release` 三个 profile 各自六格全绿
- ~~`client-clippy` 未按新 driver 验~~ **已清**（并因此改对了一处，见 §5.8）
- ~~glibc 下限未钉~~ **已清**（见 §5.8）
- ~~`script-*` feature 图仍未验~~ **已验，且结论是负面的**（见 §5.11）。P0 范围内无剩余缺口。

---

## 5.5 执行回执 · P1 带外 KVM 回路（2026-08-25）

### 已达成：K / V / M 三样对活 guest 全部对上

靶子是一台**无盘 UEFI guest**——不需要任何操作系统镜像就能得到帧缓冲和输入设备，
所以 P1 完全不依赖 P2 的装机进度，「两个不确定性不叠加」这条排序兑现了：

```
qemu-system-aarch64 -machine virt -accel hvf -cpu host -m 512 \
    -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
    -device virtio-gpu-pci -device qemu-xhci -device usb-kbd -device usb-tablet \
    -qmp unix:<sock>,server,nowait -vnc 127.0.0.1:1

cargo run -p agenterm-vnc --example vnc-kvm-probe -- 127.0.0.1 5901 <out> agenterm
```

| 判据（§4 P1） | 结果 |
|---------------|------|
| `send_key` / `send_mouse` 能改变 server 端画面 | **PASS**：guest UEFI shell 提示符从 `Shell> _` 变成 `Shell> agenterm_` |
| `request_full_refresh()` 拿整帧，`Frame.rgba` 落 PNG | **PASS**：1280×800，before/after 两张 |
| 同一操作序列两次截图可比 | **PASS**：逐像素 diff 得 317 / 1024000（0.03%） |

证据：`~/.local/share/agenterm/evidence/six-cell-20260824T175415Z/p1-kvm-probe/`
（`before.png`、`after.png`、`result.txt`），等级 `native`——aarch64 guest 走 Apple Hypervisor
虚拟化，无指令模拟。

**改动面**：`crates/agenterm-vnc/examples/vnc-kvm-probe.rs` 新增；该 crate 的
`Cargo.toml` 加一条 `png` dev-dependency。`agenterm-vnc` 的机制代码**一行未改**——
§2 D8「K/V/M 三样已就位，不要重写」经实测成立。

### 一条实测教训（写进 §6）

**探针必须敲可见字符，不能按导航键。** 首版用 ESC + 方向键，在 UEFI shell 提示符下
它们本就无意义，结果 diff 为 0——而「按键无意义」和「按键根本没到」在像素层面
完全不可区分，失败信息因此不可读。改成敲可打印字符后立刻可判。这条对后面
Windows / Linux guest 同样成立。

### 尚未做

- ~~只验了 aarch64 guest~~ **已清**（见 §5.10）
- ~~QMP 通道未实际调用~~ **已清**（见 §5.10，并挖出一个静默陷阱）
- ~~视觉回归的变量钉定未做~~ **已清**（见 §5.12）。**P1 范围内无剩余缺口。**

---

## 5.6 执行回执 · P2 运行时（2026-08-25，进行中）

### 已达成：lnx × aarch64 全闭环（CLI + GUI + 带外 KVM）

VM2 从零立起，全程无人工点击：

| 环节 | 做法 |
|------|------|
| 镜像 | Debian 13 arm64 **cloud image**（约 410 MB），不走安装器 |
| 首次配置 | cloud-init NoCloud seed（`hdiutil makehybrid` 造 `CIDATA` ISO），建 `agent` 用户 + 专用 ed25519 密钥 + README 那串 X11 依赖 |
| 显示 | lightdm 自动登录 + openbox，Xorg 占 vt7，1280×800，virtio-gpu |
| 带外 | QEMU `-vnc 127.0.0.1:2`，由 `vnc-kvm-probe` 驱动 |
| 产物入 VM | `scp`（约 414 MB 四件） |

| 判据 | 结果 |
|------|------|
| CLI smoke | **PASS**：`agenterm cli --help` exit 0，完整 usage banner |
| GUI 起得来 | **PASS**：AgenTerm 0.1.16 窗口完整——tab 树 `@1 bash`、工具栏 `<Tabs`/`New`/`Control Center`/`Settings`/`En|Zh`/`A-`/`A+`、活 bash 提示符、Composer + Send、状态栏 `CWD · ~/agenterm` 与 `IME: off` |
| 带外交互闭环 | **PASS**：带外键盘敲的字符出现在 app 的终端 pane 里，再由带外帧缓冲截回 |

证据：`~/.local/share/agenterm/evidence/six-cell-20260824T175415Z/p2-lnx-aarch64/`，
等级 `native`。

### 第二条实测教训（补进 §6.4）

**`settle()` 等「画面安静」在活桌面上是死等。** 无盘 UEFI guest 静止，所以 P1 没暴露；
接上有光标闪烁的真桌面后，帧永不停，探针挂死五分钟。已给 `settle()` 加总预算上限。
根因不是 bug 而是概念：**活屏幕的截图本质是采样，不是稳定态**，
§6.4 「先钉死变量」因此不是优化建议而是前置条件。

### 未达成（P2 剩余）

| 格 | 阻塞 |
|----|------|
| osx × aarch64 | **全部 PASS**：CLI smoke、GUI 启动、结构化互动往返（§5.14）。带外截图仍缺 TCC 授权 |
| osx × x86_64 | **等 Rosetta**：`softwareupdate --install-rosetta` 需 sudo 密码，本机无免密。已实测确认阻塞——`cli --help` 退出 127（`Bad CPU type in executable`），产物本身是好的 |
| lnx × x86_64 | ~~等 Rosetta~~ **已解除**：改走 QEMU TCG 全模拟，CLI smoke **PASS**（见 §5.7） |
| win × aarch64 | **等 ISO**：Evaluation Center 下载需先填注册表单，须由人完成 |
| win × x86_64 | 同上（与 win×aarch64 同一台 VM） |

---

## 5.7 执行回执 · lnx × x86_64 与 P3 一键化（2026-08-25）

### lnx × x86_64：绕开 Rosetta，拿到更强的证据

原计划这一格等 Rosetta（§4 P2 的 VM3），而 Rosetta 卡在 sudo 密码上。
但 §4 P2 本来就写了备选：「需要**真 x86_64 内核**时改用 QEMU TCG 全模拟」。
直接取备选，阻塞自然消失——**而且证据比原计划强**：

| | 原计划 Rosetta-in-Linux | 实际 QEMU TCG |
|---|---|---|
| 内核 | aarch64 | **真 x86_64**（6.12.101+deb13-amd64） |
| 用户态 | 翻译 | 原生 x86_64 指令，整机模拟 |
| 前置 | 要 sudo 装 Rosetta | 无 |

`agenterm cli --help` exit 0，74 行 usage banner。等级 `emulated=tcg`，性能数字仍然无效。
cloud-init 刻意不装任何包——这一格按 §5.1 只欠无头 CLI smoke，TCG 下的 apt 花费远超它买到的覆盖。

证据：`~/.local/share/agenterm/evidence/six-cell-*/p2-lnx-x86_64/`

### P3：一条命令走完构建 + 运行时

```
AGENTERM_BOOTSTRAP_TASK=six-cell-qualify ./scripts/bootstrap.sh dev
```

```
Six-cell qualification [dev] build=PASS
  PASS     aarch64-apple-darwin        [native]
  BLOCKED  x86_64-apple-darwin         [emulated=rosetta2]
  PASS     aarch64-unknown-linux-gnu   [native]
  PASS     x86_64-unknown-linux-gnu    [emulated=tcg]
  BLOCKED  aarch64-pc-windows-msvc     [native]
  BLOCKED  x86_64-pc-windows-msvc      [emulated=prism]
  6 cells, 0 failed, 3 blocked
```

**新增面**：`scripts/rh/six-cell-qualify.rh`、`scripts/six-cell-runners.json`、
task + contract `six-cell-qualify`。

设计要点，都是 §3 那条「不得静默跳过」的直接后果：

- **构建是宿主的事，运行不是**，所以 runner 描述符是这两者之间的缝：
  每格声明 `host` / `ssh` / `blocked`。
- **`blocked` 必须带 `reason`，并原样进回执**。跳过一格在回执上看起来和通过一格
  没有区别——这正是这个 task 存在的理由。
- **`level` 从不推断**，由描述符显式声明。分不清 native 与 emulated 的回执不是证据。
- **BLOCKED 不算失败**（退出码 0），但 `totals.blocked` 永远在回执里，
  不可能被误读成覆盖。

### 三条 rh 语言实测（写给下一个改 `.rh` 的人）

| 写法 | 结果 |
|------|------|
| `let x = ();` | AOT native pack 不支持 unit 字面量 |
| `f(g())` 嵌套调用 | native pack 不接受「调用作为另一调用的实参」 |
| 对 JSON 派生值取 `.len` / `sub_string` | 编译期 `rh_fail: json_path: len`。描述符改成 home 相对路径字段即可绕开 |

另有一条 `std::time::system_time_now` **在 task manifest 的 capability 清单里，
但 AOT native pack 未实现**，全仓无第二处使用。回执因此不带时间戳，以文件 mtime 为准。

### 一条实测 bug（我自己的编排写错，已修）

首版 scp 直接覆盖远端 binary，在 VM2 上 FAIL：`dest open "agenterm/agenterm": Failure`。
根因不是磁盘或网络，是**那台 VM 里 GUI 还开着，Linux 不允许覆写正在执行的可执行文件**
（ETXTBSY）。改为先传 `<name>.new` 再 `mv -f` 落位——`rename(2)` 对运行中的 binary 安全，
活进程留在旧 inode 上。**跑过 GUI smoke 的 VM 必然处于这个状态**，所以这不是偶发。

---

## 5.8 执行回执 · P0 欠账清理（2026-08-25）

### glibc 下限：D3 兑现，而且数字是量出来的不是选出来的

D3 承诺「可钉 glibc 下限」时随手写了 `2.17`——那是业界惯用的「最老还有人跑」的数字。
**实测链接失败**：

```
ld.lld: error: undefined symbol: getrandom
  >>> referenced by entropy.rs:9 (crates/agenterm-platform/src/adapters/linux/entropy.rs:9)
```

`getrandom(2)` 的 glibc wrapper 是 **2.25** 才有的，本仓的 Linux entropy adapter 直接调它。
所以这棵树的 glibc 下限不是可选项，是被自己的代码钉死的：**≥ 2.25**。
取 2.25 之上第一个有发行版意义的台阶 **2.28**（RHEL 8 / Debian 10），链接通过，
并在 VM 内用 `objdump -T` 验证产物最高只要 `GLIBC_2.28`：

```
GLIBC_2.17  GLIBC_2.18  GLIBC_2.25  GLIBC_2.27  GLIBC_2.28
```

`build.rh` 新增 `--glibc VERSION`（只允许配 `cargo-zigbuild`），`build-all.rh` 为两个
linux 格钉 `2.28`。**调高这个数字会静默缩小能跑发布件的人群，调到 2.25 以下则根本链接不过。**

### clippy：我写错了一处，实测纠正

§5.4 里我写「`cargo-zigbuild` 的 clippy 走 plain `cargo clippy`，因为 clippy 只出 metadata
不需要 linker」。**错了。** `ring` 在 build script 里编 C，交叉目标仍然需要 zig 的 `cc`：

```
error: failed to run custom build command for `ring v0.17.14`
```

而且 `cargo zigbuild clippy` 不存在——`cargo-zigbuild` 的 `clippy` 是它自己的子命令。
改法是把 `cargo-zigbuild` 当**独立程序**调用（而非 `cargo` 的子命令），
`build` → `zigbuild`，其余 action 原样传。改后 `PASS client Clippy linux-aarch64`。

### 三个 profile 全过

| profile | 六格 |
|---------|------|
| `dev` | PASS ×6 |
| `release-fast` | PASS ×6 |
| `release`（opt-level z + thin LTO + strip） | PASS ×6 |

### 一个顺带量出来的产品事实（**不在本 goal 范围，只作报告**）

`release` 的 GUI 体积跨平台差得很远，而 README 的公开预算写的是 GUI 4 MiB：

| 格 | `agenterm` |
|----|-----------|
| win × aarch64 | 3.9 MiB（在预算内） |
| osx × aarch64 | 6.6 MiB |
| lnx × aarch64 | 7.9 MiB |

Unix 侧多出来的是 winit / softbuffer / wayland / x11 那条栈，Windows 走 Win32/GDI 没有。
**这属于产品预算口径问题，本 goal §3 明令不碰产品语义，故只报不改。**
值得注意的是：这个事实本来只有 CI 能看到，现在本机六格一跑就出来了——
这正是把回路拉回本机的价值。

### Windows guest 已预置到位（只差 ISO）

`~/vm/win-arm64/` 已备好 64 GB qcow2、UEFI code + varstore、virtio-win 驱动 ISO（483 MB）、
以及 `run.sh`（`install` 参数挂安装介质；含 `usb-tablet`、`-vnc 127.0.0.1:4`、QMP socket、
ssh 端口转发 2224）。人把 Evaluation Center 的 ISO 放到 `~/vm/images/win11-arm64.iso`，
`sh ~/vm/win-arm64/run.sh install` 即可开装；装完把 `scripts/six-cell-runners.json` 里
两个 windows 格从 `blocked` 改成 `ssh`。

---

## 5.9 执行回执 · 阻塞格的静态验证（2026-08-25）

「不能执行」不等于「什么都验不了」。三个阻塞格此前是**零覆盖**——
连产物是不是目标架构都没人看过。`six-cell-qualify` 现在对**每一格**先做静态验证，
再决定跑不跑运行时：

```
  PASS     aarch64-apple-darwin        [native]             static=PASS
  BLOCKED  x86_64-apple-darwin         [emulated=rosetta2]  static=PASS
  PASS     aarch64-unknown-linux-gnu   [native]             static=PASS
  PASS     x86_64-unknown-linux-gnu    [emulated=tcg]       static=PASS
  BLOCKED  aarch64-pc-windows-msvc     [native]             static=PASS
  BLOCKED  x86_64-pc-windows-msvc      [emulated=prism]     static=PASS
```

判据是描述符里的 `expect_file`（`file(1)` 输出必须包含的子串）。
**它抓的是「交叉构建静默产出宿主架构」这一类失败**——这是六格构建最危险的静默错误，
因为产物看起来完全正常。

### 闸本身经过反向验证

没验过失败的闸不是闸。把 win×aarch64 的 `expect_file` 故意改成 `x86-64` 后：

```
FAIL aarch64-pc-windows-msvc
  file(1) says "PE32+ executable (GUI) Aarch64, for MS Windows", expected "PE32+ executable (GUI) x86-64"
```

退出码非 0，诊断精确到实际值与期望值。

### 三条 rh 语言实测（补充 §5.7 那三条）

| 写法 | 结果 |
|------|------|
| 函数多出口（`return ""` + 尾部 String） | `infer_return_kind` 被污染 |
| 单出口 helper 仍失败 | 参数被推成 `i64`，调用点报 `expected i64, found String` |
| `string.contains(变量)` | AOT codegen 不支持。全仓只有「数组 contains 变量」与「字符串 contains 字面量」两种用法 |

绕法：**把字符串匹配推进进程**（`file -b "$1" \| tee "$3" \| grep -qF "$2"`，
一次同时拿到描述文本和退出码），rh 只看 `success`。
另外 `agenterm rh compile <file>` 能定位这类问题——`rh check` 会过，AOT codegen 才报错。
**但这个工具有使用边界，见 §5.18：它不解析 `import`，对引用了模块的脚本会给假失败。**
上面这几条是在**无 import 的自写脚本**上得到的，不受影响。

---

## 5.10 执行回执 · P1 剩余欠账（2026-08-25）

### x86_64 guest 的同一回路：PASS，而且覆盖了另一条输入路径

`vnc-kvm-probe` 打 VM3（`-machine q35 -accel tcg`）的控制台，敲 `agent`，
字符出现在 tty1 登录提示符上，diff 184 / 1024000。

**这一格的键盘走 PS/2**——VM3 没挂 `usb-kbd`，而 VM2 走 USB。
所以两次验证覆盖的是两条不同的虚拟输入设备路径，不是同一条跑了两遍。
等级 `emulated=tcg`。

### QMP `send-key`：组合键路径闭合（§6.3）

```json
{"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":"ctrl"},
                                           {"type":"qcode","data":"alt"},
                                           {"type":"qcode","data":"f2"}]}}
```

VM3 控制台从 tty1 切到 tty2。§6.3 此前只是「据说要走 QMP」，现在是实测。

### QMP `screendump`：能用，但有一个**静默说谎**的陷阱

| guest | 显示设备 | 有无 VNC 客户端 | 结果 |
|-------|----------|----------------|------|
| VM3 | 默认 VGA（文本控制台） | 无 | 真实画面 |
| VM2 | virtio-gpu（X 在跑） | **无** | **`Display output is not active.`** |
| VM2 | virtio-gpu（X 在跑） | **有** | 真实的 AgenTerm GUI |

**两种情况都返回 `{"return": {}}`。** 失败是静默的，而且长得和成功一模一样——
一张「成功」的截图里什么都没有。

根因：virtio-gpu 的 console 在没有显示客户端索要 scanout 之前不激活。
排查中排掉的两个假设：`device` / `head` 参数**帮不上忙**（`device` 收的是设备 **id**，
传 QOM 路径直接 `DeviceNotFound`），而给 GPU 加 `id=` 也**不改变行为**——
在一台一次性 guest 上验证过，带不带 `device` 参数都能截到真图。

**实践结论**：要用 QMP `screendump` 截 virtio-gpu guest，就得在截图期间保持一个
显示客户端连着；否则别用它，直接走 RFB 那条路。两条路本来就都通，
`screendump` 的价值是作为 RFB 之外的独立交叉验证——但**必须先确认它没在拍占位图**。

证据：`~/.local/share/agenterm/evidence/six-cell-*/p1-x86_64-kvm/`、`.../p1-qmp/`

---

## 5.11 执行回执 · `script-*` feature 的 C 编译边界（2026-08-25）

§6.9 要求「必须单独验一次，不得默认它通」。验了，**六个组合里三个不通**——
而且 §6.9 自己的预判（「zig cc 侧应无碍，xwin 侧走 clang-cl 通常可行」）**两半都错**。

| feature | lnx × aarch64（zigbuild） | win × aarch64（xwin） |
|---------|--------------------------|----------------------|
| `script-lua` | **FAIL** | **FAIL** |
| `script-sql` | PASS | **FAIL** |
| `script-qjs` | PASS | PASS |

### 三个互不相同的根因（都不是配置错误）

**1. `script-lua` × linux-aarch64** — `mlua-sys` → `luajit-src`，链接阶段：

```
ld.lld: error: undefined symbol: _Unwind_GetCFA / _Unwind_DeleteException
                                 _Unwind_SetGR / _Unwind_SetIP / _Unwind_Find_FDE
```

LuaJIT 需要的 unwinder 符号没有被 zig 提供的链接过程拉进来。

**2. `script-lua` × windows-aarch64** — `luajit-src/src/lib.rs:284` 直接 panic：

```
failed to find cl
```

**LuaJIT 的构建按名字找 MSVC 的 `cl.exe`。** cargo-xwin 提供的是 clang-cl 加
MSVC CRT/SDK，满足不了一个硬编码的 `cl` 查找。这是 `luajit-src` 的性质，不是缺个 flag。

**3. `script-sql` × windows-aarch64** — 依赖链是
`psm` ← `stacker` ← `recursive` ← `sqlparser` ← `agenterm-sql`：

```
psm@0.1.32: src/arch/aarch_aapcs64.s:30:1: error: unrecognized instruction mnemonic
```

`psm` 带的是 GAS 语法的 aarch64 汇编，clang-cl 的 MSVC 模式内置汇编器不认。

### 这对本 goal 意味着什么

**六格宿主化成立的前提是默认 feature 图。** D4 的根因写着「默认 feature 图是纯 Rust」——
这句话是对的，也正因为如此它**不能外推到 scripting 引擎**。要在本机出带
`script-lua` / `script-sql` 的 windows 产物，得先解决上面两个上游约束，
或者继续让 CI 承担这两格。

**未尝试修复**：§6.9 要的是验证，不是让它通过；在这里发明绕法超出本 goal 边界。
`script-qjs` 两侧都过，说明 `rquickjs` 的 C 构建对两个 driver 都友好——
问题不在「交叉编译 C」这件事本身，而在特定上游 crate 的构建假设。

证据与三份完整日志：`~/.local/share/agenterm/evidence/six-cell-*/p0-script-features/`

---

## 5.12 执行回执 · 视觉回归的变量钉定（2026-08-25）

P1 此前的 diff **依赖 guest 恰好静止**，那不是判据，是运气。现在是一道带基线的闸。

### guest 侧：先钉死变量，再谈比较

```
xsetroot -solid '#101010'     # 平底根窗口，壁纸不贡献噪声
xset s off -dpms              # 关屏保与省电熄屏
```

分辨率由探针**断言**而非假设：尺寸不符直接报错，而不是把它算成「99% 像素变了」——
后者会用一个看起来合理的比例把真实原因盖掉。

### 探针侧：基线比的是**输入前**那一帧

回归闸要回答的是「空闲屏幕还是不是原来那样渲染」。
把探针自己敲的字算进去，每一轮都会因为跟渲染无关的原因漂移。
因此新增 `TEXT=""` 的**纯观察模式**：只采集不发输入——
一个测量装置不该扰动它测量的对象。

```
vnc-kvm-probe <host> <port> <out> [TEXT] [BASELINE.png] [MAX_CHANGED_PERCENT]
```

| 场景 | drift | 退出码 |
|------|-------|--------|
| 干净复跑 | 0 / 1024000（**0.00%**） | 0 PASS |
| 扰动后 | 734 / 1024000（0.07%） | **1 FAIL** |

### 容差是算出来的，不是取的整数——而且我第一版取错了

第一版默认容差拍了 `0.50%`。**反向测试当场证明它没用**：
扰动屏幕后闸照样 PASS。

算一下就明白：1280×800 的 0.5% 是 **5120 像素**，
而**一整行终端文字只有约 600 像素（0.06%）**。
0.5% 的容差会让「一整行渲染错了」大摇大摆走过去。

实测空闲噪声（钉死变量后）是 **0.00%**，所以默认改成 **0.02%**——
高于噪声有余量，又比一行文字低一个量级。

**这条能被抓到，只因为反向测试真的跑了。** 一个只验过 PASS 的闸，
和没有闸的区别只是多一行绿色输出。

证据：`~/.local/share/agenterm/evidence/six-cell-*/p1-visual-baseline/`

---

## 5.13 执行回执 · UI 互动反馈测试（2026-08-25）

键盘 smoke 只能证明「按键到了 PTY」。**它证明不了鼠标坐标真的点中了控件**——
而 §6.2 警告的正是这个：没有 tablet 设备时指针被当成相对移动，点击会飘，
**而每一次 `send_mouse` 调用照样返回 Ok**。这条警告在此之前从未被真正执行过。

探针新增 `--steps` 脚本化互动模式（`click:X,Y` / `type:` / `key:` / `wait:`），
每步之后取整帧、落 PNG、报本步像素增量。全程带外：QEMU `-vnc` +
`SessionHandle::send_mouse`。

| 步骤 | 目标 | 像素变化 | 结果 |
|------|------|----------|------|
| `click(1135,144)` | `A+` | 3367 | 字号变大 |
| `click(1135,144)` | `A+` | 4445 | 再变大 |
| `click(966,144)` | `Settings` | **675998** | 面板展开（约 66% 屏幕） |
| `click(525,417)` | `Classic Day` | 675991 | 主题由暗翻亮，选中标记移到 `Classic Day *` |
| `click(693,564)` | `Cancel` | 676000 | 还原 |

**每一次点击都命中了它指名的控件。** Settings 面板内容在截图里可读
（渲染器、`Size 12 pt`、四个外观预设、Cancel / Apply）——`Size 12 pt` 这一项
反过来独立佐证了前两次 `A+` 也落在了实处。

### 往返比单次点击更重要

预览 → 还原让 guest 回到起始状态，所以这个互动测试**可重复**，
不会把 app 状态悄悄累积给下一轮。一个会改变自身前提的测试，第二次跑就已经不是同一个测试了。

证据：`~/.local/share/agenterm/evidence/six-cell-*/p2-ui-interaction/`

---

## 5.14 执行回执 · osx × aarch64 的互动反馈（2026-08-25）

宿主这一格的**带外**通道被 TCC 挡着（`screencapture` 报
`could not create image from display`），所以它用了带内通道里最强的信号：
`cli ui-snapshot` 返回的是**结构化 UI 状态**，不是像素。

**为什么这里结构比像素强**：用 app 自己的截图验它自己的渲染是循环论证；
而结构化快照陈述的是「app 认为自己处于什么状态」——**这是另一类断言**，
所以即便通道是带内的，跨动作比较它仍然成立。

```
cli new-window -n probe-tab
  after-new   tabs=2  names=['zsh', 'probe-tab']  active=@2
cli kill-window -t 1
  after-kill  tabs=1  names=['zsh']               active=@1
```

还原后 app 与开测前一致，测试可重复。

### 一条值得记的自我纠错

这个断言的第一版读的是 `layout.tabs`，**动作前后都读到 0**。
一个在动作前后返回同样空值的断言什么都没证明，却长得像通过——
tabs 实际在快照**顶层**（`tabs[]`），不在 `layout` 下。
定位办法是拿已知的 tab 名去遍历快照，而不是相信猜出来的路径。

**这与 §5.12 的容差、§5.9 的静态闸是同一条纪律**：
闸必须在该红的时候真的红过，否则它和没有闸的区别只是多一行绿色输出。

证据：`~/.local/share/agenterm/evidence/six-cell-*/p2-osx-interaction/`

---

## 5.15 执行回执 · 在 minicon 上做实践（2026-08-25）

这套装置一直只在 agenterm 这棵树上证明过。**一个只在它诞生的那棵树上work 的做法不是方法。**
`~/repos/minicon` 是独立仓库，而且它按 git rev 消费
`agenterm-platform` / `agenterm-ui-core`——是那个 crate 的**真实外部消费者**。
它构建还快，这一点本身就切题：**用一个慢回路是没法测试回路的**。

### 六格构建：一次全过，零适配

| 格 | driver | 结果 | 体积 |
|----|--------|------|------|
| aarch64-apple-darwin | cargo | PASS | 1.4M |
| x86_64-apple-darwin | cargo | PASS | 1.4M |
| aarch64-unknown-linux-gnu | cargo-zigbuild | PASS | 4.6M |
| x86_64-unknown-linux-gnu | cargo-zigbuild | PASS | 5.5M |
| aarch64-pc-windows-msvc | cargo-xwin | PASS | 664K |
| x86_64-pc-windows-msvc | cargo-xwin | PASS | 716K |

同样三个 driver，一行没改。顺带证明 **`agenterm-platform` 作为库能交叉构建给外部消费者**。

### 互动：两条互不相干的通道互相印证

在 VM2（lnx×aarch64，原生虚拟化）上，用 minicon 自己的控制端点驱动：

```
send-text "echo SIXCELL_CROSSCHECK_7F3A"  -> {"sent_bytes": 28}  rc=0
send-keys Enter                           -> {"sent_keys": 1}    rc=0
wait-text --timeout-ms 15000 <MARK>       -> {"matched": true}   rc=0
capture-pane                              -> 面板文本含该标记
```

然后**用带外帧缓冲拍同一块屏**，像素里是同样那几行。

**这个一致性才是重点。** app 自己的截图验不了自己的渲染，
而帧缓冲单独也说不出 app 认为发生了什么。
**两条不共享实现的通道报出同一内容，是任何一条单独都做不出的判断。**

`wait-text` 让它可重复而非 flaky：脚本阻塞在条件上，不是睡一个猜出来的间隔。

### 一个产品观察（只报不改，同 §5.8 的规矩）

minicon README 把「Executable ~760 KB，测试套件强制 1 MiB 上限」
与「Supported Windows…; Linux; macOS」并列。但强制它的
`tests/minicon_load_portability.rs` 是 `#![cfg(windows)]`，
其 `shipped_binary()` 还硬编码 `minicon.exe`——**这个上限是 Windows 专属承诺**，
表格却读起来像可执行文件的固有属性。实测：Windows 664K/716K（在限内），
macOS 1.4M，Linux 4.6M/5.5M；而 `strip = true` 本来就开着，
所以 Linux 那个数是**代码不是符号**。与 agenterm §5.8 同形同因：
unix GUI 栈 vs Win32/GDI。

证据：`~/.local/share/agenterm/evidence/six-cell-*/minicon-practice/`

---

## 5.16 判据自身的一处错误，与一次未通过的 smoke（2026-08-25）

### §5.2 原文引错了任务

原文写「六格各自 `agenterm cli --help` 与既有 **`cli-smoke`** 通过」。
查了才发现 `cli-smoke` 在 `agenterm.tasks.json` 里是
`"platforms": ["windows"]`，参数是 `dist/agenterm.exe` / `dist/agenterm.com`——
**它对 Linux 和 macOS 两族格根本不适用**，这句判据从写下起就无法在四个格上成立。

仓里本来就有分平台的对应任务：

| 平台 | 任务 |
|------|------|
| windows | `cli-smoke` |
| linux | `unix-frontend-linux-smoke`、`control-center-linux-smoke`、`platform-ux-parity-smoke-linux` |
| macos | `unix-frontend-macos-smoke`、`control-center-macos-smoke`、`platform-ux-parity-smoke-macos` |

判据已改为「跑该平台对应的既有 smoke」。

**这条值得单独记**：判据本身写错，比某一格没跑通更危险——
一个引用了不存在或不适用之物的判据，永远不会红，只会一直悬着。

### `unix-frontend-macos-smoke` 未通过（实测；**根因至今未定**，§5.17 亦未能解释）

```
AGENTERM_NO_ACTIVATE=1 AGENTERM_BOOTSTRAP_TASK=unix-frontend-macos-smoke \
  ./scripts/bootstrap.sh <gui> <cli> --platform macos
→ EXIT=3  {"code":"host_hard_timeout","exit_class":"limit",
           "message":"script worker ... exceeded the host deadline"}
```

无产物、无残留进程。**未定因**——一个未经验证的假设是：该 smoke 要驱动真实 GUI
并观察原生焦点，而本会话是后台驱动，拿不到交互式会话（与宿主 `screencapture`
被 TCC 拒是同一族问题，见 §5.14）。

**不在本 goal 内深挖**：§3 划的边界是只增构建/测试装置、不碰产品语义，
调一个既有产品 smoke 属于产品侧。记在这里是因为 §3 另一条同样有效——
**不得静默跳过**。这一格的 CLI smoke 通过，平台 smoke 未通过，两者都要写在脸上。

---

## 5.17 `unix-frontend-smoke.rh` 不能 AOT 转译（2026-08-25）

§5.16 把判据改成「跑该平台对应的既有 smoke」之后，去 VM2 里跑 `unix-frontend-linux-smoke`——
**它压根编不出来**：

```
rh transpile error: unsupported expression in native pack:
  test_harness::require( !=(out, "") , "unix_frontend_macos_frontmost_empty")
  @ scripts/rh/unix-frontend-smoke.rh:288
```

> ⚠ **本节原先还引了一段「宿主上用 `rh compile` 复核」作为佐证。那段佐证是无效的，已删。**
> 原因见 §5.18——那个工具不解析 `import`，会把能正常工作的脚本也报成失败。
> **下面保留的是仍然成立的证据。**

仍然成立的两条：

1. **失败发生在真实的 task 路径上**（`rh task run unix-frontend-linux-smoke`），
   不是某个旁路工具的输出。
2. **报错点的源码确实是那个构造**——`scripts/rh/unix-frontend-smoke.rh:288`：

```rhai
test_harness::require(out != "", "unix_frontend_macos_frontmost_empty");
```

`!=` 是一个运算符调用，被直接写进 `require(...)` 的实参位，正是 §5.7 记过的
**「native pack 不接受调用作为另一调用的实参」**。

`unix-frontend-linux-smoke` 与 `unix-frontend-macos-smoke` **共用同一个 entry 脚本**，
所以这一条对两个任务同时成立。

**但范围仅限这一个脚本。** 我一度用 `rh compile` 批量筛过四个 smoke 脚本并得出
「四个全挂」，那个结论**建立在未校准的工具上，已作废**；其余三个脚本能否运行，
现在的状态是**未知**，不是已知失败。

### 为 VM 内跑 rh 任务铺的路（可复用）

VM 里没有 cargo，也不需要：worker 就是已经交叉构建好的 linux 产物。

```
tar czf - --exclude='./target' --exclude='./.git' . | ssh <guest> 'tar xzf - -C ~/agenterm-repo'
AGENTERM_BOOTSTRAP_WORKER=$PWD/agenterm-worker \
  ./agenterm-worker rh task run <task> --manifest $PWD/agenterm.tasks.json -- . <gui> <cli> --platform linux
```

绕开 `bootstrap.sh`（它会先重建 worker，VM 里没有工具链）。仓库树 30 MB，
guest 里没有 `rsync`（最小 cloud image），用 tar over ssh。

### 一处**没查清**的事，如实记

macOS 那格走同一个脚本，却报 `host_hard_timeout` 而不是转译错误——
两格预算都是 300 s，都没有 dependencies，宿主的 AOT 共享 target 已预热（64 MB），
而 `rh compile` 在宿主上是**秒级**报错的。**所以「转译失败」解释不了 macOS 的超时**，
两者症状不同，我没有查出 macOS 那条路径在超时前把五分钟花在哪里。

不继续挖的理由同 §5.16：修一个既有产品 smoke 属产品侧，§3 划在本 goal 之外。
但**把没查清的事写成查清了，比不查更糟**——所以它以「未解释」的形态留在这里。

---

## 5.18 我用了一个没校准过的工具（2026-08-25）

### 事情

§5.9 我写下「`agenterm rh compile <file>` 是定位这类问题的正确工具」，
之后就一直拿它当权威——包括用它批量筛四个 smoke 脚本，得出「四个全挂」。

拿一个**已知能工作**的输入去校准它，这一步我从没做过。做了之后：

```
$ agenterm rh compile scripts/rh/build.rh
rh transpile error: unsupported expression in native pack:
  bootstrap_timing::facts() @ 22:23
```

**`build.rh` 是本 goal 每天在跑的脚本**——`client-build-all` 六格构建全走它。
而走真实 task 路径时它编得好好的（能进到脚本自己的参数解析并报
`build_unknown_argument`）。

### 根因

`rh compile <file>` **不解析 `import`**。脚本里任何对被导入模块的调用
（`bootstrap_timing::facts`、`test_harness::require`…），在它眼里都是
未知命名空间，一律报成 `unsupported expression in native pack`。
task 路径会先把模块解析好，所以不受影响。

两种错误的**形状**其实是能分开的：

| 形状 | 含义 |
|------|------|
| `name: "facts", args: []` — 命名空间调用，实参里没有嵌套调用 | 多半是 import 未解析的**假失败** |
| `require(!=(out, ""), ...)` — 实参位里真有一个调用 | 是 §5.7 那条**真限制** |

我当时看到两者都写着 `unsupported expression in native pack`，就没往下分。

### 影响面

- §5.9 那句已加边界：该工具**只对无 import 的脚本可信**。
  我自己写的 `six-cell-qualify.rh` 没有 import，所以它当时确实帮我抓到了三个真 bug——
  **工具没坏，是我把它的适用范围放大了。**
- §5.17 的「宿主复核」段已删；该节结论收窄为**只对 `unix-frontend-smoke.rh` 成立**，
  依据是真实 task 路径的失败 + 源码里那一行确实是嵌套调用。
- 「四个 smoke 脚本全挂」**作废**。其余三个是**未知**，不是已知失败。

### 教训

这跟 §5.9 的静态闸、§5.12 的容差、§5.14 的空断言是同一件事，只是换了层：
**闸要在该红时红过才算闸；工具要在已知能过的输入上绿过才算工具。**
我给别人的闸做了反向测试，却对自己手里的工具跳过了正向测试。

---

## 5.19 三个 linux smoke 全部不可运行——这次用对了方法（2026-08-25）

§5.18 作废了我那次「四个全挂」的筛查，因为它出自未校准的 `rh compile`。
作废之后剩下的状态是**未知**，不是已知失败——所以得用对的方法重做：
**走真实 task 路径**，并用 §5.18 那个形状判别器过滤假失败。

三个 linux 任务全部在 VM2 里以真实路径跑过，全部失败，**且三条都是「真限制」形状**
（实参位里确实有一个调用），不是零参数命名空间调用那种假失败：

| 任务 | 报错位置与构造 |
|------|----------------|
| `unix-frontend-linux-smoke` | `unix-frontend-smoke.rh:288`　`require(out != "", ...)` |
| `control-center-linux-smoke` | `:296`　`bounded_record_text(std::fs::read_to_string(path), 2048)` |
| `platform-ux-parity-smoke-linux` | `:979`　`require(args_len >= 4, ...)` |

三者都是把一个调用（`!=` / `>=` 运算符，或 `std::fs::read_to_string`）直接写进
另一个调用的实参位——§5.7 记的那条 AOT 限制。

### 这不是「我本来就是对的」

结论看起来和被作废的那条相近，但**证据是全新的，性质也不同**：
之前是旁路工具的输出且未校准，现在是产品自己的执行路径 + 形状判别。
一个结论正确与否，和得到它的方法是否成立，是两件事——
**前者碰巧成立不能追认后者**。

### 对本 goal 的实际影响

§5.2 要求「跑该平台对应的既有 smoke」。对 lnx 两格来说，
**当前没有任何一个可运行的平台 smoke**——三个候选全部编不出来。
所以这一格的账是：`cli --help` 通过、GUI 起得来、带外交互闭环通过（§5.6 / §5.13），
**平台 smoke 无可用者**。

`cli-smoke`（windows）仍是**未知**：没有 Windows guest 就无法走它的真实路径，
而它的 standalone 报错形状（`Dot { output.stdout }`，属性访问）既不属于已知假失败，
也不属于已确认的真限制，不能据此判定。

修这三个脚本属产品侧，§3 划在本 goal 之外。

---

## 5.20 把仓库自己的测试套件跨架构跑起来（2026-08-25）

此前所有运行时证据都止于「产物能跑」。更强的一层是**仓库自己的测试套件在目标格上通过**。
minicon 的三个套件在 macOS 上交叉构建、在 VM2（lnx×aarch64，原生虚拟化）里执行：

```
cargo-zigbuild test --release --target aarch64-unknown-linux-gnu.2.28 --no-run \
    --test minicon_accessibility_linux --test minicon_control --test minicon_alignment
```

| 套件 | 结果 |
|------|------|
| `minicon_control` | **PASS** — `gui_control_surface_isolated_multitab_black_box` |
| `minicon_alignment` | **PASS**（复现烧死路径之后，见下） |
| `minicon_accessibility_linux` | FAIL — **环境所致**，见下 |

`minicon_control` 那条尤其值钱：**一个构建宿主根本无法执行的 GUI 黑盒套件**，
在这里交叉构建、在目标格上通过。

### 坑：交叉构建的测试二进制会带着**构建机的绝对路径**

`env!("CARGO_BIN_EXE_minicon")` 与 `env!("CARGO_MANIFEST_DIR")` 是**编译期**宏，
二进制里存的是 `~/repos/minicon/...` 这样的字面量。
**在运行时设同名环境变量毫无作用**——这是我第一反应试的，失败点纹丝不动。
真正的解法是在 guest 里把那些路径原样造出来，之后 `minicon_alignment` 立刻通过。

要把测试二进制送去另一台机器跑，就得复现这些路径，否则套件会因为
**与代码无关的理由**失败。

### 这一轮真正的目的：检验一个当时无法检验的改动

本会话早些时候把 SEND 控件拆成了 SEND / NEWLINE。
`real_atspi_tree_edits_command_and_activates_send` 测的正是那个控件，
而它是 **Linux 专属**测试——在构建宿主上跑不了，所以那个改动**是在未经它检验的情况下发出去的**。

它现在报 `timed out waiting for composer focus`。**我没有猜原因**：
把改动前的提交按同一目标构建、跑同一套件——**以完全相同的方式失败**
（同一测试、同样 20.11 s 超时）。所以失败是环境的（guest 没有完整 AT-SPI 会话），
拆分不是原因。

**这没有证明的是**：a11y 的断言仍然没有真正跑过，
所以拆分对它们**仍然缺少正面验证**。已确立的只是「它没有引入这个失败」。

### 补记：那个环境缺口已补上，拆分现在有正面验证了

套件需要一条 **D-Bus 会话总线**。guest 里有 system bus，也有 X 会话自己的 bus，
但**经 ssh 启动的进程两条都继承不到**。把套件放进 `dbus-run-session` 跑，
at-spi-bus-launcher 被 dbus 激活、SpiRegistry 认领 `org.a11y.atspi.Registry`：

```
DISPLAY=:0 dbus-run-session -- ./mc-tests/minicon_accessibility_linux
→ SpiRegistry daemon is running with well-known name - org.a11y.atspi.Registry
→ test result: ok. 1 passed   (0.13 s，复跑两次一致)
```

**`real_atspi_tree_edits_command_and_activates_send` 带着 SEND/NEWLINE 拆分通过了。**
上面那句「仍缺正面验证」到此关闭——而且是在**唯一测这个控件、且构建宿主根本跑不了**的那个测试上。

**先出错的是我的检查，不是被检查的东西。** 烧死路径处那份二进制与宿主构建的 sha 不一致，
看起来像是「这个 PASS 报错了对象」。我第一次去证实时用 `strings` 搜 `^NEWLINE$`——什么也没搜到。
但 Rust 字面量在 `.rodata` 里是**连成一片**的，锚定匹配本来就命不中。
去掉锚点，标记就在那里：`enzhPASTE FAILEDNEWLINESEND TO @`。
被测的确实是带改动的二进制；sha 差异来自宿主后来重建过，不是源码不同。

证据：`~/.local/share/agenterm/evidence/six-cell-*/minicon-cross-tests/`

---

## 6. 已知坑（开工前先读）

1. ~~**`winresource` build-dep 需要 `llvm-rc`**~~ **已证伪（2026-08-25，见 §5.4）**。
   两个 windows 格在 llvm 完全不在 `PATH` 的情况下直接编过，`.exe` / `.com` / `.dll` 齐出。
   **不要为此装 llvm。** 保留这条是因为它示范了本 goal 的纪律：立项预判写进 §6 是为了被实测
   证伪或兑现，不是为了被当成事实执行。
2. **绝对坐标要 tablet 设备**：QEMU 不加 `-device usb-tablet`（或 virtio-tablet）时 VNC 指针是相对移动，`send_mouse(x, y)` 定位会飘。最经典的坑。
   **已实测兑现（§5.13）**：挂了 `usb-tablet` 后五次点击全部命中具名控件
   （`A+` / `Settings` / 主题预设 / `Cancel`）。注意失败形态是**静默的**——
   坐标飘掉时 `send_mouse` 照样返回 Ok，所以必须用「点击后画面是否按预期变化」来验，
   不能用调用是否成功来验。
3. **组合键**：Windows guest 的 Ctrl+Alt+Del 走 QMP `send-key` 才稳，别指望 RFB keysym。
   **已实测（§5.10）**：`send-key` + `qcode` 数组可用，`ctrl+alt+f2` 在 Linux guest 上
   确实切了 VT。注意命令名是 `send-key`（带连字符），不是 `sendkey`。
   **同一通道的 `screendump` 有静默陷阱**：virtio-gpu guest 在没有显示客户端连接时
   截出的是 `Display output is not active.` 占位图，**且照样返回 `{"return": {}}`**。
   要用它就得在截图期间挂着一个客户端。
4. **视觉回归先钉变量**：固定分辨率、关动画、纯色壁纸，截图前必须 `request_full_refresh()` 取整帧而非 delta。
   **容差要算不要拍（§5.12）**：1280×800 上一行终端文字约 600 px = 0.06%，
   钉死变量后的空闲噪声是 0.00%。看似合理的 0.5%（=5120 px）**实测放过了
   一个 16 字符的回归**。默认取 0.02%。
   **实测追加（§5.6）**：等「画面安静」在活桌面上是死等——光标闪烁就足以让帧永不停，
   探针会挂死。任何 settle 逻辑必须带总预算上限。根因不是 bug 而是概念：
   **活屏幕的截图本质是采样，不是稳定态**，所以这条不是优化建议而是前置条件。
   另：探针要敲**可打印字符**，不要按导航键——导航键在某些界面本就无意义，
   而「按键无意义」和「按键根本没到」在像素层面不可区分，失败信息会不可读。
5. **UTM 不对外开 VNC**：其显示走 SPICE，内部用 QMP 与 QEMU 通信但不暴露。要么在 VM 设置的 QEMU Additional Arguments 加 `-vnc 127.0.0.1:1`（可能与 UTM 自身 display 冲突，需实测），要么走 D2 的裸 QEMU 路线。
6. **Eval 90 天**：装完立刻打快照，到期回滚快照或 `slmgr /rearm`。
7. **推产物进跑过 GUI 的 VM 不能直接覆盖**：Linux 对正在执行的可执行文件返回 ETXTBSY，
   `scp` 报 `dest open ...: Failure`。先传 `<name>.new` 再 `mv -f` 落位——`rename(2)` 对
   运行中的 binary 安全。**跑过 GUI smoke 的 VM 必然处于这个状态**，不是偶发（§5.7）。
8. **改 `.rh` 前先读 §5.7 / §5.9 的六条语言实测**：unit 字面量、嵌套调用、
   对 JSON 派生值取 `.len`、函数多出口、helper 参数被推成 `i64`、
   `string.contains(变量)` 全都过不了 AOT codegen。
   定位工具是 `agenterm rh compile <file>`（`rh check` 会过，AOT 才报错），
   **但它不解析 `import`**——对引用模块的脚本会给假失败，判断前先看错误形状：
   实参位里真有嵌套调用才是真问题，零参数的命名空间调用多半只是 import 没解析（§5.18）。
9. **script-* feature 是另一条边界** —— **已验，结论为负（§5.11）**。
   原文预判「zig cc 侧应无碍，xwin 侧走 clang-cl 通常可行」**两半都错**：
   `script-lua` 两侧都挂（linux 缺 `_Unwind_*` 符号；windows 上 `luajit-src` 硬编码找 `cl.exe`），
   `script-sql` 在 windows 挂（`psm` 的 GAS 语法 aarch64 汇编 clang-cl 不认）。
   只有 `script-qjs` 两侧都过。**本机六格能力仅对默认 feature 图成立**，
   带 scripting 引擎的 windows 产物仍需 CI 或上游修复。

---

## 7. 外部来源（D6/D7 依据，2026-08-25 核）

- Evaluation Center：Windows 11 Enterprise 25H2 提供 x64 ISO 与 **Arm64 ISO**，90 天，免产品密钥
- 官方消费者 ARM64 多版本 ISO：`microsoft.com/en-us/software-download/windows11arm64`（约 5 GB，生成链接约 24 小时过期）
- Prism（Win11 24H2+）模拟 x86/x64 **用户态** app；明确**不能**模拟内核驱动——本仓四个 bin 均为用户态，不踩线
- UTM 4.7.5（2026-01-03）；Windows guest 须走 QEMU 后端 + Apple Hypervisor 加速，Apple Virtualization 后端只支持 Linux/macOS guest
