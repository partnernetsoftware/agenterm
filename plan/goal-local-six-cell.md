# goal-local-six-cell

状态：active（2026-08-25 立项，P0 起步）
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

### P2 — VM 落地

- **VM1 Windows 11 ARM64**（QEMU + hvf，Enterprise Eval Arm64）→ win×aarch64 原生 + win×x86_64 经 Prism
- **VM2 Linux arm64**（QEMU + hvf）→ lnx×aarch64 原生 + GUI 交互验证
- **VM3（可选）Linux x86_64**：Apple Virtualization + Rosetta 的无头 CLI smoke；
  需要**真 x86_64 内核**时改用 QEMU TCG 全模拟，只在发布前跑

产物入 VM 走 ssh/scp 或 `utmctl file push` / `exec`（需 guest tools）。
**禁止共享文件夹拖拽**——进不了脚本。

### P3 — 一键化与证据

- 六格构建 + 四格 smoke 收敛为单一入口，产出结构化回执
- 回执落 `~/.local/share/agenterm/evidence/six-cell-<UTC>`，与现有 evidence 惯例一致

---

## 5. 完成判据

| 层 | 判据 |
|----|------|
| 构建 | 一条命令产出六格全部产品 bin，含 SHA-256 清单；任一格失败必须显式红，不得静默跳过 |
| 运行时 · CLI | 四格各自 `agenterm cli --help` 与既有 `cli-smoke` 通过 |
| 运行时 · 交互 | win×aarch64、lnx×aarch64 两格在 VM 内经**带外 VNC** 完成按键→截图闭环 |
| 诚实性 | 每条证据标注 `native` 或 `emulated=<...>`；无标注的模拟结果视为无效证据 |

---

## 6. 已知坑（开工前先读）

1. **`winresource` build-dep**：win 目标要 resource compiler。mac 上需 `brew install llvm` 取 `llvm-rc` 并喂给它，否则 win 两格在 build script 就挂。**P0 最可能的唯一硬阻力。**
2. **绝对坐标要 tablet 设备**：QEMU 不加 `-device usb-tablet`（或 virtio-tablet）时 VNC 指针是相对移动，`send_mouse(x, y)` 定位会飘。最经典的坑。
3. **组合键**：Windows guest 的 Ctrl+Alt+Del 走 QMP `sendkey` 才稳，别指望 RFB keysym。
4. **视觉回归先钉变量**：固定分辨率、关动画、纯色壁纸，截图前必须 `request_full_refresh()` 取整帧而非 delta。
5. **UTM 不对外开 VNC**：其显示走 SPICE，内部用 QMP 与 QEMU 通信但不暴露。要么在 VM 设置的 QEMU Additional Arguments 加 `-vnc 127.0.0.1:1`（可能与 UTM 自身 display 冲突，需实测），要么走 D2 的裸 QEMU 路线。
6. **Eval 90 天**：装完立刻打快照，到期回滚快照或 `slmgr /rearm`。
7. **script-* feature 是另一条边界**：开 `script-lua` / `script-qjs` / `script-sql` / `script-wasmcore` 会真编 C（lua-src、libsqlite3-sys、rquickjs-sys、wasmtime）。zig cc 侧应无碍，xwin 侧走 clang-cl 通常可行，但**必须单独验一次**，不得默认它通。

---

## 7. 外部来源（D6/D7 依据，2026-08-25 核）

- Evaluation Center：Windows 11 Enterprise 25H2 提供 x64 ISO 与 **Arm64 ISO**，90 天，免产品密钥
- 官方消费者 ARM64 多版本 ISO：`microsoft.com/en-us/software-download/windows11arm64`（约 5 GB，生成链接约 24 小时过期）
- Prism（Win11 24H2+）模拟 x86/x64 **用户态** app；明确**不能**模拟内核驱动——本仓四个 bin 均为用户态，不踩线
- UTM 4.7.5（2026-01-03）；Windows guest 须走 QEMU 后端 + Apple Hypervisor 加速，Apple Virtualization 后端只支持 Linux/macOS guest
