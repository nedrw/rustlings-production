# M0 构建日志（分段记录）

> 这是 [handoff-m0.md](handoff-m0.md) 背后的**构建日志**：按时间顺序记每一段的进度、实测数字与踩过的坑。
> 验收结论与现状清单在简报里，不在这里。
>
> 读法：要接着做某一步、或要重做一个已经量过的决定时，先在这里搜那一步的 id、那个门的名字。
> 日志只代表当时的状态——正文里出现的「尚未实现」「还没写」是那一段的真实情况，不要当现状读。
> 追加方式：新的一段写到文件末尾，编号递增，标题照 `## 第N段 · 日期 · 做到 X` 的格式，段尾写清当段实测的命令与数字。

## 索引

- [第二段 · 2026-10 · T1 1.4–1.6 完成](#第二段--2026-10--t1-1416-完成) — T1 1.4–1.6 完成；`exit-code` /
  `docs:public` 落地
- [第三段 · 2026-10 · 停在 1.9 门口（1.7/1.8 完成，1.9 设计定稿）](#第三段--2026-10--停在-19-门口1718-完成19-设计定稿) — 1.7/1.8
  完成；1.9 设计定稿（镜像前缀、规格自带截止时间）
- [第四段 · 2026-10 · 做到 1.9（Postgres 与迁移，`images` 门落地）](#第四段--2026-10--做到-19postgres-与迁移images-门落地) —
  做到 1.9：Postgres 与迁移；`images` 门、补丁解析器修好
- [第五段 · 2026-10 · 做到 1.10（领域类型）](#第五段--2026-10--做到-110领域类型) — 做到 1.10：领域类型；探针 crate 的写法约定
- [第六段 · 2026-10 · 做到 1.11（注册）](#第六段--2026-10--做到-111注册) — 做到 1.11：注册；sqlx / argon2 的实测事实
- [第七段 · 2026-10 · 做到 1.12（确认令牌）](#第七段--2026-10--做到-112确认令牌) — 做到 1.12：确认令牌；反向验证成为常规动作
- [第八段 · 2026-10 · 做到 1.13（邮件端口与假实现）](#第八段--2026-10--做到-113邮件端口与假实现) — 做到 1.13：邮件端口与假实现；与设计的两处不同
- [第九段 · 2026-10 · 做到 1.14（后台投递 worker）](#第九段--2026-10--做到-114后台投递-worker) — 做到 1.14：后台投递 worker（后面跟
  1.14 设计与 1.13 的设计原文留档）
- [第十段 · 2026-10 · 做到 1.15（幂等投递）](#第十段--2026-10--做到-115幂等投递) — 做到 1.15：幂等投递；patch 不能有 rename 段
- [第十一段 · 2026-10 · 做到 1.18（授权边界）](#第十一段--2026-10--做到-118授权边界) — 做到
  1.16–1.18：会话、newsletter、授权边界（第一个没有 scaffold 的步骤）
- [第十二段 · 2026-10 · 做到 1.20（指标），并留下 1.15–1.19 的坑](#第十二段--2026-10--做到-120指标并留下-115119-的坑) — 做到
  1.19–1.20：mailpit、指标；四个环境/工具教训；1.21 的两次尝试留档
- [第十三段 · 2026-10 · `links` 闸门落地：判树，不判外链](#第十三段--2026-10--links-闸门落地判树不判外链) — `links` 门落地：判树不判外链；留给
  1.22 的两个约束
- [第十四段 · 2026-10 · 做到 1.21（容器化与 `container-repro`）](#第十四段--2026-10--做到-121容器化与-container-repro) —
  做到 1.21：容器化；`container-repro`；基础镜像解析与可选构建缓存
- [第十五段 · 2026-10 · 1.22 落地，T1 22/22 写完](#第十五段--2026-10--122-落地t1-2222-写完) — 1.22 落地：design 步骤模板、夹具
  `#![allow(dead_code)]`、runbook 的链接
- [第十六段 · 2026-10 · M0 收尾：三份呈现自检、一次不可复现的红，与它的真因](#第十六段--2026-10--m0-收尾三份呈现自检一次不可复现的红与它的真因) — 三份呈现自检（正文 / CLI / 文档，30 条发现）、T1 文档整理、
  以及那次不可复现的红：真因是 1.18 没等待的投递断言，端口碰撞是另一处，一并修了

## 第二段 · 2026-10 · T1 1.4–1.6 完成

T1 1.4–1.6 内容完成（正文 + 规格 + scaffold + 参考解），
`exit-code` 与 `docs:public` 两个闸门已实现，`cargo xtask ci` 全绿，`kiln selftest` 对 1.1–1.6 全绿，
`kiln verify mailer` 里 6 步通过、16 步未写（exit 3 = 还有内容没写，见 cmd/verify.rs）。
本段与规划有两处出入，都已改文档：1.5 的「状态码映射」在还没有 HTTP 的这一步落成**退出码**映射
（1.7 起同一套分类映射到状态码）；1.6 的「每次请求一个 request id」改为「一次运行一个关联 id」
（工作单位是 run 的 span，1.7 用同一机制包住每个请求）。另外 `kiln verify` 发现 1.1 的
`main.rs` 行数预算（20）撑不过边界自然变大（1.6 时是 58），已放宽到 100 并写清它的含义。
下一步从 1.7 继续，按同样的顺序：正文 → 规格 → scaffold → 参考解 → 接闸门。

## 第三段 · 2026-10 · 停在 1.9 门口（1.7/1.8 完成，1.9 设计定稿）

T1 1.7（HTTP 骨架）与 1.8（app 工厂 / 测试基建）已完
成并提交，`cargo xtask ci` 全绿，`kiln selftest` 对 **1.1–1.8 共 8 步**满足契约（scaffold 必败 /
参考解必绿 / must 闸门全绿）。1.9-postgres 已经调研并设计到位，但**尚未提交**，下一段从这里接着做。

### 1.9 已经确定的事（不要再重新调研）

**容器可用，走镜像前缀。** 本机 podman 6.1.2 能跑，但 Docker Hub 直连拉取挂住；`docker.1ms.run`
镜像源可用（`docker.m.daocloud.io`、`dockerproxy.net` 也留有缓存）：

```sh
podman pull docker.1ms.run/library/postgres:17-alpine     # 294 MB，本地已缓存
podman run -d --name kiln-pg -e POSTGRES_PASSWORD=postgres -p 127.0.0.1::5432 docker.1ms.run/library/postgres:17-alpine
```

课程侧的口径：镜像按 `KILN_REGISTRY_MIRROR` 前缀解析（`kiln doctor` 已实现该报告），步骤声明
`images = ["postgres:17-alpine"]`，`images` 闸门（**尚未实现**）要做的就是「本地镜像库里有没有这个
镜像 + 没有就给带前缀的 pull 命令」。

**测试用的 Postgres 由规格自己起。** 新增的 `tests/support/postgres.rs`（scaffold 创建、由规格用
`#[path = "support/postgres.rs"] mod postgres;` 引入）按 podman→docker 顺序探测，`-p 127.0.0.1::5432`
拿随机宿主端口，`pg_isready` 轮询到就绪，`psql -c` 建每测试独立数据库，`Drop` 时 `rm -f` 容器。
容器是 per-test 的（5 秒级），符合「测试之间不共享任何东西」。

**sqlx 的两个事实（实测）。** `sqlx::migrate!("./migrations")` 不需要数据库就能编译；`sqlx::query!`
/`query_scalar!` 宏需要「编译期能连到数据库」或 `.sqlx/query-<hash>.json` 缓存（宏找不到缓存时的报错
是「set `DATABASE_URL` … or run `cargo sqlx prepare`」，缓存查找顺序是 `SQLX_OFFLINE_DIR` →
crate 的 `.sqlx/` → workspace 的 `.sqlx/`）。本机**没有** `sqlx-cli`，`cargo sqlx prepare` 需要先
`cargo install sqlx-cli --no-default-features --features postgres,rustls`（未做，耗时较长）。
因此 1.9 的这一条查询用运行时检查的 `sqlx::query_scalar`，编译期检查的宏留给 1.11 的第一条真实查询，
届时把 `.sqlx/` 作为 scaffold 内容一起发布；这一点要写进 1.9 正文与 curriculum §6 的注记。

**1.9 的设计（已定，未落盘）。** `src/db.rs`：`Database { pool }` + `connect`/`migrate`/`ready`/`pool`；
`migrations/0001_subscribers.sql` 建 `subscribers`（email 唯一、status、confirmation_token/expires、
confirmed_at、subscribed_at，附两条 CHECK，1.10–1.18 直接在此之上加迁移）；启动顺序 = 配置 → 日志 →
**连库 + 迁移** → 服务；`http::AppState { config, database: Option<Database> }` + `GET /readyz`（无库
或库不可用 → 503，liveness `/healthz` 仍 200）；**数据库可选但一旦配置就必须可用**（未配置时启动并
警告，配置了却连不上就退出非 0）——这是为了让 1.6/1.7/1.8 的规格在最终树上仍然成立（它们直接启动二进
制、没有数据库），也为 1.22 的断连演练留出「运行中失去数据库」的语义。

**规格里必须给自己设截止时间。** 1.9 的「库连不上必须退出」测试用 `Child::try_wait` + 10 秒期限 +
`kill`，不能用 `Command::output()`：在 scaffold 状态下桩实现会照常启动并一直服务，没有期限就会挂住
（本次就是因此耗掉了最后一段预算）。

## 第四段 · 2026-10 · 做到 1.9（Postgres 与迁移，`images` 门落地）

T1 1.9（Postgres + 迁移）已完成并提交——正文、规格、scaffold（`sqlx`
依赖 + `src/db.rs` 接口 + `migrations/0001_subscribers.sql` 占位 + 容器/`psql` 测试装置
`tests/support/postgres.rs`）、
参考解（连接池·迁移·就绪 + `AppError` 两个变体 + `AppState::start` 的启动顺序 + `/readyz` + 工厂同序建状态）。
`cargo xtask ci` 全绿，`kiln selftest` 对 **1.1–1.9 共 9 步**满足契约（scaffold 必败 / 参考解必绿 / must 闸门全绿，
1.9 是 9/9，多出来的是新实现的 `images` 闸门）。

### 本段新落地的机制

**`images` 闸门与 `kiln images`。** 步骤声明 `images = ["postgres:17-alpine"]`，闸门问引擎本地有没有
（`podman image inspect` 级别的探测），没有就给出带镜像前缀的 pull 命令；没引擎算 `blocked`，不算过。
`kiln images` 报告全课程声明的镜像，`kiln images pull` 一次拉齐。解析规则调了两条（写进 plan.md §9.3）：
声明的是 Docker Hub 名字（除非自带 registry），走镜像时官方镜像加 `library/`；**本地存在性按「仓库路径」
判断**，所以 `docker.1ms.run/library/postgres:17-alpine` 算 `postgres:17-alpine` 就位，而
`myorg/postgres:17-alpine`
不算。本机 `KILN_REGISTRY_MIRROR` 根本没设、镜像却带着前缀，靠的就是第二条——否则闸门会因为名字不同而误报。

**patch 解析器的规则修了。** hunk 里的一行**以它自己的第一个字符为准会被骗**：删掉的 SQL 注释到 patch
里长成 `--- 0001 …`，被当成文件头，于是 1.9 的参考解「hunk 声明 8 行却带 0 行」而拒应用。新规则：hunk 带
多少行由 `@@ -a,b +c,d @@` 的计数决定（和真 diff 一样），行的首字符只是内容。回归测试在 `patch.rs`。

**1.6 的规格之前只在它自己的树上成立（已修）。** 它用 `Command::output()` 等日志进程自己退出；1.7 加上
监听后那个进程永不退出，于是 `kiln verify`（拿最终树判每一步）挂了 40 分钟，项目自己的 `cargo xtask ci`
（`spec_ci` 会启动它）也挂。现在规格用「空闲端口 + 读到启动结束就停手」的写法，退出码只在确实有退出时才
断言；这条已写进 authoring.md §6.8（“一条规格要在它会被拿去判的每一棵树上都成立”）。1.3 的规格也因此从
55 秒跑完（它嵌套跑 `cargo test --workspace`，现在包含 1.6 与 1.9 的规格）。

**1.9 的一处偏离规划（已改文档）。** 标题原本写「compile-time checked SQL」，但编译期校验的
`query!`/`.sqlx` offline 需要编译期能连库或 `.sqlx/`（本机没有 `sqlx-cli`），所以 1.9 用运行时的
`sqlx::query(..).fetch_optional(..)` 保证「没有数据库也能编译」，宏与离线元数据跟着 1.11 的第一条真实查询一起上。
步骤标题改为「an empty database, up to date before serving」，curriculum §4.2/§6 已同步。

### 下一步

**1.13 起按序写在 1.12 的树上；1.13 的设计已定（见第九段之后的「1.13 的设计原文」），直接写代码即可。** 从 1.11 起，一个
用到 SQL 的步骤开工顺序是：把这一步要用的语句写全 → 用一个跑着的空库
`cargo sqlx prepare`（起 `docker.1ms.run/library/postgres:17-alpine`，8 秒就绪；`sqlx migrate run` 建表，
`DATABASE_URL=… cargo sqlx prepare`）→ 把新生成的 `.sqlx/query-*.json` 作为 scaffold 内容发布，正文里给出
语句原文。1.13 不新增语句（它没有数据库工作）。`container-repro`（1.21）与 `links`（1.22）两个闸门仍未实现；
**`kiln watch`（M0 §1/§2.6 的目标）还没写**，现在是这条 track 之外剩下的唯一框架缺口。
别用 `podman images` 的**空输出**判断镜像是否还在：本段开工时它一次返回空、第二次就列全了。

## 第五段 · 2026-10 · 做到 1.10（领域类型）

T1 1.10（领域类型）已完成并提交：`src/domain.rs`（`Email`/`SubscriberName`/`Password` + 五个限额常量，
scaffold 里的三个 `parse` 什么都不查、`Password` 带着 derive 出来的 `Debug`，参考解把它们变成解析器 + 手写的
遮蔽 `Debug`）、`Error::InvalidInput { field, expected }`（**不带值**——值可能是密码），`pub mod domain;`。
`cargo xtask ci` 全绿，`kiln selftest` 对 **1.1–1.10 共 10 步**满足契约（1.10：8/8 must），
`kiln verify --json mailer` = 10 passed / 0 failed / 12 not authored（exit 3）。

**这一段的写法约定（下一段照抄）**：一个步骤如果需要库 API 而 scaffold 又不该改 `lib.rs`，就用
**探针 crate**：spec 自己写一个 `Cargo.toml`（`mailer = { path = <项目> }`）、`src/lib.rs` 和 `tests/*.rs`，
在一个临时目录里编译并运行（同 1.5 的做法；`CARGO_TARGET_DIR` 指向项目下的独立目录，第二次起几秒）。
好处：spec 本身不引用还得由学员加入的模块，所以它在 scaffold 上**编得过、跑得过、只失败在断言上**；探针能直接
`matches!`、能写 property，失败信息就是编译错误或断言行。代价：冷启动一次性多花约 20 秒。

## 第六段 · 2026-10 · 做到 1.11（注册）

T1 1.11（注册）已完成并提交（`121caf8`）：`src/signup.rs`（`SignupRequest` 三个原始字符串 +
`subscribe(pool, request)` + `hash`）、`Error::AlreadySubscribed { email }`（退出码 2，409）与
`Error::PasswordHash { source }`（1，500）、`http.rs` 的 `Error → StatusCode` 表（与 `Error::exit_code`
同一条规则：调用方能修的 4xx、部署的 5xx，且 5xx 从不带原因）、`POST /signup` 的 201/400/409/503。
`cargo xtask ci` 全绿 3m06s，`kiln selftest` 对 **1.1–1.11 共 11 步**满足契约（1.11：8/8 must），
`kiln verify --json mailer` = 11 passed / 0 failed / 11 not authored（exit 3）。另有一轮**空 `app/` 的
全新走查**：1.1 → 1.11 每步「scaffold 必败 → 参考解全过 → 每个 must 闸门全绿」，1.11 是
`0 passed; 6 failed` → `6 passed` → 全绿。

**这一段实测的事实（不要再花时间验证）**

- **`sqlx` 0.9 的 `query!`/`query_scalar!` 只吃字符串字面量**（`expand_query!(source = $query)` 内部
  按 `LitStr` 或 `"a" "b"` 拼接解析）。所以设计里那个「scaffold 给 `pub const INSERT_SUBSCRIBER`」是行不
  通的：语句写在**正文**里，参考解用字面量（可以 `\` 换行，`syn` 解出来的值与一行写法逐字相同）。
- **`.sqlx` 的键是语句文本的 SHA-256**（`hash_string(&input.sql)`），宏与 `cargo sqlx prepare` 用同一
  个函数；`prepare` 的做法是 `cargo check` + `SQLX_OFFLINE_DIR=<cache>` + `SQLX_TMP=<target/sqlx-tmp>`，
  由宏把 JSON 写进临时目录再搬到 `.sqlx/`，所以它需要**能连库或已有缓存**且 `cargo check` 成功。缓存
  目录按 `SQLX_OFFLINE_DIR` → `<manifest_dir>/.sqlx` → `<workspace_root>/.sqlx` 找；`DATABASE_URL`
  未设时自动走离线；`prepare` 会先删掉缓存目录里的旧 `query-*.json`（所以语句改了会自动清理）。
- **`argon2 0.6` 的 API 与 0.5 不同**：`Argon2::default().hash_password(bytes)` 自带 `getrandom` 取盐
  （默认 features = `alloc`+`getrandom`+`password-hash`，不需要 `SaltString`/`OsRng`），返回
  `password_hash::phc::PasswordHash`，`to_string()` 就是 PHC 串；`password_hash::Error` 实现了
  `core::error::Error`，可以直接当 `#[source]`。`getrandom 0.4` 的函数是 `getrandom::fill(&mut [u8])`，
  `getrandom::Error` 同样实现了 `core::error::Error`，加 `getrandom = "0.4"` 不会引入新下载。
- **并发场景的规格写法**（1.11/1.12 都用）：`TestApp` 自动是 `Sync`，所以
  `std::thread::scope` + `std::sync::Barrier::new(N)` 就够了，不需要 `Arc`；N 取 4 比 2 更容易把
  「先读后写」的实现打出来。
- **反向验证是这一步的常规动作**：把参考解改成那个阱的形状，看规格真的红（1.11：`[201, 500, 500, 500]`；
  1.12：`[200, 200]`）。
- 本机 `podman images` 有一次返回空、下一次才列全；镜像没了就
  `podman pull docker.1ms.run/library/postgres:17-alpine`（8 秒）。

## 第七段 · 2026-10 · 做到 1.12（确认令牌）

能力 `sec/token-lifetime@apply`；hint 是「expired / replayed / tampered 三种失败都要设计」。
落地结果：`src/confirmation.rs`（`Token`、`TOKEN_BYTES=32`、`CONFIRMATION_WINDOW_SECS=86400.0`、
`parse`/`issue`/`as_str`/手写 `Debug(<redacted>)`、零依赖的 `hex`），`GET /confirm?token=…`，
过期由库的 `now()` 写与比，一次性由 UPDATE 条件里的 `status = 'pending'` 保证，令牌不清空（清空就分不出
重放和伪造），三个新的 4xx 变体 + `TokenGeneration`（500/退出 1），
`SignedUp` 改名 `SubscriberState`。scaffold 只新增文件（`src/confirmation.rs` 里 `issue` 返回 64 个 0、
`.sqlx/` 三条新语句的元数据、`Cargo.toml` 的 `getrandom`），`time_hint` 仍是 90m，闸门列表未变。
1.12 的参考解**删掉了 1.11 那条 INSERT 的 `.sqlx` 条目**（语句改了，`cargo sqlx prepare` 也会删），
这是解法可以删文件、scaffold 不能的第一次使用，正文里写明了。

**1.12 实测的规格口味（下一步照抄）**：`signed_up()` 用 `psql` 直接读 `confirmation_token`（服务永远不
把它给调用方，规格要当数据库的读者）；过期用一条 `UPDATE … SET confirmation_expires_at = now() -
interval '1 minute'` 造出来，不睡觉；重放除了断 409，还断言 `confirmed_at` **没有被第二次请求挪动**。
反向验证同样做了：把 `confirm` 改成「先读后写」，`two_confirmations_at_once_confirm_once` 报
`[200, 200]`；把 `issue` 留成 scaffold 的常量，第二次注册 500（`confirmation_token UNIQUE`）。
`cargo xtask ci` 全绿 3m30s，`kiln selftest` 对 **1.1–1.12 共 12 步**满足契约（1.12：8/8 must），
`kiln verify --json mailer` = 12 passed / 0 failed / 10 not authored（exit 3）。

## 第八段 · 2026-10 · 做到 1.13（邮件端口与假实现）

T1 1.13（邮件端口 + 假实现）已完成并提交（`c1bd05d`）：`src/email.rs`（`Message`/`SendError`/
装箱 future 的 `EmailSender`/`Logging`/两个常量）、`tests/support/email.rs`（`Recording` 假实现，
`fail_with`/`clear_failure`/`attempted`/`delivered`）、`AppState` 的 `Arc<dyn EmailSender>` 与
`start_with_sender`/`sender()`（二进制仍走 `start`，未改一行）、`signup::subscribe(pool, sender,
request)` 在插入成功后才发信、发失败只 `warn!` 不改 201；`tests/spec_email_port.rs` 五条。
`cargo xtask ci` 全绿 3m45s，`kiln selftest` 对 **1.1–1.13 共 13 步**满足契约（1.13：8/8 must），
`kiln verify --json mailer` = 13 passed / 0 failed / 9 not authored。反向验证：把发送挪到插入**之前**，
`an_address_that_is_already_subscribed_gets_no_second_email` 报 `left: 2, right: 1`。

**落地时与设计的两处不同（下一段照现状写）**：① `confirmation_message` 放在 `src/signup.rs`（不在
`src/email.rs`）——端口模块因此不知道「订阅」这个概念，而 1.14 的 worker 要重发时正好需要它；
② 规格里的夹具叫 `App`（不是 `AppHarness`），并且只实现了 `post`（本步所有 HTTP 断言都是 POST /signup）。
另：这一步的 scaffold 没有「故意写错的函数」，它错在**缺缝**——scaffold 上规格是**编译不过**的
（`AppState::start_with_sender` 不存在），`kiln selftest` 把编译错误计为「scaffold 失败」，这符合
authoring.md §6.2；正文的 TASK 因此把那条签名写清楚了。

## 第九段 · 2026-10 · 做到 1.14（后台投递 worker）

T1 1.14（后台投递队列）已完成并提交（`4acf2f1`）：`src/delivery.rs`（有界 `tokio::sync::mpsc` +
one worker、`try_reserve`/`Permit` 当「座位」、`watch` 停机信号、`tokio::time::timeout` 包裹的
`drain`、full jitter 用 `getrandom`、五条 `start_paused` 单测）；`AppState` 持有队列（`sender()`
删掉——发送端现在只住在 worker 里）、handler 在 INSERT **之前**拿座位、`serve_with_shutdown` /
`serve_on_with_shutdown`（`axum::serve(...).with_graceful_shutdown`）、`main.rs` 等 SIGTERM/ctrl_c
→ 排空 → 退 0/1、`QueueFull` 在状态表里是 503、`tokio` 加 `test-util`。规格三条：答案不等发送
（挂死的 sender 下仍 201，且队满 503 且**没写行**）、失败一次的发送由重试送到、SIGTERM 排空 + 日志
+ 退出码 0。`cargo xtask ci` 全绿 4m06s（14/14 步满足契约），`kiln verify --json mailer` = 14
passed / 0 failed / 8 not authored，`spec_worker` 4 条 11.7s。反向验证：`drop(seat)` →「34 个注册全被
接受、一个都没被拒」；`MAX_ATTEMPTS=1` →「10s 内 ada 的邮件没到」。

**这一步定下的、后面几步要照做的三件事**：

1. **队列容量是常量 `QUEUE_CAPACITY = 32`**（原本设计里想放 `config.rs`，本段缩范围改成常量）。
   32 而不是 64：规格靠「一个接一个的注册」把队填满，而每个注册都要在 debug 下算一次 argon2
   （约 230ms），64 会让这一步自己的 spec 跑 18s。把它变成设置属于 1.19/1.21 的部署工作。
2. **规格不能依赖 `Queue` 自己的 API**：1.15 的幂等投递几乎一定要让 job 带上订阅者身份（重发、
   去重、状态机），`Queue::start/reserve/drain` 的形状会变。所以这一步的规格只从 HTTP 与日志看
   行为（`QUEUE_CAPACITY`/`MAX_ATTEMPTS` 两个常量除外）——`verify` 会在 1.22 的树上重跑它。
3. **1.19 必须保留「没配置邮件服务器时用 `Logging` 适配器」这个默认**：1.14 的子进程规格断言
   SIGTERM 前有 `an email would be sent`，而 1.7 起所有「二进制直接跑起来」的规格也隐含这一点。

**一处「没抓到」的记录（写进提交信息了）**：把 `main.rs` 里显式 `drain` 删掉，SIGTERM 规格**仍然
通过**——丢掉队列会关掉 channel，tokio 的 runtime drop 又会让 worker 把手上那条做完。也就是说
**drain 的截止时间在「适配器永不阻塞」的二进制里无法观测**，所以正文把它归给单测（TRAP 第 7 条明说）。
若要真正判它，得让 1.19 的适配器可注入或可配置成慢速——那时可以把这条规格补强。

### 1.14 设计（已按此实现，留档）

**调研结论（外部事实，别再花时间查）**

- **`mpsc::Sender::send` 在 `select!` 里被取消会丢消息**（tokio 1.53 文档原话：取消 `send` 会丢掉队列里的
  位置，要拿 `Permit` 再发才是 cancel-safe）。生产写法因此是 **`reserve()` / `try_reserve()` +
  `permit.send(msg)`**：容量先拿到手，消息永不丢——这正好也是「先占坑再干活」的写法。
- **队列原语选 tokio 自带的 `mpsc`（有界）**，不引 kanal/flume/crossbeam：第三方在微基准上快 2–10 倍，
  但本步负载是每秒几十到几百封、瓶颈在邮件服务器与 argon2，吞吐差异没有意义；而 tokio mpsc 与
  `select!`/`axum::serve`/`tokio::time` 共享同一套取消与时钟语义，零新依赖，停机链路不用自己发明。
- **停机用 axum 自带的 `Serve::with_graceful_shutdown(fut)`**（停止 accept、等在途请求结束），而不是
  自己写 accept 循环。生态里 `tokio_util::CancellationToken` + `TaskTracker` 是通用替代；**本课不引
  tokio-util**，用 `watch`/`JoinHandle` 表达同样的两件事（一个信号 + 一组能 join 的任务）。
- **重试用 full jitter**：`sleep = random(0, min(cap, base·2^attempt))`（AWS《Exponential Backoff And
  Jitter》三种里最简单、界最清楚、可断言的那种；decorrelated jitter 在他们模拟里总耗时略优，放进
  FURTHER）。抖动值用已经有了的 `getrandom`，**不要拿时钟当种子**。
- **队列不是真相来源**：内存队列在进程被 kill 时丢掉它持有的东西。正文必须写明这条边界——`subscribers`
  这张表才是「谁还没确认」的真相，1.15 的幂等键、1.17 的事务、以及将来 T3/T4 的 outbox + 认领才是
  「一定送到」；1.14 只教有界内存队列本身。

**接口与顺序（决定）**

- 新文件 `src/delivery.rs`（scaffold 给形状）：`pub const MAX_ATTEMPTS: u32 = 3`、
  `pub const DRAIN_DEADLINE: Duration = 10s`、`pub struct Job`（装 `Message` 就够：重发不需要查库）、
  `pub struct Queue`（`mpsc::Sender<Job>` + `Sender::try_reserve` 的包装）、一个纯函数
  `pub fn backoff(attempt: u32, base: Duration, cap: Duration) -> Duration`（可单测：上界与单调封顶）。
- **请求里的顺序就是课程**：`let permit = queue.try_reserve()?` → 解析/哈希/INSERT → `permit.send(msg)`。
  队满 → 新变体 `Error::QueueFull` → **503**（按 1.5 的表：部署方的问题 → 退出码 1），而且它发生在
  **写行之前**——客户端重试不会撞上 409 死胡同；`Permit` 还顺手解决了 `send` 的取消安全问题。
- **停机链路**：`main.rs` 用 `signal()`（`ctrl_c` + `#[cfg(unix)]` SIGTERM）→ `serve(...)
  .with_graceful_shutdown(...)` → 之后 `queue.drain(DRAIN_DEADLINE)`。干净排空退 **0**（课程验收就是这么
  写的），超时 `error!` 记下没发出去的条数并退 **1**。
- **配置**：新增 `delivery.queue_capacity`（默认 64）与 `delivery.retry_base_ms`（默认 250），按 1.4 的
  defaults/file/env/flags/report 机制加（**学员手改 `config.rs`，没有标记块**；这也是第一次让学员自己
  加设置，19 的 `public.base_url` 照抄）。`MAX_ATTEMPTS`/`DRAIN_DEADLINE` 先用常量。

**已知问题（两次观测，未修）**：`cargo xtask ci` 里的 `cargo test --workspace` 偶尔在
`-p kiln --test gates` 上红一次（慢/负载高时），单独重跑那条测试 14/14 全绿、整套 ci 重跑也绿。两次都
只在机器同时跑着学员项目的编译/容器时出现，怀疑是几条 gate 测试各自起 `cargo`（编译临时工程）互相
抢锁/抢 CPU 造成；测超时值本身不紧（300s/60s，`kiln/tests/gates.rs`），所以不是单纯调大超时能修。
**已修一部分（`29cbf7f`）**：闸门报告只截 cargo stderr 的前 12 行，而冷/争用时的前 12 行全是 cargo 在
讲自己——`Blocking waiting for file lock on package cache` + 一长串 `Compiling …`：学员拿到「没有失败的
失败报告」，而断言报告里有 lint 的闸门测试也会因为与 lint 无关的原因红——这正是「负载下偶发」的形状。
现在 `process::is_progress` 成了 `pub(crate)`、被 `clip` 统一用于过滤，并且学会忽略缩进（cargo 的旁白是
缩进的，而报告保留缩进，所以这个谓词以前**只匹配过第 0 列**的行——过滤器看着在工作，实际什么都没滤），
`clip` 在没有诊断时回退到原始行首，报告永不空白；回归测试就是那个 fixture（11 行旁白 + 一行
`error[E0308]`，截 12 行必须包含错误）。**但要说清**：带负载连跑 6 遍没有复现原来那次偶发，所以这不是
「偶发已经消失」，只是把一类会伪装成它的真实缺陷去掉了。再遇到就按上面的复现办法抓具体那条测试。

**1.14 的施工清单（本段已经推演出细节，照这个写，别重新设计）**

1. `src/delivery.rs`（新）：`Job` 直接用 `Message`；`MAX_ATTEMPTS=3`、`RETRY_CAP=30s`、
   `DRAIN_DEADLINE=10s`；`pub fn backoff(attempt, base, cap) -> Duration`（full
   jitter，`getrandom::fill`
   取 8 字节转 `u64`/`u64::MAX`，**不要用索引切片**——`--lib --bins` 禁 `indexing_slicing`，也不要
   `unwrap/expect/panic`）；`Queue { sender: mpsc::Sender<Message>, worker: Arc<Mutex<Option<JoinHandle<
   Result<usize, Error>>>>> , retry_base }`（`Clone` 靠 `Arc`），`Queue::start(sender, capacity,
   retry_base)` 里 `tokio::spawn` worker，worker 循环 `rx.recv()` → 逐个 `deliver()`（失败按 `backoff`
   重试到 `MAX_ATTEMPTS`，最后 `error!`），返回成功投递条数。
2. **`subscribe` 的签名改成 `(pool: &PgPool, seat: Permit<'_, Message>, request)`**：哨位由 handler 在
   **插入之前** `state.delivery().reserve()?` 拿到（队满 → `Error::QueueFull` → 503，且发生在写行之前），
   插入成功就 `seat.send(confirmation_message(email.clone(), &token))`（不 await，容量已保证），失败/已订阅
   分支直接 drop 哨位（容量自动还回去）。类型本身就强制了「先占坑再干活」。
3. `http.rs`：`serve`/`serve_on` 各加一个带 shutdown future 的孪生函数
   (`serve_with_shutdown`/`serve_on_with_shutdown`) 用
   `axum::serve(...).with_graceful_shutdown(...)`，
   原函数委托它们并传 `std::future::pending()`——**这样 `tests/support/app.rs` 一个字都不用改**。
4. `main.rs`：`shutdown()` = `#[cfg(unix)]` SIGTERM + `ctrl_c`（`signal(...)?` 走 anyhow，bin 里禁
   `expect`）；`serve_with_shutdown(state.clone(), &id, async { … }).await?` 之后 `drop(state)` 并
   `delivery.drain(DRAIN_DEADLINE)`：干净排空 `info!(delivered)` 退 0，超时 `Error::DrainTimeout` →
   `error!` + 退 1（1.5 的表：部署的问题）。
5. `config.rs`：`delivery.queue_capacity`（默认 64）与 `delivery.retry_base_ms`（默认 250），照 1.4 的
   defaults/file/env/flags/report/validate 五处各加一行；`Error::QueueFull` 与 `Error::DrainTimeout` 进退出码
   表的 `1` 一行。
6. 规格：进程内那条（用 1.13 的 `support/app.rs` 夹具 + 规格自己写的 `Hanging` 发送器，容量设 1）——
   **断言写成「至少一次 503」+「行数 == 201 的条数」**（worker 何时取走第一条是竞态，别断确切分布）；
   子进程那条（`spec_http.rs` 套路）发 SIGTERM → 退出码 0、日志里有 `email would be sent` 与
   `the delivery queue drained`；时间相关的放 `#[tokio::test(start_paused = true)]` 单测（尝试次数、退避
   上界、抖动非常量、drain 超时）。
7. **1.13 的规格已经先改好了**（`e0b3545`：`wait_for` 轮询 + 「至少一次尝试」/「grace 收到」这类跨树断言），
   所以异步化之后 1.13 的规格在最终树上仍然成立——这一条本来是本步最大的坑，已经拆掉。

**落地情况（M0 第九段已实现，上面三条「后面几步要照做」在后来的进度小节里）**：`src/delivery.rs` 与
单测、`Error::QueueFull`/`DrainTimeout`/`WorkerLost`、`subscribe(pool, seat, request)`、`AppState` 的
队列与 `serve_*_with_shutdown`、`main.rs` 的信号与排空、`spec/1.14/worker.rs` 三条，都已落地；
与下面的施工清单的两处差异：容量与重试基数**是常量**（不是在 `config.rs` 加设置），且规格的进程内
部分用「一个挂死的 sender」而不是「容量设 1 的设置」。

**规格怎么做（决定，含一个要改旧文件的地方）**

- 时间相关的断言放**单测**（`src/delivery.rs` 内，`#[tokio::test(start_paused = true)]` + 假发送器）：
  尝试次数 = `MAX_ATTEMPTS`、退避有界、抖动不是常数、超过 `DRAIN_DEADLINE` 时 drain 返回「没排空」。这样
  不为测试引入设置，也不睡觉。
- **「队满 → 503 且没写行」必须在进程内做**（子进程里没有假发送器）：把 1.13 规格里那个 `App` 夹具**搬到
  `tests/support/app.rs`**（scaffold 可以新增文件），1.13 与 1.14 的规格都 `#[path = "support/app.rs"]
  mod app;`——代价是 1.13 的规格被改动 ⇒ 要重新 `kiln spec manifest` 并对 1.13 重跑 selftest（允许，
  课程内容是自己维护的）。夹具要加一个「挂在第一个 job 上不返回」的发送器（`Hanging`），配合
  `MAILER__DELIVERY__QUEUE_CAPACITY=2` 就能用 4 个请求把队填满。
- **SIGTERM → 排空 → 退出码 0** 用子进程做（`spec_http.rs` 的套路：起二进制、发信号、等退出码、读 JSON
  日志）。

### 1.14 的动手前置（原始备注，留档）

（原标题写成「1.13 的原始备注」，正文讲的是 1.14 动手前要确认的两处；按内容改名。
1.13 自己的设计原文在下一节。）

1.14 是 120m 的异步步（`async/graceful-shutdown@apply` + `async/backpressure@aware`，概念：有界队列、
停机排空、带抖动的重试），需要 `cfg/layered-config@apply` 与 `test/fake-external@apply`（都已具备）。
动手前先把这两处读一遍，因为它们决定 scaffold 能不能只新增文件：

- **`src/main.rs` 现在的停机路径**：1.7 起 `serve_on` 用 `tokio::select!` 收一个 oneshot，二进制里
  `tokio` 的 `signal` feature 已经开着，但 SIGTERM→排空的完整链路要按现状确认；停机截止时间、队列
  上限这类设置要进 `config.rs`（**没有标记块，学员手改**，跟 1.4 的 defaults/file/env/flags/report
  机制一起；1.19 之前 `public.base_url` 也该在这时补上）。
- **`async/backpressure@aware` 的闸门怎么判**：`gates` 里目前没有能观察「队列满时请求怎么答」的闸门，
  这一步的规格（`spec/1.14/worker.rs`）要么用假发送器把队列吃满（把 `Recording` 改造成「每次 send 都
  等一个信号」的慢发送器），要么断言 HTTP 答复（429/503）；后者更接近课程的一贯口味。

### 1.13 的设计原文（已按此实现，留档）

能力 `test/fake-external@apply`；60m → 建议改 **90m**（见下）；requires 已满足（1.5 的错误链、1.12 的
令牌）。**这一步的关键约束是注入缝**：`tests/support/mod.rs`（1.8 的 app 工厂）没有 scaffold 标记块，
新 scaffold 只能新增文件，所以 1.13 的规格**自己写一个能注入发送器的 app 夹具**，不碰 `TestApp`。

**端口**：新文件 `src/email.rs`（scaffold 内容，`Cargo.toml` 不需新增依赖）：
`pub struct Message { pub to: Email, pub subject: String, pub body: String }`（收件人用 1.10 的领域类型，
不是 `String`）；`pub struct SendError(Box<dyn Error + Send + Sync>)`（1.5 的规矩：保留 cause，实现
`Display`/`Error`）；`pub trait EmailSender: Send + Sync + 'static`，方法签名
`fn send(&self, message: Message) -> Pin<Box<dyn Future<Output = Result<(), SendError>> + Send + '_>>`
——**必须装箱**，因为 `AppState` 要拿 `Arc<dyn EmailSender>`，而 AFIT 在 1.98 还不能 dyn 分发；正文里
写明这一次分配是 `dyn` 的代价。第一个适配器 `pub struct Logging`（`send` 记一条
`info!(to = %message.to, subject = %message.subject, "an email would be sent")` 并返回 Ok）：1.13 没有
SMTP，真实的适配器是 1.19 的 mailpit。链接由 `pub fn confirmation_message(to: Email, token: &Token) ->
Message` 造，基址是 `pub const CONFIRMATION_BASE_URL`（**本步先不做成配置项**：`config.rs` 没有标记块，
加设置要学员手改 1.4 那套 defaults/file/env/flags/report 机制，把它留给真正需要它的 1.19/1.21；正文
TRAP 里说明这一点）。规格只断言 body 里有 `/confirm?token=<行里的令牌>`，不钉基址。

**注入缝**：`AppState` 增加 `sender: Arc<dyn EmailSender>` 与
`pub async fn start_with_sender(config, id, sender: Arc<dyn EmailSender>) -> Result<Self, Error>`
（`start` 内部用 `Logging`），二进制继续用 `start`。这条缝是**公开的**，因为测试装置在库看不见的地方，
正文里写明理由（「注入是构造参数，不是 mock 框架」）。

**用法**：1.11 的 `subscribe` 在插入成功、拿到 token 之后发一封信（`confirmation_message(email, &token)`）
——`subscribe` 需要拿到 token，所以它的返回值要从 `Email` 变成 `(Email, Token)`？**不要**：让
`subscribe` 自己发（`pool` 之外再收 `&dyn EmailSender` 或 `&Arc<dyn EmailSender>`），返回值不变。
**发送失败不改变答复**：行已经写好、订阅者已经在，信是投递问题不是注册问题 → 仍然 201，`warn!` 记一条
（1.14 的 worker 与 1.15 的幂等重投是它的续集；这也是 1.14「把发送挪出请求路径」的引子）。已被订阅的地址
**不发信**（409 之前不能发）。`Logging` 的失败注入不需要：假的 `Recording` 才需要。

**假实现**：scaffold 新增 `tests/support/email.rs`：
`pub struct Recording { sent: Mutex<Vec<Message>>, failure: Mutex<Option<String>> }`，
`new()`、`fail_with(&str)`（之后每次 send 都返回 `Err(SendError::…)`）、`clear_failure()`、
`sent() -> Vec<Message>`；`impl EmailSender for Recording` 先查失败再记录。规格用
`#[path = "support/email.rs"] mod email;` 引入（1.9 的 postgres.rs 同一套路），夹具是规格里的
`AppHarness`：`AppState::start_with_sender` + `std::net::TcpListener::bind("127.0.0.1:0")` +
自己的线程跑 `http::serve_on`，请求用 spec_http 那种裸 TCP 写法（不要复制 TestApp 的整份代码，
只要 get/post 两个方法）。

**规格**（`spec/1.13/email_port.rs`，五条）：① 注册成功 → 假发送器**恰好**收到一封信、收件人是规整后的
地址、body 里有 `/confirm?token=<行里的令牌>`（表里的令牌与信里的令牌必须一致）；② 同一地址第二次
→ 409 且**没有第二封信**；③ `fail_with` 之后注册 → 仍然 201、行仍然是 `pending`、信没发出去（订阅者不
因为邮件服务器挂了而消失）；④ 恢复（`clear_failure`）后注册第三个地址 → 又是一封信（失败不粘住）；
⑤ 端口本身的一致性：`Logging` 与 `Recording` 都是 `EmailSender`，对「可投递的消息」都返回 Ok
——这可以用
`let senders: Vec<Box<dyn EmailSender>> = vec![Box::new(Logging), Box::new(Recording::new())]`
在一个测试里跑同一段断言表达出来（`test/fake-external` 的「假实现要与真实现同契约」）。

**正文 TRAP 至少覆盖**：`send` 失败把注册变成 500（订阅者已存在，这是投递问题）；给已被订阅的地址也发
信（钓鱼/骚扰面）；把链接基址写死在正文示例里而正文又说它该是配置项（要么是常量要么是设置，不能是
两者）；`AppState` 里存具体类型而不是 `Arc<dyn _>`（测试没法注入）；用 `async fn` 直接写 trait 方法
（dyn 分发编不过）；假实现只存在于测试文件里却断言真实现的行为（假货会漂移）。正文 DONE WHEN 里要
能复制的 `kiln run/check` 两条命令，并加上 `cargo test --lib`。

## 第十段 · 2026-10 · 做到 1.15（幂等投递）

T1 1.15（幂等投递）已完成并提交（`32c5489`）：迁移 `0002_confirmation_delivery.sql`（`subscribers` 加
`confirmation_claimed_at`/`confirmation_sent_at`/`confirmation_attempts`）、新模块 `src/deliveries.rs`
（claim/mark_sent/release/outstanding 四个状态机函数）、`Job` 改成携带订阅者 id（不再携带 `Message`）、
worker 的重试改成“过一次数据库”（claim → 发一次 → 记账或释放）、`AppState` 的 `Option<Queue>`（没数据库
就没有投递记录，也就没有队列）、重复注册会重新“报一次”仍欠的投递（答案仍是 409）、`main.rs` 排空可选队列。
规格四条：一次注册一封邮件且行里有记录；同地址再注册（两种写法）仍只有一封；持续失败时 attempts 到
`MAX_ATTEMPTS`、释放占用、之后再注册能送出且只送一次；八个并发注册 → 一个 201、七个 409、一封邮件。
`cargo xtask ci` 全绿 5m42s（15/15 步满契约），`kiln verify --json mailer` = 15 passed / 0 failed /
7 not authored。

**这一步定下/发现的四件事**：

1. **`.sqlx` 离线元数据的做法（后续改 SQL 的步骤照抄）**：起一个 Postgres 容器 →
   `DATABASE_URL=postgres://postgres:postgres@127.0.0.1:<port>/postgres cargo sqlx migrate run --source migrations`
   → `cargo sqlx prepare --workspace`（在 `app/mailer` 里跑）。Docker Hub 拉不动，用
   `docker.1ms.run/library/postgres:17-alpine`。**scaffold 自带它自己那几个查询的 `.sqlx` 文件**（否则学员
   declare 模块后离线编译不过），solution 用新的哈希文件替换旧的（旧的要删）。
2. **课程 patch 不能有 rename 段**：`git diff` 把 `.sqlx` 改名写成 `rename from/to`，而 `kiln` 的 applier
   只会删/增/改，会把它当“修改旧路径”应用——症状是三步以外的“run `cargo sqlx prepare`”。已修（`5f3fe9d`）：
   applier 现在直接报错并告诉你用 `git diff --no-renames`。**以后生成 patch 一律加 `--no-renames`**。
3. **1.9 的规格改成可扩展的**（同一提交）：它原来断言“迁移恰好一条、列恰好九个”，任何后续步骤都不可能
   满足。现在断言 1.9 承诺的那些列**存在**（子集），以及第二次启动不改变已应用迁移的集合。1.17 再加迁移
   （0003）时不会再碰它。
4. **`confirmation_attempts` 的口径**：第一版把“claim 次数”当 attempts，而 worker 在**一次** claim 里重试
   发送 → 列里读完是 1（规格当场报“10s 内没能给出放弃的证据”）。现在是“一次 claim 一次尝试”，重试循环穿过
   数据库；这样计数才是“这条消息被真正递给邮件服务器的次数”，重启后不会归零、多个 job 也会累加。

**留给后面的两处已知缺口**：① 进程在 claim 与 send 之间死掉，会留下一个永远“被占用”的订阅者——需要
reaper（`claimed_at < now() - interval …`）或 outbox，属于 T3/T4；正文第 9 条 TRAP 已写明。② 目前只有一种
消息，所以状态用 `subscribers` 的列表达；**1.17 的 newsletter 是第二种消息**（“某个订阅者的某一期”才是投递
身份），那时该上 `deliveries` 表 + `UNIQUE (subscriber_id, kind)`——迁移文件里的注释已经把这层关系写下了。
另：1.18 的授权要用 1.16 的会话；1.19 必须保留“未配置邮件服务器时用 `Logging`”（见第九段第 3 条）。

## 第十一段 · 2026-10 · 做到 1.18（授权边界）

T1 1.16（会话/认证，`16b5404`）、1.17（newsletter 事务 + fan-out，`a651074`）、1.18（授权边界，
`69cf287`）已完成。1.18 是**第一个没有 scaffold 的步骤**（authoring.md §5.2）：route + 受会话限定范围的查询 + 一个错误
变体，1.16 已经把每种形状都演示过了，`track.toml` 里写明了为什么删掉 `scaffold` 键。到 1.18 为止：
`cargo xtask ci` 全绿 7m11s（18/18 步满契约），`kiln verify --json mailer` = 18 passed / 0 failed /
4 not authored。

**这一段新落地的机制（后面几步直接用）**：

1. **`tests/support/service.rs`（1.16 的 scaffold 给的）**：一个“真进程 + 能发 header 的客户端”夹具
   （`Service::start(env)`、`call/get/get_with/post`、`log_lines`、`stop/wait_for_exit`）。1.13 的进程内夹具
   只能发 body，cookie 与 `Authorization` 这类东西它发不了——认证类规格必须用这个。**日志断言要在 env 里
   加 `("MAILER__LOG__FORMAT", "json")`**（默认是 pretty，1.7 的规格也是这么做的）。
2. **会话凭据**：`sha2`（新增依赖，scaffold 往标记块里插的）对 token 做 SHA-256——“高熵秘密用快哈希，
   可猜的秘密用慢哈希”是这一步的课；cookie 属性固定为 `HttpOnly; SameSite=Lax; Path=/; Max-Age=…`，
   **没有 `Secure`**（本机无 TLS），正文 TRAP 第 7 条写明这是决定而非疏漏。
3. **1.17 的表结构**：`issues`（两个 CHECK：非空 subject/body）+ `issue_deliveries`，后者带
   `UNIQUE (issue_id, subscriber_id)`（投递身份是“哪一期的哪一份”），claim/mark_sent/release 与 1.15 同形。
   事务 + **commit 之后**才 fan-out；audience 是 `INSERT … SELECT … WHERE status = 'active'`（不是 Rust 循环）。
   队列满时那些“没排上去”的副本就留在表里——真正的 relay 属于 T3/T4，正文明写了这个缺口。
4. **`Job` 现在是 enum**（`Job::confirmation(subscriber)` / `Job::issue(delivery)`），worker 按种类分派；
   两种循环长得像但不抽象化（正文解释了为什么）。任何依赖 `Job` 形状的规格都会因此打破——目前没有。

**1.19（真实邮件服务器容器）动手前必须解决的三个限制**（这三条是被前面步骤“记了账”的）：

- 二进制一直用 `Logging` 适配器，所以 **1.14 的 drain 截止时间、1.15/1.17 的失败路径在子进程规格里都
  无法观测**（1.14 提交信息与 1.15 正文都写了这件事）。1.19 要把适配器**可配置**（SMTP），并且
  **在没配置时仍然默认 `Logging`**（1.7 起的子进程规格都隐含依赖这一点，1.14 更是明文断言了
  `an email would be sent`）——这样新规格可以指向容器里的邮件服务器，旧规格一字不改。
- 镜像：`axllent/mailpit`。Docker Hub 直连在本机拉不动，用 `docker.1ms.run/axllent/mailpit` 缓存；
  `images` 门与 `tests/support/` 里的镜像判定按“仓库路径”而不是字符串相等（1.9 定下的规则）。
- SMTP 客户端：`lettre`（新增依赖，插在标记块里；不带 TLS 的 feature 组合，mailpit 的 1025 是明文）。

## 第十二段 · 2026-10 · 做到 1.20（指标），并留下 1.15–1.19 的坑

T1 1.19（真实邮件服务器容器，`a141cd8`）、1.20（metrics + SLO 草稿，`29d522f`）已完成。1.19 把 SMTP 适配器
变成可配置的（`email.smtp_host`/`email.smtp_port`），**没配置时仍然是 `Logging`**（前面所有子进程规格都靠这一
点，1.14 更是明文断言 `an email would be sent`）；`tests/support/mailpit.rs` 是一容器一测试的邮件服务器夹具；
它的第三条规格是**第一次把 1.14 的 drain 截止时间从进程外判下来**（SIGTERM → 10s → 退 1）。1.20 加了
`/metrics`（手写 Prometheus 文本格式）、按**匹配到的路由**计数的中间件、以及 `deliveries_total{outcome}` 的
三种结局；规格里有一条 `# HELP` 家族快照（重命名即红）。

**这一段的四个环境/工具教训（很重要，别重踩）**：

1. **容器夹具必须带 `-v` 删卷**（`7ed238b`）：postgres 镜像把数据目录声明成 VOLUME，每个测试容器都会留一个匿名
   卷，而 `podman rm -f` 不删卷。本 session 累积到 2022 个卷后引擎报 `exceeded num_locks (2048)`，**所有**容器
   规格一起崩，症状离原因极远（当时看起来像两个 `perf(kiln)` 提交导致的回归）。两个夹具的 `Drop` 现在都是
   `rm -f -v`，注释写明了原因。**如果以后又出现“所有容器测试同时失败”，先看
   `podman volume ls | wc -l`，再 `podman volume prune -f`。**
2. **课程 patch 不能追加到骨架 `Cargo.toml` 末尾**（`aa4f6e6` 回退）：那样会让 `1.2` 的 solution 在
   `git apply` 下失败（kiln 自己的 applier 容忍，git 不容忍）。骨架的每一行都是后面 patch 的上下文。
3. **共享构建目录是真正的性能开关**（`62436a6`）：`kiln process` 现在接受一个进程级 `CARGO_TARGET_DIR`，
   `selftest` 指向 `$XDG_CACHE_HOME/kiln/target`（或 `$HOME/.cache/...`）。单步 selftest 从 1m30s →
   **25-57s**，
   `cargo xtask ci` 从 7m30s（基准）→ **6m46s**，而 `kiln verify` 里每步的同一批门现在只算一次（`perf(kiln)` 的
   树指纹缓存）。`verify` 剩下的时间（~2m30s）几乎全是 spec 自己：每个测试各起一个容器（这是刻意的隔离，
   也是 1.19 教的 order independence），不要再为了快把它去掉。
4. **两个工作流建议已落地**：长验证交给子 agent（scout，冻结的树 + 只回报结论行/耗时/失败诊断前几行）；
   在验证跑着的时候不动文件。这一轮三次验证都是这么做的，上下文省很多。

**一处未复现的红（留档，别当成已修）**：`2d0f004` 上的 `kiln verify --json mailer` 报 `mailer/1.3-ci` 失败
（它的 `spec_ci` 测试会在 `app/mailer` 里重跑项目自己的 `cargo xtask ci`）。**同一棵树、同一条命令手工跑是绿的**：
`cd app/mailer && KILN_CI_NESTED=1 cargo xtask ci` → “xtask ci: every check passed”；
`cargo test -q -p mailer --test spec_ci` → 2 passed in 70s。同一次 `cargo xtask ci`（selftest 的沙箱副本）里 1.3
也是绿的（scaffold 失败 / solution 通过 / 5/5 门）✓。所以红只出现在 `verify` 那条路径上，且当时刚跑完 6 分钟的
`ci`（机器有负载）。`verify` 全程只用了 2m29s，说明 `spec_ci` 是**很快**失败的（不是超时），但报错行被 kiln 截成
一行（`exit code 101: error: test failed …`），看不到内部是哪一步。

**已经量过了**：`4810e6a` 上连空跑三遍 `kiln verify --json mailer` → 三次都是 **passed 20 / failed 0 /
not-authored 2**（exit 3 = 内容未写完），1.3 三次全绿，五个门全 passed。所以它不是确定性的失败。
**但这不等于排除负载**：这三遍跑在已经被预热过的 `app/mailer/target` 上，内部那次 `cargo xtask ci` 每次都是
增量构建；原来出问题时的条件（冷构建 + 刚跑完 6 分钟 ci 的机器）没有被复现。结论：**最可能是负载/冷构建相关，
不是 app 状态相关、也不是确定性失败** —— 下一次再遇到就按这条走：
1. 保留 `kiln verify --json mailer` 的完整 JSON（对 1.3 的 `gates` 数组和失败行，别只看控制台那一行）；
2. 顺手看一眼 `podman volume ls | wc -l`（容器类 spec 的并发量是内部 `cargo test --workspace` 的产物）；
3. 冷启动复现：`rm -rf app/mailer/target` 后带负载重跑那条路径，而不是只重跑 `verify`。
**不要用重跑把它盖过去**——这正是 1.14 段记录过的同类问题（那时也是靠“带负载才出现”才难抓）。

**T1 只剩 1.22 一步了**（1.21 已于 `0a7e7da` 落地，见第十四段）：

**以下 1.21 的草稿与第一次尝试记录已归档**（草稿仍在 `docs/drafts/1.21-container/`，作参照物：它的 16.5 MB /
热构建数字与落地的版本对得上，但挂载路径、base image 解析、截止时间都已按第十四段改写）：

**1.21 的第一次尝试与实测结论（归档，动手前仍值得一读，省你一次 28 分钟）**：按“多阶段 Dockerfile + 容器内 `cargo build
--release`”写了一版（`rust:1.98-slim` builder + `debian:bookworm-slim` runtime，依赖层用 stub 预编译），
`podman build` 在这台机器上跑了 **28 分钟**然后在依赖/stub 那层以 exit 101 失败（`cargo build --release
--locked`，错误被 buildah 截成一行）。两个基础镜像已在本机（`docker.1ms.run/library/rust:1.98-slim` 828MB、
`docker.1ms.run/library/debian:bookworm-slim` 100MB），但那 28 分钟说明：**课程 spec 的 300s 上限装不下一次
冷构建**，`cargo xtask ci` 里 1.21 的 selftest 也会因此变成 30 分钟级别。所以那一版已经从 `app/mailer` 里撤掉
了（`Dockerfile`/`.dockerignore`/`tests/support/container.rs`/`tests/spec_image.rs`），树仍是 1.20 的绿状态。

**1.21 的可行设计（已被落地的版本取代：镜像内编译保留了，构建靠缓存变快；这一段留档）**：镜像构建走**宿主机已经构建好的 release 二进制**，
不要容器内编译：

1. `cargo build --release -p mailer` 在宿主跑一次（缓存后是增量的），spec/Dockerfile 都只 copy
   `target/release/mailer` 进 runtime 层。这恰好让 teaches 里的“the image runs the same binary you
   tested”变成**字面为真**（镜里就是同一面二进制）——正好也是 `container-repro` 门要判的东西。
2. “multi-stage”这层意思改用**两阶段但零编译**表达：一个 `FROM debian:bookworm-slim AS builder` 只做
   `useradd`/准备目录，runtime 再 `COPY --from=builder`——或者干脆在正文里诚实地把 multi-stage 说成
   “CI 里由 CI 构建、本地由宿主机构建”的两种模式，并把容器内容器编译列为 FURTHER（带上面那 28 分钟的实测）。
3. 镜像内编译若要保留，就必须把 `podman build` 挪出 spec（比如 `kiln images pull` 那样的准备命令），
   并让学习者用 `kiln --timeout 1800 run mailer/1.21-container`；`container-repro` 门也要接受这个超时。
4. 已经写好的规格设计可以直接用（撤掉前是完整的，四个断言都在正文/handoff 里）：容器里 `/healthz` 200、
   `/readyz` 503 且说明缺数据库、未知路径的 404 体与宿主同形；`command -v cargo/rustc/cc` 全非零、`/src`
   不存在（“不是编译环境”）；`id -u` ≠ 0；给了数据库时 `/readyz` 转 200 且 `subscribers` 表在——容器得用
   `MAILER__LISTEN__HOST=0.0.0.0`（默认 `127.0.0.1` 在容器里等于没有发布端口），数据库地址要把
   `127.0.0.1` 换成 `host.containers.internal`（podman machine 下才到得了宿主机发布的端口）。

- **1.21-container**（120m，`release/container-image@apply`，spec `spec/image.rs` →
  `tests/spec_image.rs`，
  `run.cmd = cargo test -p mailer --test spec_image`，**已落地**：见第十四段）：多阶段 Dockerfile + 探针 + “先迁移后收流量”；
  下面这段当时的建议已经实现（实现与实测见第十四段；基础镜像最终用 alpine/musl）：原记录是「必须先实现 `container-repro` 门（构建镜像、跑起来、
  打 `/healthz`、用同一面 binary 的 `--check-config` 对一下）；基础镜像加进 `images = [...]`；笔记：spec 超时是
  300s，镜像构建很可能超》。
- **1.22-runbook**（120m，design 步骤，`ops/runbook-drill@apply`）：两条线（演练规格 + 人读的 runbook），
  它的 gates 里有 **`links`**——**已落地**（见第十三段：`links` 判树不判外链，外链只在 `kiln links --external`）。
  写内容时按 authoring.md §4.4 的硬规则来：页面在项目里、占位目标算不过、相对目标与锚点要对得上。
- **`kiln watch` 已做**（`kiln/src/cmd/watch.rs`）：按内容指纹轮询项目（`target/` 不算，同字节重写也不算），
  首轮立即跑，之后每变一次重跑一次 `kiln run <step>`；实测“runs: 2 · reruns: 1”。M0 验收里的
  “reruns on save”因此已满足。

## 第十三段 · 2026-10 · `links` 闸门落地：判树，不判外链

按计划表里那句 `lychee` 做落地时改了实现方式（**决定+理由写在 plan.md §6 末尾**，authoring.md §4.4 写了对写页面的硬规则）：
`links` 是**纯离线闸门**，外链连通性不进闸门、不进 `cargo xtask ci`，只在 `kiln links --external`（要 `curl`）。

落地的东西与证据（全部在本机实测）：

1. `kiln/src/gates/links.rs`：递归扫描项目里的 markdown，判相对目标（逐段精确比对目录项，**不靠文件系统的大小写宽容**）、
   `#锚点`/`page.md#锚点` 是否命中标题（GitHub 风格 slug，支持中文标题与重名标题的 `-1`
   后缀）、占位符（`TODO`/`TBD`/`FIXME`/`XXX`/`placeholder`、
   空目标、空锚点）、绝对路径、越出项目（`../..`）、指向 `target/`；代码块与行内代码不算链接（那是例子）；没有 markdown、
   或全树没有任何链接 → 失败（“没写的页面不能被判成过关”）。12 个单元测试。
2. `kiln/src/cmd/links.rs`：`kiln links [PATH] [--external]`。离线那半与闸门**同一个 `scan`**（口径不会不一致）；
   `--external` 用 `curl` 逐个抓（15s/条，`-o /dev/null -w %{http_code}`），**只有 404/410 令命令以 1 退出**，
   403/429/5xx/超时/解析不到主机只打 `?`，网络不通时退出 2（无法判定）。2 个单元测试。
3. 接线：registry 里 `links` 从 Pending 变 Ready（无外部工具）；`CACHEABLE` 收进 `links`（它的答案确实只是树的属性）；
   `kiln/tests/gates.rs` 新增一条走真实 runner 的集成测试（干净页 passed / 半写的页 failed 并点名 `docs/runbook.md:3 TODO`
   /
   没有页面 failed）。
4. 实测（本机，冻结树）：`cargo test -p kiln` → 175 lib + 14 gates 集成 + 1
   另一目标全绿；`cargo clippy -p kiln --all-targets -- -D warnings` 与 `cargo fmt --all --check` 均干净；
   **课程自己这棵树扫一遍** `kiln links` → 26 页 / 141 链 / 110 外链，**0
   findings**（含中文标题锚点、多级目录相对链接——这也是对检查器无假阳性的实测）；
   故意做坏的夹具 → 7 条 finding + exit 1；`--external` 试跑（example.com/404/超时）→ `✓ 200` / `✗ 404` /
   `? timeout`，1 answered · 1 unclear · 1 gone，exit 1。

**下一段（写 1.22 的人）接这里的两个约束**：
- scaffold 的 runbook 页要用**占位目标**把清单写出来（如 `[dashboard](TODO)`），这样学员 `kiln run` 一上来就看到红；
- 参考解的 runbook **所有链接必须留在 `app/mailer` 内**（不能指回课程 `docs/`）：`kiln selftest` 的沙箱里项目是独立副本，
  指出去就会在沙箱里红、在本机绿——这正是这条闸门专门避开的那类不一致（外链同理，别写在页面上当目标）。

## 第十四段 · 2026-10 · 做到 1.21（容器化与 `container-repro`）

T1 1.21（容器化）已完成并提交（`3d125d0` 框架 + `0a7e7da` 内容），**22 步里剩 1.22**。
设计细节写在 plan.md §6 的两个新小节（`container-repro` 与 `links`）里，写页面的硬规则在 authoring.md §4.4。

**这一步新落地的机制（后面几步直接用）**：

1. **`container-repro` 门**（`kiln/src/gates/container_repro.rs`）：按 `Dockerfile` 重新构建镜像（`--layers` +
   共享缓存，热态十二秒），判 25 MB 体积上限，然后用镜像里的二进制与**宿主二进制**答同一道题 `--check-config`，逐行比对。实测在 1.21 上是
   `16.5 MB / 25 MB · 7 行完全相同`。
2. **`run = { cmd, timeout = 1800 }`**（`kiln/src/info.rs`）：步骤自己的截止时间，取与 `kiln --timeout`
   的**较大值**，规格与闸门都用它。只有会构建东西的步骤该写。
3. **基础镜像名的解析**（`kiln/src/runtime.rs`）：多个镜像源并存时同一仓库路径存两份（各一架构），`podman build --pull=never` 对非本机那个回
   `image not known`（第三阶段才报）；`runtime::images` 现在优先选本机架构的那份，`images`
   门因此也报出真正能用于构建的名字（这台机器上：`alpine:3.20` 只有 daocloud 那份是 amd64，1ms 那份才是 arm64）。
4. **构建缓存是「可以要」不是「强加」**：夹具挂 `CARGO_TARGET_DIR` 传路径，不认这个参数的 Dockerfile 照常构建；认的那个要在同一个 `RUN` 里把二进制
   `cp` 出挂载点（`COPY --from=builder /src/target/...` 在挂载下会得到空镜像——这就是 CI cache mount 给每个 Rust
   项目设的坑，本步让它变成必修）。

**实测（本机，冻结树）**：scaffold 规格 150s（116s 构建 + 四条全红，红在「容器只监听 `127.0.0.1:8080`，外面连不上」）；solution 规格 **6.5s
热态 4 passed**；`kiln check mailer/1.21-container` **10/10 must 门、12s**（含 `images` 与
`container-repro`）；`kiln selftest mailer/1.21-container` **169s、1/1 满契约**（scaffold 必败 / solution 通过
/ 10 门全绿）。

**顺手修的三件事**（都是这一段的实测发现，不要当成噪声）：

- **scaffold 内容必须是 rustfmt-clean**：夹具只是 `tests/support/` 里的模块文件，`cargo fmt --all` 只在它能从某个 test
  target 被 `mod` 到时才看得到它；归档补丁前必须 `rustfmt --edition 2024` 显式格式一遍，否则学员项目的 `fmt` 闸门会红（本段踩过一次）。
- **`links --external` 抓到三个真死链**（`200e319` 修）：`std/num/struct.NonZeroU32.html` →
  `type.`、`sqlx/postgres/struct.PgPoolOptions.html` → `type.`、prometheus 的 `instrumentation/…-…` →
  `instrumenting/…_…`。三者都是同一主机下兄弟路径 200、只有它 404，是真腐烂。以后再跑一次 `--external` 值得。
- **`links` 提交里漏了 rustfmt**（`9450353` 的 `kiln/tests/gates.rs`）：`cargo xtask ci` 从
  `cargo fmt --all --check` 开始，所以那一次 ci 在第一个子检查就红了；工作树里下一次 `cargo fmt --all`
  已经把它格式化成对的，但**历史里那个提交仍是不干净的**——以后提交前跑一次 `cargo fmt --all && cargo fmt --all --check`。

## 第十五段 · 2026-10 · 1.22 落地，T1 22/22 写完

1.22（runbook + 首次故障演练）已提交（`0c62815`）。T1 的正文/规格/scaffold/参考解/闸门接线全部就位，12 个闸门里只剩 `timeout`（刻意留在 “由
runner 自己实现，不是逐步门” 的状态）。

**这一步的形状（design 步骤的模板，T2–T5 的 design 步可以照抄）**：交付两样彼此验证的东西——
可执行的演练（`tests/support/drill.rs` 的 `replay(Recovery)`）和要人读的页面（`docs/runbook.md`）。
演练用 1.13 的 `Recording::fail_with` 把邮件服务器“拿走”，先用**数据库里的次数**等到第一次投递真的失败（前提为真才谈得上恢复），
再决定服务器回不回来，最后从**服务自己的记录**算 `delivered` / `given_up`。规格跑两个世界：
回来则一无丢失，不回来则必须报出丢失——**一个不会红的演练就是装饰**。页面那半由 `links` 门查（离线结构）。
scaffold 的两个洞正对应这两半：演练“报告它希望发生的事”、页面用 `TODO` 目标——所以 scaffold 上 2/3 个测试红，而且红得各有原因。

**实测**：scaffold 1 passed / 2 failed；solution 3 passed / 3.1s；`kiln check` 9/9 门 / 9s（`links` 数出页上 9
个目标）；`kiln selftest mailer/1.22-runbook` **29s、1/1 满契约**。

**这一步又摸出两个坑**：

1. **夹具的 `#![allow(dead_code)]` 不是摆设**：`tests/support/email.rs`（1.13）比后面四个夹具早，没有那一行；
   任何通过自己的 drill 引入它的规格都会因为「有些方法没用」而 clippy 红。现在它有了（改动落在 1.13 的 scaffold 补丁里，
   已用“从补丁重建整棵树”（`/tmp/b23`）与本步 selftest 验证一致）。
2. **runbook 里用反引号写路径等于没写**：`links` 门因此判“一页没有一个链接”——它判得对，凌晨三点的人要的是能点的东西。
   参考解的页面现在链到
   `src/delivery.rs`、`src/deliveries.rs`、`src/newsletter.rs`、`src/metrics.rs`、`src/logging.rs`、`src/http.rs` 与演练文件本身。

## 第十六段 · 2026-10 · M0 收尾：三份呈现自检、一次不可复现的红，与它的真因

M0 的三条验收命令在 `c08f33a` 上是绿的，但那验的是**功能**。这一段做的是**呈现**：把 22 篇正文逐句
对拍规格与 scaffold、把每个命令的输出实跑一遍、把维护文档与 CLI 的现实对一遍；随后追平了那次
**不可复现的红**。三份审计共 30 条发现，没有一条是纯粹的功能缺陷——全是措辞、格式或「说了不成立的话」。

### 改了什么（十二个提交）

- `65345cd` **课程正文** 13 处：1.4/1.14 的 TRAP 引用的是注释里的句子而不是规格真正打印的那句；
  1.12/1.13/1.16 把 rustdoc 的 `[`X`]` 语法漏进正文（页面渲染出方括号）；1.10 的行内代码里套了反引号；
  1.8 的 FURTHER 把 `std::net::TcpStream` 标成 `http::Request`；1.19 唯一一个没标语言的围栏；
  1.22 把 `backoff` 记在 1.15 名下（是 1.14 的）；1.16 把「不是令牌」的 400 记在 1.10（应 1.12）；
  1.6 说 `RunId` 什么都没做（scaffold 已实现）；1.9 把三个文件数成四件事；
  curriculum §6 说 1.1 用 `cargo metadata` 断言布局（实际是 `cargo check --lib`）。
  另外 `course.toml` 的结课语让学员跑 `kiln list relay`——那条命令当时 exit 2，改成 `kiln caps`。
- `6e618e3` **kiln 的输出**（最大的一块）：`doctor` 与失败的闸门打印 `exit code Some(101)`（Rust 调试格式）
  且 podman 的 ANSI 会渗进后面的行；已计划未写的 track 现在答「is not authored yet」并 **exit 3**
  （文档里的「3 = 内容未写」在此之前不可达）；给 track 型命令一个 step id、给一个空 id 都有人话回应；
  无 `app/` 的树里跑中段步骤不再报补丁解析器的「文件不存在」，而是说清哪条命令建项目（骨架步是例外），
  也不再留下空目录；失败的规格与通过的闸门都补上「下一步」指向；`caps` 的
  「(track not authored yet) not authored yet」、`check` 的「0 not implemented yet」、
  `images` 的提示位置都改了；`run`/`watch` 把子进程的流直接接到终端（`process::run_streamed`），
  转录顺序与发生顺序一致（此前 cargo 的进度行落在测试结果之后）；
  `list` 的行从最长 179 列收到 100 列、id 列按本 track 最长 id、`kind · time · teaches` 挪到第二行、
  `◐` 有页脚定义、`exit capabilities` 会折行；裸 `kiln` 的第一屏补上正文路径与项目目录；
  `watch` 每轮打时间戳（`[00:19]`，从开始计起）、头部只打一次、变更点名哪个文件变了
  （`cmd::project_files` 与闸门缓存共用同一次遍历）。
- `dc58e23` / `54fd694` / `d69440b` / `6ddabf6` / `6c09ea3` / `7dbba4f` **文档**：handoff 拆成简报 + 本日志；
  README 的状态段重写成实测数字；草稿标注为已归档；plan.md §11 记 M0 结束、§5 指名 design 步的样本、
  §10 的依赖基线改成「T1 已钉、T2 照做」。
- `573eb65` **T1 文档整理**：新增 **[t1-mailer.md](t1-mailer.md)**——22 步一览（交付物 + teaches +
  新接的门）、项目最后的形状（18 模块 / 10 条路由 + 兜底 / 4 个迁移 / 设置 / 退出码 / 镜像 /
  谁引的依赖）、8 个测试夹具与谁在用、可照抄到 T2–T5 的 11 个模式、实测数字、T1 范围内的缺口。
  curriculum §6 删掉与正文重复的「过关条件」列并指向它；handoff-m0 从 116 行收到 87 行、改成导航表；
  authoring §9 开头让人先看参考实现。
- `5145e34` / `62e7c5c` **报告看得懂**：`selftest` 的报告此前优先 stderr，而测试的 panic、断言消息与
  失败测试名在 **stdout**——红了也看不出是哪个测试。现在 `Ran::first_message()`（`verify` 与
  `selftest` 都走它）先找「说出失败是什么」的那一行（`thread '…' panicked at`、
  `---- <name> stdout ----`、`test result: FAILED`），找不到才退回 stderr。一条规则，`process.rs` 里，
  两处共用。
- `61a44c7` **端口的 TOCTOU**：`tests/support/service.rs`（1.16 的 scaffold）先 bind `127.0.0.1:0`、
  关掉监听，再把端口交给子进程；两个 spec 文件并行时可能拿到同一个端口——输的那个服务以
  `Address already in use` 退出，更糟的是**测试的连接会打到另一个测试的服务上**（任何 listener 都会
  接受连接）。改成 1.7 规格里早就演示过的握手：端口传 `0`，由内核交给**这个**子进程，装置从服务自己的
  `listening address=…` 行里读回来（pretty 与 JSON 都认），等待就绪前先问子进程是否还活着。1.7 与 1.14
  里各自复制的同类装置一并修掉。
- `62e7c5c`（同一个提交里）**1.18 的断言没有等待**：`a_subscriber_reads_their_own_deliveries` 发布一期
  之后立刻断言投递行「已发送」；而 `POST /newsletter` 答 `202` 只是排进队列（这正是 1.17 要教的），
  worker 先 claim、再发、最后才写 `sent_at`。六路并行下它的读数落在窗口里（`sent_at: null,
  attempts: 1`）。现在照 1.17 的写法等这个事实（`delivered_history`，10 秒上限），失败信息说明它在等什么。

### 那次红，与它的真因

`cargo xtask ci` 红过一次：527.51s、`21/22 step(s) hold the contract`，`mailer/1.18-authz-boundary`
（scaffold ✓ / 参考解应用 ✓ / 规格仍失败，exit 101），而**重跑五次全绿**（单步 selftest 30s、全量
selftest 500s、`cargo xtask ci` 530s、同一天 `verify` 22 passed、`kiln run` 六次全过）。

修完之后再看，**最可能的真因不是端口碰撞，而是 1.18 自己那条没等待的断言**：同一个步骤、同一个测试，
六路并行下两次里必红一次，而当时的报告看不出是哪个测试（这正好是 `5145e34` 要修的）。端口碰撞是同一段
harness 里另一处真缺陷（会让测试连到别人的服务上），一并修了——两件事都不是噪声。

**过程中还暴露了三处自己的毛病**（记下来，因为都是「验证本身不诚实」这一类）：
① 我漏跑 `cargo fmt --all`，`cargo xtask ci` 第一步就红，后面五步根本没跑（`5145e34` 的测试代码是
`style_edition = "2024"` 下的一行断行差异）；
② 调试端口碰撞时留下的临时探针文件 `app/mailer/tests/spec_probe_collision.rs` 没删，它是 git-ignored 的，
但 `fmt` 门会扫到它——于是 `verify` 里 21 个步骤的 `fmt` 门全红、1.3 的 spec 也红（它跑项目自己的
`cargo xtask ci`，第一步就是 fmt）。删掉后 `verify` 22/22；
③ 六路并行把 4 vCPU / 4 GiB 的 podman VM 打崩（`Cannot connect to Podman … podman machine start`），
残留 9 个 `kiln-pg-*` 容器，也一并清了。**三路并行是这台机器能承受的上限**，而 CI 与 selftest 本身是
逐步串行的，不受影响。

另外两条已知缺口（都不改，写在这里）：`xtask ci` 不跑 `kiln links`（只有每步的 `links` 门，且它只看
项目内那一页），所以仓库文档的链接腐烂只有手工 `kiln links` 拦得住；`spec/1.6/logging.rs` 与
`spec/1.9/migrations.rs` 仍自己挑端口——1.6 断言的是「启动行报告配置的那个端口」而其中一档是
`level=error`（那时服务什么都不说，端口无从得知），1.9 要的端口就是「这里没人听」这个前提本身；
两者的失败都会是响的。

### 复测

`kiln verify --json mailer`（`62e7c5c`，3m01s）：`passed 22 · failed 0 · blocked 0 · not_authored 0 ·
clean true`。`kiln selftest mailer/1.18-authz-boundary`：1/1 满契约。三路并行的 spec 二进制连跑四轮
（含 `spec_authz`）全绿——同一负载此前的失败率是两次一红。`kiln links` 31 页 / 206 链 / 0 findings；
`kiln --lang ja list` 正常回退。`cargo xtask ci` 在 `62e7c5c` 上的结果：见下一条更新。
