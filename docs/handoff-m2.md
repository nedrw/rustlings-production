# M2 交接简报（T3 `jobq` 的 27 步 + 慢闸门 + conformance 套件）

> M0/M1 见 [handoff-m0.md](handoff-m0.md) 与 [handoff-m1.md](handoff-m1.md)（机制、命令、约定、环境事实）。
> 这份文件是 M2 的压缩上下文：慢闸门的分级决定与实现契约（§1）、T3 的 27 步与项目形状（§2）、
> 施工中钉下来的决定（§3）、实测数字与验收证据（§4）、缺口（§5）。
> 施工过程分段记在 [handoff-m2-log.md](handoff-m2-log.md)。

## 0. M2 的范围与验收（plan.md §11）

| 验收项 | 判据 |
| --- | --- |
| T3 `jobq` 27 步 | 正文、只读规格、加性 scaffold、参考解、闸门接线全部就位；`kiln verify jobq` 在最终树上全绿 |
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

**实测成本（本机、热缓存、`time` 墙钟）：先看这张表再看下面各门的契约**，因为「慢门」里真正慢的只有两个：

| 门 | 每次判定做什么 | 实测 | 大头在哪 |
| --- | --- | --- | --- |
| `fmt` | `cargo fmt --all --check` | 0.18 s | 进程启动 |
| `spec-integrity` / `caps` / `i18n` / `unsafe-budget` | 读文件、算 hash、扫源码（不建房） | 毫秒 | — |
| `exit-code` | 带探针参数跑两个二进制 | ~0.1 s | 两次进程启动 |
| `images` | 问本地镜像库 | 秒内 | 引擎 CLI |
| `semver@baseline` | 把见证文件当测试目标编一次 | **~2.5 s** | 一次 cargo 重建 + 链接 |
| `clippy@N` | `cargo clippy --all-targets` | 单跑 23 s；同一棵树上的 10 个快门**合计 15 s** | rustc 跨 target 的检查 |
| `no-todo` / `docs:public` | 同一套 clippy 机制（不同 `-D` 集合） | 与上一行共享产物 | 同上 |
| `slow:conformance@2` | 同一条 `run.cmd` 跑两遍（memory + postgres） | 0.2 s + 7.5 s ≈ **8 s** | 每个 durable 用例自己起一个容器 |
| `slow:bench-budget@P` | 跑预算命令一次 | 一次构建 + 跑 ≈ 2.5 s + 基准自身时间 | 一次 cargo |
| `slow:?coverage@N` | 带 `-C instrument-coverage` 的构建 + 分析 | 本机 blocked（无 llvm-tools）；若真跑：一次冷重建 **22 s**（现在有自己的目录，见下） | 一次完整重建 |
| `slow:mutants@N` | 基线 1 次 + 最多 12 次尝试 + 收尾 1 次，每次都重建+链接+跑 | 走完 12 次尝试 **60 s**（≈ 5 s/次，含一次卡死被 25 s 拒掉） | 尝试次数 × 一次 cargo |

一个 `selftest` 的固定成本 ≈ 首次建项目 + 快门那 10 项（合计约 15 s）：热缓存下 3.5（11 个门）**19.7 s**、
3.15（10 个门）**25.6 s**。所以 CI 的半小时是「很多次 20–30 秒」，不是某个门特别贵；
**从 3.20 起每步多花的 ~60 s 全部来自 `mutants`**（成本 = 尝试次数 × 一次 cargo，而尝试次数是它自己决定的），
所以 §4.4 的两个时钟（每次 `4×基线+15s`、整轮 300 s）就是给它的。

**`coverage` 的构建目录是它自己的**（提交 `kiln(coverage): the instrumented build keeps a directory of its own`）：
换 `RUSTFLAGS` 就是换一次构建，以前它用沙箱共享目录，会把普通构建的产物扔掉（下一步命令付 22 s）；
现在它在 `<cache>/kiln/coverage-target` 里建，第二次跑还是增量的。这靠 `process::prepared` 的一条规则：
**调用方点名的 `CARGO_TARGET_DIR` 优先于沙箱默认值**（沙箱共享目录是默认值，不是命令）。

**`mutants@N`（慢，限本步模块）**

- 变异的范围**不是**人工清单：取本步参考解补丁 `solutions/<step>.patch` 里改动/新增的 `.rs` 文件，
  只保留 `src/` 下的生产代码（跳过 `tests/`、`benches/`、迁移、`Cargo.toml`）。
  这样「这一步的模块」由课程自己的元数据定义，新增步骤不会忘记登记。
- 变异算子（文本级、确定性、按文件行序）：`==`↔`!=`、`<`↔`<=`、`>`↔`>=`（仅当两侧都有空白：`fmt` 写出的类型是紧的、
  比较是带空的）、`&&`↔`||`（仅当左侧有操作数——闭包的 `||` 与双引用的 `&&` 不算）、删掉 `!`（仅当左邻不是标识符
  字节——`format!` 这类宏的 `!` 不算）、`+`↔`-`、整数常量 `0`↔`1`，`true`↔`false`。
  箭头（`->`、`=>`）不是位置；**构造子改写已删除**（`Ok(`↔`Err(`、`Some(`↔`None(`）：它们是换类型而不是换行为，
  `None(x)` 根本不可调用（`E0532`），实测 12 次尝试里占了 3 次、一次都没判成（§4.4）。
- 每个变异体：把项目整树复制到沙箱、改一个位置、用 `run.cmd` 跑本步规格。
  - 规格红 = **杀死**；规格绿 = **存活**（记为缺陷）；编译失败/超时 = **不计分**（`skipped`），报告要逐条给出原因
    （`path:line:col 改写 — does not build: error[…]` / `did not finish within Ns`），否则一个 `blocked` 的闸门没人能处理。
- **上限**：每步最多判 `MUTANT_CAP = 6` 个变异体，候选按行号等距取样（确定性）；`ATTEMPT_CAP = 12` 是*尝试*上限，
  所以一个编译不过的位置浪费一次尝试而不是一个名额。参数 `N` 是百分数阈值，`killed / (killed + survived) >= N%`，
  分母为 0 时判 `blocked`（不是 pass）。
- **模块是「补丁碰过的整份文件」**（3.26 的实测后果）：它的补丁要碰 `store/{memory,postgres,mod}.rs`，取样于是落在**别的步骤拥有**的代码上
  （`memory.rs:536` 的每队列在飞上限过滤、`441` 的 rank aging 算术、`106` 的迹行身份），三个存活变异体只能由**这一步的规格**杀掉 →
  3.26 的 spec 从 8 例长到 11 例，其中两例断言的是 `jobq/3.9` 建立过的语义。判据本身是对的，但后来者要预期这件事，
  并在正文里说清那些用例属于谁的承诺。
- **两个时钟**（§4.4 的教训）：单个变异体最多跑 `4 × 未变异基线耗时 + 15s`（再被 `--timeout` 封顶）——变异体只差一个算子，
  超过四倍还在跑就是卡住了；整轮最多 `WALK_BUDGET = 300s`，到点就停止并声明“走路被预算截断”。报告里带这轮花了多少秒。
- 报告行必须点名存活的变异体（文件、行、列、原文），否则学员不知道该补什么测试。

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

## 2. T3 `jobq`：27 步、项目形状、门阶梯

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
| 3.26 | `3.26-dlq-cli` | deepen | `ops/dlq-tooling@operate`、`cli/contract@design` |
| 3.27 | `3.27-runbook-drill` | deepen | `ops/runbook-drill@design` |

> `3.26-dlq-cli` 是 2026-10 追加的第二十七步（用户决定：3.15 的 FURTHER 承诺了“对死信下手的命令”，
> 而 3.16 按任务书只做了端口那一半）。它插在 runbook 演练**之前**，因为演练要用这些命令；
> 代价最小：只把演练从 3.26 移到 3.27，`3.16` 正文里对它的引用、`capabilities.toml` 的槽位（
> `ops/dlq-tooling.operate`、`cli/contract.design`）与 `ops/runbook-drill.design` 同步改。

### 2.2 门阶梯（只增不减）

基准是每条 T3 步都带的持久属性：`spec-integrity`、`caps`、`i18n`、`fmt`、`no-todo`、`docs:public`、`exit-code`。
在此之上：

| 从哪一步起 | 新增 |
| --- | --- |
| 3.1 | `clippy@2`（T2 的终点，T3 的起点） |
| 3.2 | `semver@baseline`（must，快）——基线一旦建立就**持续**到 3.27 |
| 3.3 | `clippy@3`（`missing_errors_doc` + pedantic 白名单；`disallowed_methods` 要等 3.8 写 `clippy.toml` 才生效） |
| 3.5 | `slow:conformance@2`（must）＋ `images`（该步要 Postgres 才给） |
| 3.20 | `slow:mutants@80`（must）＋ `clippy@4` |
| 3.21 | `slow:?coverage@80`（should，工具缺失时 blocked） |
| 3.22 | `slow:bench-budget@10`（must） |
| 3.24 | `unsafe-budget@0`（must，快） |
| 3.25 起 | `links`（交付页面的步骤） |

**3.20 以后是累积的**：每一步的门清单＝上表在它这一步时已生效的全部行（这就是“只增不减”）；
`track.toml` 里 3.20–3.27 的清单在插 3.26 时一并补齐过（之前 3.21–3.26 只写了自己新增的那一行，
与这张表不一致），因此那几步的作者要预期自己的步骤带上：`slow:mutants@80`、`slow:?coverage@80`、
从 3.22 起的 `slow:bench-budget@10`（并交 `course/tracks/jobq/budget/<step>.toml`）、
从 3.24 起的 `unsafe-budget@0`、从 3.25 起的 `links`。`selftest` 因此每步大约多花一分钟（§4.4）。

`conformance@2` 只写在「这一步判的就是端口」的步骤上（3.5、3.6、3.7、3.9、3.12、3.16、3.23、3.26 之类的现场），
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
  tests/support/**    # client / app / service / postgres / store（store 抽 `KILN_BACKEND` 造后端）
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
- **`semver@baseline` 的见证文件是「建立它的那一步」一份**，不是每个受判步骤一份：3.2 交出
  `api/3.2-public-api-baseline.rs`，从 3.2 起每一步都声明这个门；闸门取 `api/` 里**编号 ≤ 本步的最新一份**
  （所以 3.3–3.26 用的都是 3.2 那份；将来某一步要做一次故意的主版本级变更，就自己再加一份 `api/<它的 id>.rs`，
  同一规则会自动改用它，闸门不用改）。推论：**从 3.2 起公开面只能加不能改**（新 trait 方法、新访问器、
  `#[non_exhaustive]` 的新字段可以；改名、删项、改签名不行）。
- **见证文件能用编译器证明什么、不能证明什么**：它能把「公开面还在」与「`#[non_exhaustive]` 在位」
  编成硬事实（它的通配臂在 `deny(unreachable_patterns)` 下编译，所以少标一个就是编译错误），
  但**证明不了隐私与密封**——那需要一个必须编译失败的测试，而这门课没有这样的工具。
  所以 3.2 的规格用结构断言（读源码）盖住那两项，**每条断言都写了理由**（authoring §6.6）。
- **错误类型的 `#[source]` 不点名驱动**：`Error::Store` 的 cause 是 `Box<dyn Error + Send + Sync>`，
  而不是 `sqlx::Error`——在后端还没引入（3.5）之前就把驱动写进公开枚举，会让 3.4 的
  `--no-default-features` 构不出来；调用方用 `downcast_ref` 拿回具体类型。
- **3.4 的 `[features]` 表把 `postgres` 写成多行列表**并放在标记块里，就是为了给 3.5 留插入点：
  scaffold 只能加行（`dep:sqlx` 在 sqlx 存在之前写不出来，两种写法都是硬报错）。
- **六个质量闸门已实现，与 §1.3 的差异照实记下**（提交 `kiln(gates): the six quality gates…`）：
  ① `mutants` 每步最多花 **8 次**规格运行而不是 6 次——多了「先在未变异副本上跑一次」（否则项目自己的规格
  本来就红时每个变异体都会被当成杀死，这是这个门最不能给的错答案）与「共享 target 时最后再跑一次好树」
  （cargo 按包名+版本选构建槽，否则最后一个变异体二进制会发给学员自己的下一次 `cargo test`）；
  ② 沙箱副本的文件时间戳要往前推一秒（`fs::copy` 会带原时间戳，cargo 判新鲜度后默默用陈旧二进制）；
  ③ `coverage` 的「工具在位」路径**在本机没有端到端验证过**（`llvm-tools-preview` 未装）：集成测试断言
  的是诚实的 `blocked` + 安装行，测量逻辑用合成文档单位测试。
- **重建 `dead_letters` 与 `replay` 是端口的事，且语义已被 3.16 钉死**（提交 `course(jobq/3.16-dlq-tooling)`）：
  死信不是一个新表，是 `failed` 里的行（`dead_letters()` 返回一个 `JobFilter` 视图，不是取行口）；重放是
  **另一个生命**而不是一次状态迁移（`transition` 里没有这个目的地，合法）；`revived(job, now)` 是唯一算术：
  **计数保留、上限抬到 `attempts + 1`**（与 `relay/2.10` 的重置相反，因为逆迹行的身份是 `(job, kind, attempt)`）、
  `last_error` 保留、租约字段清空、`run_at = store 自己的时钟`；`Store::replay` 是唯一回程（读锁下决定、
  行与迹同事务），授权按 `QueueAction::Claim` 判、拒绝顺序是 `NotFound` → `Forbidden` → `Invalid`，
  迹里多一个 `EventKind::Replayed`。后来步骤可以**加**方法/事件，不得改这几条的语义。
- **`Store::list` 按句柄的可见性过滤，两个后端都得这样**（提交 `course(jobq/3.15): a listing answers what this handle may see`）：
  memory 那一侧曾只按 `JobFilter` 过滤（租户句柄能列出别人的队列），已在 3.15 的参考解与规格里改正；
  `visible_queues` 是整个列表问一次的那个问题（每行问一次 `visible` 是把声明表走 N 遍）。从 3.16 起，
  凡是新增“列出来”的接口（CLI、报表、DLQ 视图）都得守同一条。
- **`src/engine/**` 的文档注释里不要写 `crate::service`**（3.18 的作者踩过）：3.2 那条“引擎不依赖服务层”的检查
  读的是源码文本，散文里的一个路径也会让它红；它管的是代码，不是意图。
- **等待一定要有截止条件**（§4.4 的实测）：一个只有“队列空才退出”的循环会把变异门从 20 秒变成半小时。
  纯函数式的步骤（无容器、无时钟、无等待）的变异门只要 9 秒（3.20 的实测）。
- **质量闸门从 3.20 起是累积的，且带两个时钟**（提交 `kiln(mutants)`、`kiln(process)`，详见 §1.3 与 §4.4）：
  每步的 `selftest` 因此大约多花一分钟；`bench-budget` 那一行意味着每步要交
  `course/tracks/jobq/budget/<step-id>.toml`；超时只会杀直接子进程的时代结束了——超时会带走子进程的整棵后代树，
  所以“某个命令卡住”不会再把整台机器拖慢。

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
- **`src/engine/**` 里不得出现 `pub mod`**（3.2 的规格按声明查，不按名字查）：新增模块写成
  `pub(crate) mod <x>;`，需要公开的东西在 `src/engine/store/mod.rs` / `src/engine/mod.rs` 重导出
  （3.5 的 Postgres 后端就是这么加的）。
- **公开枚举的变体字段也是接口**：`#[non_exhaustive]` 只买到「加变体」，买不到「给已有变体加字段」。
  3.3 的规格会构造 `Error::Store { source: … }` 这类值并钉住 Display 文案，所以后续要带更多信息时
  **加变体**（如 3.6 的 `Conflict`、3.13 的 `Forbidden`），不要改已有变体的字段或文案。
- `Job` / `SubmitRequest` 是 `#[non_exhaustive]` 结构体：可以加私有字段（加租约、加去重键都行），
  但 3.2 见证文件钉住的访问器签名不能改；新访问器只会在更晚的见证文件里出现。

## 4. 实测数字与验收证据

### 4.1 验收演练的固定做法（M2 收尾时执行，结果写在这里）

验收标准是「故意引入的劣化能被 `mutants` 与 `bench-budget` 抓到、`semver` 能拦住破坏性改动」。
做法固定成下面四步，每步都留原始输出：

```sh
# 1) 在 app/jobq 里得到整条 track 的参考解状态（真项目，不是夹具）
#    注意：这个版本的 `kiln skip` 没有 `--track`，`--all` 走的是 course.toml 的默认 track（mailer）；
#    铺 jobq 的树要按路径序逐个 skip（`selftest`/`verify` 才有 `--track`）。
for s in 3.1-engine-boundary 3.2-public-api-baseline 3.3-library-errors 3.4-feature-assembly \
         3.5-store-port-conformance 3.6-transaction-isolation 3.7-leases-and-recovery \
         3.8-clock-injection 3.9-priority-fairness 3.10-cron-schedules 3.11-delayed-timers \
         3.12-dedup-window 3.13-queue-authz-attribution 3.14-job-state-machine 3.15-cli-contract; do
  CARGO_TARGET_DIR=/private/tmp/demo-target XDG_CACHE_HOME=/private/tmp/xdg-demo \
    cargo run -q -p kiln -- skip "jobq/$s" --force
done

# 2) 四类故意劣化，每次只改一处，改完跑那一步的判定
git -C app/jobq diff            # 先存下“好树”
#   a) semver：把 Engine::submit 改名或改返回类型
#   b) mutants：改一处行为而不动测试（该步模块里留下一个存活变异体）
#   c) bench-budget：把被预算盯着的路径换成 O(n^2) 扫描（或去掉索引）
#   d) conformance：让 PG 后端与内存后端对同一问题给出不同答案。要选**读-然后-写**的 claim
#      （先选行再更新）；把单语句里的 `SKIP LOCKED` 删掉是行为上看不见的（输者等锁后重查谓词，结果一样）
CARGO_TARGET_DIR=/private/tmp/demo-target XDG_CACHE_HOME=/private/tmp/xdg-demo \
  cargo run -q -p kiln -- check jobq/<受影响的那一步> --track jobq

# 3) 每次都用 git -C app/jobq checkout -- . 回到好树，确认同一个命令变绿（对照实验）
```

判据：劣化 → 对应的门必须红且点出位置（文件:行 / 指标名 / 编译错误）；回到好树 → 同一个命令绿。
两次结论一致才算数。

### 4.2 施工中的数字

（**27/27 步交付**，全部在**最终树**上重取过；取法：worktree 里对应命令 + 私有 `CARGO_TARGET_DIR`/`XDG_CACHE_HOME`。）

| 项 | 值 |
| --- | --- |
| 逐步 `selftest`（每步两遍，第二遍全新缓存） | 3.1 `8/8` · 3.2 `9/9` · 3.3 `9/9` · 3.4 `9/9` · 3.5 `11/11` · 3.6 `11/11` · 3.7 `11/11` · 3.8 `9/9` · 3.9 `11/11` · 3.10 `9/9` · 3.11 `11/11` · 3.12 `11/11` · 3.13 `10/10` · 3.14 `10/10` · 3.15 `10/10` · 3.16 `11/11` · 3.17 `10/10` · 3.18 `10/10` · 3.19 `9/9` · 3.20 `10/10` · 3.21 `11/11` · 3.22 `12/12` · 3.23 `13/13` · 3.24 `12/12` · 3.25 `13/13` · 3.26 `15/15` · 3.27 `13/13` must gates，全部 `hold the contract`（3.23 曾因共享构建槽在 `conformance@2` 上假红，根因见 §5.2，已修） |
| 全步判定 | 在 `skip` 出的尾巴树上 `kiln verify jobq --slow --json` → **`{passed: 27, failed: 0, blocked: 0, not_authored: 0, clean: true}`**（22 步 3 m 17 s、23 步 6 m 1 s、24 步 5 m 15 s、25/26 步 5–6 m、27 步 5 m 级；3.25 的 `mutants` `6/6` 38 s、3.26 `6/6` 26 s、3.27 `6/6` 13 s）。六个步骤的 `slow:conformance@2` 都是 `2 backend(s) pass the same suite: memory ✓, postgres ✓`；**3.20 的 `slow:mutants@80` 是 `6/6 mutant(s) killed (100%, needs ≥80%) (the walk took 9s)`** |
| 默认特性全量套件 | `cargo test -p jobq` → **210 passed · 0 failed**（81 → 99 → 116 → 134 → 154 → 164 → 173 → 187 → 199 → 210）|
| postgres 后端子集 | 真正建 `PostgresStore` 的那 **10** 个规格（`authz cli dedup dlq isolation leases port_conformance state_machine timers tracing`）跑在 `KILN_BACKEND=postgres` → **71 passed · 0 failed**。`spec_fairness` 只建内存存储，不该算进来（§4.2 早先写的“11 个规格 → 75”把名单当成了后端口径，已改正）|
| 无特性装配 | `cargo test -p jobq --no-default-features --test spec_port_conformance` → **8 passed**（修复前是 `E0432`，见 §5.2）|
| 课程自己的检查 | `lint-caps` → `no issues.`（**97 capabilities · 136 slots · 119 ledger positions · 73 authored steps**，`checked 70 of 73 step bodies`）；`probe --check` → `40/40`；`links` → **84 page(s) · 449 link(s) · 279 external target(s)**，`✓ every target resolves`；`i18n status` → `structure: 0 error(s)`，`drift: 0 warning(s)` |
| CI | **`cargo xtask ci` → exit 0 · 32 m 12 s · `xtask ci: all checks passed` · `27/27 step(s) hold the contract`**（逐条走 mailer/relay/jobq 三条 track）|

### 4.3 验收演练（已做两腿，原始输出）

在 `skip` 出的尾巴树上（`/private/tmp/fixcheck2`，等于 `app/jobq` 的参考解状态）按 §4.1 的做法：每次只改一处，
跑判定，再回滚，再跑同一个命令。

**a) `semver@baseline`（第 3.2 步）：删掉一个被见证文件钉住的公开 `impl`。**
劣化：删掉 `impl Default for JobId`——仓库里没有任何代码用它，所以只有见证文件会注意到。

```
✗ semver@baseline the course's public-API witness (api/3.2-public-api-baseline.rs) no longer builds
    against this tree, which is a breaking change to `jobq`'s public API:
    error[E0277]: the trait bound `JobId: Default` is not satisfied
      --> tests/semver_baseline.rs:64:27
    64 | const _: fn() -> JobId = <JobId as Default>::default;
7/9 must gates passed · … ✗ 2 must gate(s) did not pass
```

（第二个红的是 `clippy@2` 的 `new_without_default`，删 `Default` 的附带后果；两者都点出了位置。）
回滚后同一个命令：`9/9 must gates passed`、`✓ semver@baseline … still compiles`。

**演练顺手查出来的边界（写下来免得下一个人重查）**：把 `Engine::submit` 的参数从 `SubmitRequest` 放宽成
`impl Into<SubmitRequest>`，这个门**不会**变红。原因是陈述而不是实现：门判的是“见证文件还能编过”，而这个改法对
所有可能存在的调用方都是源码兼容的——`Engine` 的五个方法因为 `async fn` 的返回类型不可命名，只能用闭包包住
future 来钉；`Store` 的五个方法是真的函数指针强转（line 136–140），比它严得多。所以“参数变成泛型”在**这个 API
形状下**不是破坏性改动，门不红是对的。

**d) `slow:conformance@2`（第 3.5 步）：让两个后端对同一个问题给不同答案。**
劣化：把 durable `claim` 的**外层**（决定答案顺序的那一层）排序从 `run_at asc` 改成 `run_at desc`。

- 后端各自跑同一条命令：`KILN_BACKEND=memory` → `8 passed · 0 failed`；`KILN_BACKEND=postgres` →
  `7 passed · 1 failed`，失败信息是 `then the job that has been due the longest: …  left: JobId(1288ea57…) right:
  JobId(dc63d344…)`。
- 慢闸门本身：`kiln verify jobq --slow` → `14/15 judged step(s) pass · 1 failed`，红的那一步是
  `✗ slow:conformance@2 the suite fails on 1 of 2 backend(s): memory ✓, postgres ✗ (exit code 101: … panicked at
  tests/spec_port_conformance.rs:231:5)`。注意同一步的 `spec ✓`——**只跑内存后端的快速路径是绿的，是慢闸门抓到了分歧**。
- 回滚后同一个命令：`15/15 judged step(s) pass · 0 failed`。

**b) `slow:mutants@80`（第 3.20 步）：把一步的规格掏空。**
3.20 声明这个门，它的模块是 `src/engine/mutation.rs`（策略、分数、报告）。劣化：把裁决那套断言的规格换成一份
只“碰一下”模块的最小规格（`Score::new(0,0).judged()` 与 `MutationPolicy::new(80,1).threshold()`），
所以被抽样的变异体没有任何断言可撞。原始行（`verify jobq --slow --json` 的 3.20）：

```
totals: {passed: 19, failed: 1, blocked: 0, not_authored: 7}
== jobq/3.20-mutation-threshold: failed — the specs pass but a must gate does not
   ✗ slow:mutants@80 (must): 6 of 6 mutant(s) survived (0% killed, needs ≥80%) (the walk took 10s)
     — no test noticed src/engine/mutation.rs:74:21 `self.killed + self.survived` (`+` → `-`);
       src/engine/mutation.rs:149:9 `!(self.min_judged == 0 || self.threshold > 100)` (`!` → `nothing`);
       src/engine/mutation.rs:149:27 `!(…)` (`==` → `!=`); src/engine/mutation.rs:149:30 `!(…)` (`0` → `1`);
       src/engine/mutation.rs:149:50 `!(…)` (`>` → `>=`);
       src/engine/mutation.rs:162:41 `let floor = self.min_judged.max(1);` (`1` → `0`)
```

**对照实验**：同一棵树、同一条命令，规格完好时是 `20/20 judged · 0 failed`，而 3.20 自己的门是
`6/6 killed (100%)`。门不但红，还把每个存活变异体的**文件:行:列**、原文与改写都点名了。

**两次失败的尝试也值得记下**（都是“演习自己脏了树”）：① 只掏空两个用例不够——规格本身有冗余，别的用例仍会杀掉同一个变异体（这是好事，说明套件不是只有一处关注）；
② 掏空时留下了：双空行→`fmt` 红，未用导入→`clippy` 红，误伤了辅助函数→整个树编不过。在**共享树**上做演习，先把自己弄得 lint-clean，
否则你读到的是别的门。附带收获：树编不过时，`mutants` 拒绝判分，说“the spec does not pass on the project as it is, so no mutant can
be judged”（它不会把编译错误当成杀死）。

**e) `unsafe-budget@0`（第 3.24 步）：往生产代码里加一个没有理由的 `unsafe` 块。**
劣化：在 `src/lib.rs` 追加一带有 `#[allow(unsafe_code)]`、**没有** `// SAFETY:` 的 `unsafe` 块。
`verify jobq --slow` 只有 3.24 一步红，三层防线依次亮（原始行）：

```
slow:bench-budget@10 (must): 2 of 3 metric(s) moved out of budget: unsafe_occurrences is 1, 1 of the
  baseline 0 (more than ±10%); undocumented_max is 1 and has to stay ≤ 0 (it is an upper bound)
unsafe-budget@0 (must): 1 `unsafe` occurrence(s) in production code and this step allows 0: src/lib.rs:21
spec ✗ … test the_crate_holds_its_own_budget … FAILED
```

（编译器那一层也亮：没有 `#[allow]` 时 `cargo check` 直接报 `usage of an unsafe block … -D unsafe-code`。）
补上 `// SAFETY:` 之后 `unsafe_occurrences=1`、`undocumented_max=0` —— **仍然 Over**，也就是说贵的是那个数字，不是注释。
回滚后 spec `8/8`、harness 三个零、门全绿。

**c) `slow:bench-budget@10`（第 3.22 步）：把被预算盯着的路径换成 O(n²) 扫描。**
劣化：`Pull::next` 每次发放都从批次开头重扫（丢掉游标），**答案不变、代价变了**。原始行：

```
✗ slow:bench-budget@10 (must): 2 of 3 metric(s) moved out of budget: scan_cells is 32896, 32640 of
  the baseline 256 (more than ±10%); scale_ratio_max is 3.992 and has to stay ≤ 2.2 (it is an upper bound)
```

同一棵树、同一条命令的对照：`3 metric(s) within ±10% of the course baseline: claim_statements 192,
scan_cells 256, scale_ratio_max 2` ✓。三点值得记：
① 这次劣化同时被另外两道门抓到——规格（`the_pull_remembers_where_it_stopped` 红：计数器的语义是规格内容的一部分）
与 `clippy`（我的循环用了切片下标）。3.22 的预算内容被**三重锚定**（代码常量、页面上的数字、harness 的负载），
而只有预算门知道**基线**，所以只有它能给出“这个指标离开预算多少”的判决。
② 只把 harness 的负载 256 改成 512（“负载涨了但没人重测预算”）也能让门红
（`claim_statements is 384, 192 of the baseline 192`、`scan_cells is 512, 256 of the baseline 256`），
代价是同时触发页面对照的规格用例——那是课程有意的性质，不是缺陷。
③ 两次演习都在共享树里，所以先把自己弄干净：`fmt`/`clippy` 一旦被演习本身弄红，读到的就是别的门。

### 4.4 `selftest` 为什么越来越慢（实测、根因、三步修法）

3.20 起每一步都会声明 `slow:mutants@80`，于是这条慢路径的成本变成了要盯的问题。
在一棵不带 `mutants` 的树上先取基线（都是热缓存、同一台机器、`time` 量的墙钟）：

| 命令 | 慢 | 说明 |
| --- | --- | --- |
| `selftest jobq/3.5-store-port-conformance`（真课程，11 个 must 门） | **19.7 s** | setup 只做补丁应用（不跑早期步骤的规格/门），大头是建项目 + 本步的门 |
| `selftest jobq/3.15-cli-contract`（真课程，10 个门） | **25.6 s** | 步数增长带来的只是项目变大 |
| 同一棵树 + 3.5 声明 `slow:conformance@2`（起容器） | **1 m 23 s** | 两后端套件是真正的大头之一 |
| 干跑：把 `slow:mutants@80` 加到 3.5 上 | **>30 min，两次，无结果** | 见下 |
| 同上，修完之后 | **1 m 23 s**（12/12 绿） | 同一命令、同一缓存 |

根因有三层，按影响排序：

1. **超时只杀了直接子进程，孙进程变成了孤儿**（`kiln/src/process.rs`）。`cargo test` 是两个进程，干活的是下一层的
   测试二进制。一次卡住的规格让那个二进制活了 **40 分钟**：占一个核，而且**握着共享构建目录的锁**，于是紧接其后的
   每一次命令、同一台机器上每个 worker 的构建都在等锁、都显得更慢——每卡一次再漏一个，这就是“越来越慢”的真身。
   修法：先收集子进程的整棵后代树（`ps -A -o pid=,ppid=`）、再一起杀（`kill -KILL`），两个都是 POSIX，std 里没有
   发信号给别的 pid 的办法。新增回归测试：`sh -c 'sleep 37'` 被超时之后，`ps` 里不能再有任何 `sleep 37`。
2. **一个变异体能让规格卡死，而当时唯一的防线是全局 `--timeout`（默认 300 s）× 最多 12 次尝试 = 一小时**。
   实测的卡死点：`MemoryStore::claim` 里 `&&` → `||` 让过滤条件对**所有**行成立，规格那个
   `loop { claim; if empty break }` 永远拿不空——在 `spec_port_conformance.rs:521` 处 `park`。
   修法：单个变异体限 `4 × 基线 + 15 s`（再被 `--timeout` 封顶——变异体只差一个算子，四倍还不够就是卡住），
   整轮限 `WALK_BUDGET = 300 s`；两者都在报告里写明（`did not finish within 25s`、`the walk took 60s`）。
3. **取样里大多数位置根本编不过**：类型里的尖括号（`Vec<JobRow>`、`impl<S: Store>`、`BoxFuture<'a, …>`）、
   `format!` 的 `!`、闭包的 `||`、match 臂的 `=>`、构造子改写（`Some(`→`None(` 是 `E0532`）。干跑里 12 次尝试
   只有 3 次判成，`blocked`/低分母让闸门看起来像噪声。修法见 §1.3；顺带把报告从 `文件:行` 改成 `文件:行:列`
   （`a && b && c` 是两个变异体，之前两者在报告里长得一模一样）。

修完之后同一命令的原始行：

```
1 of 6 mutant(s) survived (83% killed, needs ≥101%) (5 of 12 attempts could not be judged: no build,
or no finish; the walk took 60s) — no test noticed src/engine/store/postgres.rs:92:40
`if request.max_attempts == 0 {` (`0` → `1`)
    src/engine/store/memory.rs:59:47 `&&` → `||` — did not finish within 25s
    src/engine/store/memory.rs:59:80 `&&` → `||` — did not finish within 25s
    src/engine/store/mod.rs:41:60 `+` → `-` — does not build: error: expected `while`, `for`, `loop` or `{` after a label
```

（那句话里 `needs ≥101%` 是为了把报告逼出来而临时改的阈值；实时门是 `@80`，同一轮结果 `83% killed` → 绿。）

给后续步骤作者的推论：从 3.20 起，一个声明 `mutants` 的步骤 `selftest` 大约多花一分钟——前提是它的规格能在一个
算子变错后**在一分钟内失红**，而不是永远等下去。规格里任何“等到空为止”的循环都要有界；这是 3.20/3.21 的步骤书要写死的一条。

## 5. 缺口

### 5.1 probe 覆盖（T3 新增槽位）

题库从 19 道扩到 **40 道**（`probe --check` → `40 exercise(s) · 40 reference(s) passed`），
T3 新增或加深且能用单文件 `std` 诚实地考的槽位都有了题（包括 `obs/metrics-slo@design`、
`obs/structured-logs@design`、`obs/tracing-propagation@operate`、`db/schema-online-evolution@apply`、
`delivery/idempotency@design` 这五道后补的）；**没出题的槽位与理由**
（与 handoff-m1-log.md 第七段同一口径：当单文件只能把机制换成假件时，那道题就是假证据，比没有更糟）：

| 槽位 | 为什么没有题 |
| --- | --- |
| `api/library-design@aware`（3.1） | `aware` 按政策不出题（curriculum §11） |
| `err/typed-errors@design`（3.3） | 增量（`#[non_exhaustive]`、Display 快照）已被 `api-library-design.apply` 从表面侧盖住 |
| `cfg/layered-config@design`（3.4） | 机制是 cargo 编出一个 feature 矩阵，单文件只能给一组 feature 解出一个配置 |
| `db/transactions@design`（3.6） | 注入序列化冲突 + 有界重试，与 `delivery-retry-budget.design` 重叠（见 §1.3 的写法：要补就只写「注入冲突下收敛」这一半）|
| `async/graceful-shutdown@operate`（3.7） | operate 的增量是跨进程（滚动升级下的 drain、双领窗口），单文件只能假装第二个进程；3.7 的租约那一半已由 `db-atomic-claim.design` 承担 |
| `http/rate-limit@design`（3.9） | 同一步的公平调度已有 `sched-fairness.design` |
| `sec/authz-tenant-isolation@operate`（3.13） | 配额那一半是 `sched-fairness.design` 的，剩下的是计数报表 |
| `delivery/dlq@operate`（3.16） | 与 `ops-dlq-tooling.apply` 同一步同一件事 |
| `ops/runbook-drill@design`（3.27） | 证据是一页文档 + 对它的评审，探针只能查步骤表的形状 |

### 5.2 其他

- **durable 那一半套件需要 `--features postgres`，这是设计而不是缺陷。** 15 个 spec target 里有 9 个按设计点名
  `PostgresStore`（`spec_authz`、`spec_cli`、`spec_dedup`、`spec_isolation`、`spec_leases`、`spec_state_machine`、
  `spec_timers`，以及 3.23 起会新增的），所以 `cargo test -p jobq --no-default-features` 永远编不过它们——规格是钉死的
  文本，“两个后端对同一问题给同一个答案”这件事必须点名背单词。夹具不同：3.5 的 `tests/support/store.rs` 已经改成在
  任何装配里都能编译（导入、`Backend::Postgres` 变体与它的分支加 `#[cfg(feature = "postgres")]`，提交
  `course(jobq/3.5): the conformance fixture has to compile in every assembly`），默认后端 `memory` 在无特性装配里
  可跑（`spec_port_conformance` 8 passed，修复前根本不过编译）。生产项目里表达这条边界的机制是 Cargo 的
  `[[test]] required-features = ["postgres"]`；这里没做，因为那要一份随每一步成长的 target 清单，是课程设计决定，
  留给 M2 之后（3.24/3.25 的步骤书里不要再把它当成缺陷重报）。
- **`kiln verify <track>` 判的是 `app/<track>` 里的真实项目**：worktree 里没有 `app/` 时逐个报
  `app/jobq does not exist yet`（`0/0 judged`）。取“全步判定”的数字要先铺树（§4.1 的第 1 步），再在那棵树里跑；
  本次是 15/15。
- **历史噪声未清理**：`9d9cceb..` 之间曾有 5 个 `wip(...)` staging 提交（作业者拿提交当检查点），最终树干净、补丁可
  精确重现；要发布就把那 9 个提交压成两个步骤提交——**未经确认不要重写 `main`**（它会同步到用户主检出）。
- **真缺陷（已修，提交 `jobq(3.9): the cap bounds the rows a claim takes, not just the count`）**：**durable 的每队列在飞上限（cap）没有真正限住一次领取的行数**。`postgres.rs` 的 cap 判据是
  `where` 里的**非相关**子查询（`(select count(*) from jobs running where …) < $6`），它只被求值一次，`limit $3` 于是把**所有到期行**拿走；
  内存后端边取边计数，只取到上限。复现（3.26 作者）：两条重放的死信 + `Policy::strict().with_cap(1)` + `claim(8)` → postgres 返回 **2**，memory 返回 **1**。
  `jobq/3.9` 的规格只建内存存储，所以连它的 `slow:conformance@2` 也抓不到。
  **修法（已落地）**：让 `limit` **容量感知**——3.9 的语句取 `least($3, case when $6 = 0 then $3 else greatest(0, $6 - (select count(*) … running …)) end)`，
  3.13 引入 owner 上限后在同一行再接一项；给 3.9 的规格补了一条**两后端都跑**的用例（`the_cap_bounds_the_rows_one_claim_takes`，经 conformance 夹具建库）。
  **复核**：`selftest jobq/3.9` → `11/11 must green`（含 `slow:conformance@2`：memory ✓、postgres ✓）、`selftest jobq/3.13` → `10/10`。
  **留下的窄窗口**：两条**并发**的 claim 仍可能各自读到同一份余量（语句级快照），因此队列瞬时可能超出上限一行；
  彻底修需要按队列串行（例如先 `select … from queues for update`），超出本次范围，记在这里。
  这条也让一个设计代价变具体了：“`conformance` 只写在判端口的步骤上”——3.9 的规格本来只建内存存储，于是**对后端的承诺没有人判**。
- **`ops/dlq-tooling@operate` 没有同深度的探题**（3.26 作者发现）：`kiln plan` 自己说得很清楚——"there is no exercise at operate; the closest is
  `kiln probe ops-dlq-tooling.apply` (apply), which re-checks the habit and not this depth"——所以 `kiln skip jobq/3.26-dlq-cli` 必须带 `--force`。
  §5.1 的探题表里没有这个槽位的行（`probe --check` 仍是 40/40）。要么补一道 operate 深度的题，要么在 §5.1 里写明为什么没有。
- **探题的句子与门不一致（收尾时对齐）**：`course/probes/unsafe-budget.apply.rs` 写着“`// SAFETY:` 在**同一行或上面三行**”，
  而 3.24 的规格与 `unsafe-budget@0` 门都要求**在上面**三行内。探题从不测同行注释，所以没有红；对齐它要么改那句话、要么改门的判据（建议改探题那句话，因为门与项目的 `// SAFETY:` 习惯一致）。
- **3.23 的 `slow:mutants@80` 是 `5/6 (83%)`**（过线但在边上）：它的模块里有一个变异体没有规格能杀。留给收尾时决定是否补一条断言。
- **`selftest` 与 `verify` 在 3.23 上不一致——已找到根因并修好**（提交 `kiln(mutants): the copy builds in a
  directory of its own`）：报出来的现象是 `verify --slow` 让 3.23 全绿、而 `selftest` 两次都在
  `slow:conformance@2` 红（两个后端都停在页面读取）。根因不是“解法没落地”（我当时的怀疑），而是**共享构建槽**：
  cargo 按包名+版本选槽，`slow:mutants@80` 在**副本**里构建，它的二进制却落在沙箱共享槽里；紧随其后的
  `slow:conformance@2` 会再跑一次本步命令，于是执行了**副本的二进制**，而那份二进制的 `CARGO_MANIFEST_DIR`
  指向一个已经被删掉的 `kiln-mutants-*/project` → 读不到本项目的页面。3.25 的作者用两个实验定死了这一点
  （沙箱里的页面其实全程都在；只把 mutants 的最后一次“恢复运行”挪进项目，规格就改在**行为**断言上红，
  说明问题在二进制/槽而不只是路径），并验证了修法。我落地并复核：mutants 的副本现在建在
  `<cache>/kiln/mutants-target`（与 `coverage` 的插桩目录同一规则），那一轮“清理运行”因此不再需要；
  `selftest jobq/3.23` 由红转 **`13/13 must green`**，`jobq/3.24` 保持 `12/12`。
- **一条给后来者的一般规律**（§3 也有）：**任何“在副本里跑 cargo”的装置都必须给副本自己的构建目录**，
  否则它污染共享槽、让下一个跑同一条命令的人执行别的树的二进制。判据：命令的 `CARGO_MANIFEST_DIR`
  指向哪里、那个目录是否还存在。
- **构建槽共享导致的幽灵失败已从 kiln 侧封掉**（提交 `kiln(process): the project's build directory is not the caller's`）：
  cargo 按**包名+版本**选构建槽，不按目录，所以两棵树共用一个 `CARGO_TARGET_DIR` 时，一棵树的二进制会服务另一棵；
  而这门课的规格用 `CARGO_MANIFEST_DIR`（编译期烘进二进制）读项目文件，于是"陈旧二进制"不是跑旧代码，而是**读别的项目的文件**
  （实测：`docs/budget.md` “cannot be read: No such file or directory”，而文件就在，源码也没变；换一个干净的构建目录就绿）。
  kiln 现在把**自己启动时带的 `CARGO_TARGET_DIR` 从每个子进程里清掉**：命令要么建在项目自己的 `target/`，
  要么建在沙箱共享目录（`selftest`/`probe` 仍然共享，这是刻意的），闸门自己点名的目录（`coverage` 的插桩构建）优先于两者。
  复核：带一个外来 `CARGO_TARGET_DIR` 跑 `kiln verify jobq --json`，改前 `20/22`（两例幽灵失败），改后 **`22/22`**。
- **已收口的缺口**：3.15 的 FURTHER 与 `src/cli/mod.rs` 的模块文档曾把死信命令行许给 3.16（而 3.16 只交端口那一半）——
  现在两处都指向 `jobq/3.26-dlq-cli`（提交 `course(jobq/3.15): the dead-letter CLI is its own step…`），
  探题 `ops-dlq-tooling.apply` 描述的面也将在 3.26 交付时对齐。
- **派单经验（三次死亡后总结）**：**一次派一步**。两批“两步一个任务”的作业者都无声死掉（3.16/3.17 那对在 rustfmt 附近停下、
  留下一个 `dlq.rs` 草稿；3.23/3.24 那对什么都没留下），而单步重派都交付了（3.16、3.23）。任务书里已写死：每个产物一做好就提交、
  长命令输出重定向只看尾部、卡住就停并报告。最后一批（3.25、3.26、3.27）按单步派。
- **M2 收尾（已做完的）**：验收演练五条腿全有原始行（§4.3）；`cargo xtask ci` → exit 0（`27/27 step(s) hold the contract`，32 m 12 s）；
  `docs/t3-jobq.md` 已写（与 `t1-mailer.md`/`t2-relay.md` 同构）；`README.md` 的 track 表与 Status、`plan.md` §11、`curriculum.md` §8 的数字已同步。
- **M2 收尾（仍未做/未定）**：① 历史上那 5 个 `wip(...)` staging 提交要不要压掉（**要用户确认**，会重写 `main`）；
  ② `ops/dlq-tooling@operate` 的同深度探题（要么补一道，要么在 §5.1 写明为什么没有）；③ `coverage` 装上 `llvm-tools-preview` 后那条测量路径的端到端验证；
  ④ 3.23 的 `slow:mutants@80` 83% 要不要补一条断言（过线但在边上）。
