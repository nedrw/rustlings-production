# T4 `lakelet`：遥测存储（参考实现）

> 与 [t1-mailer.md](t1-mailer.md)、[t2-relay.md](t2-relay.md)、[t3-jobq.md](t3-jobq.md) 同构。M3 的计划与决策在
> [m3-plan.md](m3-plan.md)，施工记录在 [handoff-m3.md](handoff-m3.md)（契约、规则与缺口）与
> [handoff-m3-log.md](handoff-m3-log.md)（分段日志，逐批的原始行）。

## 1. 一句话与资产

T4 是一个用步一层层长出来的**遥测存储**：采集走二进制分帧协议（长度前缀 + CRC + 版本字），写入是一本会被重放的
日志；刷写把一批记录封成按列排布、zstd 压缩的段，段的清单**就是**店；合并是值、读者看不到半次合并，保留期按整段丢弃；
查询是受限 SQL（`EXPLAIN` 打印十二行计划），索引是段旁边的派生侧车；冷数据经一个 blob 端口（本地目录与进程内 S3 形状端点
两个实现）分层出去；备份的清单本身就是一本日志，恢复是一次「先全验后写、清单最后写」的写；容量是带单位整数的模型，
并且**与店自己的数在同一个门里判**；密钥是有重叠有效期的环，轮换在一台运行中的守护上不留缝；最后，runbook 是一页程序，
而 `tests/runbook_harness.rs` 是把那页跑一遍的证据。

| 资产 | 位置 |
| --- | --- |
| 24 步正文（英文） | `course/docs/steps/en/lakelet/` |
| 25 份只读规格 + hash 清单 | `course/tracks/lakelet/spec/`、`spec/manifest.toml` |
| scaffold / 参考解（逐步） | `course/tracks/lakelet/{scaffold,solutions}/` |
| 课程侧参数与基线 | `sim/*.toml`、`soak/*.toml`、`budget/*.toml`、`fuzz/*.toml`、`skeleton/` |
| 探题 | `course/probes/`（全课程 41 道；T4 的 24 道**待补**，见 §6）|

## 2. 24 步一览

| 步 | 主题 | 建立的槽位 | 这一步把什么变成接口 |
| --- | --- | --- | --- |
| 4.1 | 采集协议 | `proto/framing@apply` | 帧布局（`MAX_PAYLOAD`/`OVERHEAD`/版本字）与引擎边界 |
| 4.2 | 种子 fuzz | `test/fuzzing@apply` | 变器 + 语料是作品；`slow:fuzz@3` 只认条数 |
| 4.3 | 写路径 ADR | `store/write-path@design` | 何时等设备是值（`Policy`），`Losses` 写明会丢什么 |
| 4.4 | WAL 恢复 | `store/wal-recovery@apply` | 日志是前缀：证明不了就停在原地 |
| 4.5 | 批量写 | `perf/write-amplification@apply` | 一批多少载荷共享一次调用（`flush::Stats`）|
| 4.6 | 采集背压 | `async/backpressure@apply` | 队列的策略与守恒式，拒绝计到调用方 |
| 4.7 | 列式段 | `store/columnar@apply` | 段的布局（金向量）与 `unsafe_code = "deny"` |
| 4.8 | 合并 | `store/compaction@apply` | 清单就是店；合并是值，退休给最后一个读者 |
| 4.9 | 保留期 | `store/retention@apply` | 按整段丢弃，floor 写进清单；读者看不到它离开 |
| 4.10 | 降采样 | `query/downsampling@apply` | 窗口在绝对网格上，`lakeletq` 落地 |
| 4.11 | 查询引擎 | `query/engine@design` | 受限语法、分类拒绝、`EXPLAIN` 的计划 |
| 4.12 | 聚合正确性 | `num/aggregation-correctness@apply` | Kahan–Neumaier 与本地日的 23/25 小时 |
| 4.13 | 模型化测试 | `test/model-based@apply` | 种子决定的行走必须与模型逐操作一致 |
| 4.14 | 索引与剪枝 | `query/index-pruning@apply` | 侧车索引是派生状态：只损失速度，不损失记录 |
| 4.15 | 预算与归因 | `perf/bench-budget@operate` | 按相位计「工作」的计数，回归能点到相位 |
| 4.16 | 剖析 | `perf/profiling@operate` | 采样入口 + 页面携带产出它的那次运行 |
| 4.17 | soak | `perf/soak@apply` | 长跑看五个活计数器；门判计数不判分钟 |
| 4.18 | 基数与成本 | `obs/cardinality-cost@apply` | 上限是带计数的政策；账本从写路径重建 |
| 4.19 | 备份与恢复 | `ops/backup-restore@apply` | 副本的清单是一本日志；恢复先全验后写，清单最后写 |
| 4.20 | 容量模型 | `ops/capacity-cost@design` | 模型是带单位的整数，与店实测在同一个门里判 |
| 4.21 | 冷数据分层 | `store/tiering@apply`、`outbound/http-client@design` | blob 端口两个实现、SigV4、幂等重试、分页 |
| 4.22 | 可复现构建 | `build/reproducibility@apply` + 四个 design 槽 | 同一份源码两次构建逐字节一致（或差异被点名）|
| 4.23 | 密钥轮换 | `sec/secrets-management@design` | 令牌是首载荷；环按连接读；窗口是算术 |
| 4.24 | runbook 演练 | `ops/runbook-drill@operate` | 页面就是流程，装置把页面的命令真跑一遍 |

## 3. 项目最后的形状

`app/lakelet` 是一个包：`src/protocol/**`（分帧、批次、语料/变器/嗅探）、`src/engine/**`（spool、wal、flush、segment、
compaction、retention、query、index、trace、soak、cardinality、backup、tier、cost、rng）、`src/service/**`（config、
logging、ingest、backpressure、soak、keys）、`src/cli/**`（query / inspect / backup / restore / rotate）、
`src/bin/{lakeletd,lakeletq,lakelet-load}.rs`、`docs/**`（protocol、format、adr/0001、crash-points、write-amplification、
query、perf、profile、soak、cardinality、dr-drill、capacity、rotation、build、supply-chain、runbook、slo）、
`tests/spec_*.rs`（课程拷入的只读规格）+ `tests/support/**`（学员的夹具）+ 课程装置（`budget*.rs`、`attribution.rs`、
`fuzz_protocol.rs`、`sim_*.rs`、`soak.rs`、`s3_stub.rs`、`runbook_harness.rs`）+ `conformance.toml`、`Dockerfile`、
`deny.toml`、`rust-toolchain.toml`、`.github/workflows/ci.yml`、`scripts/repro.sh`。

## 4. 装置与可复用模式

- **一套套件、N 个后端**：`KILN_BACKEND=local|s3` + `tests/support/tier.rs`；`slow:conformance@2` 就是拿同一条命令跑两遍。
- **种子 + transcript**：`slow:sim@N` 的种子决定整个剧本，transcript 只有计数与摘要，**同意的那次不打印种子**，`replay` 的种子逐字节重放。
- **计数而不是钟**：`soak@N`（下界 + 上界）、`bench-budget@P`（±10% 与 `_max`）、`fuzz@N`（只认条数）——三条都拒绝把墙钟写进判据。
- **页面就是作品，且块要被重跑**：`docs/{profile,dr-drill,capacity,runbook}.md` 的块由规格重跑逐行比对（4.19 实测改一个数就红）。
- **后面的步骤不重写前面的格式**：清单停在 v2、tier 的「第几段在哪」由 listing 回答、备份的清单就是 4.4 的日志格式——新能力都通过旧树已有的东西接上来。
- **每一步都有一次演练**：正文的 TRAP 与批次日志的 drill 说出「弄坏哪里会红」，红→回滚→绿的原始行记在 [handoff-m3-log.md](handoff-m3-log.md)。

## 5. 实测数字（这台机器）

| 项 | 值 |
| --- | --- |
| 逐步 `selftest` | 4.1–4.24 全部 `1/1 step(s) hold the contract`（scaffold 红、参考解绿、must 门全绿）|
| 全步判定（24 步尾巴树，`kiln verify lakelet`，快路）| **23/24 judged step(s) pass · 1 failed · 0 not prepared · 0 without content**（98 s）；唯一失败的是 4.22 的 `container-repro`——**本机容器引擎坏着**（`creating overlay mount … input/output error`），不是工程的问题 |
| `kiln verify lakelet --slow --json` | **待收尾时跑**（`{passed:24, failed:0, …}` 需要先修好容器引擎）|
| 全量套件（尾巴树）| `cargo test -p lakelet`：各规格 + 装置全绿（4.24 那次：spec_runbook 15 passed · runbook_harness 1 passed；4.17 的 soak 目标 ~138 s）|
| 慢门实测 | `fuzz@3` 4.2（≥20000 次 + 语料条数）；`sim@3` 4.4/4.6/4.8/4.13/4.19/4.23（各自的三个种子与摘要）；`soak@4000` 4.17/4.18（~146 s）；`mutants@80` 十二步：多数 6/6 或 5/6，4.12/4.14 10/12，4.17 6/8（阈值 `@70`），4.19 12/12，4.21 10/12；`bench-budget@10` 4.5/4.7/4.8/4.9/4.10/4.14/4.15/4.20 |
| 课程自有检查 | `lint-caps` `no issues.`（97/136/119/97）；`links` **113 page(s) · 525 link(s) · 329 external target(s)**，全部解析；`i18n` 0 error；`probe --check` 41/41；`cargo test -q -p kiln` **344 passed**（322+21+1）|
| 4.22 的可复现构建 | `sh scripts/repro.sh`：`artifacts_compared=4 identical=4 differing=0 verdict=reproducible`（手跑 34 s）|
| `cargo xtask ci` | **待收尾时跑**（M2 的记录：30–39 分钟，最后一行是 `<n>/<n> step(s) hold the contract`）|

## 6. T4 范围内剩下的缺口

- **容器引擎坏着，plan §9 腿 6 的第二半欠着**：本机 `podman run` 对任何镜像都报 overlay 挂载 I/O 错误，所以 4.22 的
  `container-repro` 无法判；引擎恢复后要重跑（配方与暖 registry 的位置在 [handoff-m3.md](handoff-m3.md) §5）。
- **探题的 5 个槽位按政策不出题（其余 24 道已写成，见 `docs/authoring.md` §11 的写法）**：
  `store/wal-recovery@aware`（`aware` 深度不出题——loader 直接拒绍 `aware` 槽位的题）、
  `ci/pipeline@design` 与 `release/container-image@design`（机制是流水线与容器，单文件题会变成假证据，
  已由 `xtask ci` 与 `container-repro` 覆盖）、`repo/toolchain-pinning@design`（机制是文件与版本的关系）、
  `supply/deny-policy@design`（需要真实依赖图；本机没有 `cargo-deny`）。24 道题里多数是 6–10 条用例、
  约 500–1200 行（含给定的词汇），每道都写明了它**不是**那一步的什么机制。
- **没有只读的 manifest 入口**（4.19、4.21、4.24 三处撞上）：`Segments::open` 会写，因此 `copy` 会改源、冷店打不开；
  修法是一个只读的 `compaction::Manifest::load`，并让 4.19/4.21 改用它（是否放进 M3 收尾待定）。
- **tier 在 CLI 里没有入口**：`sweep`/`hydrate` 只有引擎侧，所以 4.24 的 runbook 阶段点不出 tier 命令。
- **店的日志从不截断**：它占这台店字节的 **885‰**（4.20 的 TRACK 8）；容量模型把它计进去了，但「封段即截日志」的机制不存在。
- **`query` 只看段，`inspect series` 看日志 + 活索引**：记录还在日志里时两者答案不同（4.24 有意用它，但操作员会跌进去）。
- **`container_repro.rs` 只把 `MAILER__` 前缀的环境变量带进宿主运行**（潜伏；lakelet 的 Dockerfile 因此不写 `ENV`）。
- **`kiln` 的补丁器忽略 `new file mode`**，所以可执行位随补丁交付不了（4.22 的 `scripts/repro.sh` 靠 `sh` 跑）。
- **没有「只检查钉」的 `kiln spec manifest --check`**；手钉的行只能在副本里重跑再 diff 来验。
