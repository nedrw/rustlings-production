# M1 构建日志（分段，按段检索）

> 与 [handoff-m0-log.md](handoff-m0-log.md) 同构：每一段记**当时**做了什么、量到了什么数字、踩了什么坑。
> 结论与导航在 [handoff-m1.md](handoff-m1.md)；机制与取舍在 [plan.md](plan.md)。

## 索引

- [第一段 · 2026-10 · 去重机制的运行时部分](#第一段--2026-10--去重机制的运行时部分) — 能力记录、probe、`review --due`、`plan --fast`、`skip`、lint R5 的 RECALL 检查
- [第二段 · 2026-10 · T2 `relay` 的元数据、骨架与第一步](#第二段--2026-10--t2-relay-的元数据骨架与第一步)
- [第三段 · 2026-10 · probe 题库第一批，与一次「假红」的真因](#第三段--2026-10--probe-题库第一批与一次假红的真因)

## 第一段 · 2026-10 · 去重机制的运行时部分

M1 的一半是「让 plan.md §3 D4 的规则真的可运行」。落地顺序：先状态，再命令，最后 lint。

### 落地的形状

- `state.rs`：`CapabilityState { level, last_verified, evidence }`，一张按能力 id 的表；
  写入时机是「步骤的规格 + must 闸门都绿」或「probe 自己通过」。`StepState` 多了 `skipped`，
  `StepStatus` 多了 `Skipped`（`»`，不计入完成）。
- `kiln probe`：`course/probes/<slug>.rs`（题）+ `course/probes/solutions/<slug>.rs`（参考答案）；
  学员副本在 `.kiln/probes/<slug>/`（只写一次，`--solution` 对照、`--reset` 回滚）；
  `--check` 把**所有**参考答案在一个临时 crate 里编一遍跑一遍，缺了/错了就红。
- `kiln review [--due [DAYS]]`：45 天窗口，按「最旧/无日期优先」排序，每行给重新抽查的命令；
  没有 probe 的槽位直说。旧 state（没有能力表）也能工作：已完成的步骤会被当作证据，但**没有日期**，
  因此排在最旧的一档——猜它很新是错的。
- `kiln plan [--fast]` / `kiln skip <step>|--all [--force]`：跳过规则 = 该步 `teaches` 的每一个槽位
  都有**通过的 probe**（步骤自己的完成记录不够）；`skip` 应用参考解并记录 `solution_applied`，
  `list` 显示 `»`，`reset` 放回。
- `lint-caps` R5 的第二个半边：`deepen`/`transfer` 的 RECALL 必须点名本课程**更早**的步骤
  （`track/<phase>.<index>` 且不能是自己或更晚）、必须点深度、且那个步骤必须真的建立过本步
  `requires` 或「本步要深化的、更浅的槽位」。这是「RECALL 写清上次那步的 id 与深度」的可执行版本。

### 数字

| 项 | 值 |
| --- | --- |
| `cargo test --workspace` | 200 个单测 + 15 集成 + 1（新增约 20 个：state 的能力记录、plan 的判定、probe 的解析、R5 的 RECALL） |
| `cargo xtask ci`（框架改动后，T1 未动） | **绿**，末行 `22/22 step(s) hold the contract` |
| `xtask ci` 新增 | `rustfmt --check` 扫 `course/tracks/**` 与 `course/probes/**`（`skip_children`，因为 spec 的 `mod support;` 只在项目里能解析）；`kiln probe --check`（exit 3 = 还没有题，允许并说出来） |

### 坑

1. **`lint-caps` 的 R5 一开始写太严**：最初要求 `deepen` 的 RECALL 指向「本步要深化的那个能力」的旧步骤，
   但 2.24 那种「深化 runbook 演练、同时新增一个能力」的步骤并不满足。改成统一规则：引用步骤必须
   建立过本步的 `requires`，或本步某个 `teaches` 槽位更浅的深度。
2. **`rustfmt` 会跟着 `mod` 走**：直接对 `course/tracks/*/spec/**/*.rs` 跑 `rustfmt --check` 会因为
   `mod support;` 找不到文件而报错——那是 scaffold 之后才存在的装置。`--config skip_children=true`
   才是对的（每个文件都在命令行上列出来了）。

## 第二段 · 2026-10 · T2 `relay` 的元数据、骨架与第一步

### 做了什么

- `course/tracks/relay/track.toml`：24 步全表（`teaches` 对齐 `capabilities.toml` 的槽位、
  `requires` 全部可达、闸门阶梯 `clippy@2` 起、`images` 只给 Postgres 步骤、`links` 只给交付页面的
  步骤、`?deny` 是 should 门）。`kiln lint-caps` 从 22 authored 变 46，零 issue。
- `course/tracks/relay/skeleton/`：模块签名 + `todo!()`（`config`/`error`/`logging`/`db`/`domain`/`auth`/`http`）
  + 已经能用的测试装置（`client`/`app`/`service`/`postgres`），后者从 T1 的装置改写而来。
- `spec/2.1/skeleton.rs`（4 个测试）+ `solutions/2.1-project-skeleton.patch` + 正文
  （`origin = transfer`，RECALL 点名 `mailer/1.1`、`1.4`、`1.6`、`1.9`）。
- `kiln selftest --track relay`：新增参数，否则 selftest 只会走默认 track。

### 数字

| 项 | 值 |
| --- | --- |
| `kiln selftest relay/2.1-project-skeleton --track relay` | **绿**：`scaffold ✓ the spec fails as it must`（4/4 失败，全部是 `not yet implemented: step relay/2.1-project-skeleton`）、`solution ✓ the spec passes`、`gates ✓ 9/9 must green` |
| 规格本身 | 4 个测试通过用时 **2.4 s**（含两个 Postgres 容器：一个 in-process app 一个真进程） |
| `relay` 的依赖 | axum 0.8.9、sqlx 0.9.0（`runtime-tokio`/`postgres`/`migrate`/`macros`/`uuid`/`chrono`）、tokio 1.53.1、toml 0.9.8、uuid 1.18.1、chrono 0.4.42、sha2 0.10、tracing-subscriber 0.3.23（+`env-filter`）；全部写在骨架的标记块里 |
| 骨架规模 | `src` 7 个文件（签名 + `todo!()`）、`tests/support` 4 个文件（约 700 行装置） |

### 坑

1. **课程 patch 必须在临时仓库里生成，不能在工作树里删文件**：这次生成 patch 的脚本一开始在
   `app/relay` 里 `rm -rf src migrations` 再用骨架重建「base」，第二次运行时把参考实现删掉了
   （`cp -a X src` 在 `src/` 已存在时会把 X 拷*进* src）。修法：patch 生成完全在 `/tmp` 的临时
   git 仓库里做，工作树只读。代价是重写了一遍 2.1 的实现——这条写进 handoff-m1 的约定里。
2. **`docs:public` 要求 `Result` 函数有 `# Errors` 段**：参考解里 11 个函数缺段（`missing_errors_doc`），
   骨架里也必须同步补上，否则学员从第一步起就红。
3. **`rustfmt` 不只是 `src/`**：`tests/support/*.rs` 也会被 `fmt` 门扫到（它就在项目里），骨架的装置
   必须一并格式化，否则第一步的 `fmt` 门就红。
4. **`--check-config` 的规约要给死**：`database.url = set` / `not set`、`bootstrap.tenants = <n> (名字)`——
   规格是只读的，学员必须从正文读到这个形状。写进 2.1 的正文与 `Config::report` 的文档里。

## 第三段 · 2026-10 · probe 题库第一批，与一次「假红」的真因

### 做了什么

- `course/probes/`：8 道 (能力, 深度) 抽查题 + 参考答案（`course/probes/solutions/`），全部单文件、纯 `std`、
  离线、秒级；35 个测试，逐题验证「练习副本必失败 / 参考答案必通过 / `--reset` 可回滚」。
  覆盖 `err/typed-errors`、`cfg/layered-config`、`domain/parse-dont-validate`、`sec/token-lifetime`、
  `db/unique-as-concurrency`、`delivery/idempotency`、`sec/authz-tenant-isolation`、`http/rate-limit`（都是 `apply`）。
- 每道题的正文里写清**这道题诚实地收窄在哪**：单文件、无依赖的前提下，CSPRNG（token 的不可猜性）、
  Postgres 的唯一约束、真实队列、HTTP 栈都用「注入的假件 + 形状断言」替代——这正是 probe 机制的规则
  （只考形状，不考 API），写出来是为了让学员知道这个替代是有意的。
- 顺带修了 `kiln` 的一个真缺陷（worker 发现的）：`verify_references` 把参考答案写成 `tests/<slug>.rs`，
  而 slug 的深度后缀必是点，rustc 拒绝带点的 crate name（`error: invalid character '.' in crate name`），
  于是 **`probe --check` 对任何题都不通过**。修法是一处归一化 `test_target_stem`（`.` → `_`）+ 回归单测；
  学员副本那半（`scratch_manifest`）本来就把 `.` 换成 `-`，所以只有 `--check` 这半坏了。

### 数字

| 项 | 值 |
| --- | --- |
| `kiln probe --check` | `8 exercise(s) · 8 reference(s) passed` |
| 逐题 `kiln probe <slot>` / `--solution` | 8/8 在练习副本上失败（`not yet implemented`，0 编译错误）、8/8 参考答案通过 |
| probe 的测试总数 | 35 |
| `review --due`（带一份 406 天前的 probe 记录） | 列出 `delivery/idempotency … re-check: kiln probe delivery-idempotency.apply`；没有 probe 的槽位直说 |
| `plan --fast`（`delivery/idempotency@apply` 有 probe 证据） | `22 steps · 20 to work on, 2 skippable by probe`，`» mailer/1.15-idempotent-delivery … proved by probe:delivery-idempotency.apply, 406 days ago` |

### 「假红」的真因（重要，以后别再踩）

全量回归在一棵内容正确的树上红了 3 步（`mailer/1.18-authz-boundary`、`1.19-test-layering`、`1.20-metrics`），
症状是 `the service stopped before it said where it listens: []` 与 `BROKEN STEP: the spec passes on the scaffold` ——
两种**相反**的假症状，同一棵树上不同时刻各出现一次。

归属验证：单跑那一步仍红 → `XDG_CACHE_HOME=$DELTA_SCRATCH_DIR/xdg`（换掉共享的
`~/.cache/kiln/target`）再跑 → **✓ 通过**。

真因：**本机所有 `kiln` 沙箱共用同一个 `CARGO_TARGET_DIR`**（`scratch::shared_target_dir`），当时外部
agent 正在同一台机器上编译，并发使用让某一步拿到不属于它的构建产物。**不是内容缺陷、也不该改判据。**

规则（写进 handoff-m1 的约定）：
- 跑 `kiln selftest` / `kiln verify` 时，本机不要同时跑别的 cargo；
- 报出的红如果与实现无关，先单跑那一步、再换干净 `XDG_CACHE_HOME` 复跑，两次一致才算真红；
- 长验证（`xtask ci` 约 10–20 分钟）交给子 agent 在**冻结的 commit** 上跑，跑的时候谁都不碰这棵树。

## 第四段 · 2026-10 · T2 2.2–2.7，与 probe 第二批

### 交付与数字

| 步 | 规格测试 | `selftest --track relay` 结论（原样） | must 门 |
| --- | --- | --- | --- |
| 2.2 幂等接收 | 5 | `scaffold ✓ the spec fails as it must` / `solution ✓ the spec passes` / `9/9 must green` | 9 |
| 2.3 契约优先 | 6 | 同上 / `8/8 must green`（本轮补齐 `images` 门） | 9 |
| 2.4 游标分页 | 7 | 同上 / `9/9 must green` | 9 |
| 2.5 原子领取 | 7 | `scaffold ✓ … (unresolved import relay::claim) — note: it does not compile on the scaffold` / `9/9 must green` | 9 |
| 2.6 至少一次 | 7 | 同上（`unresolved import relay::delivery`）/ `10/10 must green`（含 `links`） | 10 |
| 2.7 出站客户端 | 10 | 同上（`unresolved import relay::outbound`）/ `8/8 must green` | 8 |

- 合并验证：`selftest relay/2.5 relay/2.6 relay/2.7 --track relay` → `3/3 step(s) hold the contract`；`kiln verify relay` 在 2.4 那棵树上把 2.1–2.4 全判通过（早期步骤在最终树上复判成立）。
- probe 第二批 6 道（`db/atomic-claim`、`delivery/ordering`、`delivery/retry-budget`、`delivery/dlq`、`sec/ssrf-defense`、`obs/metrics-slo`），33 个测试；全库 `probe --check` → `14 exercise(s) · 14 reference(s) passed`。
- 并发相关的两题（`db-atomic-claim`、`delivery-ordering`）参考答案各连跑 40 次 `cargo test`，0 次失败（不靠 sleep 同步：Condvar 门 + 通道 + `Barrier`）。

### 施工中钉下来的决定（后续步骤要么遵守、要么同步改并重钉）

- **迁移号**：`0004` 被 2.4 占了，2.5 用 `0005`；两个同号迁移会让 sqlx 拒绝**全部**迁移（`duplicate key value violates unique constraint "_sqlx_migrations_pkey"`，实测踩到）。
- **`mark_dead` 也把 `attempts` +1**，否则「超过上限 → dead」与「重试计数」对不上。
- **`claim` 的 `RETURNING` 直接 join 出 target 与 event**：一个 tick 一条语句，也去掉两次查询之间的窗口。
- **fan-out 在 `delivery::fan_out(&mut PgConnection, …)`**，由 `events::ingest` 在同一个事务里调用。
- **崩溃窗口的模拟**：parked 假 sender + 丢弃 tick future（= 发出去了、记账前进程消失），再用 `claim::release` 把行交回来（租约回收属于 2.15）。好处是后续给 `SendRequest` 加字段不会弄红 2.6。
- **重定向语义**：301/302 保留方法与 body，只有 303 变 GET（webhook 的 payload 不能在路上被丢掉）。
- **契约面**（改签名要同步改 2.5/2.6/2.7 的规格并重钉 manifest）：`claim::{claim,release,mark_sent,mark_retry,mark_dead}`、`delivery::{tick,Sender,SendRequest,SendError,Receipt}`、`outbound::{Client,Policy,Outcome,HttpSender,Request,Response}`；2.7 的规格用 `..Policy::default()`，加字段不破。
- **2.6 的重试上限没钉常量**（规格只要求「12 轮内到 `dead`」且 `attempts >= 2`）：谁把它搬进配置，默认值必须 ≤ 12。
- **2.12 与 2.7 的已知张力**（写给 2.12 的作者）：2.7 的规格必须打 `127.0.0.1`（离线要求）。SSRF 的拒绝应当来自**配置层**（默认 `delivery.allow_private_targets = false`），而不是改 `Policy::default()`；若要改默认，必须在同一个提交里改 2.7 的规格并重钉 manifest。
- **2.3 的契约检查是一条持续性约束**：新路由必须进 `document()`，且它可能返回的每个状态都要有 schema——2.10 的 DLQ 路由是第一个撞上它的。

## 第五段 · 2026-10 · T2 2.8–2.11（重试预算、顺序、DLQ、签名）

### 数字

| 步 | 规格测试 | `selftest` 结论（原样） | must 门 |
| --- | --- | --- | --- |
| 2.8 重试预算 | 10 | `scaffold ✓ … (unresolved import relay::backoff)` / `solution ✓ the spec passes` / `9/9 must green` | 9 |
| 2.9 顺序与并发 | 5 | `scaffold ✓ … (unresolved import relay::ordering)` / `9/9 must green` | 9 |
| 2.10 DLQ | 6 | `scaffold ✓ the spec fails as it must (exit code 101: test a_replay_does_nothing_to_a_delivery_that_is_not_dead … FAILED)` / `9/9 must green` | 9 |
| 2.11 签名 | 9 | `scaffold ✓ … (unresolved import relay::signing)` / `9/9 must green` | 9 |

- `kiln verify relay` → **`11/11 judged step(s) pass · 0 failed · 0 not prepared · 13 without content`**（81 s）
  —— 2.1–2.7 的规格一个字节未改（`git diff` 为空），2.5–2.7 的契约签名（`Sender`/`SendRequest`/`claim::*`）保持不变。
- `lint-caps` `no issues`；`kiln links` 全仓 `43 page(s) · 288 link(s) · 181 external` ✓（这是 `dd47c11` 之后的新默认：报表跳过 `app/` 并点名跳过；合并前的旧数字按旧默认量的）；四个新规格与新装置（`tests/support/lanes.rs`、`keys.rs`）显式 rustfmt 干净。

### 施工中钉下来的决定

- **抖动退避取窗口上半段（equal jitter），不是教科书 full jitter**：full jitter 会答出近似 0 的等待，
  而 2.6 的规格在失败后**立刻**再跑一轮并断言「没到期就不能领」——那一步会在最终树上红。ADR 的
  `## Alternatives` 逐条论证了三种植；`delay` 仍由 `(uuid.as_u64_pair(), attempt)` 的 splitmix64 决定
  （确定性、无 rand）。
- **`hmac = "0.12"` 而不是 0.13**：0.13 要 `digest 0.11`（即 `sha2 0.11`），而项目里 `sha2 = "0.10"`
  那一行 scaffold 不能改写、且正被 API key 哈希使用。0.12 + sha2 0.10 + subtle 2.6 是自洽的一组。
- **签名密钥走端口（`signing::Secrets` + `DbSecrets`），不给 `SendRequest` 加字段**：2.7 的规格用**结构体字面量**构造它，加字段会让那份规格在自己那棵树上编译不过（2.11 的正文与 rustdoc 都写了这条；
  产品理由同样成立：密钥不进队列、不进触碰投递的结构体的 `Debug`）。
- **顺序是「lane」**：2.9 在 worker 内为每个 endpoint 建一条 lane（`Claimed::due_at` 带上
  `next_attempt_at`，两条 turn 路径都先按 `(due_at, delivery)` 排序再尝试），跨进程的顺序留给 2.15。
- **2.9 的正文是 teach 模板 + 额外的 `RECALL`/`DELTA` 路标**：`origin = teach`（ordering 首次教），
  五段必需项齐全；多出来的两段指向 2.6/2.7 与 `mailer/1.14`——`lint-caps` R5 接受，且这正是
  「相邻能力要写边界」那条清单规则的样本。authoring.md §4.1 记了这一点。

## 第六段 · 2026-10 · T2 2.12–2.15（SSRF、限流、背压、停机租约），与一次跨步骤的同步改动

### 数字

| 步 | 规格测试 | `selftest` 结论（原样） | must 门 |
| --- | --- | --- | --- |
| 2.12 SSRF 防护 | 9 | `scaffold ✓ … (unresolved import relay::target)` / `solution ✓ the spec passes` / `9/9 must green` | 9 |
| 2.13 限流 | 7 | 同上（`unresolved import relay::ratelimit`）/ `9/9 must green` | 9 |
| 2.14 背压 | 6 | 同上（`unresolved import relay::concurrency`）/ `9/9 must green` | 9 |
| 2.15 停机租约（deepen） | 8 | 同上（`unresolved import relay::lease`）/ `9/9 must green` | 9 |

- 合并 `4/4 step(s) hold the contract`；`kiln verify relay` → **`15/15 judged step(s) pass · 0 failed · 0 not prepared · 9 without content`**。
- `lint-caps` `no issues`（R5 只跳 9 篇未写正文）；course 下 80 个 `.rs` rustfmt 干净；`kiln links` `49 page(s) · 319 link(s) · 199 external` ✓；`i18n status` 结构 0 错、漂移 0 警告。

### 一次必要的跨步骤同步改动（记下来，因为它是「规格必须在每棵树上成立」的代价）

2.12 让**默认配置**的服务在**注册**与**投递**两处都拒掉私网/回环目标（这是 SSRF 的正确默认），而 2.5/2.6/2.8/2.9/2.10/2.11 的规格都用 `POST /endpoints` 注册 `http://127.0.0.1:9/...`。
不改它们，这 6 步会在最终树上红——所以那 6 份规格各加了一行显式放行：

```rust
var("RELAY__DELIVERY__ALLOW_PRIVATE_TARGETS", "true"),
```

用**字符串字面量**而不是 `tests/support/app.rs` 的常量：那个常量在 2.5 那棵树里还不存在，用它会让 2.5 的 scaffold 态因为错误的理由编译失败。这 6 份规格在各自的树上行为不变（未知环境变量被忽略），在最终树上由 `verify` 判过（全 `passed`），manifest 已重钉。

### 施工中钉下来的决定

- **`Policy::default()` 保持宽松，规则来自配置层**（2.7 的规格打 `127.0.0.1`，这是当初 2.7 交接里点名的张力，按预定方案解决）。
- **`target.rs` 不复述 scheme 白名单**：`http`/`https` 是 `domain::EndpointUrl`（2.1）的规则——第二份会忘记的地方不要有。
- **限流层放在认证之外**、匿名请求共享一个桶、桶表有上限（`SWEEP_AT = 64`，否则桶数随攻击者编造的 key 增长）。
- **2.14 的 503 来自「队列有底」**：`delivery.max_backlog`（默认 10000），检查放在 `events::ingest` 接受事件的那个事务里（拒绝时什么都不写、重放不会被 503）。
- **公平 claim 只能「每租户一条语句」**：PostgreSQL 不允许 `FOR UPDATE` 与窗口函数同用（容器里实测 `ERROR: FOR UPDATE is not allowed with window functions`），所以按队列位置排名的单语句写法不存在。
- **2.15 的停机是两条流都被 await**：第一版写成 `select!`，规格立刻抓到（服务端先返回 → 进程退出 → drain 被丢 → `claimed` 还剩 1 行）——正是 `mailer/1.14` 点名的 TRAP；`delivery.lease_seconds` 与 `delivery.drain_seconds` 进了配置。
- **`reclaim_expired` 计数、`release_held` 不计数**：无人旁观的那次尝试必须计数，否则反复崩溃的 worker 会让投递永远重试（计数是唯一能把它送进 DLQ 的东西）；主动交回的行没失败。
