# T1 `mailer`：已交付的 22 步（参考实现）

> 这份文件把 T1 当**完成品**来记：它交付了什么、每一步的元数据与门、项目最后的形状、
> 可以照抄到 T2–T5 的东西，以及实测数字。
>
> 分工：[plan.md](plan.md) 记机制与取舍（为什么这样设计），本文件记**现状与样本**
> （要看什么、抄哪个文件），[handoff-m0-log.md](handoff-m0-log.md) 记施工过程与踩过的坑，
> `course/docs/steps/en/mailer/` 是学员读的正文。冲突时以 `course/` 为准。

## 1. 一句话与资产

`mailer` = 邮件订阅微服务：Rust 1.98 / edition 2024，axum + sqlx(Postgres) + tokio，
从「一个 crate、一个 `main.rs`」长到「多阶段镜像 + runbook」。22 步全部 `origin = "teach"`：
T1 只教新能力，不深化也不迁移（那是 T2 起的事）。

| 资产 | 位置 | 数量 |
| --- | --- | --- |
| 正文（学员读） | `course/docs/steps/en/mailer/<id>.md` | 22 篇 |
| 规格（只读、哈希钉死） | `course/tracks/mailer/spec/<phase>.<index>/<name>.rs` + `manifest.toml` | 22 个测试目标 |
| scaffold（加性补丁 + 骨架） | `course/tracks/mailer/scaffold/*.patch`、`skeleton/` | 19 补丁 + 1 骨架 |
| 参考解（可删改的补丁） | `course/tracks/mailer/solutions/*.patch` | 22 个 |
| 元数据（唯一真源） | `course/tracks/mailer/track.toml` | 22 个 `[[step]]` |
| 学员项目（不跨 worktree 复制） | `app/mailer` | 走完 22 步的状态 |
| 实测草稿（参照物） | `docs/drafts/1.14-delivery.rs`、`docs/drafts/1.21-container/` | 2 份 |

某一步的 scaffold 到底加了哪些文件：`grep '^+++ ' course/tracks/<track>/scaffold/<id>.patch`。

没有 scaffold 的两步是刻意的（authoring.md §5.2）：`1.2-toolchain`（工具链与 lint 策略是学员的决定）
与 `1.18-authz-boundary`（路由 + 限定范围的查询 + 一个错误变体，形状在 1.16 已经演示过）。

## 2. 22 步一览

`门` 列只列该步**新接上**的 must 门；阶梯只增不减（plan.md §6）。每一步都带 `spec-integrity`、
`caps`、`i18n`；1.2 起带 `fmt`、`clippy@1`，1.4 起带 `exit-code`，1.5 起带 `no-todo`、`docs:public`，
1.9 起按需带 `images`，1.21 带 `container-repro` 与 `run.timeout`，1.22 带 `links`。

| id | 这一步交付什么 | teaches | 新接的门 / 其他 |
| --- | --- | --- | --- |
| 1.1-workspace | lib/bin 拆分：`main.rs` 只剩一层壳，服务进 `lib.rs` | `repo/workspace-layout@apply` | （骨架起步） |
| 1.2-toolchain | `rust-toolchain.toml`、`rustfmt.toml`、`[workspace.lints]` | `repo/toolchain-pinning@apply` | `fmt`、`clippy@1` |
| 1.3-ci | 一条命令跑完全部检查（`cargo xtask ci`）+ CI 接线 | `ci/pipeline@apply` | — |
| 1.4-config | 类型化配置：默认 < 文件 < 环境 < 旗标，`--check-config`，密钥只报「已设置」 | `cfg/layered-config@apply`、`sec/secrets-management@apply` | `exit-code` |
| 1.5-errors | `thiserror` 领域错误 + `source` 链 + 退出码表；`anyhow` 只在二进制边界 | `err/typed-errors@apply` | `no-todo`、`docs:public` |
| 1.6-logging | 结构化日志：JSON/pretty 两模式，一次运行一个关联 id（1.7 起包住每个请求） | `obs/structured-logs@apply`、`obs/tracing-propagation@aware` | — |
| 1.7-http | axum 路由、extractor、`/healthz`、诚实的 404/405（带 `Allow`） | `http/service-skeleton@apply` | — |
| 1.8-test-harness | app 工厂：随机端口、每测试独立 schema、能并发跑 | `test/app-factory@apply` | — |
| 1.9-postgres | 连接池 + `sqlx::migrate!`（迁移内嵌）+ 启动顺序 + `/readyz` 与 `/healthz` 之分 | `db/migrations@apply` | `images`（postgres:17-alpine） |
| 1.10-domain-types | `Email`/`SubscriberName`/`Password`：解析即校验，非法值不可表示 | `domain/parse-dont-validate@apply` | — |
| 1.11-signup | 注册：`argon2` 哈希、唯一约束当并发原语、`query_scalar!` + `.sqlx` 离线元数据 | `db/unique-as-concurrency@apply`、`sec/password-hash@apply` | — |
| 1.12-token | 确认令牌：签发/校验/过期/一次性（过期、重放、篡改三种失败各有状态码） | `sec/token-lifetime@apply` | — |
| 1.13-email-port | 邮件发送端口 + `Logging` 适配器 + `Recording` 假实现（注入缝，不用 mock 框架） | `test/fake-external@apply` | — |
| 1.14-worker | 有界队列 + 后台 worker：full-jitter 重试、SIGTERM 后停止接收→排空→退 0 | `async/graceful-shutdown@apply`、`async/backpressure@aware` | — |
| 1.15-idempotent-delivery | 投递状态机：claim → 发一次 → 记账，attempts 落库，重启不归零 | `delivery/idempotency@apply` | — |
| 1.16-sessions | 会话与登录：cookie 会话 + `sha2` 摘要，`/me`、`/logout` | `sec/authn-session@apply` | — |
| 1.17-newsletter | 发布一期：事务内写 issue + deliveries，commit 之后扇出 | `db/transactions@apply` | — |
| 1.18-authz-boundary | 权限边界：订阅者只读自己的投递，越权给 404 | `sec/authz-tenant-isolation@apply` | — |
| 1.19-test-layering | 真实邮件服务器（mailpit 容器）成为默认之外的第二适配器；随机顺序连跑 | `test/integration-layering@apply` | `images`（axllent/mailpit:v1.21） |
| 1.20-metrics | `/metrics`（手写 Prometheus 文本）、按路由计数、`deliveries_total{outcome}` | `obs/metrics-slo@aware` | — |
| 1.21-container | 多阶段镜像：musl 静态二进制、非 root、`0.0.0.0`、探针，先迁移后收流量 | `release/container-image@apply` | `container-repro`、`run.timeout = 1800` |
| 1.22-runbook | design 步：可执行演练（无丢单断言）+ 人读 runbook（`links` 判） | `ops/runbook-drill@apply` | `links` |

阶段划分与学习弧（curriculum.md §6 有同样的四段）：A 1.1–1.6「服务能起来」、
B 1.7–1.13「服务有接口和数据」、C 1.14–1.18「后台与边界」、D 1.19–1.22「质量与交付」。

## 3. 项目最后的形状（T2+ 的骨架样本）

`app/mailer` 走完 22 步后是 18 个模块、10 条路由 + 一条兜底、4 个迁移、一张退出码表、一面镜像。

| 模块 | 职责 |
| --- | --- |
| `main.rs` | 二进制外壳；唯一允许 `anyhow` 的地方；信号 → 排空 → 退出的编排 |
| `lib.rs` | 库入口：把下面这些模块接起来 |
| `config.rs` | 类型化配置与来源叠加（默认/文件/环境/旗标）、`report()` 给 `--check-config` |
| `error.rs` | 领域错误枚举 + `exit_code()` / HTTP 映射（同一套分类，两处口径一致） |
| `logging.rs` | `RunId`、`init`（按配置装 subscriber）、`announce` |
| `http.rs` | 路由表、extractor、优雅关闭的孪生函数（`serve_with_shutdown`） |
| `db.rs` | 连接池、迁移、`ready` 探测 |
| `domain.rs` | `Email`/`SubscriberName`/`Password` 与限额常量 |
| `signup.rs` | 注册：解析 → 哈希 → 插入 → 发确认信 |
| `confirmation.rs` | 确认令牌：`issue`/`parse`/一次性与过期 |
| `email.rs` | `EmailSender` 端口 + `Logging` 适配器 + `Message`/`SendError` |
| `smtp.rs` | `lettre` 适配器（1.19 起，配置了 SMTP 才启用） |
| `delivery.rs` | 有界队列、worker、`backoff`、`drain(deadline)` |
| `deliveries.rs` | 投递状态机：`claim`/`mark_sent`/`release`/`outstanding` |
| `session.rs` | 会话令牌与生命周期（数据库里的那部分） |
| `login.rs` | 登录/登出/查询自己的资料 |
| `newsletter.rs` | 发布一期：事务 + 扇出 |
| `metrics.rs` | 计数器、`/metrics` 的文本渲染 |

| 面 | 内容 |
| --- | --- |
| 路由 | `GET /healthz`、`GET /readyz`、`POST /signup`、`GET /confirm`、`POST /login`、`POST /logout`、`GET /me`、`GET /subscribers/{id}/deliveries`、`POST /newsletter`、`GET /metrics` + 兜底 404 |
| 迁移 | `0001_subscribers`（1.9）、`0002_confirmation_delivery`（1.15）、`0003_sessions`（1.16）、`0004_newsletter`（1.17） |
| 设置 | `listen.host`/`listen.port`、`log.level`/`log.format`、`database.url`、`email.smtp_host`/`email.smtp_port`；旗标 `--config`、`--listen-port`、`--check-config` |
| 退出码 | 0 正常；1 部署方的问题（配置、数据库、队列满、排空超时）；2 调用方的问题（1.5 的表，1.7 起同时映射 HTTP 4xx） |
| 镜像 | 两阶段 `rust:1.98-alpine` → `alpine:3.20`（musl），非 root，`ENV MAILER__LISTEN__HOST=0.0.0.0`；含 `ca-certificates` |
| 依赖（谁带来的） | `axum`/`tokio`（1.7）、`tokio:test-util`（1.14 的暂停时钟）、`sqlx`（1.9/1.11）、`argon2`（1.11）、`getrandom`（1.12）、`sha2`（1.16）、`lettre`（1.19）、`thiserror`（1.5）；`Cargo.toml` 的注释逐条写了「哪一步引的」 |

## 4. 测试装置（夹具与约定）

`tests/support/` 是 scaffold 与规格共享的装置，8 个文件，靠 `#[path = "support/<file>.rs"] mod <file>;`
引入；每个 spec 只用其中一部分，所以文件头统一 `#![allow(dead_code)]`。

| 文件 | 提供什么 | 谁在用 |
| --- | --- | --- |
| `app.rs` | 进程内 app 工厂：随机端口、`TestApp::get/post` | 1.13 起（它的 scaffold 建的） |
| `service.rs` | 真进程 + 能发 header 的客户端（cookie/`Authorization`）、`log_lines`、`stop`/`wait_for_exit` | 1.16 起（认证类规格必须用它） |
| `postgres.rs` | 一测试一 Postgres 容器、随机宿主端口、`pg_isready` 轮询、每测试独立库、`Drop` 里 `rm -f -v` | 1.9 起 |
| `mailpit.rs` | 一测试一 mailpit 容器（真实 SMTP + HTTP 收件箱） | 1.19 起 |
| `email.rs` | `Recording` 假实现：`fail_with` / `clear_failure` / `sent` | 1.13 起 |
| `container.rs` | 镜像夹具：构建（`--build-arg` 传基础镜像名、挂构建缓存）、在容器里发请求 | 1.21 |
| `drill.rs` | 故障演练：`replay(Recovery::{ComesBack,Never}) -> Report`，`Report::nothing_was_lost()` | 1.22 |
| `mod.rs` | 模块声明（1.8 的 scaffold 建的） | 1.8 起 |

三条硬约定（1.21/1.22 各踩过一次，见日志第十四/十五段）：
**夹具文件必须显式 rustfmt**（`cargo fmt --all` 看不到只能被 spec `mod` 到的文件）、
**容器夹具必须 `rm -f -v`**（volumes 泄漏到 2048 会让所有容器 spec 一起崩）、
**等待用事件/轮询条件，不睡觉**（时间相关断言用注入的暂停时钟）。

## 5. 可以照抄到 T2–T5 的模式

| 模式 | T1 的样本 | 什么时候用 |
| --- | --- | --- |
| 探针 crate | `spec/1.5/errors.rs`（spec 自己写 `Cargo.toml`，在临时目录里编译并运行） | scaffold 不该改 `lib.rs` 时：spec 要在 scaffold 上「编得过、跑得过、只失败在断言上」 |
| 加性 scaffold + 标记块 | 任意 `scaffold/*.patch` 与 `skeleton/Cargo.toml` 的 `# kiln:scaffold:begin/end` | 每步都要；插入点必须在既有标记块内 |
| 无 scaffold 的步骤 | `1.2-toolchain`、`1.18-authz-boundary` | 只改学员自己文件、或形状已演示过 |
| 注入缝而不是 mock 框架 | `EmailSender` + `AppState::start_with_sender`（1.13） | 需要假外部依赖时 |
| 假实现与真实现同契约 | 1.13 的一致性 spec；`test/fake-external` 的出处 | 引入端口时 |
| 容器夹具 | `postgres.rs` / `mailpit.rs` / `container.rs` | 任何真外部依赖 |
| design 步 = 演练 + 页面 | `spec/1.22/runbook_drill.rs` + 项目内 `docs/runbook.md` | 每条 track 的末尾；页面受 `links` 判 |
| 规格要在每一棵会被判的树上成立 | 1.6 的修法（空闲端口 + 读到启动结束就停手） | 任何会启动二进制的 spec（authoring.md §6.8） |
| `.sqlx` 离线元数据 | 1.11 起：scaffold 自带该步的 JSON，参考解替换旧的（并删掉旧哈希） | 任何新增/修改 SQL 的步骤 |
| 补丁生成 | `git diff --cached --no-renames --binary` | 每次归档 scaffold/solution |
| 反向验证后提交 | 提交信息里记「把解改成阱的形状，spec 报了什么」 | 每步 |

## 6. 实测数字（这台机器）

| 项 | 值 |
| --- | --- |
| M0 验收（冻结 `c08f33a`） | `cargo xtask ci` 486s（末行 `22/22 step(s) hold the contract`）；`kiln verify --json mailer` 205s（22 passed / 0 failed）；`kiln links` 全仓 0 findings |
| 单步 selftest（照抄给 T2 估时） | 1.21 169s（含镜像构建）；1.22 29s；多数步骤 25–60s（共享 `CARGO_TARGET_DIR` 之后） |
| 镜像 | 成品 **16.5 MB** / 预算 25 MB；构建热态 ~12s、冷态分钟级（依赖要编一遍）；`docker.1ms.run` 前缀 + 本机架构解析见日志第十四段 |
| 容器 spec | postgres 夹具 5 秒级；mailpit 一测试一容器；1.21 的规格热态 6.5s、scaffold 态 150s（含 116s 构建） |
| 已知的不可复现红 | `2d0f004` 上 `verify` 报过 `mailer/1.3-ci` 失败，同树手工连跑三遍全绿；判读与复现步骤写在日志第十二段 |

## 7. T1 范围内剩下的缺口

- **慢门未接**：`mutants`/`coverage`/`bench`/`loom`/`soak`/`sim` 与 `conformance` 都是「目录里有、实现留给后面里程碑」（plan.md §11）。
- **一处刻意保留的 sleep**：`spec/1.17/newsletter.rs` 的 `SETTLE`（给「没有别人被写」这个否定断言留时间），
  理由写在文件里，留档接受。
- **没有译文**：i18n 的回退链与漂移检测就位，任何 locale 都回退英文（`kiln --lang ja list` 会说出来）。
- **一处措辞债**：1.17 scaffold 的 `fan_out` 文档说「在记录它们的那个事务之后运行」，而 scaffold 的
  `publish` 还没有事务；同一文件的模块文档已明说这件事，改它要连带重生成参考解补丁。
