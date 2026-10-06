# T3 `jobq`：任务队列引擎（参考实现）

> 与 [t1-mailer.md](t1-mailer.md)、[t2-relay.md](t2-relay.md) 同构。M2 的施工记录在
> [handoff-m2.md](handoff-m2.md)（契约与验收）与 [handoff-m2-log.md](handoff-m2-log.md)（分段日志）。

## 1. 一句话与资产

T3 是一个用步一层层长出来的**任务队列引擎**：一个库（引擎与端口）、两个二进制（`jobqd` 守护进程、`jobqc` 操作员工具）、
加一个发布工具 `jobq-release`；两个存储后端（内存与 Postgres）跑**同一套一致性套件**；队列自己的 schema 可以在一路
服务的情况下在线演进；运维面（日志/指标/告警/追踪/死信门/演练）全部在这条链里长出来。

| 资产 | 位置 |
| --- | --- |
| 27 步正文（英文） | `course/docs/steps/en/jobq/` |
| 28 份只读规格 + hash 清单 | `course/tracks/jobq/spec/`、`spec/manifest.toml` |
| scaffold / 参考解（逐步） | `course/tracks/jobq/scaffold|solutions/` |
| 课程侧基线与装置 | `course/tracks/jobq/budget/*.toml`、`skeleton/`、`api/` |
| 探题 | `course/probes/`（全课程 40 道，`probe --check` 40/40）|

## 2. 27 步一览

| 步 | 主题 | 建立的槽位 | 这一步把什么变成接口 |
| --- | --- | --- | --- |
| 3.1 | 引擎边界 | `api/library-design@aware` | 库与守护进程的分界，引擎不依赖服务层 |
| 3.2 | 公开面基线 | `api/library-design@apply` | 见证文件（编译就是断言）：改名、删项、改签名都是编译错误 |
| 3.3 | 库的错误 | `err/typed-errors@design` | 错误分类与退出码表 |
| 3.4 | 特性装配 | `cfg/layered-config@design` | `--no-default-features` 也要是一个能用的装配 |
| 3.5 | 存储端口与一致性套件 | `db/port-conformance@apply` | 一套用例、N 个后端（`KILN_BACKEND`）|
| 3.6 | 事务隔离与迹 | `db/transactions@design`、`db/unique-as-concurrency@design` | 状态变更与迹行同事务 |
| 3.7 | 租约与恢复 | `db/atomic-claim@design`、`async/graceful-shutdown@operate` | 领取即租约；崩溃后回收 |
| 3.8 | 时钟注入 | `time/clock-injection@apply` | 时间是端口，不是 `Utc::now()` |
| 3.9 | 优先级与公平 | `sched/fairness@design`、`http/rate-limit@design` | aging 与每队列在飞上限 |
| 3.10 | cron 计划 | `sched/cron@apply` | `Schedule::next_after()` 一个问题 |
| 3.11 | 延迟定时器 | `sched/delayed-timers@apply` | 等待的代价是队列数而不是作业数 |
| 3.12 | 去重窗口 | `delivery/idempotency@design` | 键 + 窗口是接口 |
| 3.13 | 队列鉴权与归属 | `sec/authz-tenant-isolation@operate` | 看不见而不只是拒绝；配额与计数 |
| 3.14 | 作业状态机 | `domain/state-machine@apply` | `transition(state, event)` 一个全函数；列由 `check` 关住 |
| 3.15 | CLI 契约 | `cli/contract@apply` | 一条失败类一个退出码；stdout 是答案 |
| 3.16 | 死信视图与重放 | `ops/dlq-tooling@apply`、`delivery/dlq@operate` | 重放是第二个生命，不是状态迁移 |
| 3.17 | 错误预算与告警 | `obs/metrics-slo@design`、`obs/structured-logs@design` | 家族表、样本行、尝试行、预算 |
| 3.18 | 追踪采样 | `obs/tracing-propagation@operate` | 门只决定一次，上下文随行走 |
| 3.19 | 属性不变量 | `test/property-based@apply` | 种子 + 操作序列就是反例 |
| 3.20 | 变异门限 | `test/mutation@apply` | 规格必须证明自己能失败（`slow:mutants@80`）|
| 3.21 | 覆盖率政策 | `test/coverage-policy@apply` | 哪些分支必须被走到，而不是一个百分比 |
| 3.22 | 基准预算 | `perf/bench-budget@apply` | 计数而不是时间（`slow:bench-budget@10`）|
| 3.23 | 在线 schema 演进 | `db/schema-online-evolution@apply` | expand/backfill/contract 三阶段与回滚列 |
| 3.24 | unsafe 预算 | `unsafe/budget@apply` | 默认 deny，每一处例外带理由 |
| 3.25 | 发布制品 | `release/artifacts@apply` | tag/版本/校验和/changelog 是一份契约 |
| 3.26 | 死信命令行 | `ops/dlq-tooling@operate`、`cli/contract@design` | 行为要签名；不可撤销的行为要点名理由 |
| 3.27 | runbook 演练 | `ops/runbook-drill@design` | 页面就是流程；每个阶段欠四件事（命令/信号/拒绝/回滚）|

## 3. 项目最后的形状

`app/jobq` 是一个包：`src/engine/**`（引擎：端口、两个后端、状态机、调度、去重、鉴权、追踪、预算、schema 演进、runbook 读取器）、
`src/service/**`（配置、日志、worker、`open_store`）、`src/cli/**`（语法与渲染）、`src/bin/{jobqd,jobqc,jobq-release}.rs`、
`migrations/0001..0008_*.sql`、`docs/*.md`（每个主题一页）、`tests/spec_*.rs`（课程拷入的只读规格）+ `tests/support/**`（学员的夹具）+ 两个课程装置
（`tests/{budget,backfill_budget,dlq_harness,release_harness,runbook_harness}.rs`）。

## 4. 装置与可复用模式

- **一套套件、N 个后端**：`KILN_BACKEND` + `tests/support/store.rs`；`slow:conformance@2` 就是拿这条命令跑两遍。
- **课程装置 vs 规格**：`tests/*_harness.rs` 由脚手架交付（算指标），`tests/spec_*.rs` 由课程拷入（判契约），两者互不越位。
- **页面作为作品**：3.17/3.21/3.22/3.23/3.25/3.26/3.27 各交一页，规格拿代码里的表/常量对照页面，`links` 门保证它不烂。
- **预算 = 计数**：`jobq-budget k=v` 三/四行，`_max` 是上界；同一份基线在课程侧（`budget/*.toml`）。
- **两个时钟**（kiln 侧）：变异体限 `4×基线+15s`、整轮 `WALK_BUDGET`；副本必须有自己的构建目录。

## 5. 实测数字（这台机器）

| 项 | 值 |
| --- | --- |
| 全步判定（27 步尾巴树，`kiln verify jobq --slow --json`）| **`{passed: 27, failed: 0, blocked: 0, not_authored: 0, clean: true}`** |
| 全量套件 | `cargo test -p jobq` → **210 passed · 0 failed**；durable 规格在 `KILN_BACKEND=postgres` 下同样全绿 |
| 逐步 `selftest` | 3.1–3.27 全绿（13–15 个 must 门；3.5/3.6/3.7/3.9/3.11/3.12/3.16/3.23/3.26 含 `slow:conformance@2`）|
| 慢门 | `mutants` 6/6 或 5/6（3.24 是 83%）；`bench-budget` 三/四个计数在 ±10%；`unsafe-budget@0` 零 `unsafe`；`?coverage`（装了 `llvm-tools-preview`，构建在项目副本里、子进程数据也算）3.21/3.22/3.23 **100%**、3.24 95%、3.25 87%、3.27 85%、**3.26 56%**（低于它自己声明的 80%，见 §6）|
| 课程自有检查 | `lint-caps` `no issues.`（97/136/119/73）；`links` **87 page(s) · 464 link(s) · 283 external target(s)**，全部解析；`i18n` 0 error；`probe --check` 41/41 |
| `cargo xtask ci` | **exit 0 · 39 m 22.9 s · `27/27 step(s) hold the contract`**（2026-10-06，子进程 profile 那笔之后）|
| M2 验收演练 | 五条腿都有原始行（semver / mutants / bench-budget / conformance / unsafe-budget），见 handoff §4.3 |

## 6. T3 范围内剩下的缺口

- `slow:?coverage@80` 的测量路径 2026-10 跑通（装了 `llvm-tools-preview`），过程中修掉门里**八个**真缺陷（handoff §5.2）；
  插桩构建现在跑在项目的**副本**里（`gates/sandbox.rs`，与 `mutants` 共用），共享构建槽不再参与；子进程自己写的 profile 也算（`env_clear` 启动的真二进制看不到 `LLVM_PROFILE_FILE`）。真实数字（2026-10-06 复核）：
  3.21 **100%**（204/204）、3.22 **100%**（64/64，修前 `blocked`）、3.23 **100%**、3.24 95%、3.25 87%、3.27 85%、**3.26 56%（1367/2438）**。
- **3.26 的 `?coverage@80` 与「模块 = 补丁碰过的整份文件」这条规则冲突**：它的模块含 `src/cli/**`、`src/engine/store/*` 里更早步骤拥有的代码，
  所以这一步的模块到不了 80%——但那不是“工具没写测试”：它自己的文件 `src/cli/dlq.rs` 是 **90%**，剩下的未覆盖行几乎全在更早步骤拥有的大文件里。
  选项与建议见 handoff §5.2（推荐让步骤自己声明模块）；在那之前它是一个 should 红，不影响 `clean`。
- `mutants` 的模块是“补丁碰过的整份文件”：3.24 的 **5/6（83%，在线上）** 与 3.26 为别人的文件补的三条用例都是这个规则的代价。
- durable 的 cap 在**并发** claim 下仍可能各读同一份余量（handoff §5.2 记了窄窗口）。
- ~~`ops/dlq-tooling@operate` 没有同深度探题~~ 已补（`course/probes/ops-dlq-tooling.operate.rs`，题库 41 道）。
- `docs/coverage.md` 的阈值与 `ceiling` 是课程里的政策，不与任何外部 SLO 对齐（那超出这门课的范围）。
