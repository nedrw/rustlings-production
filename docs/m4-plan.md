# M4 计划：T5 `meshkv`（22 步）+ 确定性分布式基建

> 对着 [plan.md](plan.md) §11 的 M4 行写。结构沿用 [m3-plan.md](m3-plan.md)（§0–§10 同构）。
> 决策点 §7 全部给出推荐与理由；按 M3 的先例，未在开工前被否决的推荐视为拍板。
> 入口上下文：plan §3 D6（T5 是唯一允许跨 track `path` 依赖的例外）、curriculum.md §10（22 步）、
> capabilities.toml 的 23 个 `meshkv/*` 槽位、handoff-m3 §1 的 24 条规则、authoring.md 全部。

## 0. 范围与验收（对着 plan §11 的 M4 行写）

| 验收项 | 判据 |
| --- | --- |
| T5 `meshkv` 22 步 | 正文、只读规格、加性 scaffold、参考解、闸门接线全部就位；逐步 `kiln selftest` 绿；`kiln verify meshkv --slow` 在尾巴树上全绿 |
| 确定性网络模拟 | 同一 seed 重复跑结果一致（turmoil，同一 digest 重放）；网络分区注入后行为可预测且断言明确 |
| 跨 track 复用（D6 例外） | 5.20 的判据是 **`app/jobq` 的引擎 crate 零改动**被 meshkv 以 `path` 依赖接入；`jobq` 的 selftest 在接入前后都绿 |
| 集群故障演练 | 分区（少数派不可提交、多数派可恢复）、成员变更不丢写、滚动重启期间可用、审计脚本可复核 |
| 收尾 | `cargo xtask ci` exit 0（五条 track 全部 `n/n hold the contract`）；`docs/t5-meshkv.md` + `handoff-m4.md` + 日志 |

**不在 M4 里**：中文翻译、文档站、`new-track` 生成器（M5）；`openraft` 接入（见 §7 D-1 的取舍）。

## 1. 环境早验（2026-10-26 实测）

- **turmoil 0.7.2 已在本地 registry 缓存**（`~/.cargo/registry/cache/*/turmoil-0.7.2.crate`）——离线路有它；
  `madsim`、`loom`、`openraft` **未缓存**（首次构建需联网拉一次，之后进缓存）。
- **miri 组件未安装**（`rustup component list --installed` 无 miri；llvm-tools 在）。5.19 需要它：
  `rustup component add miri` 是加法、可逆、不影响钉住的 1.98.1 工具链其余部分——B0 做并写进 `doctor`。
- **容器引擎健康**（M3 段 35/36：overlay 坏了两周后自愈，2026-10-26 起 `podman run` exit 0）；
  另一个项目的容器在跑，**任何时候不重启 podman machine**。
- **registry 会漂**（段 36 的教训）：manifest 写范围、锁由解析生成，课程不预置锁；
  规格断言一律写「满足要求」而不是「版本前缀相等」；容器构建挂宿主 `$CARGO_HOME/registry`。
- **CI 时长基线**：M3 收尾 2734 s（97 步、四条 track）。T5 再加 22 步（多数带 `slow:sim`/`slow:mutants`），
  按同比例外推 ≈ 3300–3600 s；可接受，但 §6 的慢门只放在主题需要的步骤上。

## 2. T5 的 22 步（槽位 / origin / 门）

23 个槽位（5.21 教两个），全部对着 capabilities.toml 逐格；`teaches`/`requires` 在 B0c 的 track.toml 里定稿。

| # | 步骤 | 槽位 | origin | 门（新增部分） |
| --- | --- | --- | --- | --- |
| 5.1 | 多节点拓扑与节点身份 | `dist/topology@apply` | transfer（第五遍骨架，照 3.1/4.1 先例） | 基线持久组（见 §6） |
| 5.2 | 共识：日志复制与提交规则 | `dist/consensus@aware` | teach | — |
| 5.3 | 教学版 Raft：选主与复制 | `dist/consensus@apply` | teach | `slow:mutants@80`、`slow:sim@3` |
| 5.4 | 快照与日志压缩 | `dist/snapshot@design` | teach | `slow:bench-budget@10` |
| 5.5 | 成员变更 | `dist/membership@apply` | teach | `slow:sim@3`、`slow:mutants@80` |
| 5.6 | 线性一致读与租约读 | `dist/linearizable-read@design` | teach | — |
| 5.7 | 客户端路由与重定向 | `dist/client-routing@apply` | teach | `slow:mutants@80` |
| 5.8 | 会话与重试的幂等 | `api/idempotency-key@design` | teach（正文 RECALL `relay/2.x` 的 delivery 幂等） | — |
| 5.9 | mTLS 与节点间认证 | `sec/mtls@apply` | teach | `slow:sim@3`（轮换窗口） |
| 5.10 | 状态机应用与确定性 | `dist/deterministic-state-machine@apply` | teach | `slow:mutants@80`、`slow:sim@3` |
| 5.11 | 客户端 SDK 与错误语义 | `api/client-sdk@apply` | teach | `slow:conformance@2`（SDK vs 进程内客户端） |
| 5.12 | 一致性级别与受限查询 | `query/consistency-levels@apply` | teach | `slow:sim@3` |
| 5.13 | 确定性网络模拟（turmoil） | `test/deterministic-simulation@design` | teach | `slow:sim@3`（seed 重放） |
| 5.14 | 分区与脑裂演练 | `dist/partition-drill@operate` | teach | `slow:sim@3` |
| 5.15 | 集群故障转移与滚动重启 | `ops/cluster-drill@operate` | teach | 装置形态照 4.24 的 runbook 仪器 |
| 5.16 | 混沌注入深化 | `test/chaos-injection@design` | teach | `slow:sim@3` |
| 5.17 | 分布式性能预算 | `perf/distributed-budget@design` | teach | `slow:bench-budget@10`（节点数扫描） |
| 5.18 | 集群可观测性 | `obs/metrics-slo@operate` | **deepen**（mailer@aware → relay/jobq@… → 这里 operate） | `slow:bench-budget@10` |
| 5.19 | unsafe 深潜：自研结构 + miri + loom | `unsafe/hand-rolled@operate` | teach | `unsafe-budget@N>0` 首次放开 + miri 装置 |
| 5.20 | 复用 `jobq` 引擎 | `api/library-reuse@design` | teach | 装置 diff `app/jobq` 全程零改动 |
| 5.21 | 协议与升级演进 | `dist/protocol-evolution@design`、`ops/rolling-upgrade@design` | teach | `slow:sim@3`（新旧混跑） |
| 5.22 | 毕业演练：多节点故障 + 一致性审计 | `dist/consistency-audit@operate` | teach | 照 4.24 仪器 + 审计脚本 |

## 3. 项目形状、数据模型与依赖决定

- **一个包 `app/meshkv`**（库 + 两个 bin：`meshkvd` 守护进程、`meshkvc` 运维 CLI；`default-run = "meshkvd"`，
  规则 21）。模块边界由 5.1 建立：`src/raft/**`（共识）、`src/node/**`（节点与服务）、`src/kv/**`（状态机）、
  `src/net/**`（传输与 mTLS）、`src/cli/**`；规格读源码杆住边界（T4 的做法）。
- **数据模型**：KV 即 `Set(key, value)` / `Cas(key, expect, value)` / `Delete(key)` 三种提案 +
  `Get`（读路径走 5.6/5.12 的级别矩阵）；提案序号 = (term, index)。复用 jobq 的引擎类型（5.20 之后）。
- **传输**：节点间自研帧协议（复用 4.1 的 framing 教训），客户端口走 5.11 的 SDK；
  **turmoil 从 5.13 起替换真实网络作为测试传输**（生产传输仍是 tokio TcpStream）。
- **对 `app/jobq` 的 `path` 依赖（D6 例外）**：skeleton 的 `Cargo.toml` 写
  `jobq-engine = { path = "../jobq/engine" }`；5.20 之前的步骤**不用它**（先自研状态机，毕业步才接入），
  依赖在 5.20 的 scaffold 里加（标记块），装置在每一步 diff `../jobq`——零改动是判据不是期望。
  **前提**：跑 T5 的机器/砂箱里 `app/jobq` 必须存在且为参考解状态（§5 C1）。
- **门阶梯起点 = T4 的终点**（§6）；`unsafe-budget` 在 5.19 前保持 `@0`，5.19 按预算放开（写明行数与理由）。

## 4. 确定性分布式基建（turmoil 的接入契约）

- **同 seed 重放**：沿用 `slow:sim@3` 的 harness（课程侧 `sim/<step>.toml`：`seeds = [1, 2, 3]`，
  装置读 `KILN_SIM_SEED`）；5.13 起 sim 的装置底座是 turmoil——网络拓扑、分区、延迟都是**数据**
  （`topology.toml` 形状的剧本），digest 是 transcript 的哈希。同一 seed 两次运行 digest 必须相等（M4 判据）。
- **分区注入的断言面**（5.14/5.16）：少数派不提交（提案超时计数不增长）、恢复后追平（日志前缀一致）、
  脑裂期间旧主的写全部被拒绝且计数。剧本与预期行为表是课程侧文件，规格重放并逐行比对。
- **miri（5.19）**：`tests/miri.rs` 用 `cargo miri test`（装置经 `KILN_MIRI=1` 门控），判据是
  「跑过且无 UB 报告」；loom 只围 5.19 自研结构的一个并发面。两者都是**步骤作用域装置**，不进全局 CI
  （miri 慢；`doctor` 报组件在不在，缺了 5.19 的 sim/单测部分照跑、miri 部分标 skipped——plan §10 的缺工具对策）。

## 5. 基建打磨清单（B0）

- **C1 跨 track 依赖**：`kiln` 学会「一条 track 声明它需要另一条 track 的项目在场」——track.toml 增
  `requires_track = "jobq"`；`run/check/verify` 在项目缺失时给出可执行的指引（「先 `kiln verify jobq` 或
  `kiln solution` 到位」）；**selftest/verify 的砂箱在铺 meshkv 之前先把 jobq 的参考解树铺好**
  （复用现有的逐步 scaffold/solution 机制，一次性、砂箱私有）。这是 M4 最大的一笔框架改动。
- **C2 miri 组件**：`rustup component add miri`（加到 1.98.1）；`doctor` 的 profilers 一节补 miri/loom 行。
- **C3 依赖预取**：B0 里联网拉一次 `loom 0.7`（turmoil 已缓存；openraft 不用，见 D-1），进缓存后离线路成立。
- **C4 `skip --track meshkv`** 已有（M3 的 B0b），验收沿用。
- **C5 骨架与元数据**：`course/tracks/meshkv/track.toml`（22 步、槽位、门阶梯）、`app/meshkv` 骨架
  （含 `jobq` 的 path 依赖声明但 5.20 前不 use、`tests/` 夹具、全部标记块——规则 4）、
  `docs/steps/en/meshkv/5.1-*.md` 作为第一个真用例。

## 6. T5 的门阶梯（起点 = T4 的终点）

- **持久基线（5.1 起全程）**：`spec-integrity`、`caps`、`i18n`、`fmt`、`clippy@4`、`no-todo`、`docs:public`、
  `exit-code`、`unsafe-budget@0`（继承自 4.7 的持久属性；5.19 按预算放开后**不再收回**——阶梯只收紧）。
- **不继承 `semver`**：T5 没有 `api/library-design` 槽位；5.20 的复用判据是「零改动 diff」，比基线更强。
- **`links` 步骤作用域**：只写在真交页面的步骤（预计 5.3、5.4、5.9、5.13、5.14、5.15、5.17、5.18、5.19、5.20、5.22）。
- **`slow:mutants@80` 只在 5 个步骤**（5.3、5.5、5.7、5.10、5.21）：共识与状态机是正确性关键中的关键；
  演练/运维步（5.14/5.15/5.22）由 sim 与仪器判，不凑变异比。
- **`slow:sim@3` 是 M4 的主力门**（10 个步骤）：确定性是 M4 的验收主词，turmoil 剧本 + digest 重放。
- **镜像/容器**：meshkv 本身不出镜像（分布式步的容器编排不在判据里）；`images`/`container-repro` 不进阶梯。

## 7. 决策点（按推荐执行，除非否决）

| # | 决策 | 方案 | 推荐 |
| --- | --- | --- | --- |
| **D-1** | 5.3 的 Raft：接入 openraft 还是教学版 | (a) 教学版 Raft（自研、全部作为数据、规格可判、变异可走查）；(b) 接入 `openraft 0.9`（生产形状，但重依赖、API 演进快、规格对它的内部不可判） | **(a)**：`dist/consensus@apply` 教的是「应用共识」，自研才能让 sim/变异/审计对着学员的代码判；openraft 写进 FURTHER 与 `docs` 页 |
| **D-2** | 确定性模拟底座 | (a) `turmoil 0.7`（本地已缓存；tokio 生态、API 小）；(b) `madsim`（未缓存、模拟面更广）；(c) 自研虚拟网络 | **(a)**：缓存里就有（离线路成立）；教的是「确定性」不是某个框架，FURTHER 点名 madsim |
| **D-3** | 跨 track 依赖的机制 | (a) track.toml `requires_track` + 砂箱铺 jobq 参考树（上面 C1）；(b) vendor 冻结副本进 meshkv 骨架 | **(a)**：5.20 的判据是「引擎零改动被接入」，vendor 恰好把判据变成空话；(b) 还让 jobq 的后续修复无法到达 T5 |
| **D-4** | miri 进不进 CI | (a) 只做步骤作用域装置（5.19 跑、`doctor` 报告、缺组件标 skipped）；(b) 全局 CI 跑 | **(a)**：miri 慢一个数量级，且只服务 5.19 的一小段自研结构；plan §10「缺工具不改判」 |
| **D-5** | 5.1 的 origin | (a) transfer；(b) teach | **(a)**：第五遍骨架，RECALL/DELTA 比 WHY 更诚实 |
| **D-6** | T5 探题 | (a) 23 槽位里按政策排除后全出（预计排除 `aware`（5.2）与机制不在单文件里的 design（5.20 复用、5.14/5.15/5.22 演练），约 **17–18 道**）；(b) 分半两批 | **(a)**：M2/M3 同一先例（缺题的槽位只能 `--force`）；排在对应步骤之后出题 |
| **D-7** | obs/metrics-slo@operate 的 deepen 写法 | (a) 集群维度的 SLO 页 + 仪器（谁落后、谁抖动），复用 4.24 的仪器形态；(b) 再教一遍指标 | **(a)**：deepen 的定义是「同一能力在更难的失败模式下」，不重教 |

## 8. 批次与顺序

沿用 handoff-m2 §3.5 作业书 + handoff-m3 §1 的 24 条规则；一次派一步给子 agent。

| 批次 | 内容 | 谁做 |
| --- | --- | --- |
| **B0 基建** | C1–C5（跨 track 依赖机制、miri、骨架、元数据、5.1 第一个真用例） | 我自己做（框架 + 第一个步骤在一起，M0 的教训） |
| **B1** | 5.2–5.5（共识、Raft、快照、成员变更——sim/变异第一次真用） | 逐步派子 agent |
| **B2** | 5.6–5.9（读级别、路由、幂等、mTLS） | 同上 |
| **B3** | 5.10–5.13（状态机、SDK、一致性级别、turmoil 接入——M4 的主词在这里立起来） | 同上 |
| **B4** | 5.14–5.17（分区演练、滚动重启、混沌、分布式预算） | 同上 |
| **B5** | 5.18–5.22（可观测 deepen、unsafe 深潜、jobq 复用、协议演进、毕业审计） | 同上 |
| **BP** | ~17–18 道探题 + 不出题理由表 | 分批派子 agent |
| **BV 验收** | §9 的腿 + `docs/t5-meshkv.md` + `handoff-m4.md` + 日志 + 收尾 | 我自己做 |

每批收尾跑该段的 `kiln verify meshkv --slow` 并复核原始行；B3 结束给中间检查点
（那时「同 seed 重放一致」与「分区断言」都已成立——M4 判据的两大半）。

## 9. 验收演练（M4 收尾跑的腿，每条留原始输出）

1. **同 seed 重放**：同一 `KILN_SIM_SEED` 两次完整 sim → digest 逐字节相等（M4 判据原文）。
2. **分区注入**：剧本断网多数派↔少数派 → 少数派提案零提交、恢复后前缀追平；回滚剧本 → 绿。
3. **成员变更不丢写**：变更期间的已提交提案在变更后可读（装置断言无丢失窗口）。
4. **mTLS**：未授权节点的握手被拒且计数；证书轮换窗口内新旧两钥都过（照 4.23 的窗口算术）。
5. **滚动重启可用性**：一次滚一个节点，任一时刻多数派在位、读不断（5.15 的仪器）。
6. **jobq 零改动**：接入前后 `diff -r app/jobq` 除 `Cargo.lock` 外零差异；jobq 的 selftest 仍绿。
7. **全量证据**：`verify meshkv --slow --json` `{passed: 22, failed: 0, clean: true}`；离线快路 22/22；
   `cargo xtask ci` exit 0（五条 track）；`lint-caps`/`probe --check`/`links`/`i18n` 数字按 M3 格式记档。

## 10. 风险与缺口（开工前写下来）

- **C1 是新的框架面**（track 间依赖会改变 selftest/verify 的铺树顺序）——B0 最先做、最先测；
  若实现代价超预期，降级方案：meshkv 的装置自己铺 jobq 树（课程侧文件承担，kiln 不学新概念）。
- **turmoil 与 tokio 版本耦合**：turmoil 0.7.2 对 tokio 的 req 需在 B0 验证与 1.5x共存；
  冲突则锁定 turmoil 兼容的 tokio 下界并在骨架写明。
- **miri 在 1.98.1 的可用性**：B0 装组件时验证；不可用则 5.19 的判据降级为 loom + `unsafe-budget@N`
  + 手工模型（决策留给 B5 前定）。
- **CI 时长**：+22 步（10 个 sim、5 个变异）外推 CI ≈ 55–60 分钟；若超过 70 分钟，
  考虑 sim 种子从 3 降到 2（决策留给 BV 前，判据 digest 重放不受影响）。
- **5.19 的 unsafe 放开**：`unsafe-budget@N>0` 之后永不收回；N 的值由 5.19 的参考解实测定（先写 0 改最后）。
