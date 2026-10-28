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
| **M7** | 消息中间件语义：新 track **T7 `broker`** | 同上；分区/消费组/rebalance/lag 各一步 | **前四步已交付 2026-10-28**；回放、跨分区顺序、恰好一次、协议演进待写 |
| **M8** | gRPC 与流式：新 track **T8 `rpc`**（7 步） | 同上；契约、deadline、流式背压、协议演进 | 25–35 h |
| **M9** | 编排与身份：新 track **T9 `platform`**（8 步） | 同上；K8s 清单/探针/rollout、外部 secrets、OIDC | 30–40 h |
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

## 5. M7：T7 `broker` 的步表（前四步已交付）

| 步 | 能力 | 一句话 | 状态 |
| --- | --- | --- | --- |
| 7.1 | `broker/partitioning@apply` | 一个键一个分区，一个分区一个顺序；无键轮转 | **已交付** |
| 7.2 | `broker/consumer-offset@apply` | 提交是下一个要读的偏移，夹到日志末尾 | **已交付** |
| 7.3 | `broker/rebalance@apply` | 范围分配：连续、每区一次、余数在前 | **已交付** |
| 7.4 | `broker/lag@apply` | 末尾减提交、饱和为零；阈值严格大于 | **已交付** |
| 7.5 | `broker/replay@apply` | 从旧偏移重放：读位置与提交位置分开 | 待写 |
| 7.6 | `broker/ordering@design` | 跨分区没有全局顺序：把"需要顺序"的那件事放进一个键 | 待写 |
| 7.7 | `broker/exactly-once@design` | 端到端恰好一次的边界：幂等写 + 事务性提交 | 待写 |
| 7.8 | `broker/schema-compat@apply` | 事件的模式演进：加字段兼容、删字段破坏 | 待写 |
| 7.9 | `ops/lag-alert@operate` | 用 lag 告警而不是用存活告警 | 待写 |

项目形状：`app/broker` 是一个库 + 薄二进制；每个分区就是一个 `Vec`，没有 broker 进程也没有网络。
确定性来自"没有时钟也没有线程"：所有语义都在一次调用里完成。

## 6. 每个新 track 的验收清单（与 `new-track.md` 一致）

- 台账：`curriculum.md` §3 与 `capabilities.toml` 同步（`ledger_matches_curriculum` 测试会核）。
- 退出能力：`exit = [...]` 的每一项由本 track 的某一步建立（R6）。
- 规格：每条都钉在 `/tmp` 的树上验证过"脚手架红、参考解绿"。
- 补丁：由树生成（`tools/rebuild-patches.py`），`--check` 绿。
- 正文：英文 WHY/TASK/DONE WHEN/TRAP/FURTHER（或 RECALL/DELTA），中文译文与词表同步。
- 探题：每个 apply 槽位一道，`probe --check` 绿。
- 站点：`kiln site --check` 绿；`i18n status` 结构 0 error、漂移 0 warning。

## 7. 成本口径

内容按每步 2–4 小时（plan §11 的实测口径），探题按每道 1–1.5 小时（含参考解与
`probe --check`）。全量 `cargo xtask ci` 每加一条 track 增加约 20 分钟；五条 track 时约 80 分钟，
M9 结束时预计 150 分钟以上——这是到时候要把 CI 拆成"内容层 / 全量层"的现实理由（M5 提出过，
仍未做）。
