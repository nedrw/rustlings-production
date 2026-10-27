# T5 `meshkv`：分布式 KV 存储（参考实现）

> 与 [t1-mailer.md](t1-mailer.md)、[t2-relay.md](t2-relay.md)、[t3-jobq.md](t3-jobq.md)、
> [t4-lakelet.md](t4-lakelet.md) 同构。M4 的计划与决策在 [m4-plan.md](m4-plan.md)，施工记录在
> [handoff-m4.md](handoff-m4.md)（契约、规则与缺口）与 [handoff-m4-log.md](handoff-m4-log.md)（分段日志）。

## 1. 一句话与资产

T5 是一个用步一层层长出来的**共识集群**：写入是一条日志的一项，一项被多数派持有才算决定（且只有 leader 自己任期
的那一项能提交）；日志会被快照截断，快照的编号仍是集群的位置；成员变更在一份"联合"配置上决策，没有一条已提交的
写会掉出去；读要么走一轮读索引要么走租约，租约比选举超时短是算术而不是感觉；任何节点都答，只有一个节点决定；
重试用同一个幂等键，重复的答复带的是原来那次的下标；节点间用证书证明"我是 n2"，轮换靠两段有效期重叠；状态机是
日志的纯函数，测试比对摘要；SDK 是产品，错误类型就是契约；一致性的每一级都写明它承诺什么、拒绝什么；网络是数据
（turmoil 的拓扑 + 种子 + 摘要），分区、滚动重启、混沌矩阵都是剧本；预算是计数而不是钟；SLO 页是带窗口的比率；
只有一个结构用 unsafe，且它在 loom 下有一个并发面；最后，引擎 crate 以 path 依赖接进来、零改动，协议两代节点混跑，
审计把每个共同下标上的摘要都比一遍。

| 资产 | 位置 |
| --- | --- |
| 22 步正文（英文） | `course/docs/steps/en/meshkv/` |
| 22 份只读规格 + hash 清单 | `course/tracks/meshkv/spec/`、`spec/manifest.toml` |
| scaffold / 参考解（逐步） | `course/tracks/meshkv/{scaffold,solutions}/` |
| 课程侧参数与基线 | `sim/*.toml`、`budget/*.toml`、`conformance.toml`、`skeleton/` |
| 跨 track 依赖 | `track.toml` 的 `requires_track = "jobq"`（plan §3 D6） |
| 探题 | `course/probes/`（全课程 83 道；T5 18 道，5 个槽位按政策不出题，见 §6） |

## 2. 22 步一览

| 步 | 主题 | 建立的槽位 | 这一步把什么变成接口 |
| --- | --- | --- | --- |
| 5.1 | 多节点拓扑与节点身份 | `dist/topology@apply` | 节点名、地址、进程内传输与五节点骨架 |
| 5.2 | 共识：日志复制与提交规则 | `dist/consensus@aware` | 任期/项/多数派/日志四条规则，以及 `docs/consensus.md` |
| 5.3 | 教学版 Raft | `dist/consensus@apply` | 选主、心跳、复制、提交钳位（一页可读的 Raft） |
| 5.4 | 快照与日志压缩 | `dist/snapshot@design` | 快照是提交态，压缩是前缀，位置不重编号 |
| 5.5 | 成员变更 | `dist/membership@apply` | 联合配置：两套多数派都同意才算决定 |
| 5.6 | 线性一致读与租约读 | `dist/linearizable-read@design` | 读索引、租约窗口，以及"租约 < 选举超时" |
| 5.7 | 客户端路由与重定向 | `dist/client-routing@apply` | 任一节点可答、一个节点决定、重定向有跳数上限 |
| 5.8 | 会话与重试的幂等 | `api/idempotency-key@design` | 幂等键归会话，重复答复是原来的下标 |
| 5.9 | mTLS 与节点间认证 | `sec/mtls@apply` | 签发者、有效期窗口、证书名=声明名；轮换两段重叠 |
| 5.10 | 状态机应用与确定性 | `dist/deterministic-state-machine@apply` | 状态是日志的纯函数；空载荷是 no-op 项 |
| 5.11 | 客户端 SDK 与错误语义 | `api/client-sdk@apply` | 错误类型即契约，重试策略只有一处、有上界 |
| 5.12 | 一致性级别与受限查询 | `query/consistency-levels@apply` | 每一级承诺什么、拒绝什么（不悄悄降级） |
| 5.13 | 确定性网络模拟（turmoil） | `test/deterministic-simulation@design` | 拓扑/分区/延迟是数据，种子 + 摘要 + 重放 |
| 5.14 | 分区与脑裂演练 | `dist/partition-drill@operate` | 少数派不提交、恢复后追平；少数派≠孤立节点 |
| 5.15 | 集群故障转移与滚动重启 | `ops/cluster-drill@operate` | 一次一个节点，任一时刻多数派在位 |
| 5.16 | 混沌注入深化 | `test/chaos-injection@design` | 故障矩阵是数据；页面与代码同源 |
| 5.17 | 分布式性能预算 | `perf/distributed-budget@design` | 节点数扫描的计数模型：fan-out、acks、每节点均摊 |
| 5.18 | 集群可观测性 | `obs/metrics-slo@operate` | SLI/错误预算/双窗口燃烧率，指标撑不起 SLO 就拒绝 |
| 5.19 | unsafe 深潜 | `unsafe/hand-rolled@operate` | 自研 ring：下标协议、`saturating_sub`、容量掩码；loom 管一个面 |
| 5.20 | 复用 `jobq` 引擎 | `api/library-reuse@design` | path 依赖接进来，`jobq` 零改动是判据 |
| 5.21 | 协议与升级演进 | `dist/protocol-evolution@design`、`ops/rolling-upgrade@design` | 两个版本号各自走；新特性要等所有节点都会 |
| 5.22 | 毕业演练 | `dist/consistency-audit@operate` | 多节点故障后，每个共同下标上的摘要都相同 |

## 3. 项目最后的形状

`app/meshkv` 是一个包：`src/raft/**`（`mod`、`state`、`message`、`membership`、`read`、`snapshot`）、
`src/**`（`cluster`、`node`、`client`、`idempotency`、`sdk`、`security`、`consistency`、`kv`、`sim`、`drill`、
`chaos`、`budget`、`metrics`、`ring`、`reuse`、`protocol`、`audit`）、`src/bin/{meshkvd,meshkvc}.rs`、
`src/cli/**`、`docs/**`（十三页：consensus、snapshot、partitions、simulation、restarts、chaos、budgets、
slo、unsafe、reuse、protocol、audit、sdk）、`tests/spec_*.rs`（课程拷入的只读规格）+
课程装置（`tests/sim_*.rs`、`tests/budget_*.rs`、`tests/loom_ring.rs`）+ `conformance.toml`、
`rust-toolchain.toml`。`jobq` 以 `path = "../jobq"` 接入（5.20 起）。

## 4. 装置与可复用模式

- **网络是数据，不是环境**：`sim/<step>.toml` 给种子与剧本命令，`KILN_SIM_SEED` 决定投递顺序，transcript 只有
  计数与摘要；`slow:sim@N` 跑 N 个种子，重放逐字节一致。turmoil 的拓扑/分区/延迟都是数据——这是 M4 的验收主词。
- **计数而不是钟**：`budget/*.toml` + `bench-budget@10`；5.17 的模型是整数算术（quorum、fan-out、acks、
  每节点均摊与跳数），回归表现为某个计数移动，而不是机器慢了一点。
- **一套套件、N 个后端**：`conformance.toml` 的 `[backends]` + `KILN_BACKEND`，`slow:conformance@2` 把 SDK 的
  同一套用例跑在脚本后端与自己写的进程内后端上（5.11）。
- **演练是"页面 + 装置"**：分区（5.14）、滚动重启（5.15）、混沌矩阵（5.16）、毕业审计（5.22）的剧本都是课程内容，
  测试把剧本真跑一遍；页面的表格与代码里的常量同源。
- **unsafe 只在 5.19 之后**：`unsafe-budget@0` 从 5.1 起全程生效，5.19 放开到 `unsafe-budget@5`（不收回）；loom 只围
  自研 ring 的一个并发面，miri 在本机不可用（见 §6）。
- **跨 track 复用是判据**：`requires_track = "jobq"`，砂箱先铺 jobq 的参考树；5.20 的验收是 `../jobq` 零改动。
- **页面即作品**：5.2 的 `docs/consensus.md` 由 spec 逐字比对（四条规则与五行目录）；其余交页面的步骤带 `links`。

## 5. 实测数字（这台机器）

| 项 | 值 |
| --- | --- |
| 逐步 `selftest` | 5.1–5.22 全部把契约走通（scaffold 红、参考解绿、must 门全绿）：`22/22 step(s) hold the contract` |
| `kiln probe --check` | **83/83** 参考解编译并全绿（T5 新增 18 道，池 65 → 83）|
| 课程自有检查 | `kiln lint-caps`：`97 capabilities · 136 slots · 119 ledger positions · 119 authored steps`，`no issues.`；`kiln links` offline：139 页 · 557 链接 · 330 外部目标，0 findings；`cargo test -q -p kiln`：351 + 21 + 1 passed |
| 全步判定（毕业树，快路） | `kiln verify meshkv`（22 步解全铺的毕业树）：**22/22 judged step(s) pass** · 0 failed · 0 not prepared |
| 全步判定（`--slow`） | `kiln verify meshkv --slow --json`，同一棵毕业树：**`{blocked: 0, clean: true, failed: 0, not_authored: 0, passed: 22, steps: 22}`**，exit 0，**42 s**。慢门真跑：`sim@3` 9 步（transcripts 与 seed 重放）、`mutants@80` 4 步（5.3 10/12、5.5 12/12、5.7 4/5、5.10 6/6）、`bench-budget@10` 3 步（5.4 5 计数、5.17 9、5.18 9）、`conformance@2`（scripted ✓ / in-process ✓）|
| `cargo xtask ci` | **exit 0 · `xtask ci: all checks passed`**（私有缓存）：五轨 selftest `22/22 + 24/24 + 27/27 + 24/24 + 22/22`、83/83 探题、全部门与 `cargo test --workspace` |

## 6. T5 范围内剩下的缺口

- **`unsafe-budget` 按步计**：门只数**本步 scaffold/参考解写下的**文件里的 `unsafe`（`deny` 的要求也只在项目整体
  没有 unsafe 时成立）。这是收尾时改的：按全项目计数的话，毕业树上 5.1–5.18 会数到 5.19 才写下的 `src/ring.rs`，
  `verify` 只能判最后四步（见 [handoff-m4-log.md](handoff-m4-log.md) 段 14）。
- **探题的 5 个槽位按政策不出题**（其余 18 道已写成，写法见 [authoring.md](authoring.md) §11）：
  `dist/consensus@aware`（`aware` 深度不出题——loader 直接拒绝）；`dist/partition-drill@operate`、
  `ops/cluster-drill@operate`、`dist/consistency-audit@operate`（机制是剧本 + 装置 + 真实集群，单文件题会变成假证据，
  已由 `slow:sim` 与演练装置覆盖）；`api/library-reuse@design`（机制是"另一个 crate 以 path 接入且零改动"，判据是
  仓库级 diff，不是单文件）。
- **miri 在本机不可用**（1.98.1-aarch64）：5.19 的判据降级为 loom（已缓存）+ `unsafe-budget@5` + 逐条 `SAFETY`
  注释；`kiln doctor` 报告组件缺失，缺了也不改判（plan §10）。
- **集群在进程内**：节点是线程 + 通道（turmoil 在测试里替换网络），`meshkvd` 能起但**不进判据**——真实 TCP、
  握手与容器编排都不在 M4 的验收里。
- **mTLS 是证书规则，不是真的握手**：`security.rs` 判签发者、有效期窗口与证书名，5.9 的正文与规格都这么说；
  真 rustls 不在依赖里，也不该在。
- **滚动升级是调度，不是真进程重启**：5.21 的 `restart.rs`/`drill.rs` 把"一波一波地换"建模成计划并用模拟验证，
  杀进程再起的运维面没有装置。
- **5.18 的指标是计数流**：SLO 页与燃烧率的算术是真的，Prometheus 接线、抓取与告警路由不在这一步。
- **5.22 的审计覆盖它采样的位置**：每个节点上报"已应用到哪、那里摘要是什么"，审计比对共同下标；不覆盖并发写下的
  中间态（那不是可观测的位置）。
