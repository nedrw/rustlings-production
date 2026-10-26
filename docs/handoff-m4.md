# M4 交接简报（T5 `meshkv` 的 22 步 + 探题池 83 + 跨 track 依赖）

> **状态：M4 收尾中。** T5 的 22 步、18 道探题与评审修复已落地；验收的四条腿（全步 `selftest`、
> `verify --slow`、离线快路、`cargo xtask ci`）在 §4 里逐条记数。计划与决策在 [m4-plan.md](m4-plan.md)，
> 分段施工记录在 [handoff-m4-log.md](handoff-m4-log.md)。对课程侧的交接简介是
> [t5-meshkv.md](t5-meshkv.md)（与 t1–t4 同构）。

## 0. 已落地的部分（截至本次提交）

| 批次 | 内容 |
| --- | --- |
| B0 | M4 计划（`13c0830`）；`requires_track` 与砂箱铺前置参考树（`da0c803`）；T5 的 22 步元数据与 `app/meshkv` 骨架（`b44ca33`）；5.1 走通契约（`74a9faa`） |
| B1 | 5.2 四条规则与页面（`a86cb79`）；5.3 教学版 Raft（`e3d8f2c`）；5.4 快照与压缩（`62df707`）；5.5 联合配置（`1690436`） |
| B2 | 5.6 读索引与租约（`f371eb1`）；5.7 路由与重定向（`7a5d50d`）；5.8 会话与幂等键（`bcb53e7` + `2105fbd`）；5.9 mTLS 与轮换（`4f062b6`） |
| B3 | 5.10 确定性状态机（`b2420ba` + `098625e` 的 5.11）；5.12 一致性级别（`7238859`）；5.13 turmoil 与剧本（`579b37a`） |
| B4 | 5.14 分区演练（`47ec974`）；5.15 滚动重启（`797c3fb`）；5.16 混沌矩阵（`36e34e5`）；5.17 分布式预算（`b10bf0a`） |
| B5 | 5.18 可观测（`70302c2`）；5.19 unsafe ring（`d6e3fb4`）；5.20 jobq 复用（`f7a8cdd`）；5.21/5.22 协议演进与审计（`bac9e77`）；README 与计划收口（`ea3107c`） |
| 评审修复 | 框架（`MissingDependency` 的 `first`、`verify` 的 fix、state 原子写与合并、补丁回滚/拒删/锚定）+ 21 个 step 树的九处语义修复 + 内容一致性（5.6/5.8/5.10 目标名、5.16/5.21 的 `links`、5.17 的数字与措辞、5.11 的 retryable、计划的阶梯列）；见日志段 12 |
| BP | T5 的 18 道探题与参考解（池 65 → 83），四批并行、逐道复核；见日志段 13 |
| BV | [t5-meshkv.md](t5-meshkv.md) 与本文；§4 的四条验收 |

## 1. 施工中钉下来的规则

1. **`tokio::join!` 不是 future**（它在宏里 await）：两个握手要在测试里同时发生，就把 listener 那一半 spawn。
2. **规格也过 `-D warnings`**：`uninlined_format_args` 对测试目标一样生效。
3. **turmoil 只许绑 `0.0.0.0`/`::`/localhost**，地址在 `192.168.0.0/16`，连接按名字走；host 闭包是 `Fn`，
   捕获的东西只能在闭包**里面**克隆。
4. **桩必须用掉自己的参数**（`let _ = (term, index);`），否则学员第一次构建就看到一堆他改不掉的 warning。
5. **后一步要扩展前一步的文件，前一步就得留标记区**；match 里的标记区是"臂的挂载点"，**不能用 catch-all
   `_ =>` 代替**（今天 unreachable，明天吞掉真臂）。
6. **补丁生成必须在格式化之后**：先 `cargo fmt --all` 再 `diff -u`。
7. **改补丁内容必须同步 hunk 头**；更好的做法是永远不手改补丁——先修树，最后生成（段 12）。
8. **补丁段的标签与顺序以已提交补丁为模版**：`--- /dev/null` 是"创建文件"的唯一依据，`--- a/path` 会被当成修改；
   scaffold 只能加不能删。
9. **`diff` 有差异时退出码是 1**：用 `;` 而不是 `&&` 串生成命令。
10. **"只让一个节点超时"是确定性模拟里唯一诚实的选举起点**；配置的采纳是"已提交日志"的性质，不是本地
    `pending` 的性质；joint 期的 `peers()` 必须是并集。
11. **"现在"必须是参数**：租约/去重/窗口的每个函数都收 `now`，自己读钟的函数既没法测也没法重放。
12. **元数据里的 `spec` 路径来自步骤名**（`5.6-lease-reads` → `spec/5.6/lease_reads.rs`，目标
    `spec_lease_reads`）——正文的 DONE WHEN 必须用同一个名字。
13. **错误枚举里的 `use` 不能放在变体区**（`use` 是 item）；变体里需要字符串就用 `Option<String>`。
14. **`// SAFETY:` 只回看 3 行**：说明压到两行以内；**生产代码禁下标**（`clippy::indexing_slicing`）。
15. **deepen 步不许有 `## WHY`**：用 RECALL/DELTA。
16. **路径依赖的相对路径相对 crate 根**：`jobq = { path = "../jobq" }` 在 `app/meshkv` 里指向 `app/jobq`。
17. **`Box<dyn Trait>` 不会自动实现 trait**：SDK 泛型于 port，就得给 `Box<dyn Backend>` 一个转发实现。
18. **剧本要隔离一个节点，得切它与每个 peer 的链路**；而代码里 `isolated()` 判的是"够不到多数派"——
    两个概念别混用（段 12 的第 7 条修复）。
19. **CAS 在提案前拒绝**：leader 能看见的失败不要写进日志。
20. **"已证明的身份"要是一个类型**：`Identity` 没有公开构造器，"这个 peer 认证过"是类型事实。
21. **`conformance@2` 真的会抓东西**：第二个后端第一次跑就暴露了三个真缺口。

## 2. T5 的项目形状

`app/meshkv`：`src/raft/**`（日志、消息、状态机、成员、读、快照）、`src/**`（cluster、node、client、
idempotency、sdk、security、consistency、kv、sim、drill、chaos、budget、metrics、ring、reuse、protocol、
audit）、`src/bin/{meshkvd,meshkvc}.rs`、`src/cli/**`、`docs/**` 十三页、`tests/spec_*.rs` 与 `tests/sim_*.rs`
/`budget_*.rs`/`loom_ring.rs`、`conformance.toml`。详细清单与每步的接口在 [t5-meshkv.md](t5-meshkv.md) §2/§3。

## 3. 施工中钉下来的决定（架构类）

- **D6 用 (a)**：`requires_track = "jobq"` + 砂箱先铺 jobq 参考树；5.20 的验收是仓库级零改动 diff。
  代价是 meshkv 的每一步都要求 jobq 参考树在树上，`kiln verify meshkv` 比别的 track 多一次铺树。
- **miri 不进 CI（D-4）**：本机 1.98.1-aarch64 没有 miri；5.19 用 loom + `unsafe-budget@5` + 两行 `SAFETY`。
- **网络是数据**：turmoil 的拓扑/分区/延迟全是值，种子 + 摘要 + 重放；`slow:sim@3` 是 M4 的主力门。
- **预算是计数**：`bench-budget` 判模型与实测的计数，不判墙钟；5.17 的模型是整数算术。
- **探题按 D-6 出 18 道**，5 个槽位不出题（理由在 t5-meshkv §6）。

## 4. 实测数字与验收演练

| 项 | 值 |
| --- | --- |
| 全步 `selftest` | `22/22 step(s) hold the contract`（每步 scaffold 红、参考解绿、must 门全绿）|
| 全步判定（毕业树） | `kiln verify meshkv`：5.19–5.22 **4/4**；5.1–5.18 判红，唯一原因是 `unsafe-budget@0` 撞上 5.19 的 `src/ring.rs` |
| `kiln verify meshkv --slow --json` | 同上树：`{blocked: 0, clean: false, failed: 18, not_authored: 0, passed: 4, steps: 22}`；慢门全绿（sim 9/9、bench-budget 3/3、conformance 1/1），mutants 4 步被那条 must 门挡住 |
| 离线快路 | 两个 scratch 树都在 `CARGO_NET_OFFLINE=true` + 私有 `XDG_CACHE_HOME` 下建起并判定 |
| 探题池 | `kiln probe --check` → `83 exercise(s) · 83 reference(s) passed` |
| 课程自有检查 | `lint-caps` 97/136/119/119 `no issues.`；`links` 139 页 557 链接 0 findings；`cargo test -q -p kiln` 338+21+1 |
| `cargo xtask ci` | （收尾跑）|

## 5. 缺口

- T5 的 5 个槽位不出题（`dist/consensus@aware` 按政策，其余四个 mechanism 不在单文件里）：见
  [t5-meshkv.md](t5-meshkv.md) §6。
- miri 不可用（同上，loom 只围一个并发面）。
- 集群在进程内：没有真实 TCP/握手/容器编排；滚动升级是调度不是杀进程。
- 5.18 的指标是计数流，不接 Prometheus；5.22 的审计覆盖每个节点上报的已应用位置，不覆盖并发中间态。
