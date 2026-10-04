# M2 交接简报（T3 `jobq` 的 26 步 + 慢闸门 + conformance 套件）

> M0/M1 见 [handoff-m0.md](handoff-m0.md) 与 [handoff-m1.md](handoff-m1.md)（机制、命令、约定、环境事实）。
> 这份文件是 M2 的压缩上下文：慢闸门的分级决定与实现契约（§1）、T3 的 26 步与项目形状（§2）、
> 施工中钉下来的决定（§3）、实测数字与验收证据（§4）、缺口（§5）。
> 施工过程分段记在 [handoff-m2-log.md](handoff-m2-log.md)。

## 0. M2 的范围与验收（plan.md §11）

| 验收项 | 判据 |
| --- | --- |
| T3 `jobq` 26 步 | 正文、只读规格、加性 scaffold、参考解、闸门接线全部就位；`kiln verify jobq` 在最终树上全绿 |
| 慢闸门接入判定器并分级 | `mutants`、`coverage`、`bench-budget`、`semver`、`unsafe-budget` 都能跑；分级写进 plan §10 与本文件 §1 |
| `conformance` 套件 | 同一套件在内存实现与 Postgres 实现上都跑（`conformance@N`） |
| **故意劣化能被抓住** | 删掉索引 / 把 O(log n) 换成线性扫描 → `bench-budget` 红；改坏断言或删分支测试 → `mutants` 红；删公开项或改签名 → `semver` 红 |

M1 留下的环境事实全部继续有效（worktree 里不许生成 `target/`、沙箱共用缓存会假红、
`/private/tmp` 而不是 `/tmp`、容器夹具 `rm -f -v`、长验证在冻结 commit 上跑）。

## 1. 慢闸门：分级、实现契约、缺工具时的对策

### 1.1 一条原则，四个决定

原则：**凡是 `must` 的闸门，必须只用「这台机器一定有」的东西判定**——`cargo`/`rustc`/`clippy`、
课程自己的文件、以及本仓的容器运行时抽象。`cargo-mutants`/`cargo-llvm-cov`/`cargo-semver-checks`
在本机**没有安装**，把它们写成 must 就等于「一台好机器上的一步会无理由变红」，
这正是 plan §10「外部工具缺失当成一等公民」与 authoring §7.2 要避免的。所以：

- **判定权在我们自己手里**：`mutants` / `bench-budget` / `semver` / `unsafe-budget` 由 `kiln` 自己实现
  （用编译器、用课程自己的基线文件），外部的同名工具退居「生产里的做法」，由 `kiln doctor` 报出、
  在正文里点名，**不进闸门**。
- **确实需要外部二进制的闸门只能是 `should`**（目前只有 `coverage` 与 `deny`）：缺失时报
  `⚠ blocked` + 可复制的安装指令，**绝不把一步变红**。
- **must 的慢闸门必须确定性**：不允许把墙钟时间当作判据（共享机器上必然假红），判据是**计数**
  （语句数、单元格扫描数、杀死/存活的变异体数、编译成败）。
- **快路径 `<60s` 不动**：所有慢闸门都带 `slow:` 前缀，只由 `kiln verify --slow` 与 `selftest` 跑。

### 1.2 分级表（写进 plan.md §6 的表）

| 闸门写法 | 档 | 判据 | 用什么判定 | 缺工具时 |
| --- | --- | --- | --- | --- |
| `slow:mutants@N` | **must** | 本步模块的变异体被杀比例 ≥ N% | `kiln` 自己：文本级变异 + 跑本步的 `run.cmd` | 不适用（不需要外部工具） |
| `slow:bench-budget@P` | **must** | 预算harness 报的每个计数都在课程基线的 ±P% 内 | `kiln` 自己：跑 `tests/budget.rs` 解析 `jobq-budget k=v` 行 | 不适用 |
| `semver@baseline` | **must**（快） | 课程基线见证文件仍能编译 | `rustc`（`cargo test --test semver_baseline`） | 不适用 |
| `unsafe-budget@N` | **must**（快） | 生产代码里 `unsafe` 出现次数 ≤ N，且每处都有 `SAFETY:` 说明、`unsafe_code = "deny"` 在位 | `kiln` 自己（读源码 + `cargo clippy`） | 不适用 |
| `slow:conformance@N` | **must** | 项目声明 ≥N 个后端，每个后端跑同一份规格都绿 | `kiln` 自己：按 `conformance.toml` 逐后端重跑 `run.cmd` | 不适用（Postgres 走后端自己的容器） |
| `slow:?coverage@N` | **should** | 本步模块行覆盖 ≥ N% | `llvm-profdata` + `llvm-cov`（工具链组件） | `⚠ blocked: rustup component add llvm-tools-preview`，**不阻塞** |
| `?deny` | **should** | 许可/来源策略 | `cargo-deny` | `⚠ blocked: cargo install cargo-deny --locked`，**不阻塞** |

`clippy@4`（阶梯最高级）的含义据此落地为「本步的慢闸门是 must 而不是 should」，从 T3·3.20 起写进元数据；
`@4` 不新增 lint（实现已在 `kiln/src/gates/clippy.rs`，`MAX_RUNG = 4`）。

### 1.3 每个新闸门的实现契约（实现与内容都按这份写）

**`mutants@N`（慢，限本步模块）**

- 变异的范围**不是**人工清单：取本步参考解补丁 `solutions/<step>.patch` 里改动/新增的 `.rs` 文件，
  只保留 `src/` 下的生产代码（跳过 `tests/`、`benches/`、迁移、`Cargo.toml`）。
  这样「这一步的模块」由课程自己的元数据定义，新增步骤不会忘记登记。
- 变异算子（文本级、确定性、按文件行序）：`==`↔`!=`、`<`↔`<=`、`>`↔`>=`、`&&`↔`||`、
  删掉 `!`、`Ok(`↔`Err(`、`Some(`↔`None`、`+`↔`-`、整数常量 `0`↔`1`。
- 每个变异体：把项目整树复制到沙箱、改一个位置、用 `run.cmd` 跑本步规格。
  - 规格红 = **杀死**；规格绿 = **存活**（记为缺陷）；编译失败/超时 = **不计分**（`skipped`），
    只报告个数（免得用编译错误把分数灌满）。
- **上限**：每步最多 `MUTANT_CAP = 6` 个变异体，按行号等距取样（确定性），从文件的第一个可变异位置开始。
  参数 `N` 是百分数阈值，`killed / (killed + survived) >= N%`，分母为 0 时判 `blocked`（不是 pass）。
- 报告行必须点名存活的变异体（文件、行号、原文），否则学员不知道该补什么测试。

**`bench-budget@P`（慢）**

- 项目里由脚手架提供 `tests/budget.rs`（harness，学员改的是被它测的代码）＋课程提供基线，
  基线文件放在**课程侧**：`course/tracks/jobq/budget/<step-id>.toml`（与 `spec/` 同一信任级：学员可以读、
  改它没有意义，因为闸门读的是课程副本）。格式（`command` 与基线同处一个文件，闸门不需要猜怎么跑）：

  ```toml
  # jobq bench baseline, step 3.22 — measured from the reference solution on the course's toolchain.
  command = "cargo test -p jobq --test budget -- --nocapture"

  [metric]
  claim_statements = 12        # store calls per drained batch
  scan_cells = 340             # cells examined to find the next due task
  scale_ratio_max = 2.2        # work(2n) / work(n): an upper bound, not a target
  ```

- harness 在 stdout 打 `jobq-budget <name>=<value>`（浮点或整数）；闸门逐个比对
  `|value - baseline| / baseline <= P%`，名字以 `_max` 结尾的指标只判 `value <= baseline`。
- 判据是**计数**，不是时间：计数值来自被测代码自己暴露的计数（store 端口上的 `stats()`、
  `Ord` 调用计数、语句计数），所以在任何机器上同值。

**`semver@baseline`（快）**

- 基线是课程拥有的 `.rs` 见证文件：`course/tracks/jobq/api/<step-id>.rs`；闸门把它复制成
  `<project>/tests/semver_baseline.rs` 并跑 `cargo test --test semver_baseline`（`--test` 目标名固定）。
- 见证文件用**编译器能判的形式**写死公开面：函数指针强制转换（签名）、对公开枚举的穷尽 `match`
  （新增变体即红）、`#[non_exhaustive]` 的 `..`（不写死字段数）。删除项、改签名、加枚举变体 → 编译失败。
- 破坏性改动**允许**，但要求同一步里显式升 `Cargo.toml` 的主版本号并在 `CHANGELOG.md` 写明——
  那是 3.25 的纪律；不允许的是「悄悄破坏」。

**`unsafe-budget@N`（快）**

- 判三件事：① 生产源文件里 `unsafe` 词出现的处数 ≤ `N`；② 每处前面 3 行内必须有 `// SAFETY:` 注释；
  ③ workspace lints 里有 `unsafe_code = "deny"`（「默认拒绝，放行要写理由」）。
- 计数只在非测试代码上（`#[cfg(test)]` 模块里的 unsafe 不计），与 clippy 的「目标类型」分工一致。

**`conformance@N`（慢）**

- 项目的 `conformance.toml` 声明后端：

  ```toml
  # One suite, many implementations: `kiln` re-runs this step's spec once per backend.
  backends = ["memory", "postgres"]
  ```

- 闸门：`backends.len() >= N`，然后对每个后端用 `KILN_BACKEND=<name>` 重跑本步的 `run.cmd`，全部绿才通过。
  规格侧的约定：`tests/support/store.rs` 按 `KILN_BACKEND` 造 store（默认 `memory`），所以「同一套件跑所有实现」
  是**一份规格跑 N 次**，不是 N 份规格。

**`coverage@N`（慢，should）**

- 工具链里找 `llvm-profdata`/`llvm-cov`（`rustc --print sysroot` + `lib/rustlib/<host>/bin/`）。
- 找到：`RUSTFLAGS="-C instrument-coverage"` + `LLVM_PROFILE_FILE` 跑本步 `run.cmd`，merge 后导出
  `--summary`，只判**本步模块**（= 本步参考解补丁碰过的生产 `.rs` 文件）的行覆盖 ≥ N%。
  找不到：`blocked` + `rustup component add llvm-tools-preview`。

### 1.4 must 慢闸门在 CI 里怎么可复现

- `cargo xtask ci` → `kiln selftest` → 慢闸门。可复现的三个来源：
  1. **判据是编译器与计数**，没有一项依赖墙钟时间或网络；
  2. **确定性取样**：变异体按行号等距取前 6 个，预算基线是课程文件，conformance 的后端列表是项目文件；
  3. **工具缺失不改判**：`must` 闸门不需要外部 cargo 工具；`should` 缺失时打印指令并以 `⚠ blocked` 记在报告里。
- 代价（要写进正文与 README 的数字）：`mutants` 每步最多 6 次规格运行（含容器的话 6×秒级），
  `bench-budget` 一次 harness，`conformance` 每个后端一次规格运行。都是 slow 路径的事。

## 2. T3 `jobq`：26 步、项目形状、门阶梯

题材：把 T2 的场景换成**给别人用的引擎**——库的公开面 + worker 守护进程 + CLI，
重点从「写服务」转向「设计接口」与「用工具证明质量」（curriculum §8）。

### 2.1 origin 分布与能力槽位（与 `capabilities.toml` 逐行对齐）

`lint-caps` 的 R0 要求每一步的 `teaches` 与台账逐槽一致，所以下表就是从台账反推出来的：
**11 teach + 1 transfer + 14 deepen**（T2 是 18+1+5）。

| # | 步骤 id | origin | teaches |
| --- | --- | --- | --- |
| 3.1 | `3.1-engine-boundary` | **transfer** | `api/library-design@aware` |
| 3.2 | `3.2-public-api-baseline` | deepen | `api/library-design@apply` |
| 3.3 | `3.3-library-errors` | deepen | `err/typed-errors@design` |
| 3.4 | `3.4-feature-assembly` | deepen | `cfg/layered-config@design` |
| 3.5 | `3.5-store-port-conformance` | teach | `db/port-conformance@apply` |
| 3.6 | `3.6-transaction-isolation` | deepen | `db/transactions@design`、`db/unique-as-concurrency@design` |
| 3.7 | `3.7-leases-and-recovery` | deepen | `db/atomic-claim@design`、`async/graceful-shutdown@operate` |
| 3.8 | `3.8-clock-injection` | teach | `time/clock-injection@apply` |
| 3.9 | `3.9-priority-fairness` | deepen | `sched/fairness@design`、`http/rate-limit@design` |
| 3.10 | `3.10-cron-schedules` | teach | `sched/cron@apply` |
| 3.11 | `3.11-delayed-timers` | teach | `sched/delayed-timers@apply` |
| 3.12 | `3.12-dedup-window` | deepen | `delivery/idempotency@design` |
| 3.13 | `3.13-queue-authz-attribution` | deepen | `sec/authz-tenant-isolation@operate` |
| 3.14 | `3.14-job-state-machine` | teach | `domain/state-machine@apply` |
| 3.15 | `3.15-cli-contract` | teach | `cli/contract@apply` |
| 3.16 | `3.16-dlq-tooling` | deepen | `ops/dlq-tooling@apply`、`delivery/dlq@operate` |
| 3.17 | `3.17-error-budget-alerts` | deepen | `obs/metrics-slo@design`、`obs/structured-logs@design` |
| 3.18 | `3.18-trace-sampling` | deepen | `obs/tracing-propagation@operate` |
| 3.19 | `3.19-property-invariants` | teach | `test/property-based@apply` |
| 3.20 | `3.20-mutation-threshold` | teach | `test/mutation@apply` |
| 3.21 | `3.21-coverage-policy` | teach | `test/coverage-policy@apply` |
| 3.22 | `3.22-bench-budget` | teach | `perf/bench-budget@apply` |
| 3.23 | `3.23-online-schema` | deepen | `db/schema-online-evolution@apply` |
| 3.24 | `3.24-unsafe-budget` | deepen | `unsafe/budget@apply` |
| 3.25 | `3.25-release-artifacts` | teach | `release/artifacts@apply` |
| 3.26 | `3.26-runbook-drill` | deepen | `ops/runbook-drill@design` |

### 2.2 门阶梯（只增不减）

基准是每条 T3 步都带的持久属性：`spec-integrity`、`caps`、`i18n`、`fmt`、`no-todo`、`docs:public`、`exit-code`。
在此之上：

| 从哪一步起 | 新增 |
| --- | --- |
| 3.1 | `clippy@2`（T2 的终点，T3 的起点） |
| 3.2 | `semver@baseline`（must，快）——基线一旦建立就**持续**到 3.26 |
| 3.3 | `clippy@3`（`missing_errors_doc` + pedantic 白名单；`disallowed_methods` 要等 3.8 写 `clippy.toml` 才生效） |
| 3.5 | `slow:conformance@2`（must）＋ `images`（该步要 Postgres 才给） |
| 3.20 | `slow:mutants@80`（must）＋ `clippy@4` |
| 3.21 | `slow:?coverage@80`（should，工具缺失时 blocked） |
| 3.22 | `slow:bench-budget@10`（must） |
| 3.24 | `unsafe-budget@0`（must，快） |
| 3.25 / 3.26 | `links`（交付页面的步骤） |

`conformance@2` 只写在「这一步判的就是端口」的步骤上（3.5、3.6、3.7、3.9、3.12、3.16、3.23 之类的现场），
`images` 同样只写在真起 Postgres 的步骤上——两者都是**步骤作用域**属性（plan §6），
回归保护靠 `verify` 跑遍每步自己的闸门。

### 2.3 项目形状（与 T1/T2 同构，不重新发明）

```
app/jobq/
  Cargo.toml          # [package] name = "jobq"：lib（引擎）+ 两个 bin：jobqd（worker）、jobqc（CLI）
                      # [workspace] members 带标记块（后面步骤可以加成员 crate）
  src/lib.rs          # 引擎的公开面：api / domain / error / clock / store / scheduler / cron / …
  src/bin/jobqd.rs    # 守护进程：配置、日志、迁移、HTTP（/healthz /readyz /metrics）
  src/bin/jobqc.rs    # CLI：submit/list/inspect/replay/purge + --json + 退出码
  src/service/**      # 守护进程内部（引擎不许反向依赖它，3.1 的判据）
  tests/support/**    # client / app / service / postgres / store（按 JOBQ_BACKEND 造 store）
  migrations/**       # sqlx 迁移，前向编号
  docs/**             # runbook、capacity、coverage、adr/**、CHANGELOG.md
```

- 单包多 bin 的理由：规格总是被同步到 `<project>/tests/spec_*.rs`（`kiln/src/spec.rs` 的
  `LEARNER_TESTS_DIR`），所以受判的包必须是**项目根那个包**；`CARGO_BIN_EXE_jobqd`/`jobqc` 因此可用。
- 引擎边界（3.1）判两件事：① 库能在没有服务的情况下端到端跑通一个内存队列（行为断言）；
  ② 引擎模块里没有指向 `crate::service`/`crate::cli` 的 `use`（结构事实，正文写明理由）。
- 依赖方向：`jobqd`/`jobqc` 依赖 lib，绝不反过来；`semver` 的基线只管 lib 的公开面。

### 2.4 `exit` 能力（R6）

`api/library-design@apply`、`db/port-conformance@apply`、`db/atomic-claim@design`、
`async/graceful-shutdown@operate`、`delivery/dlq@operate`、`test/mutation@apply`、`perf/bench-budget@apply`、
`unsafe/budget@apply`、`ops/runbook-drill@design`。

## 3. 施工中钉下来的决定

（随进度追加；每条都写清「谁定、为什么、后来的步骤要么遵守要么同步改」。）

- **R1 的键是「槽位」而不是「能力」**（M2 开工时改的框架行为，提交 `kiln(caps): R1 keys on the slot…`）：
  `capabilities.toml` 本来就计划三条 track 在同一 track 内把一个能力从 `aware` 走到 `apply`
  （`jobq/3.1` → `jobq/3.2`、T4 的 WAL 对、T5 的共识对），而旧实现按能力去重，会把台账自己的规划报成
  R1 错误。规则文档（plan §3 D4 规则 1、curriculum §2.3）写的都是「同深度不得重教」，所以改的是实现。
  回归测试 `r1_keys_on_the_slot_and_not_on_the_capability`。
- **T3 的形状**（照抄 T1/T2，不重新发明）：`app/jobq` 是**一个包**（lib = 引擎，bin `jobqd` = 守护进程；
  CLI `jobqc` 随 3.15 落地），因为规格总是同步到 `<project>/tests/spec_*.rs`，受判的包必须是项目根那个包；
  `CARGO_BIN_EXE_jobqd` 因此可用。守护进程暂不起 HTTP：health/metrics 到 3.17 再说。
- **引擎的端口是「装箱 future + 显式生命周期」而不是 `async fn`**：`fn claim<'a>(&'a self, …) -> BoxFuture<'a, …>`。
  `async fn` 的 trait 不是 dyn-compatible（E0038，handoff-m0 §2），而 worker 要通过引用持有 store。
- **`no-todo` 是 must，所以每一步的「骨架 + 参考解」必须自洽**：骨架里出现 `todo!()` 的模块，该步的参考解
  必须在同一步里实现完（否则 `no-todo` 红）。这条决定了骨架只能声明**本步要教的东西**——
  Postgres 后端、worker 循环、CLI 都随自己那一步的 scaffold 新增文件，而不是提前摆在骨架里。

## 3.5 步骤作者作业书（给子 agent 的固定配方）

每一步都按这个顺序做，不要跳步，也不要在工作树里跑 cargo。

### 环境与禁令

- **所有 cargo/rustc 命令都要带** `CARGO_TARGET_DIR=/private/tmp/<你的名字>-target`；用 `kiln` 的命令
  再带 `XDG_CACHE_HOME=/private/tmp/xdg-<你的名字>`。worktree 里**绝不允许**出现 `target/`（Delta 的 VFS 会因此卡住）。
- 路径用 `/private/tmp/...`，不用 `/tmp/...`（本机 podman 的 VM 只共享 `/Users`、`/private`、`/var/folders`）。
- 别同时跑两个 cargo。红得可疑时：单跑那一步 → 换全新 `XDG_CACHE_HOME` 复跑，两次一致才算真红。
- **不要**在 `app/jobq` 里手工折腾（`app/` 是学员状态，`kiln` 会自己建）；本地验证用 `kiln selftest`（它在沙箱里走）。

### 每一步要交付的东西

1. **正文** `course/docs/steps/en/jobq/<id>.md`，front-matter 三行（`id` = `jobq/<phase>.<index>-<slug>`、`title`、`locale = "en"`），
   段标按 `origin`：`teach` → WHY/TASK/DONE WHEN/TRAP/FURTHER；`deepen`/`transfer` → RECALL/DELTA/TASK/DONE WHEN/TRAP
   （**不得有 WHY**）。RECALL 必须写 `<能力>@<深度> — <track>/<phase>.<index>`，且那一步必须真的建立过本步的
   `requires` 或要深化的槽位（R5 会查）。TRAP 必须写**具体失败现象**，不是忠告。
2. **规格** `course/tracks/jobq/spec/<phase>/<name>.rs` —— 只读、哈希钉死、能被 `kiln run` 同步成
   `tests/spec_<name>.rs`。只能依赖骨架与已交付步骤里已有的签名。多用 `mod support;` 拿夹具。
3. **scaffold**（可缺）`course/tracks/jobq/scaffold/<id>.patch` —— **只能新增文件或往标记块里插行**，绝不删行；
   本步要教的东西里「学员必须自己决定的那部分」不要给。
4. **参考解** `course/tracks/jobq/solutions/<id>.patch` —— 普通 patch，可以新增/修改/删除，但**必须在树上精确匹配**；
   它必须把该步 scaffold 引入的每个 `todo!()` 都实现掉（`no-todo` 是 must 门）。
5. **元数据**：`track.toml` 里该步的 `scaffold`（没就删行）、`gates`。

### 补丁怎么生成（记住 M1 的坑：不要在项目目录里 rm -rf）

在**临时 git 仓库**里做两次 diff：

```sh
WT=.../rustlings-production                    # 你的 Delta worktree
rm -rf /private/tmp/<step>-base /private/tmp/<step>-sol
mkdir -p /private/tmp/<step>-base
# base = 上一步交付后的状态：把 app/jobq 或沙箱里的那棵树拷过来，或者用 skeleton + 之前的 patch 依次 apply
# 1) 先把 base 提交：cd /private/tmp/<step>-base && git init -q && git add -A && git commit -qm base
# 2) 在 base 上写入 scaffold 的内容 -> git add -A && git diff --cached --no-renames --binary > scaffold.patch
# 3) 提交 scaffold，再写参考解的内容 -> git add -A && git diff --cached --no-renames --binary > solution.patch
```

`git diff` 一律 `--no-renames --binary`（patch 解析器拒绝 `rename from/to`）。

### 验收（每一步都要跑，并把原始结论行写回）

```sh
CARGO_TARGET_DIR=/private/tmp/kiln-target XDG_CACHE_HOME=/private/tmp/xdg-<name> \
  cargo run -q -p kiln -- spec manifest --track jobq      # 钉规格哈希
CARGO_TARGET_DIR=... XDG_CACHE_HOME=... \
  cargo run -q -p kiln -- selftest jobq/<id> --track jobq # 必须：scaffold ✗ / solution ✓ / gates N/N must green
CARGO_TARGET_DIR=... cargo test -p kiln                   # 框架单测（改了 kiln 才需要）
CARGO_TARGET_DIR=... cargo run -q -p kiln -- lint-caps    # 必须 no issues
rustfmt --edition 2024 --config-path rustfmt.toml --config skip_children=true --check <你改过的每个 .rs>
```

规格与骨架里的 `.rs` 都要 rustfmt 干净（`xtask ci` 会查）；`tests/support/*` 这类只有被 `mod` 引用到的
文件，要在有 `mod support;` 的树里 `cargo fmt` 才会被格式化——手写之后跑一次。

### 每步收尾清单

- [ ] 规格在 scaffold 上失败（`selftest` 自己会报 `BROKEN STEP`，别自己放宽）
- [ ] 参考解在规格上全绿、全部 must 门绿
- [ ] 正文段标与 origin 匹配，DONE WHEN 里是确切命令
- [ ] `lint-caps` 无 issue；`spec manifest --track jobq` 已重跑
- [ ] 如果改了共享契约面（结构体签名、Store 方法、配置 schema、迁移号），**同步改早期规格并把哈希重钉**，
      并在报告里明说改了哪几份早期规格
- [ ] 报告里给原始命令与结论行（退出码、测试数、耗时），不要转述

### 早期规格钉住的契约（后续步骤不得破坏）

`verify` 与「学员做到后面再回头跑早期步骤」都会拿最终树重跑早期规格，所以下面这些是**累计约束**：

- `jobqd --check-config` 必须继续打印 `store.url`（不含密码）、`worker.name`、`worker.queues`、`worker.batch`、
  `log.level`、`log.format`，且三层（文件 < 环境变量 < flag）依旧生效；`--worker-name` 仍要认。新增设置只能加行，不能改行。
- 守护进程启动后必须先打印**含 `draining`** 的日志行（夹具的就绪标记），再等信号；`SIGTERM` 后退出码必须是 0。
  改这句文案就会弄红 3.1 的规格。
- `src/engine/**` 里不得出现 `crate::service` / `crate::cli`（3.1 的结构断言），新文件同样受约束；
  `src/engine` 下至少 4 个 `.rs`。
- 内存 store 的语义：`claim` 即 `attempts += 1` 且 `state = running`；`complete` 只接受 running；
  `fail` 按预算决定 `queued`/`failed` 并把原因写进 `last_error`；`get` 找不到返回 `Ok(None)`，
  而 `Engine::inspect` 把它翻成 `Error::NotFound`。
- `Store` 端口是密封的（3.2 起）：新增方法只能加在 trait 上（crate 内实现），不要在外部类型上实现它。

## 4. 实测数字与验收证据

（每 5–6 步同步一次；收尾时写完整。）

## 5. 缺口

### 5.1 probe 覆盖（T3 新增槽位）

题库从 19 道扩到 **35 道**（`probe --check` → `35 exercise(s) · 35 reference(s) passed`），
T3 新增或加深且能用单文件 `std` 诚实地考的槽位都有了题；**没出题的槽位与理由**（与
handoff-m1-log.md 第七段同一口径：当单文件只能把机制换成假件时，那道题就是假证据，比没有更糟）：

| 槽位 | 为什么没有题 |
| --- | --- |
| `api/library-design@aware`（3.1） | `aware` 按政策不出题（curriculum §11） |
| `err/typed-errors@design`（3.3） | 增量（`#[non_exhaustive]`、Display 快照）已被 `api-library-design.apply` 从表面侧盖住 |
| `cfg/layered-config@design`（3.4） | 机制是 cargo 编出一个 feature 矩阵，单文件只能给一组 feature 解出一个配置 |
| `async/graceful-shutdown@operate`（3.7） | operate 的增量是跨进程（滚动升级下的 drain、双领窗口），单文件只能假装第二个进程 |
| `http/rate-limit@design`（3.9） | 同一步的公平调度已有 `sched-fairness.design` |
| `delivery/dlq@operate`（3.16） | 与 `ops-dlq-tooling.apply` 同一步同一件事 |
| `ops/runbook-drill@design`（3.26） | 证据是一页文档 + 对它的评审，探针只能查步骤表的形状 |
| `db/transactions@design` 等 | **可加**：注入序列化冲突 + 有界重试（与 `delivery-retry-budget.design` 重叠）、去重窗口边界、配额与归因计数、错误预算/告警噪声、追踪采样、expand→backfill→contract —— 这六项是「范围」而不是「诚实度」的取舍，留给收尾前补（每项的形状已写在子 agent 报告里）|

### 5.2 其他

（收尾时写。）
