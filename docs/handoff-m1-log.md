# M1 构建日志（分段，按段检索）

> 与 [handoff-m0-log.md](handoff-m0-log.md) 同构：每一段记**当时**做了什么、量到了什么数字、踩了什么坑。
> 结论与导航在 [handoff-m1.md](handoff-m1.md)；机制与取舍在 [plan.md](plan.md)。

## 索引

- [第一段 · 2026-10 · 去重机制的运行时部分](#第一段--2026-10--去重机制的运行时部分) — 能力记录、probe、`review --due`、`plan --fast`、`skip`、lint R5 的 RECALL 检查
- [第二段 · 2026-10 · T2 `relay` 的元数据、骨架与第一步](#第二段--2026-10--t2-relay-的元数据骨架与第一步)

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
