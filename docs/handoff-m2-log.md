# M2 构建日志（分段，按段检索）

> 与 [handoff-m1-log.md](handoff-m1-log.md) 同构：每段记「做了什么 / 数字 / 坑与判读」，
> 按段检索。接手时先读 [handoff-m2.md](handoff-m2.md)。

## 索引

- 第一段 · 2026-10 · M2 开工：慢闸门分级、两处框架修正、T3 第一步
- 第二段 · 2026-10 · 3.2–3.4（公开面/错误/特性装配）与题库扩到 35 道
- 第三段 · 2026-10 · 六个慢闸门落地，3.2–3.4 的 `semver` 由 pending 转真
- 第四段 · 2026-10 · 全量 `cargo xtask ci` 第一次跑通（含逐 track selftest）
- 第五段 · 2026-10 · 3.5（存储端口与一致性套件）：一次假红、两处我自己的缺陷、一个夹具加固
- 第六段 · 2026-10 · 3.6–3.7（事务隔离、租约与恢复）
- 第七段 · 2026-10 · 3.8–3.11（时钟注入、公平策略、cron、延迟定时器）
- 第八段 · 2026-10 · 3.12–3.15，以及一个装不进“无特性”装配的夹具
- 第九段 · 2026-10 · 3.16 交付、`selftest` 变慢的根因、以及 3.15 的一个真缺陷
- 第十段 · 2026-10 · 第二十七步：DLQ 的命令行那一半单独成步
- 第十一段 · 2026-10 · 3.17–3.18，以及把两个“以后再说”的指针收口
- 第十二段 · 2026-10 · 3.19–3.20，以及验收标准里 `mutants` 那一腿的原始证据
- 第十三段 · 2026-10 · 3.21–3.22，`bench-budget` 那一腿，以及一次“幽灵失败”的追根
- 第十四段 · 2026-10 · 3.23 由我接手完成，以及一个 `selftest`/`verify` 的不一致
- 第十五段 · 2026-10 · 3.25，以及 `selftest`/`verify` 不一致的根因（由 3.25 的作者找到）
- 第十六段 · 2026-10 · 3.27，cap 真缺陷的修复，以及 M2 验收（CI 全绿）
- 第十七段 · 2026-10 · `coverage` 搬进项目副本：3.22 从 `blocked` 变数字，两个真缺陷，一个待定项
- 第十八段 · 2026-10 · “coverage 应该如何提高”：先量，然后发现门丢掉了子进程的 profile
- 第十九段 · 2026-10 · 用户定下 coverage 的模块规则（方案 ③），以及一次被中断的 CI
- 第二十段 · 2026-10 · “覆盖率为什么不前置”：解释清楚了，不前置
- 第二十一段 · 2026-10 · 一份独立 review 的十项发现：A/B/C/E 落地，D 待定

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

## 第三段 · 2026-10 · 六个慢闸门落地，3.2–3.4 的 `semver` 由 pending 转真

### 做了什么

- **六个质量闸门实现完**（提交 `kiln(gates): the six quality gates…`）：`mutants@N`（变异本步参考解碰过的
  `src/**.rs`，复用同一份项目副本、最多 6 个等距变异体、杀/存活计分、编译失败与时延不计分、
  失败点名 `file:line` + 原文 + 改写）、`bench-budget@P`（跑课程基线文件里的 `command`，解析
  `jobq-budget k=v`，逐指标 ±P%，`_max` 后缀判上界）、`semver@baseline`（取 ≤ 本步的最新见证文件，
  当 `tests/semver_baseline.rs` 编译，报第一段编译错误）、`unsafe-budget@N`（源码里 `unsafe` 计数 +
  每处 `SAFETY:` 注释 + N=0 时要求 `unsafe_code` deny）、`conformance@N`（按 `conformance.toml`
  逐后端用 `KILN_BACKEND` 重跑本步 `run.cmd`）、`coverage@N`（找工具链里的 `llvm-cov`/`llvm-profdata`，
  缺就 `blocked` + 安装行）。
- 两个共享支撑模块：`gates/lexer.rs`（字符串/字符/注释的字面级分类，保证闸门不会改写或统计字面量里的文本）、
  `gates/sources.rs`（「本步模块」= 参考解补丁碰过的文件；「≤ 本步的最新见证」规则）。
- `deny` 仍为 `Pending`，理由改写成「它是 should 门（`cargo-deny` 不在工具链基线里）」；`tool.rs` 不再声称那些工具在判门。
- **集成验证**：`kiln selftest jobq/3.2 jobq/3.3 jobq/3.4 --track jobq` 从 `8/9 (semver pending)` 变成
  **每步 `gates ✓ 9/9 must green`**——闸门实现与课程见证文件第一次在真步骤上接上。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| `cargo test -p kiln` | `264 passed` + `21 passed (tests/gates.rs)` + `1 passed (ledger)` |
| 六个新集成测试 | 每个都证明「劣化 → 红 / 好树 → 绿」，且各自点名了劣化的位置（例如 `src/lib.rs:3` 的存活变异、`scan_cells is 1024 … baseline 10`、`memory ✓, postgres ✗`）|
| `kiln selftest jobq/3.2 jobq/3.3 jobq/3.4 --track jobq` | `3/3 step(s) hold the contract`；每步 `gates ✓ 9/9 must green` |
| `kiln lint-caps` | `no issues.`（`checked 50 of 72 step bodies, 22 not authored yet`）|
| `cargo fmt --all --check` / `clippy -D warnings` | 都干净 |

### 坑与判读

- **变异门必须先在未变异的副本上跑一次**：否则项目自己的规格本来就红时，每个变异体都会被当成“杀死”，
  这是这个门最不能给出的错答案；共享 `CARGO_TARGET_DIR` 时还要在最后再跑一次好树，否则 cargo 按包名+版本
  选的构建槽会把最后一个变异体二进制给学员自己的下一次 `cargo test`（两条都由集成测试证明）。
- **`fs::copy` 会带着原时间戳**：沙箱副本看起来比已有产物旧，cargo 判新鲜度后默默用了陈旧二进制；
  解法是把副本文件的时间戳往前推一秒（不是 `cargo clean -p`——共享 target 下它会删掉别的包的测试二进制，已验证）。
- **`coverage` 的工具存在路径在本机没有验证过**（`llvm-tools-preview` 未装）：集成测试断言的是诚实的
  `blocked` + 安装行，测量逻辑（导出 JSON → 行覆盖比例 → 阈值）用合成文档单位测试。这条写在这里，
  不假装它被端到端验证过。

## 第四段 · 2026-10 · 全量 `cargo xtask ci` 第一次跑通（含逐 track selftest）

### 做了什么

- 在**冻结快照**上跑端到端 CI（`git archive <commit> | tar -x` 到 `/private/tmp/ci-snapshot`，
  私有 `CARGO_TARGET_DIR`/`XDG_CACHE_HOME`）。三次冻结：

| 次 | 冻结 commit | 结果 |
| --- | --- | --- |
| A | `ec3b154` | **exit 1**，12.58 s，红在 `cargo fmt --all --check`（我 R1 修复里那行 102 字符）|
| B | `0aad030` | **exit 1**，3.52 s，仍红在 `fmt`（第二处：`assert_eq!` 一行 82 字符，撞 `fn_call_width = 60`）|
| C | `0927803` | **exit 0**，**1634.07 s（～27 min）**，`xtask ci: all checks passed` |

### 数字（run C 原样）

```
xtask ci: cargo fmt --all --check                                  （静默）
xtask ci: rustfmt --check on 149 course file(s) under course/     （静默）
  50 spec file(s) pinned, 50 step(s) declare their own
  97 capabilities · 134 slots · 118 ledger positions · 72 authored steps
  63 page(s) · 392 link(s) · 238 external target(s) (not fetched)
probe --check · 35 exercise(s) · 35 reference(s) passed
xtask ci: kiln selftest --track mailer
xtask ci: kiln selftest --track relay
xtask ci: kiln selftest --track jobq
xtask ci: all checks passed
```

三条 track 的结论行：**mailer `22/22`、relay `24/24`、jobq `4/4`**（jobq 当时只有 4 步已写；
其中 3.2–3.4 的 `gates ✗ 8/9 (semver@baseline is pending)` 仍算 hold the contract——Pending 不计 breaking）。
`cargo test --workspace`：`209 + 15 + 1` 全绿。**这是 `xtask ci` 的逐 track selftest 循环第一次真实执行**
（之前只跑默认 track）。
run C 不覆盖最新 HEAD（它之后又落了 `kiln(gates)…` 一次提交）；M2 收尾时需再跑一次。

### 坑与判读

- **fail-fast 的代价**：`xtask ci` 第一阶段就红，后面 clippy/tests/lint-caps/links/probe/selftest 一次都没跑；
  所以“CI 红了”不代表“只有一处坏”。这次的归因靠的是：把剩下的阶段**按同序手跑一遍**（在同一个冻结快照上），
  发现除 fmt 外还有一处真红（两处硬编码计数测试），而非猜测。
- **冻结快照 + 共享 target 会“假装绿”（新环境事实）**：`git archive` 会带着提交时的 mtime，
  共享 `CARGO_TARGET_DIR` 里的旧产物看起来比刚解包的源码新，cargo 不重编——于是旧提交跑出了**新提交才会有的绿**
  （实测：同一次重跑第一次“通过”且二进制里是新断言文案，把源码 mtime 向前推一秒后同一条命令变成预期的红，
  两次结论一致）。
  **规则**：冻结不同 commit 跑 CI 时，要么每次用**独立的 target dir**，要么解包后把源码 mtime 前移一秒。
  这与闸门实现里 `fs::copy` 要 `make_fresh` 是同一类问题（同一周踩到两次）。

## 第五段 · 2026-10 · 3.5（存储端口与一致性套件）：一次假红、两处我自己的缺陷、一个夹具加固

### 做了什么

- **3.5 交付**（提交 `course(jobq/3.5)`）：加性 scaffold（`sqlx` + `dep:sqlx` 插进 3.4 的多行 feature 列表、
  `src/engine/store/postgres.rs` 五个 `todo!()`、`migrations/0001_jobs.sql` 空壳、容器夹具、
  `tests/support/store.rs` 按 `KILN_BACKEND` 造后端、`conformance.toml`）+ 8 例规格（一次文件、逐后端各跑一次）+
  参考解（DDL、单语句 claim、`pub(crate) mod postgres;` + 重导出、把 claim 顺序写进端口文档、
  把内存 store 的 claim 从“读-然后-写”改成同一临界区）+ teach 正文。
- **`slow:conformance@2` 第一次在真步骤上跑了两次后端**（内存 + 真 Postgres 容器），
  `gates ✓ 11/11 must green`。回归：容器夹具按机器规则把**并发启动限到 3**（八个用例同时 `docker run`
  会让虚机资源打架，红色来自 `docker run` 失败而不是断言），并在启动失败时按名字清掉容器
  （否则那个值的 `Drop` 永远不会跑，就是泄漏的来源）。
- **修了三处我自己写错的步骤 id**（提交 `course(jobq): T3 cited three step ids…`）：
  `mailer/1.4-layered-config`/`1.6-structured-logs`/`1.9-migrations` → 真实 id `1.4-config`/`1.6-logging`/`1.9-postgres`；
  做法是全树字符串替换（含 3.1 参考解补丁里的新增行，同行替换不动任何 hunk 头），重钉 3.1 规格哈希。
  发现方式是写了一个小扫描：把 jobq 树里每个 `track/N.M-slug` 引用拿去和三份 track.toml 对（允许裸编号、不允许错 slug）。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| `kiln selftest jobq/3.5 --track jobq` | `scaffold ✓ the spec fails as it must (E0432: unresolved import support::store)` / `solution ✓ the spec passes` / `gates ✓ 11/11 must green`；连跑三次（含全新私有缓存）一致 |
| `KILN_BACKEND=postgres cargo test --test spec_port_conformance` | `8 passed`，五个容器用例秒级到十秒级，跑完 `podman ps -a` 无残留 |
| 并发用例的牙 | 把内存 store 的一锁修复回退后，10 次里 6 次报“two workers came back with the same job” |

### 坑与判读

- **一次假红**：`slow:conformance@2` 报 `postgres ✗ (… tests/support/postgres.rs:84 …)`——那是“容器起不来”的 panic，
  不是断言。按 M1 协议单跑那一步（全新私有缓存）→ 绿；再连跑两次 → 绿。归因为环境（八个容器同时启动），
  但 `must` 门会 flake 本身就是缺陷，所以做了上述夹具加固。
- **启动失败会泄漏容器**：`docker run` 被 deadline 杀死时容器已 `Created` 但没起，而持有 `Drop` 的那个值
  还没构造——失败路径现在按名字 `rm -f -v`。（机器上另有两个更早的残留 `kiln-pg-37015-3`、
  `kiln-relay-pg-6462-4`，不是这次产生的，没去动它们。）
- **`KILN_BACKEND` 与 `JOBQ_BACKEND`**：§1.3 的闸门契约与作业书写的是 `KILN_BACKEND`，§2.3 里的 `JOBQ_BACKEND`
  是早期笔误，已改正。
- **探测与规格的分歧被记下来**：`course/probes/db-port-conformance.apply.rs` 的正文写着“回来的是**集合**，顺序是后端自己的事”，
  而本步把 claim 顺序写成了端口的承诺并断言它——两句话可以共存（没有承诺顺序的端口不该被断言顺序；
  这个端口把顺序承诺写进了文档），已写进规格头与正文的说明里。

## 第六段 · 2026-10 · 3.6–3.7（审计轨迹/约束仲裁/有界重试 + 租约/续约/回收/worker 循环）

### 做了什么

- **3.6 交付**（提交 `course(jobq/3.6)`）：状态变更与它的审计行**同一事务**（`job_events`：`completed`/
  `attempt_failed`/`reclaimed`；claim 不是 settle，理由写在 `events.rs`）；**唯一索引作仲裁**——
  `(job_id, attempt, kind)` 上的唯一索引 + `insert … on conflict do nothing`，重复追加是 no-op；
  **有界重试**——`retry(DEFAULT_ATTEMPTS, closure)` 认 SQLSTATE `40001`/`40P01`（经 3.3 的 `is_retryable`），
  规格用两条连接、`repeatable read`、相反顺序触同一批行**真注入**冲突。新增 `Error::Conflict { source }`（新变体，
  不动旧变体字段）。
- **3.7 交付**（提交 `course(jobq/3.7)`）：租约（`lease_owner`/`lease_expires_at`、`renew`、`release`、
  `reclaim_expired`）在**两个后端**上语义一致；`src/service/worker.rs` 的 worker 循环（`Handler` 端口、
  claim → handle → settle、处理期间按 `lease/2` 续约、`SIGTERM` 后排空并退 0），`serve` 真的开始 drain；
  迟到 worker 的写入被 `lease_owner = $worker` 的守卫拦住。
- **“计数”的含义钉死**：尝试在**领取**时计数（3.5 的规则），所以 `reclaim_expired` 不重复加计数，
  而是在预算耗尽时把作业终结——这正是让毒丸payload 会死掉的原因（规格里有一条“没人失败它也会死”的用例）。
- **`Error::` 与端口**：`migrate` 移到端口上（内存答 `Ok(())`），因此 3.5 的夹具只调一次；守护进程的 handler 是
  **一个程序**（payload 走 stdin，退出码就是判决），否则“kill -9 在途作业”“SIGTERM 时还有在途作业”
  这两件事在测试里根本看不见。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| `kiln selftest jobq/3.6 jobq/3.7 --track jobq` | `2/2 step(s) hold the contract`；每步 `gates ✓ 11/11 must green`（含 `images` 与 `slow:conformance@2`）；两位作业者各自跑了两三次、含全新私有缓存，一致 |
| 最终树上的回归 | 3.1 `6` / 3.2 `4` / 3.3 `4` / 3.4 `2` / 3.5 `8`(memory)+`8`(postgres) / 3.6 `7`+`7` / 3.7 `11`+`11` |
| 有牙（故意破坏 → 点名用例红 → 回滚绿） | 3.6 把 settle 降到 `read committed` → “the loser of the race is refused rather than quietly re-read”；唯一索引降成普通索引 → “no unique or exclusion constraint matching the ON CONFLICT specification”；3.7 去掉 `lease_owner` 守卫 → “expected Error::Invalid, got Ok(())”；续约换成 `pending()` → “the row went back to Queued while its worker was still handling it” |
| `lint-caps` | `no issues.`（`checked 53 of 72 step bodies`）|
| 容器 | 跑完无新增残留（机器上仍有两个更早的 `kiln-pg-*`/`kiln-relay-pg-*`，不是本批产生）|

### 坑与判读

- **作业者修了 3.2 规格里的一个潜在匹配 bug**：`file_declaring` 用 `text.contains("pub struct Job")`，
  于是新的 `pub struct JobEvent` 会替 `Job` 作答；改成整名匹配 `declares` 并重钉 3.2 的哈希（3.2 复跑 9/9 绿）。
  这是「早期规格被后来的名字污染」的实例，值得记住。
- **历史里留了 staging 提交**：本轮作业者用 `wip(...)` 提交把工作副本当检查点（因为 harness 两次丢掉未跟踪文件），
  最后用 `chore(jobq): drop the staging tree` 删掉整棵树。**最终树干净**（`wip/` 不在 tip 里，两个补丁重现交付项目
  已用 `diff -r` 验证），但历史里有 5 个 `wip(...)` 提交与它们携带的整棵副本。没有重写历史（`main` 会同步到用户主检出，
  未经确认不动共享历史）；**若要发布，收尾时可把 9d9cceb..HEAD 压成两个步骤提交**。
- **`jobq/3.6-isolation` 不是步骤 id**（真实 id 是 `3.6-transaction-isolation`），作业书里我写错了引用；
  作业者按原样跑了一遍并报了 `unknown step`，再用真 id 继续。教训：作业书里的步骤 id 也要过一遍扫描。

## 第七段 · 2026-10 · 3.8–3.11（时钟注入、公平策略、cron、延迟定时器）

### 做了什么

- **3.8 时钟注入**（提交 `course(jobq/3.8)`）：`Clock` 端口（**故意不密封**，调用方必须能实现它）+ `SystemClock`；
  两个后端用 `with_clock` 构造注入，十三个直接读时钟的点位全部改走端口；项目 `clippy.toml` 禁 `Utc::now`/`Local::now`
  /`SystemTime::now`，`SystemClock` 是唯一带 `#[allow]` 的地方。规格的假时钟停在 **2030 年**：读系统时钟的实现会在
  “还没到期”处答“可领取”，用例因此有牙。
- **3.9 优先级与公平**（提交 `course(jobq/3.9)`）：`Policy`（aging + 每队列在途上限）构造注入，**默认等于这一步之前的行为**；
  ranking 放进**领取语句自身的排序**（`for update skip locked` 下事后排序只会改变报告）；守护进程接线两个旋钮并写进
  `--check-config`（默认 aging 300s、上限 0）。
- **3.10 cron**（提交 `course(jobq/3.10)`）：`chrono-tz` + `Schedule::next_after()` 一个问题；DST 语义与已交付的探针
  `sched-cron.apply` 对齐（时钟跳过的那次当天不发生、重复的那次发生两次）；错过执行的政策留给调用方，不提供
  catch-up 助手。
- **3.11 延迟定时器**（提交 `course(jobq/3.11)`）：`ReadySet`（“等待的代价是队列数而不是作业数”）+ 端口新增
  `Store::next_due(queue)`（两个后端都实现）+ worker 的等待改为“到下一个到期时刻，`poll_interval` 是下限”。
- 两处**跨步骤真缺陷**（都是“在最终树上重跑早期规格”抓到的，不是猜的）：
  ① 作业者发现 **3.9 的参考解** 给 `open_durable` 加了第三个参数（policy），却漏了 `#[cfg(not(feature = "postgres"))]`
  的孪生函数，于是 3.9 一应用，`cargo check --no-default-features` 就是 `E0061`——3.4 的 feature 矩阵规格在最终树上
  弄红才暴露（提交 `jobq(3.9): the assembly without postgres takes the policy…`）；
  ② 同一轮里我自己的 3.8 把 `clippy.toml` 一列出来，`disallowed_methods` 对**所有 target** 生效，于是 3.5/3.6/3.7
  三份需要真实时刻的规格加了 `#![allow]` 并写明理由（authoring §7.4）。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 逐步 `selftest` | 3.8 `gates ✓ 9/9`；3.9 `gates ✓ 11/11`（含两后端的 `slow:conformance@2`）；3.10 `9/9`；3.11 `11/11`；两两各跑两遍（含全新缓存）一致 |
| 最终树上的早期规格 | `spec_clock 5 · spec_cron 12 · spec_engine_boundary 6 · spec_fairness 4 · spec_feature_assembly 2 · spec_isolation 7 · spec_leases 11 · spec_library_errors 4 · spec_port_conformance 8 · spec_public_api 4 · spec_timers 6` → 两后端各 `69 passed · 0 failed` |
| 课程自己的工具（`kiln verify jobq --slow`） | `11/11 judged step(s) pass · 0 failed · 0 not prepared · 15 without content`；逐步 3.1 8/8、3.2 9/9、3.3 9/9、3.4 9/9、3.5 11/11、3.6 11/11、3.7 11/11、3.8 9/9、3.9 11/11、3.10 9/9、3.11 11/11 |
| 有牙（故意回退） | 3.11：忽略 ready set → 只有“比下一次 poll 更早到期”那例红；`wait` 去掉下限 → 表格与“远处到期不变成长时间睡眠”两例红；已到期的 deadline 等零 → 表格红。两个端到端用例连跑 30 次（内存）/6 次（postgres）无失败 |
| `lint-caps` | `no issues.`（`checked 57 of 72 step bodies`）|

### 坑与判读

- **cargo 会复用同名同版本的测试二进制**（`CARGO_TARGET_DIR` 共享时）：作业者中途见过一次“20/20 失败”，实际是回归探针留下的
  陈旧二进制；所有数字都用**每棵树私有 target dir** 重取过一遍。这正是 `mutants` 门要 `make_fresh` 的同一个坑。
- **scaffold 不能“接线 worker”**：scaffold 只能新增文件或往标记块插行，而 `src/service/worker.rs` 没有标记块——所以 3.11 的
  scaffold 只给 `src/engine/timers.rs`，模块声明与端口方法由参考解做（规格在 scaffold 态因此是 `E0432`，与 3.9 同形）。
- **`chrono-tz = "0.10.4"`** 沿用文件自己的惯用法（精确版本在 `Cargo.lock` 里），不写成 `= 0.10.4`。

## 第八段 · 2026-10 · 3.12–3.15，以及一个装不进“无特性”装配的夹具

### 做了什么

- **用户决定**：`3.15` 的 FURTHER 写了“3.16 是让这个语法长大的地方（两个对死信下手的命令）”，而 3.16 按任务书只交了
  端口那一半（视图 + `Store::replay`）。选择是**单独开一步**，而不是把那句话删掉：命令行的门是操作员真正走的地方，
  它有自己的判断（哪些子命令存在、破坏性命令动手前要什么、事后审计查得到什么）。
- **位置：插在 runbook 演练之前，编号 `3.26-dlq-cli`，演练移到 `3.27`**。理由：演练阶段的页面会用到这些命令（工具先到、
  演练后到）；而且只挪一步的代价最小——8 份已交付文件里引用了 `3.17/3.19/3.23/3.24/3.25` 的 id，把 3.17 往后挤要重验两个
  参考补丁并重写正文，只为了把新步放在 3.16 旁边，不值。
- **槽位**：`ops/dlq-tooling.operate = jobq/3.26`、`cli/contract.design = jobq/3.26`（都是新槽位，不是重教别人的）；
  `ops/runbook-drill.design` 改指 `jobq/3.27`；演练的 `requires` 从 `ops/dlq-tooling@apply` 改成 `@operate`（它演练的就是这个门）。
- **顺手补齐了门阶梯**：handoff §2.2 一直说 3.20 起是累积的，但 `track.toml` 里 3.21–3.27 各自只写了自己新增的那一行——
  于是 3.21/3.22/3.23 少了 `slow:mutants@80`，3.23–3.26 少了 `slow:bench-budget@10`，3.26/3.27 少了 `unsafe-budget@0`。
  现在元数据与表一致（3.23 的 `images` 保留，新步也带上——它的规格真的起容器）。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 元数据 | `kiln list jobq` → `0/27 steps done`，`3.26-dlq-cli` 贴在 3.25 与 3.27-runbook-drill 之间 |
| `lint-caps` | `no issues.`（`checked 62 of 73 step bodies, 11 not authored yet`）|
| 受影响的交付件 | 只改了 `3.16` 正文里的一处引用（`3.26-runbook-drill` → `3.27-runbook-drill`）；8 处其他 later-id 引用未动 |

### 坑与判读

- **下一批（3.17/3.18）已派**（子 agent `a1841fabda3f4213`）；它们的 id 和门清单都没变（新步在 3.26）。
- **不要因为元数据改动就去重验已交付步骤**：这次只动了 `track.toml`/`capabilities.toml`/文档与一处正文引用，
  没有任何 scaffold/解补丁/规格被碰，所以 3.1–3.16 的 `selftest` 结论仍然成立（`lint-caps` 已复验）。

### 做了什么

- **3.12 去重窗口**（提交 `course(jobq/3.12)`）：`Dedup`（键 + 窗口）构造注入，键在同一队列同一窗口内只答一个
  `JobId`；迁移 `0004` 建唯一索引，端口新增按键查回的口。
- **3.13 队列鉴权与归属**（提交 `course(jobq/3.13)`）：`Queue` 的声明、`Scope`/`QueueAction` 与拒绝入表；迁移 `0005`；
  配额与越权的作业“看不见”而不只是“拒绝”。
- **3.14 作业状态机**（提交 `course(jobq/3.14)`，作业者 `1a6bf93e5b444153`）：`transition(state, event)` 成为唯一的状态规则
  （两个后端的 settle 语句改成“问模型要目的地”，SQL 里只剩谓词），非法迁移有具名拒绝值 `Illegal`
  （`Reason::{Terminal, NotRunning, NotQueued}`，`From<Illegal> for Error` 映射到 3.7 钉的 `Error::Invalid`/退出码 2）；
  `JobState::parse` 是列的读回口，`migrations/0006_job_states.sql` 用具名 `check` 关住那一列——“手写进 psql 的非法状态”
  是内存后端答不了的问题，所以这一步自己起容器（为此补了 `images` 声明）。scaffold 只给新文件 `src/engine/state.rs` + 迁移
  stub（scaffold 态因此是 `E0432`）。
- **3.15 CLI 契约**（提交 `course(jobq/3.15)`，同一作业者）：`src/cli/mod.rs`（语法 + `Outcome`：退出码与两个流是一个值）
  + `src/bin/jobqc.rs`（11 行），`--json` 不改变退出码，stdout 是答案、stderr 是诊断；端口新增 `Store::list` + `JobFilter`
  （按 `claim` 文档的顺序、有界页、越权的作业直接看不见），`impl Store for Arc<T>` 让两个二进制在运行时挑后端；
  `service::open_store` 转公开，`jobqc` 与 `jobqd` 从同一处、同一份设置建 store（且 `jobqc` 不迁移）。
- **一个 pre-existing 缺陷：一致性夹具装不进“无 postgres”的装配**（提交 `course(jobq/3.5): the conformance fixture…`）。
  3.5 的 scaffold 建的 `tests/support/store.rs` 无条件 `use jobq::engine::PostgresStore`，于是
  `cargo test -p jobq --no-default-features` 在**课程自己发的文件**里报 `E0432`——而这个夹具的默认后端就是 `memory`，
  且“无特性也要能装配”正是它前一步（3.4）的判据。作业者报了它但按规矩没动别人的产物；我在主树上修：只加
  `#[cfg(feature = "postgres")]`（导入、`Backend::Postgres` 变体与它的分支），没有特性又点名 `postgres` 时给一句指明装配的话；
  3.7 打在同一文件上的 hunk 按新内容重新生成（`@@ -82,10 +82,9 @@` → `@@ -98,11 +98,9 @@`）。两个拥有该文件的步骤都重跑了
  `selftest`（`11/11` 与 `11/11`）。
- **边界记在 handoff §5.2**：需要特性的不是夹具而是**规格**——15 个 spec target 里有 9 个按设计点名 `PostgresStore`，
  所以“durable 那一半套件要 `--features postgres`”是构造性的，不是夹具的毛病。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 逐步 `selftest`（两遍，第二遍全新缓存） | 3.12 `11/11`、3.13 `10/10`、3.14 `10/10`、3.15 `10/10` must gates；3.14/3.15 都是 `1/1 step(s) hold the contract` |
| 全步判定（在 `skip` 出的尾巴树上） | `kiln verify jobq --slow` → `15/15 judged step(s) pass · 0 failed · 0 not prepared · 11 without content`；逐步 `8/9/9/9/11/11/11/9/11/9/11/11/10/10/10` |
| 默认特性全量套件 | `cargo test -p jobq` → **92 passed · 0 failed**（3.13 基线 81，两步加了 11 例；夹具修完再取一次仍是 92） |
| postgres 后端子集 | `authz 6 · dedup 6 · fairness 4 · isolation 7 · leases 11 · port_conformance 8 · state_machine 5` → `47 passed · 0 failed` |
| 夹具修复的两条证据 | 无特性：`cargo test -p jobq --no-default-features --test spec_port_conformance` → **8 passed**（此前 `E0432`）；同一构建点名 `KILN_BACKEND=postgres` → `panicked at tests/support/store.rs:131` “needs the `postgres` feature this build was made without…”（是句子，不是编译错误）；有特性 + `KILN_BACKEND=postgres` → `8 passed`（6.28 s，真起容器） |
| 课程自己的检查 | `lint-caps` `no issues.`（`checked 61 of 72 step bodies`）；`probe --check` `40/40`；`links` `74 page(s) · 414 link(s)` `✓ every target resolves`；`i18n status` `structure: 0 error(s)` |
| 验收演练 a 腿（`semver`） | 删 `impl Default for JobId` → `✗ semver@baseline … error[E0277] … tests/semver_baseline.rs:64`（另有 `clippy@2` 附带红）；回滚后 `9/9 must gates passed` |
| 验收演练 d 腿（`conformance`） | durable `claim` 外层排序 `run_at asc`→`desc`：memory `8 passed` / postgres `7 passed · 1 failed`；`kiln verify jobq --slow` → `14/15 … 1 failed`，红在 `slow:conformance@2 … memory ✓, postgres ✗`；回滚后 `15/15` |

### 坑与判读

- **演练先否掉了我自己的第一个劣化方案**：把 `Engine::submit` 的参数放宽成 `impl Into<SubmitRequest>`，`semver` 门不红——
  查过见证文件后确认这是对的（`Engine` 的 `async fn` 只能用闭包钉，对任何存在的调用方该改法都是源码兼容的；
  `Store` 的五个方法才是真函数指针强转）。换“删一个被钉住的公开 `impl`”才算真破坏性改动，§4.3 里有这条边界。
- **`app/` 在 `.gitignore` 里**：在铺出来的树上做演练时 `git add -A` 加不进 `app/jobq`，所以“改一处 → `git checkout -- .`
  回滚”在那种临时树里不成立；要么 `git add -f app`，要么把劣化写成可逆的脚本替换（这次是后者）。
- **“durable 规格名类型”不是缺陷，是设计**：夹具可以做到“任何装配都能编译”，规格做不到——它们是钉死的文本，
  要判“两个后端对同一问题给同一个答案”就必须点名背单词。生产项目里表达这条边界的机制是 Cargo 的
  `[[test]] required-features`；这里没做，因为那要一份随每一步成长的 target 清单，是课程设计决定（handoff §5.2）。
- **`kiln skip --all` 走的是默认 track（mailer），不是 `--track`**：这个版本的 `skip` 没有 `--track`（`selftest`/`verify` 有），
  要铺 jobq 的树得按路径序逐个 `kiln skip jobq/<step> --force`——交接文档里那条命令写错了，已在 §4.1 改正并换成循环。
- **`kiln verify <track>` 判的是 `app/<track>`**：worktree 里没有 `app/` 时逐个报 `app/jobq does not exist yet`，
  `0/0 judged`（这不是“全绿”）。取数字要先铺树。
- **3.15 的退出码 4 用例用“这个构建打不开的连接串”而不是“不可达的主机”**：`PgPoolOptions` 对后者会等自己的 30 s 超时，
  会把该步的快速路径拖到 ~37 s；类别相同（`Error::Store`/退出 4），用例 15 ms 拿到结论。
- **打包旧 target dir 的诱惑**：修复后用共享 target dir 跑过一轮，正是交接文档警告的“同名同版本测试二进制被复用”场景；
  修复的验证全部改用 `/private/tmp/fixcheck2-target`（全新）重取。
- **3.16（DLQ 工具）与 3.17（预算告警）的作业者已经在跑**（子 agent `475cb3f144864910`），任务书里写明了：不要改
  `tests/support/store.rs`（我在主树上动），需要的夹具辅助放新文件。

## 第九段 · 2026-10 · 3.16 交付、`selftest` 变慢的根因、以及 3.15 的一个真缺陷

### 做了什么

- **3.16 作业者死了一次，换成单步重派后交付**（提交 `course(jobq/3.16-dlq-tooling)`，作业者 `2f3f05ae7dcb4410`）：
  死掉的那个作业者（`475cb3f144864910`）在 rustfmt 附近停下，工作树里什么都没留下，但它的草稿
  `src/engine/dlq.rs`（`revived(job, now)`：只有死信有第二次生命、计数保留而上限抬高、上次死因保留）在
  `/private/tmp/jobq316-sol` 里，我把它收成 `/private/tmp/jobq-316-dlq-draft.rs` 交给新作业者。新作业者按草稿的脊梁交付：
  `dead_letters()` 是一个 `JobFilter` 视图、`Store::replay` 是唯一回程（读锁下决定、行与迹同事务）、
  `EventKind::Replayed`；步骤标题/`concepts`/`hints` 按“端口而不是 CLI”的实际范围改过。
- **`selftest` 越来越慢的真因**（提交 `kiln(process)`、`kiln(mutants)`、`docs(m2)`，详见 handoff §4.4）：
  超时只杀了直接子进程，`cargo test` 的**孙进程（测试二进制）变成孤儿**，卡死的那个活了 40 分钟，占一个核并握着共享
  构建目录的锁，于是后面每一次命令、每个 worker 都变慢——每卡一次再漏一个，这才是“越来越慢”的真身；
  修法是先收后代再一起杀（`ps` + `kill`，都是 POSIX）。第二个真因：一个变异体能让规格永不停（
  `MemoryStore::claim` 的 `&&`→`||` 让过滤对所有行成立，规格的 `loop{claim;if empty break}` 永不空），
  当时唯一的防线是全局 `--timeout` 300s × 最多 12 次；现在每个变异体限 `4×基线+15s`、整轮限 300s，报告里写
  `did not finish within 25s` / `the walk took 60s`。第三个：取样里大多位置根本编不过（类型里的尖括号、`format!` 的 `!`、
  闭包的 `||`、`==` 臂、构造子改写），修了表并把报告从 `文件:行` 改成 `文件:行:列`。
- **3.15 的一个真缺陷**（提交 `course(jobq/3.15): a listing answers what this handle may see`）：
  `MemoryStore::list` 只在注释里写了“按句柄的规则过滤”，实际只用了 `JobFilter`——租户句柄能列出别的租户的队列；
  durable 那一侧绑了 `self.scope()`，一直是对的。3.16 的作者在自己的第一个草稿里撞上了（memory 红 / postgres 绿），
  按规矩没动别人的产物。修法用文件里已有的 `visible_queues`（整个列表问一次，而不是每行问一次），
  并把那个断言加到 **3.15 自己的规格**里，且写成用 conformance 夹具建库的形式——于是步自己的 `slow:conformance@2` 门会
  在两个后端上跑它。`MemoryStore` 的 `list` 之外没有别的路径变。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 3.16 交付 | `selftest` 三遍（含全新缓存）`11/11 must green`；尾巴树 `cargo test -p jobq` **98 passed**（3.15 时 92），postgres 子集 `53 passed` |
| 3.16 的运行时长（修完之后） | `selftest jobq/3.16-dlq-tooling` 带 11 个门（含 `slow:conformance@2`）≈ **1 分半**量级 |
| `selftest` 基线 | 真课程 3.5 `19.7 s`、3.15 `25.6 s`（热缓存，`time` 墙钟）；同树 + `slow:conformance@2` `1 m 23 s` |
| 修前的干跑（3.5 临时声明 `slow:mutants@80`） | **>30 min，两次，无结果**；修后 **`1 m 23 s` · 12/12 绿** |
| 变异门报告（修后，阈值临时提到 `@101` 逼出来） | `1 of 6 mutant(s) survived (83% killed, needs ≥101%) (5 of 12 attempts could not be judged…; the walk took 60s)`；卡住的那个写成 `memory.rs:59:47 \`&&\` → \`||\` — did not finish within 25s` |
| `kiln` 自己 | `cargo test -p kiln` **271 + 21 + 1**；新增回归：`sh -c 'sleep 37'` 被超时后 `ps` 里不能再有它、两个时钟的纯函数、报告列号 |
| 3.15 修复的牙 | 回退修复 → 新用例红（`the memory backend answered ["acme-mail", "bob-mail"] instead`）；修复后 `spec_cli` 两后端各 `7 passed`；尾巴树 **99 passed · 0 failed**、postgres 子集 **66 passed · 0 failed** |
| 3.15/3.16 重验 | `selftest jobq/3.15` 两遍（含全新缓存）`10/10`；`jobq/3.16` `11/11`（证明 3.16 的补丁仍能落在被改过的 `memory.rs` 上） |

### 坑与判读

- **改了早期步骤的产物，就要重跑后期的 `selftest`**：3.15 的参考解改了 `MemoryStore::list`，而 3.16 的参考解也在同一文件里
  加 `Store::replay`——两个补丁共用上下文。重跑 `selftest jobq/3.16`（它按序应用 3.1–3.16）才证明“改早期不破后期”。
- **规格是 `.rs` 文件，`fmt` 是项目门**：手写的新用例没排过格式，`fmt` 门直接挂了（输出里就是 rustfmt 的 diff）；
  课程侧的规格要用 `rustfmt --config-path rustfmt.toml --config skip_children=true` 过一遍再钉 hash。
- **工具的进程模型是真实成本**：这次两个“越来越慢”都不是算法问题，而是*进程与锁*的问题——
  超时漏孤儿、孤儿握锁。以后遇到“同一条命令以前跑得快”，先看 `ps`，再看算法。
- **子 agent 的死亡是有规律的**：死在长命令和大输出附近（rustfmt、`cargo test` 全量、`selftest`）。
  新任务书里已经写死：输出重定向到文件、只看尾部、不在一个任务里做两步、每个产物一做好就提交。
- **两处没动的事**：① `3.15` 的 FURTHER 还写着“3.16 是让这个语法长大的地方（两个对死信下手的命令）”，而 3.16 按任务书
  只做了端口一层，没加 CLI 命令——要么改那句话，要么把 CLI 那一半单独开一步（需要用户定）；
  ② `course/probes/ops-dlq-tooling.apply.rs` 考的是 `Dlq::inspect/replay/discard` 与审计行，比 3.16 实际交的面大，
  它的参考解仍能过，但探题与步骤现在描述的是同一槽位的两半。

## 第十段 · 2026-10 · 第二十七步：DLQ 的命令行那一半单独成步

### 做了什么

- **3.17 错误预算与告警**（提交 `a000531`，作业者 `a1841fabda3f4213`，cherry-pick 自 `b441b48`）：一条规则贯穿两个槽位——
  进程导出的**家族表**（`signals()`：五个计数器、一个仪表、每个一个有限基数的标签）、**样本行**的字段就是那张表的
  名字（每个队列一个字段，什么都没发生前写零）、**尝试行**带查询需要的字段并把一次运行做成 **span**（守护进程/worker/
  每个任务的日志行都继承它），而**预算**是两次读数之差（`bad = failed + reclaimed` 除 `total = settled`，带流量下限）。
  `docs/alerts.md` 是那一页，它的规则由规格对照 `signals()` 逐条检查。
- **3.18 追踪采样**（提交 `579fd59`）：上下文是行上的值（`Job::trace()`，`migrations/0007_traces.sql` 两列），**门只决定一次**
  （`Context::of`：调用方的决定优先，否则 `Sampler::decide` 按 id 确定性地掷），worker 不再重掷、每次尝试开一个 span
  （`Hop::of`），出口只补门不可能知道的信息（`Context::exported(failed)`：失败的 hop 无论门说什么都留下）；重放原样带走它。
- **两个指针收口**（提交 `course(jobq/3.15): the dead-letter CLI is its own step…`）：3.15 的 FURTHER 与
  `src/cli/mod.rs` 的模块文档（在 3.15 的 **scaffold** 补丁里）曾把死信命令行许给 3.16；两处现在都指向 `jobq/3.26-dlq-cli`。
  改 scaffold 就重跑了 `selftest jobq/3.15`（`10/10`）——这是“改了早期产物就要重验”的同一规矩。
- **合并方式**：这次作业者的两个提交不在主分支 HEAD 上（Delta 只把文件materialize到工作树），
  我先 `git stash` 再 `cherry-pick b441b48 99f3003`，然后 `git diff 99f3003 HEAD -- course` 为空证明逐字节一致，最后扔掉 stash。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 逐步 `selftest` | 3.17 `gates ✓ 10/10 must green`、3.18 `10/10`，各两遍（第二遍全新缓存 `/private/tmp/xdg-318c`），全部 `hold the contract` |
| 全步判定（当前 18 步的尾巴树） | `kiln verify jobq --slow` → **`18/18 judged step(s) pass · 0 failed · 0 not prepared · 9 without content`**，墙钟 **2 m 19 s**（验证了 coverage/process 改动没有拖慢共享 target 那条路）|
| 默认特性全量套件 | `cargo test -p jobq` → **116 passed · 0 failed**（99 + 8 `spec_alerts` + 9 `spec_tracing`）|
| postgres 后端子集 | 11 个声明后端的规格 → **75 passed · 0 failed**；`spec_tracing` 单跑 `9 passed · 1.98 s` |
| 课程自己的检查 | `lint-caps` `no issues.`（136 slots · 119 positions · 73 authored steps）；`links` `77 page(s) · 423 link(s)`；`i18n status` `structure: 0 error(s)`；`spec manifest` 18 份钉住 |

### 坑与判读

- **作业者自己纠正了元数据**：3.17 的种子声明了 `images`，但它没有任何用例真的起 Postgres（进程那条用 `JOBQ__STORE__URL=memory`），
  于是删掉；3.18 保留并“挣到”它（行那只用例经 `support::store::backend()` 参数化，所以 `KILN_BACKEND=postgres` 会真的跑 `0007`）。
  这正是 §2.2「`images` 只给真起容器的步骤」的形状。
- **`Attempt::new` → `Attempt::of(…)`**（3.18）：追踪把构造子推到 8 个参数，`clippy::too_many_arguments` 在 `clippy@3` 是硬错，
  而 `#[allow]` 不是选项；改成收一个 `Job` 反而更好——队列/id/计数只有一个来源。3.17 规格不构造 `Attempt`，因此仍然成立。
- **两条记给下一个作者的真坑**（来自作业者）：① 在订阅器安装**之前**创建的 span 永远不会被记录（`info_span!` 在创建时选 dispatcher），
  所以 `logging::init` 先装、后开 span；② “引擎不依赖服务层”那条源码文本检查会被**文档注释**里提到的 `crate::service` 触发
  （它在 `src/engine/trace.rs` 的散文上响过）。
- **作业者没跑 `verify --slow`**（它的验收清单只要求套件）；这条由我在合并后补上 ↑。
- **下一步**：3.19+3.20 一批（3.20 起要接 `slow:mutants@80` 与 `clippy@4`，且每步要交 `budget/<step>.toml` 的日子从 3.22 开始）。

## 第十一段 · 2026-10 · 3.17–3.18，以及把两个“以后再说”的指针收口

### 做了什么

- **3.19 属性不变量**（提交 `54a9b2c`，作业者 `0f343fa967d648fb`，这次提交就在主分支上）：新增 `src/engine/walk.rs`——
  `Rng`（splitmix64，只有一个种子）、`Step`/`Invariant`/`Model` + `Machine`（用的是引擎自己的 `transition`，不是副本）、
  `Counterexample`（种子 + 操作序列）、`walk`/`replay`/`shrink`；走着**每一步**都问一次不变量，把拒绝也算一步，
  报告到第一次说“不”的那一步为止。
- **3.20 变异门限**（提交 `b489e6d`）：新增 `src/engine/mutation.rs`——`Score`（空跑时 `percent()` 是 `None` 而不是 `0`）、
  `MutationPolicy`（阈值 + 下限 + `passed`/`failed`/`unproven` 三个答案，下限无论如何抬到 1）、`Survivor`、`Report`、
  `caught`（把 3.19 的 walk 对每个植入缺陷跑一遍）。它声明 `slow:mutants@80`，是**第一个真正跑变异门的步骤**。
- **验收演练 b 腿做完**（详见 handoff §4.3）：把 3.20 的规格换成一份只“碰一下”模块的最小规格，
  `verify jobq --slow --json` → `{passed: 19, failed: 1}`，唯一红的就是
  `slow:mutants@80 (must): 6 of 6 mutant(s) survived (0% killed, needs ≥80%) (the walk took 10s)`，
  并把六个存活变异体的 `文件:行:列` + 原文 + 改写全部点名；规格完好时同一条命令是 `20/20`、门是 `6/6 killed (100%)`。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 逐步 `selftest` | 3.19 `gates ✓ 9/9 must green`（17.5 s 热 / 45.0 s 全新缓存）、3.20 `10/10`（32.1 s / 30.6 s），全部 `hold the contract` |
| 3.20 的变异门（尾巴树） | `6/6 mutant(s) killed (100%, needs ≥80%) (the walk took 9s)`；作业者又手动逐个扫了 15 个候选位点，15/15 全杀 |
| 全步判定 | `kiln verify jobq --slow --json` → **`20/20 judged · 0 failed · 0 blocked · 7 not_authored`**，墙钟 **2 m 38 s** |
| 全量套件 | `cargo test -p jobq` → **134 passed · 0 failed**（116 + 9 + 9）；postgres 后端 10 个规格 → **71 passed · 0 failed** |
| 课程自己的检查 | `lint-caps` `no issues.`（136 slots · 119 positions · 73 authored，`checked 66 of 73`）；`links` `79 page(s) · 429 link(s)`；`i18n status` `structure: 0 error(s)`；`manifest` 20 份钉住 |

### 坑与判读

- **演习的两次失败尝试**：只掏空两个用例不够（套件有冗余，别的用例仍杀掉同一个变异体——这是好事）；
  掏空时留下的双空行与未用导入让 `fmt`/`clippy` 在**整棵共享树**上红了，读起来像“20 步全红”。教训：
  在共享树上做演习，先把自己弄得 lint-clean，否则读到的是别的门。附带收获：树编不过时 `mutants` 拒绝判分
  （“the spec does not pass on the project as it is”）——它不会把编译错误当成杀死。
- **作业者纠正了我 §4.2 的一个口径错误**：早先写的“postgres 跑 11 个规格 → 75 passed”把“我跑过的名单”当成了
  “建 `PostgresStore` 的规格”；真正的数字是 **10 个规格 → 71 passed**（`spec_fairness` 只建内存存储）。已改。
- **`kiln check` 没有 `--slow`**（作业者发现）：学员在 `check` 里看不到 `slow:mutants`，3.20 的 DONE WHEN 只能指向
  `selftest`/`verify --slow`。记在 §5.2 作为可选改进。
- **两条给后续作者的新约束**：①`src/engine/**` 的**文档注释**里不要写 `crate::service`——
  3.2 那条“引擎不依赖服务层”的检查读的是源码文本，会被散文触发（已在 `src/engine/trace.rs` 上响过）；
  ②任何等待都要有截止条件（§4.4），3.19/3.20 的规格是纯的，所以它们的变异门只要 9 s。

## 第十二段 · 2026-10 · 3.19–3.20，以及验收标准里 `mutants` 那一腿的原始证据

## 第十三段 · 2026-10 · 3.21–3.22，`bench-budget` 那一腿，以及一次“幽灵失败”的追根

### 做了什么

- **3.21 覆盖率政策**（提交 `78bb04c`）：策略而不是测量——`src/engine/coverage.rs`（`Kind`/`Site`/`Inventory`+`Foreign`/`Walked`/`Exempt`/
  `ceiling`/`share`/`uncovered`/`CoveragePolicy::standard()`），交付页面 `docs/coverage.md`；把“整个库的百分比说明不了这一步”做成了机制：
  模块外的地方被 `Inventory::add`/`Walked::visit` 拒（`Foreign`），别模块的 `score` 也被拒。复用 3.20 的 `Verdict`，不另造一套。
- **3.22 基准预算**（提交 `f18735d`）：`src/engine/budget.rs`（`Work`/`Row`/`Pull`/`METRICS`/`SCALE_MAX`/`report`）、scaffold 提供 harness
  `tests/budget.rs`、课程侧基线 `course/tracks/jobq/budget/3.22-bench-budget.toml`、页面 `docs/budget.md`。**指标是计数不是时间**
  （`claim_statements=192`、`scan_cells=256`、`scale_ratio_max≤2.2`），所以在负载机器上不会抖。
- **验收演练只剩 `unsafe-budget` 一条腿没做**：a（semver）、b（mutants）、c（bench-budget）、d（conformance）四条都有了原始行，见 §4.3。
- **追了一次“幽灵失败”**：`docs/budget.md` 报 “cannot be read: No such file or directory”，而文件就在、源码没变。根因是 cargo 按**包名+版本**
  选构建槽：`CARGO_TARGET_DIR` 被我从 shell 传给 kiln，又被 kiln 的子进程继承，于是同一棵树的规格跑在了别的树的构建槽里；
  而规格用 `CARGO_MANIFEST_DIR` 读项目文件（编译期烘进二进制），陈旧二进制读的是**别的项目的**文件。修法是 kiln 把继承来的变量从每个子进程清掉
  （提交 `84eb904`），复核：带外来 `CARGO_TARGET_DIR` 的 `verify jobq --json` 从 `20/22` 变成 **`22/22`**。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 逐步 `selftest` | 3.21 `gates ✓ 11/11 must green`、3.22 `12/12`，各跑两遍（含全新缓存），全部 `hold the contract` |
| 3.22 的预算门 | `3 metric(s) within ±10% of the course baseline: claim_statements 192, scan_cells 256, scale_ratio_max 2` |
| 两棵树的变异门 | 3.21 `6/6 killed (100%)`、3.22 `6/6 killed (100%)`（各 8–13 s）|
| 全步判定 | `verify jobq --slow --json` → **`22/22 judged · 0 failed · 0 blocked · 5 not_authored`**（作者冷跑 4 m 47 s，我热跑 3 m 17 s）|
| 全量套件 | `cargo test -p jobq` → **154 passed · 0 failed**；postgres 10 个规格 → 71 passed |
| 课程自己的检查 | `lint-caps` `no issues.`（`checked 68 of 73`）；`links` `81 page(s) · 435 link(s)`；`i18n` `0 error(s)`；`manifest` 22 份 |

### 坑与判读

- **演习要先把自己弄干净**：两次演习都被 `fmt`/`clippy` 先红了（我在共享树里手改代码），读起来像“全部步骤都红”。
  正确姿势：改完先 `cargo fmt` + 只看目标门，再跑 `verify`。
- **共享构建槽的坑值得大声说**：任何“把 `CARGO_TARGET_DIR` 传给 kiln”的习惯都会让 kiln 的子进程共用别的树的槽；
  现在 kiln 自己清掉了，但**在项目目录里手跑 cargo 时**仍要当心（每个作者的工作树都该有自己的 target）。
- **作业者的两个判断我认可**：3.22 的模块是“预算的算术与报告”，不是文本变异器（那会把编译器塞进测试）；
  它也没给 `Store` 加 `stats()`——那会把 `src/engine/store/memory.rs` 整文件拖进 3.22 的变异模块，而那里没有规格能杀掉变异体。

## 第十四段 · 2026-10 · 3.23 由我接手完成，以及一个 `selftest`/`verify` 的不一致

### 做了什么

- **作业者第二次无声死亡**（`d53167f90f9c422e`），但这次留下了四件产物：正文、spec、scaffold、参考解。我接手补齐了缺的部分：
  ① `course/tracks/jobq/budget/3.23-online-schema.toml`（`backfill_jobs = 16`、`burst_max = 1`、`resume_ratio_max = 0.6`——最后一个是页面承诺的**上界**，harness 打印的是实测 0.500）；
  ② 正文 RECALL 的两处 R5 违规（它点名了**本步自己**，还把 `migrations/0006_job_states.sql` 当成步骤引用）；
  ③ 参考解补丁：`src/engine/schema.rs` 不是 rustfmt 形状（`fmt` 门会红），我按 §3.5 的做法用「脚手架态 vs 解法态」的 git diff 重新生成了 `schema.rs` 的 hunk，
  中途还踩到自己的坑——直接拼接 hunk 产生了 `\ No newline at end of file`（kiln 的补丁引擎不支持）并弄坏了相邻 section，最后整份补丁都是从两棵树重新生成的（路径要项目相对，不是仓库相对）。
- **验收**：`verify jobq --slow --json` → **`23/23 judged · 0 failed · 0 blocked · 4 not_authored`**，3.23 的 13/13 must 门全绿——
  `5/6 mutant(s) killed (83%, needs ≥80%)`、`3 metric(s) within ±10%: backfill_jobs 16, burst_max 1, resume_ratio_max 0.5`、`2 backend(s) pass the same suite: memory ✓, postgres ✓`；
  尾巴树 `cargo test -p jobq` → **164 passed · 0 failed**（154 + 9 + 1 harness）；`lint-caps` `no issues.`。
- **发现一个未解的不一致**：同一条命令下 `verify jobq --slow` 让 3.23 全绿，而 `selftest jobq/3.23` 两次（暖 + 全新缓存）都在 `slow:conformance@2` 红，
  两个后端都停在页面读取（`spec_online_schema.rs:113:33`）。现场采样：沙箱里 `app/jobq/docs/migration.md` 在 t≈75 s **在**（脚手架 stub，14 个 TODO），t≈120 s **不在**。
  对照实验：3.21、3.22（它们的页面由**解法**创建，不是脚手架）用同一个 XDG 跑 `selftest` 全绿 → 触发条件看起来是「脚手架发页面 stub + 规格读页面」。
  **没有为了让命令变绿而放宽判据或改产物**；3.23 的交付以 `verify --slow` 为准，这件事记在 handoff §5.2 待查。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 全步判定（23 步尾巴树） | `23/23 judged · 0 failed · 0 blocked · 4 not_authored`，墙钟 **6 m 1 s** |
| 3.23 的门 | 13/13 must 绿：mutants `5/6 (83%)` 9 s、budget 三指标在 ±10%、conformance 两后端、coverage blocked（should）|
| 全量套件 | `cargo test -p jobq` → **164 passed · 0 failed**；3.23 的 spec 在 postgres 上 `9 passed · 4.69 s` |
| 课程自己的检查 | `lint-caps` `no issues.`；`cargo fmt --all --check` 干净；`manifest` 23 份钉住 |

### 坑与判读

- **拼补丁不如重新生成补丁**：手改 hunk 会留下 `\ No newline`、会错切 section、还会让 `skip` 静默失败（补丁不适用时 `skip` 仍然退出 0，
  于是我以为树是解法态、其实是脚手架态——一整轮“幽灵失败”都源于此）。规矩：**改了就重新生成整份补丁**（两棵树 + `git diff --cached --no-renames --binary`），并检查路径前缀。
- **`solution ✓ reference patch applies` 可能掩盖被跳过的文件**（见上面的不一致）：报“应用成功”不等于每个 section 都落了地；
  以后在 `selftest` 之后确认关键文件的状态（如 `grep -c TODO <页>`）。
- **`filter-map` 式的脚本要小心**：我用脚本扫 `cargo fmt` 的差异时只统计了文件；若不是“全部差异都在一个文件里”，重新生成的范围就得更大。

## 第十五段 · 2026-10 · 3.25，以及 `selftest`/`verify` 不一致的根因（由 3.25 的作者找到）

### 做了什么

- **3.25 发布制品**（提交 `199a58a`）：`src/engine/release.rs`（tag/版本、`Refusal` 枚举、`CHANGELOG` 与 checksum 契约）+
  第三个二进制 `jobq-release`（`exit-code` 门会探它）+ 页面 `docs/release.md` 与 `CHANGELOG.md` + `budget/3.25-release-artifacts.toml`。
  它对成本拿了个明确的决定：**规格里不跑 `cargo build --release`**（那会被 `mutants` 重复十来遍），规格只判契约（7.3 s），
  贵的读/哈希/拷贝放进 `bench-budget` 的 harness（只跑一次，6.6 s）。`mutants` 起初 5/6，它补了第 13 个用例（`--out` 指向普通文件 → 退出 1）拿到 **6/6**。
- **3.23 的 `selftest`/`verify` 不一致：根因找到并修好**。它的作者用两个实验定死了机制：
  ① 沙箱里 2 s 粒度的监视证明 `docs/migration.md` **全程都在**（3103 B stub → 7208 B 页面），所以不是文件被删；
  ② 共享槽里的 `spec_online_schema-60e0…` 二进制的 `CARGO_MANIFEST_DIR` 烘的是 **`kiln-mutants-9750-1/project`**（一个已被删掉的变异副本）——
  cargo 按**包名+版本**选槽，`mutants` 在副本里构建，紧随其后的 `conformance@2` 就执行了副本的二进制；把 mutants 的最后一次“恢复运行”挪进项目，
  规格会改在**行为**断言上红（`spec_online_schema.rs:434:5`），证明问题在二进制/槽而不是路径。
  修法：mutants 的副本建在 `<cache>/kiln/mutants-target`（与 `coverage` 同一规则，提交 `kiln(mutants): the copy builds in a directory of its own`），
  那一轮清理运行随之不再需要。**我落地并复核**：`selftest jobq/3.23` 由红转 **`13/13 must green`**，`jobq/3.24`/`3.25` 保持 `12/12`、`13/13`。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 3.25 的 `selftest` | `gates ✓ 13/13 must green`，两遍（1 m 38 s / 2 m 03 s）；我也复核了一遍 `13/13` |
| 3.25 的门 | `links` 6 页 36 链接全解析；`mutants` `6/6 (100%)` 38 s；`budget` `artifacts 2, checksum_lines 2, unverified_max 0, extras_max 0`；`unsafe-budget@0` 39 个生产文件 0 处 `unsafe` |
| 全步判定 | `25/25 judged · 0 failed · 0 blocked · 2 not_authored` |
| 全量套件 | `cargo test -p jobq` → **187 passed · 0 failed**；postgres 11 个 durable 规格 → **80 passed** |
| 课程自己的检查 | `lint-caps` `no issues.`；`links` `84 page(s) · 449 link(s)`；`i18n` `0 error(s)`；`manifest` 25 份 |

### 坑与判读

- **一条值得写进 §3 的通用规律**：任何“在副本里跑 cargo”的装置都必须给副本**自己的构建目录**，否则它污染共享槽，
  让下一个跑同一条命令的人执行**别的树**的二进制（而 `CARGO_MANIFEST_DIR` 是烘进去的，于是失败看起来像“文件不见了”）。
  判据：这条命令读的项目文件，其 `CARGO_MANIFEST_DIR` 指向的目录是否还存在。
- **作业者的偏差**：它把 body 先提交、其余产物在临时树里反复重写，最后合成一个提交而不是“每件产物一个提交”。可接受——
  它验证过最终提交的那份字节（补丁字节与规格 hash 在三棵树里一致）。

## 第十六段 · 2026-10 · 3.27，cap 真缺陷的修复，以及 M2 验收（CI 全绿）

### 做了什么

- **3.27 runbook 演练**（提交 `ecbc230`，`kind = design`）：`src/engine/runbook.rs` 是页面读取器（`Runbook::of`/`gaps`/`holds`，
  `Gap::{Empty, Missing, OutOfOrder}`，`Stage` 而不是 `Step` 以免撞 3.19 的 API），页面 `docs/runbook.md` 是流程本身，
  `tests/support/drill.rs` 的演练把三个世界（`Recovers`/`Never`/`Refused`）跑一遍。规模 13/13 must 门、`mutants` 6/6 13 s、
  `budget` 四个页面计数、`links` 8 页 50 链接。它自己的总结（我认可）：四页“这个工具做什么”不是流程——**全项目没有一页写过回滚**
  （`grep -ri rollback docs/*.md` 只命中过 *scrollback*），也没写过“一个行为被拒绝之后队列停在哪里”；演练因此补了三个阶段/一行。
  它还纠正了种子元数据（`concepts`/`hint` 曾许“一个拒绝写入的存储”，而内存后端造不出 `Store` 类错误），正文里直说了这件事。
- **cap 真缺陷修好**（提交 `cae1a2c` + `84c87e3`）：3.26 作者发现的“durable 的在飞上限没有限住一次领取的行数”已修——
  3.9 的语句取 `least($3, 房间)`（队列余量），3.13 引入 owner 上限后在同行再接一项；3.9 的规格补了**两后端都跑**的用例
  （`the_cap_bounds_the_rows_one_claim_takes`，经 conformance 夹具建库）。复核：3.9 `11/11`（conformance memory ✓ postgres ✓）、3.13 `10/10`。
  留了一个窄窗口并写进 §5.2：两条**并发**的 claim 仍可能各读同一份余量。
  过程中的两个坑：把 hunk 手工拼进补丁会让 kiln 的 applier 拒绝（`\ No newline` 与错位的 section），**改成从两棵树重新生成整份补丁**；
  以及我第一次把 3.13 才引入的 `$8`/`queues` 子查询抄进了 3.9 的语句（那时表还不存在），被 `relation "queues" does not exist` 抓住。
- **M2 验收**：`kiln verify jobq --slow --json` → **`{passed: 27, failed: 0, blocked: 0, not_authored: 0, clean: true}`**；
  `cargo xtask ci` → **exit 0 · 32 m 12 s · `27/27 step(s) hold the contract`**（逐条走 mailer/relay/jobq 三条 track）。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 全步判定（27 步尾巴树） | `{passed: 27, failed: 0, blocked: 0, not_authored: 0, clean: true}` |
| 全量套件 | `cargo test -p jobq` → **210 passed · 0 failed**（81 → 99 → 116 → 134 → 154 → 164 → 173 → 187 → 199 → 210）|
| `cargo xtask ci` | exit 0 · 32 m 12 s · `xtask ci: all checks passed` · `27/27 step(s) hold the contract` |
| 3.27 的门 | `mutants` `6/6 (100%)` 13 s；`budget` `acts 6, named 6, unnamed_max 0, unrunnable_max 0`；`links` 8 页 50 链接；`unsafe-budget@0` 42 文件 0 处 |
| cap 修复 | 3.9 `11/11`（含 conformance 两后端）、3.13 `10/10`；修前 postgres 会多拿（3.26 作者的复现：2 vs 1）|
| 课程自己的检查 | `lint-caps` `no issues.`；`links` 86 页；`i18n` 0 error；`probe --check` 40/40 |

### 坑与判读

- **作业者的一次 Delta 故障**（3.27 作者记下）：它在工作树里 `mkdir` 了一个空目录，Delta 的 materialization 随即卡住（“the checkout kept changing”），
  文件工具还能用、终端全废；删掉那个目录才解冻，代价是 Delta 把删除又传播成一次文件删除。结论：**不要在工作树里手建目录**（用文件工具写字就会自动建目录）。
- **第 5 个作业者失败模式**：3.26 的收尾者耗尽上下文，交回的不是报告而是一份“进度总结”——但那份总结里写清了实现在哪里、验证到什么程度、
  以及剩下什么，于是接手只花了它的“待办清单”而不是重做。**让子代理在上下文耗尽时留下一份可接手的总结**，这件事值得写进 §3.5 的任务书模板。

## 第十七段 · 2026-10 · `coverage` 搬进项目副本：3.22 从 `blocked` 变数字，两个真缺陷，一个待定项

### 做了什么

- **派单（一个 worker，一个任务）**：把 `coverage` 的插桩构建从“原项目 + 共享构建目录”搬成“项目的副本 + 自己的构建目录”，
  同 `mutants`（handoff §5.2 的“待做”）。任务书里给了基线数字（27 步尾巴树上 coverage 的逐行现状）与验收命令。
  交回两笔提交，五个文件全在 `kiln/`：`fa8f996`（副本 + 盖戳 + 工具链 pin，`gates/sandbox.rs` 与 `mutants` 共用）、
  `44bbc52`（逐 object 导出 + 只读这次运行链接的 object + 路径按解析后比较）。集成测试 `coverage_..._or_names_the_component_to_install`
  不再接受 `Blocked` 作为答案，而要求 `Passed | Failed` 且报告里带一个 `%`。
- **我独立复核（不只看它的报告）**：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test -p kiln`（**277 + 21 + 1 passed**）、在 skip 出的 27 步尾巴树上 `verify jobq --slow --json`
  （**exit 0 · 8 m 41.0 s · `{passed: 27, failed: 0, blocked: 0, not_authored: 0, clean: true}`**），
  以及 `cargo xtask ci`（**exit 0 · 35 m 32 s · `27/27 step(s) hold the contract`**，逐条走 mailer/relay/jobq）。
- 文档同步：handoff §1.3（成本表 + coverage 实现契约）、§3（「工具在位未验证过」改为已解决）、§4.2（七个 coverage 的逐行数字 + 复跑的 CI 与 verify）、
  §5.2（两批真缺陷 + 3.26 的待定项）、`docs/t3-jobq.md` 的慢门与缺口两处。

### 数字（原样）

| 项 | 值 |
| --- | --- |
| 尾巴树 `verify jobq --slow --json` | exit 0 · 8 m 41.0 s · `{passed: 27, failed: 0, blocked: 0, not_authored: 0, clean: true}` |
| 七个 `slow:?coverage@80` | 3.21 **204/204**、3.22 **64/64**（修前 `blocked`）、3.23 **237/237**、3.24 **205/215**、3.25 **293/334**、3.27 **121/141**；**3.26 918/2438（37%，应声红）** |
| 八个 `slow:mutants@80` | 6/6 ×7、3.24 5/6（83%）；与改动前逐条一致（搬装置没动这个门） |
| `cargo test -p kiln` | 277 + 21 + 1 passed（+3 在 `sandbox.rs`，+2 在 `coverage.rs`）|
| `cargo xtask ci` | exit 0 · 35 m 32 s · `xtask ci: all checks passed` · `27/27 step(s) hold the contract`（改动前同一条是 32 m 12 s）|
| 每个 coverage 步的代价 | +1.5–2.5 s（复制 23 ms + 每 object 一次 export ~60 ms；依赖 rlib 的时间戳跨步不变，说明只有**包**在重建）|

### 坑与判读

- **“搬进副本”不只是整洁问题，它是 3.22 从 `blocked` 变数字的直接原因**：cargo 按包名+版本选构建槽，插桩目录里可以是别的沙箱留下的二进制，
  它记的是**那棵树**的行数；而 3.22 的模块 `src/engine/budget.rs` 是这一步新建的文件，旧产物来自没有它的树。
- **一次 `llvm-cov export` 拿多个二进制，是关于其中任何一个都不成立的调用**：LLVM 的形状是“一个二进制的多个 object”。多个**二进制**一起交给它，
  它会丢掉只有部分二进制携带的文件（就是 `budget.rs`），并让同一个文件按它碰巧配上的那个 object 回答。
  这条比 3.22 那个 `blocked` 更严重：**旧实现给出的每个数字都可能是“别的视图”的数字**（3.21 的 80% 就来自一个没跑过的 bin）。
- **只读“这次运行链接的 object”**（`target/debug/deps` 里可执行且 `mtime ≥ 运行前一刻`）：共享目录里留着历次运行的插桩产物，
  llvm-cov 会拿这次 profile 去配**已经删掉的副本**的 object，报出一棵不存在的树的文件（全未覆盖）——一个“完美测量被判成 blocked”的假红。
- **路径要按解析后的形式比**：本机 `/var/folders/...` 是 `/private/var/folders/...` 的符号链接，编译器记的是解析后的路径；
  按文本前缀比会把门**自己副本**的文件判成“别的树”——这正是集成测试从前必须接受 `blocked` 的原因。
- **两条小经验（都可复现）**：① 后台跑长验证在本机不可靠——`nohup ... &` 起的进程被终端生命周期带走（`ps` 已不在），
  而 macOS 没有 `setsid`，于是**长验证一律前台跑**并记墙钟；② 我用编辑工具写文档时两次把 `\u2713`/`\u2265` 这六个字符写成了**字面**文本
  （转义只在 JSON 层成立）——渲染出来会是 `\u2265` 而不是 `≥`，已改回字面字符；改完文档跑 `grep -n '\\u[0-9a-f]\{4\}' docs/*.md` 应无输出。
- **一只没动的现场**：容器库里有两只 `Created` 状态的残留（`kiln-pg-37015-3`、`kiln-relay-pg-6462-4`，创建于 2026-10-04），
  不是这几轮验证留下的（本轮容器全由夹具清理干净），留给下次收尾决定。
- **留给用户的一个决定**：3.26 的 `slow:?coverage@80` 在参考解上是 37%，因为它的模块把**更早步骤拥有的大文件**也算进来了；
  三个选项与我的建议写在 handoff §5.2（推荐“步骤自己声明模块”）。`verify` 的 `clean` 不受 should 门影响。

## 第十八段 · 2026-10 · “coverage 应该如何提高”：先量，然后发现门丢掉了子进程的 profile

### 做了什么

- **先量，不猜**（用户问 3.26 的覆盖率怎么提高）：把尾巴树铺好，用门自己的做法（插桩构建 + merge + 逐 object 导出）写脚本重算 3.26 的模块——
  总数与门逐字节一致（918/2438 = 37%），但**按文件**一看就露了馅：`src/cli/dlq.rs`（这一步自己的文件，199 行）报 **0/199**，
  而规格在每一个用例里都在跑这个二进制（`course/tracks/jobq/spec/3.26/dlq_cli.rs:76`）。
- **根因**：规格用 `env_clear()` 启动 `jobqc`——“脚本看到什么环境”是接口的一部分，所以这是故意的；代价是子进程拿不到 `LLVM_PROFILE_FILE`，
  LLVM 把 profile 写成默认名（`default_<hash>_0_<pid>.profraw`）落在**它自己的工作目录**，也就是被测量的那棵树里（实测这次 33 个文件）。
  门只 merge 自己指的那个目录，于是丢掉的正好是真二进制跑出来的那一半数据。
- **修法**（提交 `032554b`）：`profiles_of_the_run` = 门自己的目录 + 被测量树里出现的 `.profraw`；
  同时 `sandbox::copy_project` **不复制** `.profraw`，否则“在树里找到”就可能意味着“上一次运行带进来的”。
- **量出来的效果**（尾巴树 `verify jobq --slow --json` → exit 0 · 8 m 23.8 s · `{passed: 27, ..., clean: true}`）：
  3.26 由 **918/2438 (37%) 变 1367/2438 (56%)**，其中它自己的文件 **181/199 = 90%**；其余六个 coverage 步与八个 mutants 走法逐条不变。
  未覆盖的 1071 行现在按文件是：`postgres.rs` 528、`memory.rs` 255、`cli/mod.rs` 232、`store/mod.rs` 38、`dlq.rs` 18 ——
  也就是说，“3.26 覆盖率低”里属于**门丢掉的部分已经补回来了**，剩下的缺口是模块里更早步骤拥有的代码（handoff §5.2 的待定项）。
- 复核：`cargo test -p kiln` → **278 + 21 + 1 passed**（+1 是新单测）；`cargo xtask ci` → **exit 0 · 39 m 22.9 s · `27/27 step(s) hold the contract`**（`cargo test --workspace` 里 kiln 是 278 + 21 + 1）。

### 坑与判读

- **“先量再改”这次直接抓到一个门缺陷**：只看总数（37%）看不出问题，按文件一看就清楚——0/199 不可能是真的，因为规格每个用例都在跑那个二进制。
  给后来者的做法：**对任何“百分比偏低”的判据，先要按文件的明细，再判断是内容不够还是度量不对。**
- **时间戳不能与“盖戳的副本”一起用**（我自己先踩了一遍）：`coverage` 这次本想用“比开跑新”判“是不是这次的”，
  但 `sandbox::copy_project` 的定义就是“把每个拷进来的文件盖到未来一秒”，于是**所有**被复制的文件都满足它。
  结论：要么让那些文件根本不在副本里（把 `.profraw` 排除），要么用“运行中出现”的集合差，不要拿时间戳跟一个盖戳步骤比。
- **边界写进契约、不装作解决**：现在收的是“被测量树里出现的 `.profraw`”；如果某个规格把子进程的 cwd 设到树外（目前没有），那份数据仍然收不到（§1.3）。

## 第二十一段 · 2026-10 · 一份独立 review 的十项发现：A/B/C/E 落地，D 待定

### 做了什么

- 用户要求“review 一下之前的 review 结果，然后提出修改方案”：我读了那个 review 线程（`Measure coverage over what a step brings`，它自己没有文件改动），
  逐条核过它的证据（SAFETY 窗口的四处、`executables` 的扫描面、模块后缀匹配、`copy_project` 的 `file_type`、两个无锁目录、`RUSTFLAGS` 的来源、`Scaffold::Skeleton`），
  然后给了分批方案：A（探题对齐，单独一笔）+ B（coverage 两处口径）+ C（符号链接 / 骨架步）+ D（rustflags 边界 + 共享目录的锁）+ E（`check --slow`）。
- 用户批复“按你建议拉一个子agent开始”：派一个 worker 做 A+B（任务书里带了环境事实、验收命令、“一批一提交”、“卡住就停并报告”），
  我自己接了 C 与 E（它们与 worker 改的文件不重叠，合并是干净的）。
- 落地（都在 `main`，工作树干净）：
  - **A** `5dbe3c7`（worker）：探题与参考解都改成“标记必须在**上面**三行”，`reason_above` 的窗口去掉 `=index`；
    反例用例的**形状**与任务书写的不一样（worker 报告）：探题只认行首的 `// SAFETY:`，所以“行尾同行标记”不成立；
    真正的反例是“这一行既是使用、又是它自己的标记”（扫描按词计数，标记行里的 `unsafe` 也算一处）。
  - **B** `788705e`（worker）：`executables_of_the_run` 同时读构建根与 `deps`（同一把尺：非递归、可执行位、`mtime ≥ since`）；
    模块路径改成 `strip_prefix` 相等（副本外仍是故障、副本内但不是那个文件则忽略）。两条单测都能区分（把任一处改回去，对应用例会红）。
  - **C** `35ec238`（我）：`copy_project` 跟随符号链接（`fs::metadata`）+ 已走目录集合防环；骨架步按“自己补丁碰过的文件”判（文档 + 单测钉住，理由见 §5.2）。
  - **E** `d49b4c1`（我）：`check --slow` 跑与 `verify --slow`/`selftest` 同一集合；顺手修了汇总行把“没跑的门”当“未实现”的两个计数
    （`--slow` 下的 `11/10`，以及快路径下把两个跳过的慢门报成“catalogue gate(s) not implemented yet”）。
  - `c9146b2`（我）：探题里残留的 `at or above` 断言文案。
- **合并方式**：worker 的两笔以“内容”到达我的工作树（不带提交），按老规矩 `git stash push -u` → `git cherry-pick bf36acc 9947ece` → 比对两次 diff **逐字节一致** → `git stash drop`。
- **复核（我独立重取）**：`cargo test -p kiln` → **285 + 21 + 1**；`kiln probe --check` → **41/41**；
  尾巴树 `verify jobq --slow --json` → exit 0 · **7 m 21.7 s** · 27/27 `clean`，七条 coverage 与八条 mutants 走法数字逐条不变；`check jobq/3.21-coverage-policy --slow` → **1 m 38 s**。
- **待定**：D（#5 两个共享构建目录的锁、#6 `RUSTFLAGS` 与 `.cargo/config.toml` 的边界）——两条都需要取舍（有界等待后 `blocked` / 检测到就 `blocked`），
  且只在人工并发或项目自配 rustflags 时触发；先把边界写进 §1.3，等用户定。

### 坑与判读

- **review 的一条建议会重新引入刚修掉的缺陷**：#7 建议“骨架步列出整块骨架”，而骨架带着整条 track 的 stub
  （`src/engine/error.rs` 是 3.3 的题目、`src/bin/jobqd.rs` 很后面）——列进去就是把别步的代码算进第一步的分母。改成写清楚 + 单测钉住。
- **任务书不能代替读代码（worker 的两处修正）**：探题的标记必须是行首（所以我给的反例形状不成立）；
  `Path::ends_with` 是按路径组件比的（所以 `xsrc/store.rs` 从来不是误报，只有 `backup/src/store.rs` 那类才是）。
- **`check --slow` 暴露了两个旧计数错**：汇总行过去把跳过的慢门算成“catalogue gate(s) not implemented yet”——
  学员在快路径上一直看着一行不实的提示；现在按“跑过的门”计数，并把跳过的慢门单独列出来。

## 第十九段 · 2026-10 · 用户定下 coverage 的模块规则（方案 ③），以及一次被中断的 CI

### 做了什么

- 用户在三个选项里选了 ③：**coverage 的模块 = 这一步带进来的文件**（scaffold 或参考解新建的 `src/**.rs`），
  什么都没新建时回退到补丁碰过的文件；`mutants` 保持“补丁碰过的整份文件”不变（采样得宽是它的目的）。
- 实现（提交 `7c6bcb4`）：`sources::introduced_sources` + `created()`/`touched()` 两个助手；`coverage.rs` 改用它，
  模块文档里写清两门为何读不同的集合；`sources.rs` 的模块文档改成“两个问题、两个答案”。
  两条新单测：scaffold 新建的文件算“带进来的”、只改不新建时回退。
- 量（尾巴树 `verify jobq --slow --json` → exit 0 · 8 m 33.1 s · `{passed: 27, failed: 0, blocked: 0, not_authored: 0, clean: true}`）：
  **七个 coverage 门全过，没有一条 should 是红的**；3.26 **307/325 = 94%**（`src/engine/operator.rs`（scaffold 新建）+ `src/cli/dlq.rs`（参考解新建）），
  其余六步数字不变（204/204、64/64、237/237、205/215、293/334、121/141），八条 mutants 走法不变。
- `cargo test -p kiln` → **280 + 21 + 1**；fmt/clippy 干净；`links` 87 页 464 链接全解析。

### 坑与判读

- **收窄模块对那六步没有副作用**：它们的“带进来的”文件本来就等于它们的模块（只是不再带 `src/engine/mod.rs` 那个没有可执行行的转发文件），
  所以只有 3.26 的数字动了——这正是“模块 = 这一步带进来的东西”应有的行为。
- **CI 被中断**（用户发问时打断）：这一笔的 CI 数没有取到，本地证据如上；要发或要收尾时补跑一次即可。

## 第二十段 · 2026-10 · “覆盖率为什么不前置”：解释清楚了，不前置

### 做了什么

- 用户问“覆盖率的内容为什么不一并前置”，并明确只要**解释清楚**，不要求把课/门提前。答案落在三处（作者最可能读到的地方）：
  `plan.md` §6 的表后新增一段「门的起点是能力，不是日程表」——门由**教它那一步**引入、之后一直生效，并写了被否掉的三条理由
  （判的是没教过的政策 / 要先把 15+ 个已交付步骤的参考解逐条量过并按需补规格 / 质量门管的是你已有的项目）；
  `docs/authoring.md` §7 规则 5 加一句“引入它的那一步就是它开始生效的那一步，想提前先读 plan §6”；本条（handoff §5.2）记下决定与理由。
- 学员侧的说明**本来就有**，没有改动：`jobq/3.21` 正文的 WHY/RECALL 写明了覆盖率与变异“互为补集”（可这到不达 vs 到了却不报），
  而元数据里 `3.21.requires = ["test/mutation@apply", "err/typed-errors@design"]` 就是这个顺序的根据。
- 顺手量清楚的账（供以后讨论用）：今天声明 `?coverage@80` 的是 3.21–3.27 七步；把门提前到 3.3–3.20 要先逐条量那些参考解、
  过不了的步要回去补规格（那是内容工作），CI 每步多一次插桩构建。
