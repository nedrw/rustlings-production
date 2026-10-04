# M1 交接简报（去重机制的运行时部分 + T2 `relay`）

> M1 有两半：**去重机制的运行时部分**（`probe` / `review --due` / `plan --fast` + 间隔重复）和
> **T2 `relay` 的 24 步内容**。这份文件是压缩后的上下文：目标与验收、已经落地的机制、怎么用它、
> 环境事实的增量，以及继续往下做时的约定。机制与取舍写在 [plan.md](plan.md) §3 D4 与 §10，
> 施工过程与实测数字写在 [handoff-m1-log.md](handoff-m1-log.md)。

## 1. M1 的目标与验收（plan.md §11）

| 验收项 | 判据 | 状态 |
| --- | --- | --- |
| `kiln lint-caps` 全仓通过 | R0–R6（含 R5 的 RECALL 真实性检查）零 issue | 见 §3 |
| T2 的 **6** 个 deepen/transfer 步（1 transfer + 5 deepen，见 `track.toml` 与 curriculum §7）没有 WHY 段 | `lint-caps` R5：`origin = deepen/transfer` 的正文必须 RECALL + DELTA | 见 §3 |
| `kiln plan --fast` 能正确跳过 T1 已证明的能力 | 证据必须是**通过的 probe**，不是完成的步骤 | 见 §2.3 |
| `kiln review --due` 有真实 probe 题 | `course/probes/` 里的题真的能失败、参考解真的能过 | 见 §2.2 |

## 2. 落地的机制（怎么用）

### 2.1 状态：每个能力一条记录

`.kiln/state.toml` 里多了一张表（[state.rs](../kiln/src/state.rs)）：

```toml
[capability."async/graceful-shutdown"]
level = "apply"               # 判到过的最深深度
last_verified = 1760000000     # 最后一次证明的 unix 时间
evidence = "mailer/1.14-worker"   # 谁证明的：步骤 id，或 `probe:<slug>`
```

写入时机：步骤的**规格与 must 闸门同时绿**（`kiln run` / `kiln check` 都会补这条记录），以及
`kiln probe <cap>` **自己**通过时。`--solution` 跑的参考答案**不写记录**（否则证据就不是你的了）。
旧 state 文件照常加载：缺的字段有默认值，能力表是从那一刻起才开始记的。

### 2.2 `kiln probe` / `kiln review --due`

- `course/probes/<domain>-<name>.<level>.rs` 是题目，`course/probes/solutions/<slug>.rs` 是参考答案。
  头部三行是元数据：`//! probe = "<能力>@<深度>"` / `title` / `hint`（解析在
  [probe.rs](../kiln/src/probe.rs)，文件名必须与头部一致）；
- `kiln probe` 列出题库；`kiln probe <能力>[@深度]` 在 `.kiln/probes/<slug>/` 建一份**你的**副本
  （只写一次，永不覆盖），`cargo test` 跑它，通过才写 `last_verified`；
  `--solution` 把参考答案写进你的副本（只做对照，不记证据）、`--reset` 恢复原题；
- `kiln probe --check`（在 `cargo xtask ci` 里跑）编译并运行每一份参考答案：题坏了不能悄悄躺着；
- `kiln review [--due [DAYS]]`：按上表列出「证明过什么、多久以前」，默认 45 天窗口，最旧的先列，
  每行给出重新抽查的命令。同深度的题没有时，**更浅的那道题会作为「更浅的抽查」被点名**（它抽查的是
  手感，不是这一深度，只有重做步骤才能重抽这一层）；完全没有题的槽位说「no probe for this slot」，
  不编题。
- **aware 槽位不出题**（curriculum.md §11）；probe 只用 `std`、离线、秒级。

### 2.3 `kiln plan [--fast]` / `kiln skip`

- `kiln plan [track]` 列出路径；`--fast` 给每一步判 `keep` / `skip`；
- **跳过规则只有一条**：这一步 `teaches` 的**每一个**槽位都有**通过的 probe** 作为证据
  （步骤自己的完成记录不算——那证明的是「跟着脚手架做过一遍」）。更深的深度永远不能跳；
- `kiln skip <step>` 执行一次跳过：应用 scaffold 与参考解（**记录 `solution_applied`**，项目才不会
  编译不过），`state.skipped = true`；`kiln list` 显示 `»`，**不计入完成数**，
  `kiln reset <step>` 把它放回待办。`--all` 跳过整条 track 上所有够格的点，`--force` 无证据也跳
  （报告里会写明「forced」）。
- **报告只点名题库里真有的题**：`plan --fast` 的 keep 理由与 `review --due` 的重抽查行走同一条判据
  （`probe::exercise`）——同深度或更深 → 点名那道题；只有更浅的题 → 说明它抽查的是手感而不是这一
  深度；完全没有 → 直说没有。没有这条规则时，报告会把学员送去一条必然报错的 `kiln probe` 命令。

## 3. T2 `relay`

- 进度（2026-10）：**24/24 已交付**。`kiln verify relay` → `24/24 judged step(s) pass · 0 failed ·
  0 not prepared · 0 without content`（2m34.8s）；`kiln selftest --track relay` 逐步为
  `scaffold ✓ the spec fails as it must / solution ✓ the spec passes / gates ✓ N/N must green`。
  交付物、门阶梯、项目最终形状、装置与模式、实测数字、剩余缺口见 **[t2-relay.md](t2-relay.md)**；
  施工过程与两次红（一次假红、一次真红）的判读见 [handoff-m1-log.md](handoff-m1-log.md) 第八段。
- 元数据：`course/tracks/relay/track.toml`（24 步，`exit` 能力 6 条，`clippy@2` 起、无提示）。
- 骨架与装置：`course/tracks/relay/skeleton/`（模块签名 + `todo!()`、`tests/support/{client,app,service,postgres}.rs`）。
- 步骤交付物与 T1 同构：正文 `course/docs/steps/en/relay/<id>.md`、规格 `course/tracks/relay/spec/<n>/<name>.rs`、
  scaffold / solutions patch、`track.toml` 的闸门接线在元数据里。
- 每步的验收命令：`cargo run -q -p kiln -- selftest relay/<id> --track relay`
  （`selftest` 的 `--track` 是 M1 新增的参数）。

## 4. 环境事实的增量（在 M0 那份之上）

| 事实 | 影响 |
| --- | --- |
| `kiln selftest --track <TRACK>` | 走非默认 track；沙箱里重新解析同一条 track |
| probe 副本放在 `.kiln/probes/` | 个人状态，git-ignored，不进任何 patch |
| 课程 patch 的生成 | 在**临时 git 仓库**里做 base/solution 两次 diff，`git diff --cached --no-renames --binary`；**不要**在工作树里 `rm -rf`（这是 M1 踩过的坑） |
| 共享构建目录会造**假红** | 所有 `kiln` 沙箱共用 `~/.cache/kiln/target`；与别的 cargo 并发时 `selftest` 会报「服务没起来」或反直觉的「scaffold 上就通过」。跑 selftest/verify 时旁边不要跑别的 cargo；可疑的红先单跑、再换干净 `XDG_CACHE_HOME` 复跑，两次一致才算真红（handoff-m1-log.md 第三段） |
| `App::start` 的就绪判据是端口连得上 | 而绑定发生在 migrate/seed **之前**，所以**直接读库**的规格必须先做一次 HTTP 往返握手（2.4 的 `Fixture::start` 是样本），否则可能读到空表 |
| proptest 的教学位置在 T4·4.13 | T2 的规格需要随机性时用确定性的小生成器（2.4 的做法），不引入 `proptest`，也不给骨架加 `[dev-dependencies]` 标记块 |

## 5. 继续往下做的约定

- 一个步骤 = 一个提交（正文 + 规格 + scaffold + 参考解 + 闸门接线）；框架改动单独提交。
- 每步收尾：`kiln selftest relay/<id> --track relay` 绿 → `lint-caps` 无 issue → `spec manifest --track relay` → 提交。
- 长验证（`cargo xtask ci` 约 10 分钟、`kiln verify --track relay` 分钟级）交给子 agent 在冻结的
  commit 上跑，只回报结论行与耗时。
- 每完成 5–6 步把 `docs/curriculum.md` §7 的状态与 `plan.md` §11 的进度同步一次。
