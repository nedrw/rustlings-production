# 补空白：M6–M10 计划

> 来源：2026-10-28 的空白分析（`docs/handoff-m5-log.md` 段 19 之后）。结论是课程在
> "把服务做对"这条纵轴上已经完整，缺的是**地图的横向几块**和**路径上的两个洞**。
> 本文件只定顺序、判据与成本；每一步的具体内容仍按 [authoring.md](authoring.md) 与
> [new-track.md](new-track.md) 写。

## 0. 起点（实测数字）

- 97 个能力、136 个槽位、119 步、5 条 track、83 道探题。
- 深度：`apply 79 / design 36 / operate 13 / aware 8`。
- 探题覆盖 **83/136**（61%）；**24 个 apply 槽位没有探题**——`plan --fast` 跳不过它们，
  `review --due` 也不会提醒复习。
- 完全没有的能力域：缓存、消息中间件（broker）、SQL 索引与查询计划、数据库复制/切换、
  gRPC/长连接、编排（K8s）/IaC、外部身份（OIDC/OAuth2）、批流处理、搜索。

## 1. 原则（沿用，不新造）

1. **不新增机制**，除非真负载证明需要（plan §3 D4 的同一原则）。本计划的全部内容都用现有机制：
   台账、门阶梯、`selftest`、探题、`new-track`、i18n、`kiln site`。
2. **每个阶段结束时是绿的**：`kiln probe --check` / `lint-caps` / `i18n status` / `links` /
   `site --check` / `cargo test -p kiln`；阶段边界允许时跑一次全量 `cargo xtask ci`。
3. **新 track 走 `new-track.md` 的流程**，不是例外：台账槽位先进 `curriculum.md` §3 与
   `capabilities.toml`（`ledger_matches_curriculum` 会核），再写规格、树、补丁、正文、译文、探题。
4. **一步只教一件事**；新 track 的门阶梯从 `new-track` 的默认阶梯起步，只在教它的那一步加门。

## 2. 阶段

| 阶段 | 内容 | 验收 | 估时 |
| --- | --- | --- | --- |
| **M6.0** | 补齐 24 个 apply 槽位的探题（六批，每批 4 道） | `kiln probe --check` 107/107；每个 apply 槽位都有题 | **已完成 2026-10-28**：107/107、79/79，缺一道即失败 |
| **M6.1** | 缓存与热点：新 track **T6 `cache`** | `selftest --track cache` 全绿；台账/退出能力/探题齐 | **已交付 2026-10-28**：8/8 步、113/113 探题、覆盖 85/85 |
| **M7** | 消息中间件语义：新 track **T7 `broker`** | 同上；分区/消费组/rebalance/lag 各一步 | **已交付 2026-10-28**：9/9 步、119/119 探题、覆盖 91/91 |
| **M8** | gRPC 与流式：新 track **T8 `rpc`** | 同上；契约、deadline、流式背压、协议演进 | **已交付 2026-10-28**：5/5 步、123/123 探题、覆盖 95/95 |
| **M9** | 编排与身份：新 track **T9 `platform`** | 同上；K8s 清单/探针/rollout、外部 secrets、OIDC | **已交付 2026-10-28**：7/7 步、127/127 探题、覆盖 99/99 |
| **M10** | capstone 与协作模板（不是 track） | 一个无 scaffold 项目的门与评审清单；ADR/复盘/评审模板 | 15–20 h |
| 可选 | SQL 索引深度（4 步）、DB 复制/切换演练、批流 ETL、搜索 | 各自独立评估 | — |

顺序的理由：**探题**最便宜且立刻放大现有机制（间隔重复从 61% 到 100%）；**缓存**是后台使用
频率最高的第二层存储，且能复用 mailer/lakelet 的真实场景；**broker** 与 **gRPC** 是"用现成
基础设施"的对照，正好补上 jobq 自建队列没覆盖的运维面；**编排与身份**是"能跑容器"到"能上线"
的最后一公里；**capstone** 把"跟做"变成"自己做"。

## 3. M6.0：24 道探题（六批）

每道题 = 练习文件 + 参考解（`course/probes/<slug>.rs` 与 `solutions/` 同名文件），
头三行 `probe` / `title` / `hint`，正文写清"对应哪一步、简化了什么、你写什么、测试问什么"。

| 批 | 槽位（教它的步） | 状态 |
| --- | --- | --- |
| **A** | `db/transactions`（1.17）· `api/pagination`（2.4）· `async/graceful-shutdown`（1.14）· `sec/password-hash`（1.11） | **已写（2026-10-28）**：`probe --check` 87/87，覆盖 59/79 |
| B | `repo/workspace-layout`（1.1）· `repo/toolchain-pinning`（1.2）· `ci/pipeline`（1.3）· `sec/secrets-management`（1.4） | **已写（2026-10-28）**：91/91，覆盖 63/79 |
| C | `obs/structured-logs`（1.6）· `http/service-skeleton`（1.7）· `test/app-factory`（1.8）· `db/migrations`（1.9） | **已写（2026-10-28）**：95/95，覆盖 67/79 |
| D | `test/fake-external`（1.13）· `sec/authn-session`（1.16）· `test/integration-layering`（1.19）· `release/container-image`（1.21） | **已写（2026-10-28）**：99/99，覆盖 71/79 |
| E | `ops/runbook-drill`（1.22）· `http/middleware-layering`（2.1）· `api/idempotency-key`（2.2）· `api/contract-testing`（2.3） | **已写（2026-10-28）**：103/103，覆盖 75/79 |
| F | `async/backpressure`（2.14）· `test/chaos-injection`（2.17）· `ops/rolling-upgrade`（2.22）· `ops/degradation-modes`（2.24） | **已写（2026-10-28）**：107/107，覆盖 **79/79** |

写题的三条硬规矩（来自已有的 83 道）：

1. **一文件、零依赖**：只用 `std`，离线可跑，秒级。需要的"外部世界"用最小模型代替
   （表是 `BTreeMap`，事务是 `&mut self`，时钟是参数），并在正文里说清替换了什么、为什么不是假的。
2. **每道题都要有失败面**：至少三条测试在 `todo!()` 上失败，且其中一条测的是"错误路径不留下部分结果"。
3. **参考解与练习只差函数体**：其余字节相同，这样 `kiln probe --check` 编的是学员要写的那份文件。

## 4. M6.1：T6 `cache` 的步表（八步已交付）

| 步 | 能力 | 一句话 | 状态 |
| --- | --- | --- | --- |
| 6.1 | `cache/read-through@apply` | 读穿缓存与存活时间 | **已交付** |
| 6.2 | `cache/invalidation@apply` | 写路径先写存储再删缓存 | **已交付** |
| 6.3 | `cache/single-flight@apply` | 一个键一个填充者 | **已交付** |
| 6.4 | `cache/expiry-jitter@apply` | 过期是一个区间，按键确定 | **已交付** |
| 6.5 | `cache/versioned-keys@apply` | 键里带版本：bump 前缀让整片不可达 | **已交付** |
| 6.6 | `cache/two-level@apply` | 本地/共享两层，陈旧预算写下来 | **已交付** |
| 6.7 | `obs/cache-effectiveness@operate` | 命中率、回源次数与 SLO 边界 | **已交付** |
| 6.8 | `ops/cache-failure@operate` | 缓存挂了：熔断、探针、重新闭合 | **已交付** |

项目形状（实际落地的，与草稿不同）：`app/cache` 是一个**库 + 薄二进制**，`Store` 是计数的
`BTreeMap`（"慢存储"），`Cache` 是带截止时间的条目表，**时钟是每个 API 的参数**。原草稿里
`Mutex` + "共享缓存"的模型被放弃了：这一步的判定全是单线程语义，加一个 `Mutex` 只会让测试变慢、
让失败变成调度；并发的那一半（单飞）用"进行中标记"的状态机表达，恰好是我们想让人重新写一遍的
东西。6.5 的版本化缓存自带条目表（不复用 6.1 的读穿缓存）——因为被版本化的是**缓存键**，存储的键
不变，而 `Cache::get` 只会用同一个键问存储；这个区别本身就是 6.5 的题眼。

## 5. M7：T7 `broker` 的步表（九步已交付）

| 步 | 能力 | 一句话 | 状态 |
| --- | --- | --- | --- |
| 7.1 | `broker/partitioning@apply` | 一个键一个分区，一个分区一个顺序；无键轮转 | **已交付** |
| 7.2 | `broker/consumer-offset@apply` | 提交是下一个要读的偏移，夹到日志末尾 | **已交付** |
| 7.3 | `broker/rebalance@apply` | 范围分配：连续、每区一次、余数在前 | **已交付** |
| 7.4 | `broker/lag@apply` | 末尾减提交、饱和为零；阈值严格大于 | **已交付** |
| 7.5 | `broker/replay@apply` | 重放是半开区间：不移动任何提交 | **已交付** |
| 7.6 | `broker/ordering@design` | 排序约束的连通组一个键，其余并行 | **已交付** |
| 7.7 | `broker/exactly-once@design` | 效果与偏移同表：重放不重复、崩溃不丢失 | **已交付** |
| 7.8 | `broker/schema-compat@apply` | 加法兼容、减法破坏，必填新增也算减法 | **已交付** |
| 7.9 | `ops/lag-alert@operate` | 严格高于阈值、连续 N 次；低谷重置 | **已交付** |

项目形状：`app/broker` 是一个库 + 薄二进制；每个分区就是一个 `Vec`，没有 broker 进程也没有网络。
确定性来自"没有时钟也没有线程"：所有语义都在一次调用里完成。

## 6. M8：T8 `rpc` 的步表（五步已交付）

| 步 | 能力 | 一句话 | 状态 |
| --- | --- | --- | --- |
| 8.1 | `rpc/field-numbers@apply` | 线路是号码不是名字：改名兼容、删号/复用号破坏 | **已交付** |
| 8.2 | `rpc/deadline@apply` | 一个绝对 deadline，每跳从里面付；装不下的跳先失败 | **已交付** |
| 8.3 | `rpc/status-codes@apply` | 规范状态码；可重试只含瞬时错误 | **已交付** |
| 8.4 | `rpc/server-streaming@apply` | 流是窗口不是缓冲：在途到窗口即停 | **已交付** |
| 8.5 | `rpc/multiplexing@design` | 一条连接多条流：每轮每流一项，阻塞不拖累 | **已交付** |

项目形状：`app/rpc` 是一个库 + 薄二进制；契约是数据、deadline 是数字、流是窗口——没有网络也没有
代码生成，所以每条判断都能在一次调用里复现。

## 7. M9：T9 `platform` 的步表（七步已交付）

| 步 | 能力 | 一句话 | 状态 |
| --- | --- | --- | --- |
| 9.1 | `k8s/probe-wiring@apply` | 存活说进程、就绪说服务；就绪必须有且真检查 | **已交付** |
| 9.2 | `k8s/rollout@apply` | 就绪下限 + 在途上限；0 不可用需要 surge | **已交付** |
| 9.3 | `k8s/disruption-budget@apply` | 两种写法、封顶、不为负；零次即挡排干 | **已交付** |
| 9.4 | `sec/external-secrets@operate` | 引用解析严格；清单里出现值就是泄漏 | **已交付** |
| 9.5 | `sec/oidc@apply` | 算法先验，再签发方、受众、未生效、过期 | **已交付** |
| 9.6 | `k8s/autoscaling@design` | 公式 + 容差带 + 快上慢下 | **已交付** |
| 9.7 | `k8s/rollout-drill@operate` | 逐步核两个预算，不只结尾 | **已交付** |

## 8. 每个新 track 的验收清单（与 `new-track.md` 一致）

- 台账：`curriculum.md` §3 与 `capabilities.toml` 同步（`ledger_matches_curriculum` 测试会核）。
- 退出能力：`exit = [...]` 的每一项由本 track 的某一步建立（R6）。
- 规格：每条都钉在 `/tmp` 的树上验证过"脚手架红、参考解绿"。
- 补丁：由树生成（`tools/rebuild-patches.py`），`--check` 绿。
- 正文：英文 WHY/TASK/DONE WHEN/TRAP/FURTHER（或 RECALL/DELTA），中文译文与词表同步。
- 探题：每个 apply 槽位一道，`probe --check` 绿。
- 站点：`kiln site --check` 绿；`i18n status` 结构 0 error、漂移 0 warning。

## 9. 成本口径

内容按每步 2–4 小时（plan §11 的实测口径），探题按每道 1–1.5 小时（含参考解与
`probe --check`）。全量 `cargo xtask ci` 每加一条 track 增加约 20 分钟；五条 track 时约 80 分钟，
M9 结束时预计 150 分钟以上——这是到时候要把 CI 拆成"内容层 / 全量层"的现实理由（M5 提出过，
仍未做）。**实测（2026-10-29）：九条 track 离线一次 1 小时 33 分**，比线性外推略低（`target/` 缓存
与离线解析省下的时间）。

## 10. 已知边界（M6–M10 交付后评审的结论，2026-10-29）

这些不是待办，是**写进档案的边界**——读到它们的人应该知道这门课在哪里停手，以及为什么停。

1. **T9 的 `design`/`operate` 步只有代码级证据。** 9.4/9.6/9.7 教的是清单、公式与演练的判断，
   但 `app/platform` 里没有集群：没有真的 HPA、没有真的 PodDisruptionBudget、没有真的发布。
   证据是"给定输入下这个判断对不对"，不是"在集群里发生过"。这是 plan §3 的"无运行时、可复现"原则
   （与 T7 无 broker 进程、T8 无网络同一取舍），但代价要说清：学员毕业后第一次碰真集群，仍要重新
   校准这些判断与 API server 行为之间的距离。
2. **探题只强制 `apply`。** `design`/`operate` 的槽位只在"有值得重写的一小段判断"时出题（如
   7.6 排序分组、9.6 容差带），其余没有题；`kiln probe --check` 的覆盖率只要求 `apply` 槽位全有题。
   理由在 `curriculum.md` §15：全覆盖是 165 道题的真实成本，而 `aware`/部分 `design` 的复习价值低于
   它的维护成本。
3. **规格里的「给定代码」用例。** 29 步里有 9 步至少有一条测试只调用脚手架给定的代码（`poll`、
   `may_create`、`version`…），它们在脚手架上就通过（共 14 条）。规则是：这类用例必须在上面写
   `// Pins given code: …`（2026-10-29 的评审已补齐）；`selftest` 判的仍是"整个规格文件在
   脚手架上失败"，而本步的判断必须由至少一条失败的用例负责。
4. **脚手架警告没有机器检查。** `selftest` 不判警告，`cargo xtask ci` 也不。29 步实测（每步两棵树）
   的教训是：桩的参数与只写不读的字段都要点名，否则学员第一次 `cargo build` 就看到警告。规则与判据
   写在 `authoring.md` §5.6，但执行靠作者；要机器化，需要给 `selftest` 加一个"脚手架零警告"的检查
   （本次未做，因为它会让每条 track 的 selftest 多一次全量 `cargo build`）。
5. **闸门阶梯是按 track 的，不是全课一条。** 新 track 的 `gates = [...]` 从 `clippy@1` 起，
   只有承接了前面能力（如 `tmp/backup-restore`）的步骤才升到 `@2`——理由写在 `plan.md` §7。
