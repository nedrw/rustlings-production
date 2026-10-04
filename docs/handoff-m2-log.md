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

## 第八段（待写）
