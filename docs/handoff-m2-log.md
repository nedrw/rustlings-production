# M2 构建日志（分段，按段检索）

> 与 [handoff-m1-log.md](handoff-m1-log.md) 同构：每段记「做了什么 / 数字 / 坑与判读」，
> 按段检索。接手时先读 [handoff-m2.md](handoff-m2.md)。

## 索引

- 第一段 · 2026-10 · M2 开工：慢闸门分级、两处框架修正、T3 第一步
- （后续段落按施工顺序追加）

## 第一段 · 2026-10 · M2 开工：慢闸门分级、两处框架修正、T3 第一步

### 做了什么

- **先定慢闸门的 should/must**（写在 [handoff-m2.md](handoff-m2.md) §1 与 plan §6/§10）：原则是
  「`must` 闸门只用这台机器一定有的东西判定」。因此 `mutants`/`bench-budget`/`semver`/`unsafe-budget`/`conformance`
  由 `kiln` 自己实现（文本级变异 + 跑本步规格；计数预算 + 课程基线；编译课程自己的公开面见证文件；
  源码计数 + `SAFETY:` 注释 + `deny(unsafe_code)`；按 `conformance.toml` 逐后端重跑本步规格），
  `coverage`/`deny` 保持 should（缺 `llvm-tools`/`cargo-deny` 时报 `⚠ blocked` + 安装指令，绝不把一步变红）。
  判据全部是**编译器与计数**，所以 CI 可复现；没有一项看墙钟时间。
- **框架修正一：R1 的键改成「槽位」**。台账本来就计划三条 track 在同一 track 内把一个能力从 `aware` 走到 `apply`
  （`jobq/3.1`→`jobq/3.2`、T4 的 WAL 对、T5 的共识对），而旧实现按能力去重 —— 写第二步时直接报 R1 错误。
  规则文档（plan §3 D4 规则 1、curriculum §2.3）写的都是「同深度不得重教」，改的是实现；回归测试
  `r1_keys_on_the_slot_and_not_on_the_capability`。
- **框架修正二：`xtask ci` 逐条走所有已开工的 track**。之前 `cargo xtask ci` 只调不带 `--track` 的
  `kiln selftest`（= 默认 track `mailer`），所以 **T2 的 24 步从没进过 CI**。现在从课程里发现已开工的 track
  （有正文+规格+参考解才算），按路径序逐个跑，未开工的 track 只打印一行。代价要写明：CI 从此要付每条 track 的
  selftest（mailer 579 s、relay 476 s，T3 从现在开始长）。
- **T3 第一步 `jobq/3.1-engine-boundary` 交付**：26 步元数据（台账逐槽对齐）+ 项目骨架（单包 `jobq`：lib = 引擎，
  `jobqd` = 守护进程；`Store` 端口用装箱 future；内存后端；四层配置；进程夹具等日志里的就绪行）+ 规格 6 例 +
  参考解 + 正文。骨架只声明**本步要教的东西**，因为 `no-todo` 是 must 门（骨架里的每个 `todo!()` 都要在同一步的
  参考解里实现）。
- 同时派了三个子 agent：六个慢闸门的 `kiln` 实现、T3 的 3.2–3.4、T3 新槽位的 probe 题库；
  全量 `cargo xtask ci` 在冻结快照上跑（两者报告见后段）。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| `kiln selftest jobq/3.1-engine-boundary --track jobq` | `scaffold ✓ the spec fails as it must (exit code 101: … FAILED)` / `solution ✓ reference patch applies` / `solution ✓ the spec passes` / `gates ✓ 8/8 must green` → `1/1 step(s) hold the contract` |
| `kiln lint-caps` | `97 capabilities · 134 slots · 118 ledger positions · 47 authored steps` → `no issues.`（`skipped: R5 … checked 47 of 72 step bodies, 25 not authored yet`）|
| `cargo test -p kiln --lib caps::` | `27 passed; 0 failed; 181 filtered out` |
| 提交 | `kiln(caps): R1 keys on the slot…` / `docs(m2): the slow gates' grading…` / `course(jobq/3.1): draw the engine boundary` / `course(jobq): the three surface steps need no scaffold` / `docs(m2): the step-author playbook` / `xtask(ci): walk every authored track…` / `docs(m2): curriculum §5/§8 and plan §11…` |

### 坑与判读

- **交付顺序的代价：`no-todo` 是 must 门**，所以「骨架先把整条 track 的模块都摆出来」是错的——
  每一步的骨架只能声明该步参考解能实现完的东西。Postgres 后端、worker 循环、CLI 都随自己那一步新增文件。
- **`kiln spec manifest` 报 “track `jobq` is not authored yet”**：真实原因是我在**用户的主检出**里跑命令
  （那里没有 jobq 目录），不是 `kiln` 的缺陷。教训：终端默认已在 Delta worktree 根，**不要**再 `cd` 到
  `/Users/drmin/RustroverProjects/rustlings-production`。
- **手写的 `.rs` 要 fmt，且夹具文件要被 `mod` 引用到**：`tests/support/service.rs` 在骨架里无人引用，
  `cargo fmt` 看不到它，`fmt` 闸门在把它同步进项目后会红。做法：在带 `mod support;` 的树里 fmt 一次再拷回。

## 第二段 · 2026-10 · 3.2–3.4（公开面/错误/特性装配）与题库扩到 35 道

### 做了什么

- **3.2 公开面与基线**（提交 `course(jobq/3.2)`）：内部模块改 `pub(crate)`、名字在 `jobq::engine` 重导出；
  `JobState`/`Error`/`Job`/`SubmitRequest` 加 `#[non_exhaustive]`；存储端口加**私有密封超 trait**；
  课程侧交出见证文件 `api/3.2-public-api-baseline.rs`（由 `semver@baseline` 当 `tests/semver_baseline.rs` 编译）。
- **3.3 库错误**（`course(jobq/3.3)`）：`Store`/`Io` 带 `#[source]`（不点名驱动）、`is_retryable()`、
  Display 快照、`exit_code()` 不变。
- **3.4 特性装配**（`course(jobq/3.4)`）：一个诚实的 `postgres` feature（默认开）+ 矩阵规格（`--no-default-features`、
  默认、`--all-features`、每个非默认 feature 单位；每个装配跑一个探针 crate）。
- **题库 19 → 35 道**（`course(probes)`）：16 道新题覆盖 T3 新增/加深的槽位，含两道更深的出口能力
  （`db/atomic-claim@design` 的租约/续约/回收/fence、`unsafe/budget@apply`）；没有题的槽位与理由写进 handoff §5.1。
- **两处硬编码计数测试**（`kiln(tests)`）：`cargo test -p kiln` 从 2 红回到 209+15+1 全绿；是 probe 子 agent
  用「把自己 32 个新文件挪出去再跑，同样 2 红」证明的归属。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| `kiln selftest jobq/3.2 jobq/3.3 jobq/3.4 --track jobq` | `3/3 step(s) hold the contract`；每步 `scaffold ✓ the spec fails as it must` / `solution ✓ the spec passes` / `gates ✗ 8/9 must green (semver@baseline is pending)` |
| 见证文件对四棵树手工验证 | 3.1：`E0432 unresolved imports …` + `unreachable pattern`（证明重导出与 `#[non_exhaustive]`）；3.2/3.3/3.4：`test result: ok. 0 passed; 0 failed` |
| 3.4 矩阵额外证据 | 参考解 2 passed；3.3 树 0 passed/2 failed（“declares no `postgres` feature”）；有表但旧拒绝文案 1 passed/1 failed；派生 feature（`metrics = ["postgres"]`）2 passed |
| `kiln probe --check` | `35 exercise(s) · 35 reference(s) passed` |
| `cargo test -p kiln` | `209 passed` + `15 passed` + `1 passed` |
| `lint-caps` | 每步后 `no issues.`；最终 `checked 50 of 72 step bodies` |

### 坑与判读

- **`gates ✗ 8/9 must green (semver@baseline is pending)` 不是假红也不是内容缺陷**：`semver` 在注册表里
  还挂 `Pending("T3")`（实现由另一个子 agent 在做），Pending 不计 breaking，所以步骤仍然 `hold the contract`。
  子 agent 按 §1.3 的契约手工验证了见证文件在四棵树上的行为，闸门落地后需要复跑这三步确认 `9/9`。
- **“见证文件每个受判步骤一份”是歧义，已钉死**：按「建立基线的步骤一份 + 闸门取编号 ≤ 本步的最新一份」执行
  （handoff §3），于是 3.3–3.26 共用 3.2 那份，将来一次故意的主版本变更只需再交一份。
- **隐私与密封无法用编译证明**：课程没有 compile-fail 工具（没有 trybuild），所以 3.2 的规格对这两项用
  **结构断言**并在断言处写明理由；能用编译器证明的两项（公开面还在、`#[non_exhaustive]` 在位）都由见证文件承担。
- **无 scaffold 的接口步会在 scaffold 态编译失败**（3.3 的 `E0559`）：这是 authoring §5.4 的正常形状，
  `selftest` 会加注说明，不应为了让“失败得很好看”而削弱规格。

## 第三段（待写）
