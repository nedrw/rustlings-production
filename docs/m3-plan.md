# M3 计划：T4 `lakelet`（24 步）+ 容器与性能基建

> 状态：**已开工**——§7 的六个决策点已由开发者拍板（全部按推荐：D-1(a)…D-6(a)），施工记录按 M2 的习惯写进
> [handoff-m3.md](handoff-m3.md) 与 [handoff-m3-log.md](handoff-m3-log.md)（收尾时产出）。
> 开工后的实测若与本文不符，以本文为准改实测——已改过两处：`dtrace` 其实存在（是非 root 拒绝，见 §5 C5）；
> `fuzz` 的语料落在项目的 `tests/corpus/**` 里、由学员自己写，而不是钉在规格里（见 §4.3：目录方案不需要
> “课程→项目”的复制机制，而把那个字节钉进规格等于把答案摊在学员面前，与这一步的过关条件冲突）。
>
> 入口上下文：plan.md §6（闸门）/§7（课程体系）/§11（里程碑）、curriculum.md §9（T4 的 24 行）、
> [handoff-m2.md](handoff-m2.md)（M2 的契约与实测数字，§1 的三条原则、§3.5 的步骤作者作业书）、
> [t3-jobq.md](t3-jobq.md)（上一条 track 的规格写法）。

## 0. 范围与验收（对着 plan.md §11 的 M3 行写）

| 验收项 | 判据 |
| --- | --- |
| T4 `lakelet` 24 步 | 正文、只读规格、加性 scaffold、参考解、闸门接线全部就位；逐步 `kiln selftest` 绿，`kiln verify lakelet --slow` 在尾巴树上全绿 |
| 三条新慢闸门 | `slow:sim@N`、`slow:soak@N`、`slow:fuzz@N` 由 `kiln` 自己判定（课程侧参数文件 + 计数/摘要，不看墙钟），缺工具时不改判 |
| 容器与性能基建 | 镜像盘点去重与“能不能真起容器”的**运行**探测；`kiln skip --track`；剖析路线在 `doctor` 里报出；无网快路径可复现 |
| **无网快路径** | `CARGO_NET_OFFLINE=true` + 镜像已预拉 + 私有 `XDG_CACHE_HOME` 下，T4 的快路径（不含 `--slow`）能跑完并通过 |
| **soak 抓得住泄漏** | 往参考解注入一处任务/句柄泄漏 → `soak` 变红并**点名是哪个计数器**；回滚后同一个命令变绿 |
| 收尾 | `cargo xtask ci` exit 0 且最后一行是 `24/24 step(s) hold the contract`；`docs/t4-lakelet.md` + `handoff-m3.md` + 日志；`main` 推到 `local` |

## 1. 环境早验（这一轮实测出来的，计划里不假设）

| 项 | 结果 | 命令 / 证据 |
| --- | --- | --- |
| crates.io 可达 | ✓ | `cargo fetch` 在 `/private/tmp/turmoil-probe` 成功 |
| `turmoil 0.7.2` | **取得到、编得过**：48 个包，冷构建 **8.3 s** | `cargo fetch` + `/usr/bin/time -p cargo build`，rustc 1.98.1 |
| `arrow 56 + parquet 56`（`default-features = false`，ipc/arrow/snap/zstd） | 108 个包，冷构建 **16.6 s**，73 个 rlib（含 `libparquet`、`libzstd_sys`） | 同上 |
| `datafusion 55`（无默认特性 + parquet） | 240 个包，冷构建 **62.1 s**，194 个 rlib；**它自己拉 parquet 59**，与 `arrow 56` 版本错位 | 同上 |
| MinIO 镜像 | **拿不到**：`minio/minio:latest` 直连 docker.io → `requested access to the resource is denied`；`docker.1ms.run/minio/minio` → `manifest unknown`；`docker.m.daocloud.io/minio/minio` → `denied`（各 2–4 s 就回错） | 三条路由都试过，见 §7 D-3 |
| 本机镜像清单 | 15 个引用：`postgres:17-alpine`、`alpine:3.20`、`debian:bookworm-slim`、`rust:1.98-alpine/slim`、`rust:1.88-bookworm`、`axllent/mailpit:v1.21`、两个 `localhost/kiln-*`、两个 `<none>`；**没有对象存储** | `docker images` |
| 本机剖析工具 | `sample`（macOS 自带）**不需要 sudo**，实测对一个用户进程 2 秒采样出了 68 917 字节的可读报告；`dtrace`（`/usr/sbin/dtrace`）**存在但非 root 会拒绝**：`dtrace: failed to initialize dtrace: DTrace requires additional privileges` → 装着的 `cargo-flamegraph` 在本机是陷阱；`samply`/`perf` 未装 | `/usr/bin/sample <pid> 2 -file …`（exit 0）、`dtrace -n 'BEGIN { exit(0) }'` |
| 默认工具链 | `rustc/cargo 1.98.1`（与 `rust-toolchain.toml` 同一把），所以上面三条依赖探针的数字就是课程工具链的数字 | `rustc --version`（在 `/private/tmp`） |
| `kiln images` | 每个**步骤**一行（`postgres:17-alpine` 重复 30+ 行），没有去重、没有“离线可行”汇总 | `kiln images` |
| `kiln skip` | **没有 `--track`**，只有 `--all`（走默认 track）——M2 §4.1 铺尾巴树要按路径序逐个 skip，就是这个原因 | `kiln skip --help` |
| `kiln verify --slow` 的帮助文本 | 已经写着 `(mutants, coverage, soak, …)`：框架早就为 `soak` 留了位置 | `kiln verify --help` |

## 2. T4 的 24 步（槽位 / origin / 门 / 模块边界）

来源是 curriculum.md §9 的 24 行与 capabilities.toml 里 `lakelet/4.*` 的 **29 个槽位**（R0 要求逐槽一致）。
`origin` 分布：**16 teach + 1 transfer + 7 deepen**（T3 是 11+1+14）。基线门 = T3 的终点集合，见 §6。

| # | 步骤 id | 槽位 | origin | 这一步新增的慢门 | 模块边界（谁在这一步出现） |
| --- | --- | --- | --- | --- | --- |
| 4.1 | `4.1-ingest-protocol` | `ingest/protocol@apply` | **transfer** | — | `src/protocol/**`（长度前缀 + 校验 + 版本字）、`src/service/{config,logs,ingest}.rs`、`src/bin/{lakeletd,lakelet-load}.rs`、`tests/support/**`、`docs/protocol.md`；骨架 = 整个项目 |
| 4.2 | `4.2-parser-fuzz` | `test/fuzzing@apply` | teach | `slow:fuzz@N` + `slow:?coverage@80` | `tests/fuzz_protocol.rs`（课程装置）、`src/protocol/mutate.rs`、学员的回归语料 `tests/corpus/**` |
| 4.3 | `4.3-write-path-adr` | `store/wal-recovery@aware` | teach | — | `src/engine/wal/policy.rs`（fsync 策略作为数据，类型名是 `Policy`；`Policy::DEFAULT = interval(50ms)` 由实测推出）、`src/engine/spool/durable.rs`（scaffold 新建的契约文件：六个方法签名 + `todo!()`，因为骨架的 `spool.rs` 不能被插入可被改写的桩——见 authoring.md §5 规则 6）、`docs/adr/0001-write-path.md`、`docs/crash-points.md` |
| 4.4 | `4.4-wal-recovery` | `store/wal-recovery@apply` | deepen | `slow:sim@3` + `slow:mutants@80` + `slow:?coverage@80` | `src/engine/wal/{mod,segment,replay,checksum}.rs`、`src/engine/spool/recover.rs`（scaffold 新建的契约文件：`Spool::recovered`，见 authoring.md §5 规则 6）、`tests/sim_wal.rs`（turmoil 场景，四种死法：进程/机器/撕裂/改写）、崩溃点语料 |
| 4.5 | `4.5-batch-writes` | `io/batching@design` | teach | `slow:bench-budget@10` | `src/engine/flush/{mod,batcher}.rs`、`src/engine/spool/batch.rs`（`append_batch` 要动的私有字段在 `spool.rs` 里，见 authoring.md §5 规则 6）、`tests/budget.rs`（课程装置）、`budget/4.5-batch-writes.toml`（基线＝参考解实测）、`docs/write-amplification.md` |
| 4.6 | `4.6-ingest-backpressure` | `async/backpressure@design` | deepen | `slow:sim@3` | `src/service/backpressure.rs`（有界队列 + `Block/DropNewest/DropOldest/Sample` 策略 + `Stats`/`Tally`；**队列的单位是帧**，因为 4.1 的承诺是逐字节的）、骨架 `src/service/config.rs` 的两个标记块与 `queue.{capacity,policy}` 四层配置（默认 `1024` / `block`）、`tests/sim_backpressure.rs` |
| 4.7 | `4.7-columnar-segments` | `store/columnar@apply` | teach | `slow:bench-budget@10` + `unsafe-budget@0` | `src/engine/segment/{mod,encode,decode,compress}.rs`、`docs/format.md` |
| 4.8 | `4.8-compaction` | `store/compaction@design` | teach | `slow:sim@3` + `slow:bench-budget@10` | `src/engine/compaction/{mod,plan}.rs` |
| 4.9 | `4.9-retention` | `store/retention@apply` | teach | `slow:bench-budget@10` + `slow:mutants@80` | `src/engine/retention/{mod,partition}.rs` |
| 4.10 | `4.10-downsampling` | `query/downsampling@apply` | teach | `slow:bench-budget@10` | `src/engine/query/downsample.rs`、`src/cli/**`（`lakeletq` 在这里落地） |
| 4.11 | `4.11-query-engine` | `query/engine@design` | teach | `slow:mutants@80` + `slow:?coverage@80` | `src/engine/query/{mod,lex,parse,plan,exec}.rs`、`docs/query.md` |
| 4.12 | `4.12-aggregation-correctness` | `num/aggregation-correctness@apply` | teach | `slow:mutants@80` + `slow:?coverage@80` | `src/engine/query/agg.rs` |
| 4.13 | `4.13-model-based-tests` | `test/model-based@apply` | teach | `slow:sim@3` + `slow:mutants@80` | `tests/support/model.rs`（学员的参考模型）、`src/engine/rng.rs`（自研种子 RNG，按 T3·3.19 的先例） |
| 4.14 | `4.14-index-pruning` | `query/index-pruning@apply` | teach | `slow:bench-budget@10` + `slow:mutants@80` | `src/engine/index/{mod,zone,bloom}.rs` |
| 4.15 | `4.15-budget-regression` | `perf/bench-budget@operate` | deepen | `slow:bench-budget@10`（逐阶段归因） | `src/engine/trace/counters.rs`、`tests/attribution.rs`、`docs/perf.md` |
| 4.16 | `4.16-profiling` | `perf/profiling@operate` | teach | —（判据复用 4.15 的归因计数 + 页面） | `docs/profile.md`、`lakelet-load --profile`（采样脚本入口） |
| 4.17 | `4.17-soak` | `perf/soak@apply` | teach | `slow:soak@N` | `tests/soak.rs`（课程装置）、`docs/soak.md` |
| 4.18 | `4.18-cardinality-cost` | `obs/cardinality-cost@apply` | teach | `slow:soak@N` | `src/engine/cardinality/{mod,cap}.rs`、`docs/cardinality.md` |
| 4.19 | `4.19-backup-restore` | `ops/backup-restore@apply` | teach | `slow:sim@3` + `slow:mutants@80` | `src/engine/backup/{mod,manifest,restore}.rs`、`src/cli/backup.rs`、`docs/dr-drill.md`、`tests/sim_backup.rs` |
| 4.20 | `4.20-capacity-model` | `ops/capacity-cost@design` | teach | `slow:bench-budget@10` | `src/engine/cost.rs`、`docs/capacity.md` |
| 4.21 | `4.21-cold-tiering` | `store/tiering@apply`、`outbound/http-client@design` | deepen | `slow:conformance@2` + `slow:mutants@80` | `src/engine/tier/{mod,filesystem,s3,http}.rs`、`tests/support/tier.rs`、`conformance.toml`、`tests/s3_stub.rs` |
| 4.22 | `4.22-reproducible-build` | `build/reproducibility@apply`、`repo/toolchain-pinning@design`、`ci/pipeline@design`、`supply/deny-policy@design`、`release/container-image@design` | deepen | —（快门的 `container-repro` + `images` 在这里第一次上场） | `Dockerfile`、`deny.toml`、项目自己的 `rust-toolchain.toml`、`.github/workflows/**`、`scripts/repro.sh`、`docs/build.md`、`docs/supply-chain.md` |
| 4.23 | `4.23-secret-rotation` | `sec/secrets-management@design` | deepen | `slow:sim@3` | `src/service/keys.rs`（重叠有效期的钥匙环）、`src/cli/rotate.rs`、`docs/rotation.md`、`tests/sim_rotation.rs` |
| 4.24 | `4.24-runbook-drill` | `ops/runbook-drill@operate` | deepen | — | `docs/runbook.md`、`docs/slo.md`、`tests/runbook_harness.rs`（课程装置：照页面上的命令跑） |

三点说明：

- **4.1 是 transfer，理由与 3.1 相同**（第三个骨架连做第四遍：RECALL/DELTA 而不是 WHY），RECALL 点名的是它 `requires` 里的既有步骤
  （如 `cfg/layered-config@design — jobq/3.4`）。若你更想让它当 `teach`（协议解析确实是从零讲），改一行元数据 + 正文段标，见 §7 D-5。
- **4.21 是 deepen 而不是 teach**：它同时建立新槽位 `store/tiering@apply` 与深化 `outbound/http-client`（relay/2.7 → design），
  按 3.16 的先例取“更深的那一个”；RECALL 点名 `outbound/http-client@apply — relay/2.7`。
- **4.22 是 deepen**：它的五个槽位里四个是深化（mailer/1.2、1.3、1.21、relay/2.21），这正是“把 T1 做过的事做到 design 深度”。

## 3. 项目形状、数据模型与依赖决定

与 T1/T2/T3 同构，不重新发明（`app/lakelet` 是**一个包**，因为规格总是同步成 `<project>/tests/spec_*.rs`）：

```
app/lakelet/
  Cargo.toml          # name = "lakelet"：lib（引擎）+ 三个 bin
  src/engine/**       # protocol 之外的存储与查询：wal、flush、segment、compaction、retention、query、index、tier、backup、cost
  src/protocol/**     # 摄取协议（长度前缀 + CRC + 版本字）
  src/service/**      # 配置、日志、ingest 监听、flush 循环、背压、退出
  src/cli/**          # lakeletq：query / inspect / verify / backup / restore / rotate
  src/bin/            # lakeletd（摄取+查询守护）、lakeletq（CLI）、lakelet-load（负载/soak/剖析驱动）
  tests/spec_*.rs     # 课程拷入的只读规格（每步同步）
  tests/support/**    # 学员夹具（项目目录、固定时钟、工作负载、后端选择）
  tests/*_harness.rs  # 课程装置：budget / fuzz_protocol / soak / attribution / runbook_harness
  conformance.toml    # 4.21 起声明两个 tiering 后端
  docs/**             # protocol.md、format.md、adr/**、perf.md、profile.md、soak.md、capacity.md、runbook.md、slo.md、CHANGELOG.md
```

**数据模型一句话**：`Sample { series, ts_millis, value: f64 }`，`series` = 指标名 + 标签集（基数在第 4.18 步成为成本问题）；
摄取走**二进制分帧协议**（TCP），查询走**受限 SQL**（`SELECT agg(value) FROM metric WHERE … AND ts BETWEEN a AND b [GROUP BY time(5m), series]`），
用 `EXPLAIN` 把“裁剪了哪些段、读了哪些列、估计多少行”摆到台面上，非法查询**按类别拒绝**（未知指标、非法聚合、范围超保留期、窗口不对齐、边界倒置）。

**依赖决定（三条，都是早验过的数字换来的）**：

1. **不引 `arrow`/`parquet`/`datafusion`**：段文件用课程自己的列式格式（列式编码 + RLE/字典 + `zstd` 块压缩）。
   理由：4.7 的过关条件本来就只要“编码往返 + 体积对比”，而 4.11 要教的是**计划与裁剪可见 + 拒绝非法查询**——
   把 datafusion 接进来等于把这一步要教的东西交给别人的规划器（`api/library-reuse@design` 是 T5·5.20 的事）。
   `doctor` 与正文点名三家是生产做法（与 M2 对 `cargo-mutants`/`cargo-semver-checks` 的处理一致）。
   代价写清：我们放弃“能读真实 parquet 文件”这个卖点，换来零新依赖、离线可跑、每步冷构建不涨 60 秒。
2. **首次引入两个小依赖**：`crc32fast`（4.1 的帧校验）与 `zstd`（4.7 的块压缩）——两个都在本机 crate 缓存里，
   是生产里的正确答案，且都不是“替学员做题”的依赖。
3. **`turmoil 0.7` 只作 dev-dependency**（4.4 起）：`sim@seed` 的引擎。实测 48 包 / 冷构建 8.3 s；生产代码不依赖它，
   规格侧只通过 `KILN_SIM_SEED` 与环境说话（见 §4.1）。若某台机器取不到它，`kiln doctor` 会报，而**闸门本身不需要任何外部工具**。

## 4. 三条新闸门的契约

三条都遵守 handoff-m2 §1.1 的原则：**must 门只用这台机器一定有的东西判定**（编译器 + 课程自己的文件）、
**判据是计数/摘要而不是墙钟**、**参数在课程侧**（学员可读、改了没用），实现都在 `kiln/src/gates/`。

### 4.1 `slow:sim@N`（must，慢）——确定性模拟

课程侧文件 `course/tracks/lakelet/sim/<step-id>.toml`：

```toml
# deterministic-simulation scenarios for lakelet/4.4
command = "cargo test -p lakelet --test sim_wal"   # 省略时回退到本步的 run.cmd
seeds = [1, 7, 20260101]                            # 判据是“至少 N 个种子”，种子表本身也是判据的一部分
replay = 1                                          # 这个种子要跑两遍，两遍的摘要必须逐字节相同
```

闸门做四件事，全部与墙钟无关：

1. **逐种子跑**命令，环境里带 `KILN_SIM_SEED=<n>`（与 T3·3.5 的 `KILN_BACKEND` 同一套机制，学员已经见过一次）；每个种子的运行必须绿。
2. **重放**：`replay` 那个种子再跑一遍，两遍的 `lakelet-sim … digest=<…>` 行必须**逐字节相同**——这就是“同一 seed 重复跑结果一致”。
3. **反空转**：至少两个种子要给出**不同的**摘要（一个不看种子的场景不是模拟）。
4. 报告行点名：`3 seed(s) ran, digests 8f2a/1c07/1c07, replay seed 1 identical (the walk took 12s)`。

判 `Failed` 的三种情形（各带上下文）：某个种子跑红（点名种子与失败用例）、重放摘要不一致（两个摘要都给）、
所有摘要相同。课程侧文件缺失 → `blocked` 并点名它期望的路径（不猜）。
**降级**：`sim` 门自己不需要外部工具；项目若在无网机器上取不到 `turmoil`，那是这一步的构建失败（真红），
而 `doctor` 会提前报“`turmoil` 有没有在本机 cargo 缓存里”。

### 4.2 `slow:soak@N`（must，慢）——长跑无泄漏

课程侧文件 `course/tracks/lakelet/soak/<step-id>.toml`：

```toml
command = "cargo test -p lakelet --release --test soak -- --nocapture"
cycles_min = 4000        # 工作量地板：跑不满这个循环数就是红（判据是计数，不是分钟）

[metric]                 # soak 里每个指标都是**上界**（它们都是存量，不是吞吐）
open_tasks_max = 24
open_handles_max = 8
series_tracked_max = 500
rss_growth_bytes_max = 120000000
```

闸门（形状与 `bench-budget` 同构）：跑命令 → 解析 `lakelet-soak k=v` 行 → `cycles >= cycles_min` 且每个指标 `value <= 上界`。
报告行把**墙钟作为证据**写出来（`4000 cycle(s) in 41s, 4 metric(s) inside their bounds`），但**墙钟从不参与判定**。

> 与 plan.md §6 那句“N 分钟”的差异是**有意的**：M2 §1.1 已经钉死“must 门不许把墙钟当判据”（共享机器上必然假红）。
> 4.17 的正文与页面写清两件事：CI 跑的是**加速版**（计数地板 + 存量上界），真长跑是 `LAKELET_SOAK_MINUTES=30` 的同一条命令
> ——它样本更足，但判据一模一样。这样“soak 能在 CI 里发现泄漏”与“soak 真的能长跑”都不靠运气。

### 4.3 `slow:fuzz@N`（must，慢）——种子 fuzz（M3 新增的第三条，见 §7 D-4）

课程侧文件 `fuzz/<step>.toml`：`command`、`iterations_min`、`seeds = [...]`、`corpus_cases`。
判据：命令绿（= 没有 panic）+ 装置报的 `iterations >= iterations_min` + **装置报的重放条数 ≥ `corpus_cases`**。
语料是**项目里的 `tests/corpus/**`**（`tests/corpus/parser/*.txt`，一行一个十六进制输入，`#` 与空行是注释；
读取器 `src/protocol/corpus.rs` 由 4.2 的骨架交付，规格与装置共用同一个读取器），条数地板写在课程侧文件里。

> **这一节开工后被推翻过一次，方向与直觉相反。** 原稿把那个“fuzz 找到过的输入”钉成规格里的字节常量，
> 理由是“把字节放在目录里会逼出一个课程→项目的复制机制，而规格本来就会被拷进项目、并被 `spec-integrity` 管住”。
> 实测下来三条都不成立：目录方案**不需要**复制机制（骨架把读取器与 README 送进项目，*条目*由学员在跑搜索时
> 自己写出来），而规格方案恰好毁掉这一步的过关条件——`test/fuzzing@apply` 教的是“让机器去找你没想到的输入”，
> 把出事的那个形状写进规格（学员必须读的契约）等于把答案摊开。回归网不靠那一条手工常量：规格把**装置走出来的
> 每一个输入**都喂给扫描器，并与“解码器三个答案都要出现”一起把埋的 panic 变成必然的失败面（实测：把 bug 种回去、
> walk 已实现 → 规格两条用例红）。闸门因此只认**条数**（装置报的重放数 vs 课程侧地板），不认语料在哪、也不认它的形状
> ——这样别的 track 的 fuzz 步可以把语料放在自己挑的地方。

生产工具 `cargo-fuzz` 不由闸门驱动（本机没有、且要 nightly）：`doctor` 报它，4.2 的正文点名它。

### 4.4 三条门在注册表里的位置

`kiln/src/gates/registry.rs` 里 `sim`/`soak` 现在还是 `Pending("T4/T5")`/`Pending("T4")`，M3 把它们变成 `Ready` 并新增 `fuzz`；
注册表那个“ready 名单必须逐字相等”的测试同步更新（它是故意的：名单变了就必须有人看一眼）。plan.md §6 的表也一并补三行。

## 5. 容器与性能基建的打磨清单

| # | 改什么 | 为什么（证据/理由） |
| --- | --- | --- |
| C1 | `kiln images` 按**去重后的声明引用**报：每个引用一行（解析到本地哪个名字、被哪些步骤需要、头三个 + 计数），末尾给一行 `4 declared image(s): 4 present — a step can start its container without the network once its image is here` | 今天 `postgres:17-alpine` 重复 35 行，真正的信息被埋掉（实测：改后 4 行 + 一行结论） |
| C2 | 容器**运行**探测：`runtime::smoke` 挑一个已预拉的“安静”镜像跑 `<engine> run --rm <local> true`（15 s 上限），`doctor` 与 `kiln images` 都报 `✓ containers can start (podman \`alpine:3.20\`, 0.2s)` | 这个模块自己的信条就是“探测靠运行”（compose 的 `version` 成功而 `config` 失败就是这样查出来的）；`info` 成功不等于能起容器。候选镜像限定入口点是 shell 的那些，否则 `postgres:17-alpine` 会让探测因为与引擎无关的原因失败 |
| C3 | `kiln images pull`：先打印计划（声明 → 解析名 → 有/缺）、只拉缺的（已按引用去重）、末尾汇总；有缺项时以非 0 退出（这只是命令，不是闸门） | 与 C1 同一份数据；M3 的“镜像预拉”要能被人手工重放 |
| C4 | `kiln skip --track <id>`（`--all` 也认 `--track`） | M2 §4.1 的验收演练只能按路径序逐个 skip 15 步来铺尾巴树；T4 的验收（§9）要铺 24 步 |
| C5 | `doctor` 增一节 **profilers**：`sample`（macOS，实测可用）、`samply`（未装 → 安装行）、`cargo-flamegraph`（**跑一次 `dtrace -n 'BEGIN { exit(0) }'` 才知道**它在本机能不能用：这里被拒 → 报 `!` 并说明原因）、`perf`（仅 Linux 才提） | 4.16 要教“给出可复现的剖析结论”，学员第一步会问“我用哪个”；答案必须是这台机器的实测结果，而不是通用建议。**实测修正**：`dtrace` 是存在的（`/usr/sbin/dtrace`），拒绝的是权限；所以这一节的做法是“跑一下”，不是“找一下” |
| C6 | 无网快路径写进文档并留一条可重放的验收命令（`CARGO_NET_OFFLINE=true` + 私有 `XDG_CACHE_HOME` + 已预拉镜像 → T4 快路径绿） | plan §11 的 M3 验收项；M2 只有“镜像在不在”的检查，没有“真能离线跑”的演练 |

**不做**（也是决定）：不给 4.16 加 `?profile` 形状的 should 门。剖析工具的可用性随平台变化（本机 `dtrace` 都没有），
把它做成门只会制造一条永远 `blocked` 的行；4.16 的 must 面是**归因计数（复用 `bench-budget`）+ 页面 + `links`**。

## 6. T4 的门阶梯（起点 = T3 的终点）

T3·3.27 的清单是：`spec-integrity`、`caps`、`i18n`、`fmt`、`clippy@4`、`no-todo`、`docs:public`、`exit-code`、
`links`、`semver@baseline`、`slow:mutants@80`、`slow:?coverage@80`、`slow:bench-budget@10`、`unsafe-budget@0`。
T4 的起点与差异：

- **持久基线**（4.1 起全程）：`spec-integrity`、`caps`、`i18n`、`fmt`、`clippy@4`、`no-todo`、`docs:public`、`exit-code`。
- **不继承 `semver@baseline`**：按“门由教它那一步引入”的规则，T4 没有 `api/library-design` 槽位；T4 的公开面是**线上字节**，
  它由 4.1 的规格钉死（`spec/4.1/protocol_vectors.rs` 里就是那些字节，`spec-integrity` 管哈希）。见 §7 D-2。
- **不继承 `links`**：`links` 是步骤作用域属性，只写在真交页面的步骤上（4.3、4.5、4.7、4.11、4.15、4.16、4.17、4.18、4.19、4.20、4.22、4.23、4.24）。
- **`unsafe-budget@0` 从 4.7 起**（列式位打包是第一个真的会诱发 `unsafe` 的地方；这是快门，成本是毫秒）。
- **`slow:bench-budget@P` 从 4.5 起**（写放大是第一个“代价可数的”主题），之后按上表逐步出现。
- **`slow:mutants@80` 只在 12 个步骤上**（4.2、4.4、4.7、4.9、4.10、4.11、4.12、4.13、4.14、4.17、4.19、4.21）：
  它是步骤作用域质量门（plan §6 的“持久属性”名单里没有它），判的是“这一步的规格有没有说服力”，
  挑的是模块正确性关键、且规格强度就是这一步的主题的步骤。**代价：计划按 T3 实测（13–38 s/步）外推了
  60–90 s/步、≈ 12–18 分钟进 CI，但 4.4 实测只要 10 s**（kiln 自己实现变异、每步只判最多 6 个、复用同一份
  副本），所以 12 步的实测预估降到 ≈ 2 分钟——CI 预算比计划乐观得多。见 §7 D-1。

## 7. 决策点（要你拍板的）

| # | 决策 | 方案 | 推荐 |
| --- | --- | --- | --- |
| **D-1** | `slow:mutants@80` 在 T4 的覆盖 | (a) 12 个关键步（上表）；(b) 从 4.4 起全程累积（21 步，多花 10–15 分钟 CI）；(c) 不带 | **(a)**：覆盖正确性关键模块，CI 涨幅可控；把“规格不是装饰”留在最需要它的 12 步上 |
| **D-2** | T4 要不要 `semver@baseline` | (a) 不要（线上字节由规格钉住）；(b) 从 4.7 起带一份课程见证文件（+2.5 s/步 + 见证文件维护） | **(a)**：与“门由教它那一步引入”一致；协议字节的契约由 `spec/4.1/protocol_vectors.rs` 与 `spec-integrity` 承担 |
| **D-3** | 4.21 冷数据分层的后端 | (a) 课程自带 S3 形状的**进程内桩**（`KILN_BACKEND=local\|s3`，`conformance@2` 判两个后端跑同一套件），正文点名 MinIO/S3；(b) 死等 MinIO 镜像（本机三条路由都拿不到，见 §1）；(c) 换成 Postgres 存冷数据 | **(a)**：判的是 S3 的**请求形状**（规范请求、签名头、重试/幂等、list 分页），这正是 `outbound/http-client@design` 要教的东西；无容器、离线可跑。若你以后能预拉 MinIO，同一份规格用 `KILN_S3_ENDPOINT` 指向它即可，不必改判据 |
| **D-4** | 要不要第三条新门 `slow:fuzz@N` | (a) 要（4.2 的过关条件是“fuzz 抓到过至少一个 panic 并修掉”，需要一个能复现的证据面）；(b) 不要（4.2 只用普通规格 + `?coverage`，`cargo-fuzz` 仅在正文点名） | **(a)**：`doctor` 早就为 “the `fuzz` work of T4” 留了位置；判据是计数 + 只读语料，不需要外部工具 |
| **D-5** | 4.1 的 origin | (a) `transfer`（照 3.1 的先例：第四遍骨架用 RECALL/DELTA）；(b) `teach`（协议解析从零讲，有 WHY） | **(a)**：与 T3 的写法一致；协议那一半的“为什么”放进 DELTA/FURTHER。翻成 (b) 的代价只有一份正文与一行元数据 |
| **D-6** | T4 的探题工作量 | (a) 全量：29 个槽位里 **5 个按政策不出题**，其余 24 道全出（M2 的先例：T3 新增 22 道，题库 19 → 41）；(b) 先做 12 道关键槽位，剩下的留到 M5 前补；(c) 全部留到 M5 | **(a)**：理由与 M2 §5.1 相同——`plan --fast`/`review --due`/`skip` 靠它，缺题的槽位只能 `--force`；排在存储/查询步骤之后（用词要与正文一致）。**5 个不出题的槽位与理由**：`store/wal-recovery@aware`（aware 政策）、`ci/pipeline@design` 与 `release/container-image@design`（机制是流水线/容器，单文件题是假证据，已由 xtask/`container-repro` 覆盖）、`repo/toolchain-pinning@design`（机制是文件与版本的关系）、`supply/deny-policy@design`（需要真实依赖图；本机无 cargo-deny） |

## 8. 批次与顺序

沿用 handoff-m2 §3.5 的作业书（环境禁令、补丁生成、逐步验收、收尾清单），一次派**一步**给子 agent。

| 批次 | 内容 | 谁做 |
| --- | --- | --- |
| **B0 基建** | 三条新门 + 注册表/plan §6 + C1–C5 + `skip --track`；T4 的元数据、`app/lakelet` 骨架、第 4.1 步（作为门的第一个真用例） | 我自己做（框架 + 第一个真步骤在一起，判据格式才会定型——M0 的教训） |
| **B1** | 4.2–4.4（fuzz / ADR / WAL 恢复：三条新门各自第一次被真的步骤用上） | 逐步派子 agent |
| **B2** | 4.5–4.8（批写、背压、列式、合并：`bench-budget` 与 `sim` 在这里反复用） | 同上 |
| **B3** | 4.9–4.14（保留、下采样、查询引擎、聚合正确性、模型化测试、裁剪） | 同上 |
| **B4** | 4.15–4.20（归因、剖析、soak、基数、备份恢复、容量模型） | 同上 |
| **B5** | 4.21–4.24（分层、可复现、轮换、演练） | 同上 |
| **BP** | ~20 道探题 + 6 条“不出题”的理由表 | 分批派子 agent |
| **BV 验收** | §9 的六条腿 + `docs/t4-lakelet.md` + `handoff-m3.md` + 日志 + 推 `local` | 我自己做 |

每个批次结束我跑一次该段的 `kiln verify lakelet --slow` 并复核原始结论行；**B1 结束时给你一个中间检查点**
（那时 `lakelet` 已经能端到端跑“摄取 → WAL → 恢复”，三条新门都各自被真步骤用过）。

## 9. 验收演练（M3 收尾跑的六条腿，每条留原始输出）

1. **无网快路径**（plan §11 的 M3 验收）：镜像已预拉（C1 的汇总行）+ crates 已在本地缓存（首次构建的产物）+
   `CARGO_NET_OFFLINE=true` + 私有 `XDG_CACHE_HOME` + 私有 `CARGO_TARGET_DIR` → 在尾巴树上跑 T4 快路径
   （`kiln verify lakelet`，不带 `--slow`）全绿；跑到任何“要联网”的步骤（拉 crate / 拉镜像）会直接失败，所以这条演练本身就是证据。
2. **soak 抓泄漏**：往参考解注入一处泄漏（每批多起一个任务、句柄只增不减）→ `soak` 红并点名计数器；`git checkout -- .` 回滚后同一条命令绿。
3. **sim 抓不确定性**：把归并/刷新顺序换成 `HashMap` 迭代序 → `sim` 红（同一个种子两个摘要）；回滚后绿。
4. **fuzz 抓解析器**：注入一个未分类的截断帧路径 → `fuzz`/规格红并点名输入；回滚后绿。
5. **预算与裁剪**：把 4.14 的裁剪去掉（全扫）→ `bench-budget` 红并报出裁剪率；回滚后绿。
6. **故意劣化的回归面**：删/改协议向量 → `spec-integrity` 或协议规格红；把容器那一步的 `Dockerfile` 改坏 → `container-repro` 红（4.22 的回归保护）。

加上全量证据：`kiln verify lakelet --slow --json` → `{passed: 24, failed: 0, blocked: 0, not_authored: 0, clean: true}`；
`cargo xtask ci` → exit 0 + `24/24 step(s) hold the contract`；`lint-caps`、`probe --check`、`links`、`i18n` 的数字按 M2 的格式记进 `handoff-m3.md`。

## 10. 风险与缺口（开工前就写下来）

- **内容体量**：24 步 × 2–4 小时 ≈ 50–90 小时的真实工时（plan §11 的自己的口径）。B1 检查点是为了让“要不要继续到 24 步”这个决定有据可依。
- **CI 时长**：T4 的新慢门（`sim`/`soak`/`fuzz` + 12 个 mutants）估算 +25–35 分钟。当前基线是 30 m 19 s。若超过 60 分钟，先砍的是 `?coverage` 的步骤数（它是 should），不是 must 门。
- **`turmoil` 是 M3 唯一的新依赖风险**：实测可获取、可构建（8.3 s），但无网首跑会红——`doctor` 报它，正文写清“这一步起项目多一个 dev-dependency”。
- **本机没有 MinIO**：4.21 按 D-3(a) 设计；若将来要真 S3 集成测试，那是 M4/M5 的环境工作。
- **`kiln` 自身的 lint 阶梯**：新增的三个门要过 `unsafe_code = "forbid"`/`missing_docs`/`unwrap_used` 等 workspace lints，测试用 `cfg(test)` 放宽（与既有门一致）。
