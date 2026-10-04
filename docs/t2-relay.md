# T2 `relay`：Webhook 投递服务（参考实现）

> 这份文件把 T2 当**完成品**来记（与 [t1-mailer.md](t1-mailer.md) 同构）：24 步各交付什么、门阶梯、
> 项目最后的形状、可复用的装置与模式、实测数字、以及这一步留下的缺口。
> 冲突时以 `course/` 为准；机制与取舍在 [plan.md](plan.md)，能力账本在 [curriculum.md](curriculum.md) §3/§7。

## 1. 一句话与资产

`relay` = webhook 投递服务：HTTP 接收事件（幂等）→ 持久化 → 按订阅者投递（HMAC 签名）→ 重试/退避/熔断 →
DLQ → 多租户与授权矩阵 → 限流与背压 → 观测 → 负载与容量 → 滚动升级与在线 schema 变更 → runbook v2。
24 步 = 18 teach + 1 transfer（2.1）+ 5 deepen（2.15/2.16/2.18/2.19/2.24）。

| 资产 | 位置 | 数量 |
| --- | --- | --- |
| 正文（英文源） | `course/docs/steps/en/relay/<id>.md` | 24 篇 |
| 规格（只读、哈希钉死） | `course/tracks/relay/spec/<phase>.<index>/<name>.rs` + `manifest.toml` | 24 个测试目标 |
| scaffold（加性补丁 + 骨架） | `course/tracks/relay/scaffold/*.patch`、`skeleton/` | 23 补丁 + 1 骨架 |
| 参考解 | `course/tracks/relay/solutions/*.patch` | 24 个 |
| 元数据（唯一真源） | `course/tracks/relay/track.toml` | 24 个 `[[step]]` |
| 学员项目 | `app/relay` | 走完 24 步的状态 |
| probe（间隔重复） | `course/probes/` | 与 T2 相关的 8 道（tracing / load / deny / outbound / hmac / rate-limit / atomic-claim / metrics-slo 等） |

## 2. 24 步一览

`门` 列只列该步**新接上**的 must 门；阶梯只增不减。每步都带 `spec-integrity`、`caps`、`i18n`、
`fmt`、`clippy@2`、`no-todo`、`docs:public`、`exit-code`；`images` 只给真正要 Postgres 的步骤，
`links` 只给交付页面的步骤（2.6/2.8/2.16/2.20/2.24），`?deny` 是 2.21 的 should 门。

| id | 这一步交付什么 | teaches | origin |
| --- | --- | --- | --- |
| 2.1-project-skeleton | 重建服务形状：类型化配置、结构化日志、迁移、**中间件分层**（关联 id → 请求日志 → 认证 → handler） | `http/middleware-layering@apply` | transfer |
| 2.2-idempotent-ingest | 幂等接收：`Idempotency-Key` + 唯一约束，重放答第一个事件 | `api/idempotency-key@apply` | teach |
| 2.3-contract-first | 契约优先：`GET /openapi.json` 由服务发布，规格机械校验路径/schema/兼容性 | `api/contract-testing@apply` | teach |
| 2.4-cursor-pagination | 游标分页：`(received_at, id)` 双列严格序，翻页不重不漏 | `api/pagination@apply` | teach |
| 2.5-atomic-claim | 原子领取：单条 `FOR UPDATE SKIP LOCKED`，行自己说谁持有 | `db/atomic-claim@apply` | teach |
| 2.6-at-least-once | 至少一次：ADR + 崩溃窗口的测试（不丢；可能重） | `delivery/at-least-once@design` | teach |
| 2.7-outbound-client | 出站客户端：三层 deadline、keep-alive、重定向策略、响应分类 | `outbound/http-client@apply` | teach |
| 2.8-retry-budget | 重试预算：确定性抖动、上限、每端点熔断 | `delivery/retry-budget@design` | teach |
| 2.9-ordering | 顺序：每端点一条 lane，慢端点不挡别人 | `delivery/ordering@apply` | teach |
| 2.10-dlq | DLQ：两种终态、可查询、可重放，毒丸不阻塞 | `delivery/dlq@apply` | teach |
| 2.11-hmac-signing | 签名：HMAC-SHA256 + 时间戳 + 投递 id，常数时间比较 | `sec/hmac-signing@apply` | teach |
| 2.12-ssrf-defense | SSRF：目标校验（解析一次、连解析出的地址）、重定向逐跳复验 | `sec/ssrf-defense@apply` | teach |
| 2.13-rate-limit | 限流：按租户令牌桶、`Retry-After`、桶表有上限 | `http/rate-limit@apply` | teach |
| 2.14-backpressure | 背压：有界并发 + 队列有底（503）+ 公平 claim | `async/backpressure@apply` | teach |
| 2.15-shutdown-leases | 停机深化：在途租约的回收与快速恢复（`kill -9` 后不永久卡死） | `async/graceful-shutdown@design` | deepen |
| 2.16-authz-audit | 授权矩阵 + 审计（先写行再执行）+ 无后门 | `sec/authz-tenant-isolation@design` | deepen |
| 2.17-chaos-injection | 故障注入：进程内故障代理（延迟/RST/半开/周期丢包）+ 不丢单 | `test/chaos-injection@apply` | teach |
| 2.18-delivery-metrics | 投递可观测：延迟直方图、按租户积压、固定标签集 | `obs/metrics-slo@apply` | deepen |
| 2.19-trace-propagation | `traceparent` 跨边界：同一 trace id、新 span id、落到投递行 | `obs/tracing-propagation@apply` | deepen |
| 2.20-load-testing | 负载测试与容量：确定性生成器、百分位、`docs/capacity.md` | `perf/load-testing@apply` | teach |
| 2.21-supply-policy | 供应链政策：`deny.toml` + `forbid(unsafe_code)` + licence 覆盖 | `supply/deny-policy@apply`、`unsafe/budget@aware` | teach |
| 2.22-rolling-upgrade | 滚动升级：探针语义、先迁移再 ready、双版本窗口 | `ops/rolling-upgrade@apply` | teach |
| 2.23-online-schema | 在线 schema 变更：expand → backfill → contract，老代码仍能查 | `db/schema-online-evolution@aware` | teach |
| 2.24-degradation-drill | runbook v2 + 三类可复跑演练（DB 断开、下游慢、租户风暴） | `ops/degradation-modes@apply` | deepen |

## 3. 项目最后的形状

| 面 | 内容 |
| --- | --- |
| 模块（30 个 `src/*.rs`） | `auth` `audit` `authz` `backfill` `backoff` `claim` `concurrency` `config` `db` `delivery` `dlq` `domain` `error` `events` `http` `lease` `loadtest` `logging` `main` `metrics` `openapi` `ordering` `outbound` `pagination` `ratelimit` `readiness` `signing` `target` `trace` + `lib` |
| 路由（9 条 + 兜底） | `GET /healthz`、`GET /readyz`、`GET /openapi.json`、`POST /events`、`GET /events`（游标分页 + `kind` 过滤）、`POST /endpoints`、`GET /endpoints`、`GET /dlq`、`POST /dlq/{id}/replay`、`GET /audit`、`GET /metrics`、兜底 404/405 |
| 迁移（11 条） | `0001_tenants` `0002_endpoints` `0003_events` `0004_events_cursor_index` `0005_deliveries` `0006_dead_letters` `0007_lease_reclaim` `0008_audit` `0009_trace` `0010_deliveries_endpoint_idx`（`concurrently`、`-- no-transaction`）`0011_events_payload_bytes`（可空列 + 回填索引） |
| 设置 | `listen.host`/`listen.port`、`log.level`/`log.format`、`database.url`、`bootstrap.tenants`、`delivery.{allow_private_targets,max_backlog,max_in_flight,lease_seconds,drain_seconds}`、`rate_limit.{per_second,burst}`；旗标 `--config`、`--listen-host`、`--listen-port`、`--check-config`、`--backfill` |
| 契约面（跨步骤的签名） | `claim::{claim,claim_fair,release,mark_sent,mark_retry,mark_dead,reclaim_expired}`、`delivery::{tick,Sender,SendRequest,SendError,Receipt}`、`outbound::{Client,Policy,Outcome,HttpSender,Request,Response}`；端口 `Sender`/`Secrets`/`Traces`/`Clock` 参数化 |
| 项目内页面 | `docs/adr/0001-at-least-once.md`、`docs/adr/0002-retry-budget.md`、`docs/capacity.md`、`docs/runbook.md` |
| 依赖（谁引入的） | 骨架：axum/tokio/sqlx/serde/serde_json/thiserror/tracing/tracing-subscriber/uuid/chrono/toml/sha2；2.11 引入 `hmac 0.12` + `subtle 2.6`；其余全部用标准库与已有依赖（**有意不引入** `reqwest`（2.7 手写 HTTP/1.1）、`governor`（2.13 自写令牌桶）、`proptest`（2.4 用确定性生成器）、`toxiproxy`（2.17 用进程内故障代理）——每条都在对应步骤的正文里写了理由） |

## 4. 测试装置与可复用模式

- **骨架自带**：`tests/support/{mod,client,app,service,postgres}.rs`（T1 装置的改写：HTTP 客户端可发/读头，`App` 进程内、`Service` 真进程、`Postgres` 一容器一库）。
- **步骤新增**：`target.rs`（脚本化目标服务器）、`sender.rs`（记录型假发送器）、`lanes.rs`、`keys.rs`、`faults.rs`（进程内故障代理）、`load.rs`/`workload.rs`（负载）、`drill.rs`（三类降级演练）。
- T1 的模式在这里的落法：加性 scaffold + 标记块（`src/lib.rs` 没有标记块，所以「声明新模块」由参考解做，scaffold 态是**编译红**，kiln 会加注 `does not compile on the scaffold …`）；注入缝（`Sender`/`Secrets`/`Traces`/`Clock`）；容器夹具 `rm -f -v`；不需要 `.sqlx`（全用运行时 `query`/`query_as`）。
- **T2 新增的三条模式**（记在 [handoff-m1-log.md](handoff-m1-log.md) 第四/六段与本文 §4；
  `docs/authoring.md` §9 的清单同步新增了「先查台账」「相邻能力边界」「RECALL 段」「probe 题」四条）：
  1. **跨步骤的契约面**：改 `claim::*`/`SendRequest`/`outbound::*` 的签名要同步改早期规格并重钉 `spec manifest`（2.11 为此把密钥做成端口，2.19 同理）；
  2. **配置层的安全默认**：2.12 之后默认拒私网/回环，所有打 `127.0.0.1` 的规格显式放行（`RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS=true`），且用**字符串字面量**而不是装置常量（早期树里没有那个常量）；
  3. **规格必须在每棵树上成立**：2.22 的规格按「哪个迁移建出了这个索引」而不是「目录里最新的迁移」来找目标（2.23 落地后前者才会在最终树上成立）。

## 5. 实测数字（这台机器）

| 项 | 值 |
| --- | --- |
| `kiln verify relay`（最终树） | **24/24 judged pass · 0 failed · 0 not prepared · 0 without content**，2m34.8s |
| `kiln selftest --track relay`（2.20–2.24，一条沙箱） | `5/5 step(s) hold the contract`，1m45s；单步 25–120s（含 Postgres 容器的步骤是秒级到十秒级） |
| `kiln lint-caps` | `97 capabilities · 134 slots · 118 ledger positions · 46 authored steps` → `no issues.`（46/46 篇正文齐、无 skipped） |
| `kiln links`（全仓、跳过 `app/`） | `57 page(s) · 370 link(s) · 228 external target(s)` → 0 findings |
| `i18n status` | `en (46/46) [source]`；structure 0 error、drift 0 warning |
| course 下 rustfmt | 89 个 `.rs` 全绿（`--config skip_children=true --check`） |
| 容器 | 每一步的 `images` 门在 pull 之后即绿；本机 Postgres 夹具 5 秒级 |
| 负载（2.20 页面 `docs/capacity.md` 里） | debug 构建：曲线 8.8→38.3→43.4 ms（backlog 119→237→353）；400 投递排空 364 ms；同一组 8 个 seat：1 worker 363 ms / 2 workers 551 ms / 8 workers 1.87 s |

## 6. T2 范围内剩下的缺口

- `test/chaos-injection@apply` 与 `delivery/at-least-once@design` 没有 probe 题（理由写在 handoff-m1-log.md 第七段：单文件 std 题会变成假证据）。
- `delivery.lease_seconds` 与 `outbound::Policy.deadline` 之间没有交叉校验（2.15 的 TRAP 6 点名了这个张力：`lease_seconds = 1` 配 `Policy::deadline = 30s`；2.22 只在正文里引用 2.15，没有重复它）。**决定：接受，不补规格** —— 首先，契约本身就是「至少一次」（2.6），过期租约造成的重复投递在承诺之内；其次，要做成断言就得把配置值和一个代码常量交叉校验，并且重开 2.15 这个已验证步骤的规格与参考解（所有产物要重钉、重新 verify）。若以后要补，落点是在 2.15 的规格里断言「租约短于尝试上限时启动/`--check-config` 必须拒绝」。
- `serve` 的 `Report` 只讲轮次、不讲投递数（2.18 的指标不依赖它，正文写明）。
- toxiproxy 被进程内故障代理替代（2.17 正文说明：离线、确定性、少一个镜像）。
- `?deny` 仍是 should 门（本机没装 `cargo-deny`）：2.21 的规格离线判策略与依赖树，`kiln doctor` 给安装命令。
