# rustlings-production 规划（v2）

> 状态：规划已定稿；**M0（判定器 + T1 全部 22 步）已交付**，当前在 M1。T1 交付物见
> [t1-mailer.md](t1-mailer.md)，验收见 [handoff-m0.md](handoff-m0.md) §1。
> 配套文档：[课程体系与能力模型](curriculum.md)、[编写规范](authoring.md)、[M0 构建日志](handoff-m0-log.md)。
> 环境基线（已实测，2026-10）：rustc/cargo 1.98.1、edition 2024；容器运行时是 Podman 6.1.2 的 docker
> shim，`docker compose` 插件不可用（本机另有可用的 `docker-compose` 单体版）；无 `timeout`（无 coreutils）。
> 逐条实测事实以 [handoff-m0.md](handoff-m0.md) §2 为准。

## 0. 一句话

rustlings 的训练场是「单文件小题」；这门课的训练场是**五个从简单到复杂、各自独立的生产级服务项目**，
步步可验证（规格测试 + 生产闸门），且**同一能力不会在不同项目里重教一遍**——讲解去重由机制强制，不靠作者自觉。

## 1. 已确认的约束（v2 的输入）

| 项 | 结论 | 对设计的影响 |
| --- | --- | --- |
| 时长 | 不限制，优先内容完整性 | 课程规模扩到 5 条 track / 约 118 步；慢闸门可以放心保留；里程碑以「内容完整度」验收而不是工期 |
| 项目 | 多条 track，从简单到复杂；复杂项目不得重复简单项目的内容 | 新增**能力台账（capability ledger）+ 去重 lint**，见 §3 D4 与 [curriculum.md](curriculum.md) |
| 首条 track | 邮件微服务（题材参考《Zero To Production in Rust》） | T1 `mailer`，见 [curriculum.md](curriculum.md) |
| 内容语言 | 英文为源，但预留切换接口，后续可能加语言 | 全套 i18n 目录约定 + 回退链 + 翻译漂移检测，见 §3 D5 与 [authoring.md](authoring.md) |
| 使用场景 | 自学为主 | 保留诚实度记录（轻量）；新增基于能力的间隔重复（§10） |
| 依赖 | 接受容器 | 但必须抽象运行时（本机是 podman shim、compose 不可用），见 §9 |

### 目标

- 从「会写 Rust」到「能设计、交付、运维一个 Rust 服务」；每条 track 都从空目录长到一个可部署的服务。
- 每一步都能**离线、确定性地**判定过/不过，一条命令出结果（rustlings 的核心体验）。
- 课程本身可长期维护：能力去重、翻译漂移、版本腐烂都有机械检查。

### 非目标

- 不教语法基础（前置：rustlings 全通或等价经历）。
- 不做在线判题、不做防作弊 DRM（自学场景；机制见 §3 D2）。
- 不覆盖全部生态：FFI、`no_std`/embedded、WASM、GPU 不在核心路径（后续可作第 6 条 track）。

## 2. 核心机制

| 机制 | 内容 | 对应生产现实 |
| --- | --- | --- |
| A 项目脊骨 | 5 条 track，每条都是完整项目（T1→T5 依次变难） | 真实代码库的历史负担 |
| B 不可篡改规格 | 规格测试只读 + SHA-256 校验 + `kiln spec restore` | 需求/测试不由实现者修改 |
| C 生产闸门 | lint 阶梯只升不降、变异测试、性能预算、公开 API 基线、依赖审计、unsafe 预算 | 评审与 CI 的实际标准 |
| D 运维视角 | 优雅停机、故障演练、runbook、SLO、在线迁移 | 交付之后那 90% 的工作 |
| **E 能力台账与去重** | 每个步骤声明它「教什么能力、到哪个深度」，跨 track 的重复被 lint 拦住 | 老手不需要被重教，新手不会被跨越式难度打懵 |

每一步固定模板：首次教学用 **WHY / TASK / DONE WHEN / TRAP / FURTHER**；
迁移步骤（能力已在前面 track 学过）用 **RECALL / DELTA / TASK / DONE WHEN / TRAP**——
RECALL 指向上次学的那一步，DELTA 只讲「这次有什么不一样」。模板与检查规则见 [authoring.md](authoring.md)。

## 3. 关键决策

### D1 内容形态：独立小题 vs 单项目递进 → **混合**

- **milestone**：在真实项目树里写代码，测试来自只读规格目录（主线）。
- **drill**：单文件填空，像 rustlings 一样在临时 crate 里编译，只用于纯机制训练（生命周期、`Pin`、`dyn` 兼容性、迭代器适配器），放在需要它的 milestone 前一步。
- 每步有参考解，可应用可回退；`milestone` 步另有 scaffold patch（只新增文件、只在标记块内追加依赖），见 [authoring.md](authoring.md)。

否掉的方案：纯独立小题（等于 rustlings 加难版，学不到约束累积）；纯单项目（无机制训练，且落队即卡死）。

### D2 测试可信度：全靠自觉 vs 哈希+闸门+记录 → **机制约束，不做 DRM**

- 规格测试在只读目录 `spec/`，哈希记在**生成的** `spec/manifest.toml` 里（落地时的澄清：
  哈希本来打算写进 `track.toml`，但那是手维护的文件，让工具反复重写它会淹掉真正的改动；改成规格目录旁边
  一份生成物，`track.toml` 继续只写人写的字段）；被改动则 `spec-integrity` 失败，`kiln spec restore` 一键恢复。
- `cargo-mutants`（限本步模块）证明规格测试不是空测——比人工写 kill patch 可靠；后者只保留给课程自检。
- `kiln verify --json` 如实记录 `clean` / `solution-applied` / `spec-modified`。自学场景不阻止应用参考解，但留痕。

### D3 判定器：fork rustlings vs 自研 → **自研，沿用 rustlings 的接口约定**

rustlings 的 runner 假设「把一个 `.rs` 拷进临时 crate 跑 `cargo test`」，而主线要的是「按步在学员 workspace 里跑指定测试 + 一组闸门 + 记录能力状态」。改造它比自己写更贵。
结论：复用**接口约定**（命令名、清单文件、状态文件、终端文件链接、watch 触发）与**依赖**（`notify`/`crossterm`/`clap`/`anyhow`/`serde`+`toml`），实现自研。

| rustlings | kiln | 说明 |
| --- | --- | --- |
| `rustlings` | `kiln` | 跳到第一个未完成步骤 |
| `rustlings list` | `kiln list [track]` | 分 track 列进度与能力覆盖 |
| `rustlings run <name>` | `kiln run [step]` | 跑本步规格测试 |
| — | `kiln check [step]` | 只跑闸门（不跑规格测试） |
| `rustlings watch` | `kiln watch` | 存盘即重跑 |
| `rustlings hint <name>` | `kiln hint [step]` | 提示（迁移步的提示只给差异方向） |
| — | `kiln verify` | 全量校验，CI 模式，`--json`（含 `--slow`） |
| `rustlings reset <name>` | `kiln reset [step]` | 回到本步 scaffold 状态 |
| — | `kiln spec restore` | 从哈希清单恢复规格文件 |
| — | `kiln plan [--fast]` | 按能力台账算路径：哪些步要真做、哪些可跳过 |
| — | `kiln caps` | 能力台账：深度、证据、上次验证时间 |
| — | `kiln review --due` | 间隔重复：久未用到的能力重新抽查 |
| — | `kiln lint-caps` | 课程自检：跨 track 重复教学、依赖不可达 |
| — | `kiln --lang`（全局开关）/ `kiln i18n status` | 切换内容语言；翻译覆盖率与漂移 |
| — | `kiln images [pull]` | 探测容器运行时、预拉本 track 需要的镜像 |
| — | `kiln doctor` | 一次性诊断：工具链、容器运行时、缺失的 cargo 工具、镜像、DB 连通性 |
| — | `kiln review [step]` / `solution [step]` | 我的 diff vs 参考解 + 自评清单 / 应用参考解（留痕） |
| — | `kiln selftest` | 课程防腐烂：参考解必过、规格必能杀变异、i18n 与哈希一致 |

关键约束：**唯一真源是清单文件（`course.toml` + `track.toml` + `capabilities.toml`），不是这个工具**。
清单带全部元数据，所以将来换成 `xtask`、网页端或编辑器插件都不会丢课程内容。

### D4 多 track 与「不重复」→ **能力是货币，讲解只给一次，迁移在各项目重做**

这是 v2 最重要的设计，直接答复「复杂项目不要再重复简单项目的内容」。

- 一切可学的东西注册为**能力**（`http/typed-errors`、`async/graceful-shutdown`、`store/wal-recovery`…），每个能力有四个深度：
  `aware`（能读懂改）→ `apply`（能独立实现）→ `design`（能讲清取舍并选型）→ `operate`（能在生产排障调优）。
- 每个步骤声明 `teaches = ["async/graceful-shutdown@apply"]` 与 `requires = ["async/runtime-model@aware"]`。
- 去重不是承诺而是 **lint 规则**（`kiln lint-caps`，进 CI 与 `selftest`）：
  1. 同一 track 内同一能力不得在同深度重教；
  2. 全路径上同一能力的深度**严格递增**，除非显式标 `reteach = "理由"`；
  3. 每个 `requires` 必须在该步之前已教过；
  4. 每个 `teaches` 必须绑到本步至少一个闸门或规格测试（只讲不考不允许）；
  5. `origin = deepen`/`transfer` 的正文不得出现 WHY 段，`teach` 必须出现（结构检查）；
  6. 每条 track 声明的 `exit` 能力必须被本 track 覆盖。
- **三种正文姿态**：`origin = teach`（首次，模板 WHY/TASK/DONE WHEN/TRAP/FURTHER）、`deepen`（同能力升深度）、`transfer`（同能力同深度换项目）——后两者用 RECALL/DELTA 模板，不重讲。
- 使用体验：学员走到 `deepen`/`transfer` 步时，runner 打印 RECALL 卡片——「`async/graceful-shutdown@apply` 你在 `mailer/1.14` 学过（约 62 天前）。本次 DELTA：在途任务持有数据库租约，drop 的代价从「丢一次投递」变成「30 秒内不可见」。」——并且闸门更严、提示更少。
- 想跳过：`kiln plan --fast` 允许对已证明能力跳过「讲解 + 实现」（`kiln probe <cap>` 出具证据），但**不能跳过更深深度的步骤**（那些是新东西）。**跳过资格不看年龄**：跳过是可选的，一次通过的 probe 多久之后仍然解锁那一步；45 天窗口只属于 `kiln review --due`（复习提示），不是资格上限。

结论：**讲解去重是强制的；实现在新项目里必须重做，但考的只是迁移能力**——这正是「复杂项目不重复」与「真的会长本事」的平衡点。

### D5 内容语言：英文为源 → **一个目录约定 + 回退链 + 漂移检测**

- `docs/steps/en/<track>/<step>.md` 是唯一真源，其它语言是 `docs/steps/<locale>/<track>/<step>.md`。
- 翻译文件带 front-matter：`locale` / `source_hash`（翻译时英文源的哈希）/ `translator`。
- 回退链 `zh-Hans → zh → en`，回退时在终端明确提示「本步尚无中文版，回退到英文」，不报错、不阻塞。
- `kiln` 自己的提示语也在 `locales/<locale>/ui.toml`，所以**切换接口同时覆盖课程正文与 CLI**。
- `kiln i18n status`：覆盖率、漂移（`source_hash` 与当前英文源不符 = 需要重翻）、结构缺失（标题段不全）。CI 对**结构**报错、对**漂移**告警。
- 验收标准（写进 M0）：`kiln --lang ja` 在没有任何 `ja` 文件时必须优雅回退、正常走完全流程。加一门语言 = 新建目录 + front-matter，**不改代码**。

### D6 track 之间不共享代码 → **各自独立 workspace（T5 一个例外）**

每条 track 是一个独立的 cargo workspace（`app/mailer`、`app/relay`…），不是一个大 monorepo：

- 让「跳过整条 track」是安全的（能力台账可以跳，代码依赖不会断）；
- 让每条 track 的 `rust-toolchain.toml`、CI、lints、发布流程都能独立演进——生产里也是这样；
- 代价是 5 份代码要各自保持能编译能通过，用 `selftest` 与钉工具链控制。

唯一例外：T5 允许用 `path` 依赖复用 T3 `jobq` 的 crate——那正是「把引擎抽成库再复用」的毕业验收；
当 T3 的 state 里有 `solution-applied` 时，`kiln doctor` 会提醒 T5 的结果可能不代表你的水平。

## 4. 仓库结构

```
rustlings-production/
├── README.md
├── rust-toolchain.toml            # 钉死工具链
├── kiln/                          # 判定器（学员会读到它，所以它自己就是范本代码）
│   └── src/{main.rs,cmd/,gates/,caps/,i18n/,runtime/,info.rs,state.rs}
├── course/
│   ├── course.toml                # format_version / 默认语言 / 默认 track / 欢迎语
│   ├── capabilities.toml          # 能力台账（注册表 + 每条 track 的 entry/exit）
│   ├── glossary.toml              # 术语表（翻译一致性的源头）
│   ├── docs/steps/en/<track>/*.md # 步骤正文（英文为源）
│   ├── docs/steps/<locale>/...    # 译文（可缺，缺则回退）
│   ├── locales/<locale>/ui.toml   # CLI 文案
│   └── tracks/<track>/
│       ├── track.toml             # 步骤清单（唯一真源）
│       ├── spec/                  # 只读规格测试 + 哈希清单
│       ├── scaffold/<step>.patch  # 每步的项目骨架增补（只新增、只追加）
│       ├── solutions/<step>.patch # 参考解
│       ├── skeleton/              # track 起始骨架（第一步用）
│       └── drills/                # drill 型题目
├── app/<track>/                   # 学员项目（各自独立 workspace）
├── xtask/                         # 与 CI 同源的检查入口
└── .kiln/state.toml               # 进度、能力台账状态、闸门结果、诚实度标记
```

`course/tracks/mailer/track.toml` 的一个完整步骤（其余同构）：

```toml
[[step]]
id = "1.14-worker"
title = "Background delivery worker"
kind = "milestone"                       # milestone | drill | design
origin = "teach"                          # teach | deepen | transfer
entry = "docs/steps/en/mailer/1.14-worker.md"
time_hint = "90m"                        # 只作为难度提示，不作为验收
concepts = ["bounded queue", "drain on shutdown", "retry with jitter"]
teaches = ["async/graceful-shutdown@apply", "async/backpressure@aware"]
requires = ["http/service-skeleton@apply", "db/migrations@apply"]
run = { cmd = "cargo test -p mailer --test spec_worker" }
spec = ["spec/1.14/worker.rs"]
scaffold = "scaffold/1.14-worker.patch"
gates = ["spec-integrity", "fmt", "clippy@2", "docs:public", "no-todo", "exit-code"]
hint = "Don't only await ctrl_c inside select!: stop accepting first, drain in-flight, then force-exit on deadline."
solution = "solutions/1.14-worker.patch"
images = ["postgres:17-alpine"]          # 需要容器：doctor/images 会先预拉
```

Gates 用极小字符串 DSL：`name` / `name@参数` / `name:参数`；实现在 `kiln/src/gates/`，一个闸门 = 一个函数 + 一行注册表条目，可单测。

落地时的两点澄清（M0 第一段，与上面的示例一致）：

- `id` 在 `track.toml` 里是 **track 内**的（`1.14-worker`），因为文件本身已经在某条 track 下；
  CLI、文档、日志用完全限定 id `mailer/1.14-worker`，由加载器拼出（authoring.md §1 的全局 id）。
  `kiln run mailer/1.14`、`kiln run 1.14`、`kiln run 1.14-worker` 都接受。
- `entry` 是**相对 `course/` 的英文源路径**，且必须落在 `docs/steps/<locale>/<track>/` 下；
  译文路径由 i18n 层只换 locale 那一段得到，所以加一门语言不需要改任何元数据。

## 5. 题型

| kind | 学员改什么 | 怎么判 | 用在 |
| --- | --- | --- | --- |
| `milestone` | `app/<track>` 里的真实代码 | 跑 `run.cmd`（只读规格测试）+ 闸门 | 主线 |
| `drill` | 单个 `.rs` 填空 | 拷进临时 crate 跑 `cargo test`（rustlings 同款） | 机制训练 |
| `design` | 文档（runbook / ADR / 设计文档）**＋可选的可执行演练** | 文档结构 + `links` +（有 `run` 时）跑 `run.cmd` | 每条 track 末尾 |

`design` 保留 `run` 的理由（M0 第一段的实测反馈）：课程里每条 track 末尾的步骤实际交付两样东西——
一份人要读的文档（runbook / 演练手册）和一个能重放的演练脚本（断言「无丢单」这类不可观测承诺），
所以 `design` 的判定是「文档结构 + 链接 + 可选演练」，而不是纯文档。`run` 仅在 `design` 下可选。
T1 的 `mailer/1.22-runbook` 是这个题型的参考样本（正文、规格、页面各一份，见 [t1-mailer.md](t1-mailer.md) §5）。

`origin` 不是独立题型，而是 milestone/drill 的一个属性：它决定正文模板（`teach` 用 WHY 模板，`deepen`/`transfer` 用 RECALL/DELTA 模板）与提示强度（§3 D4）。

## 6. 闸门

闸门分两档：`must`（不通过不算完成）与 `should`（告警与改进提示，不阻塞）。
理由：真实评审里两者都重要，但入门阶段被 `pedantic` 全家桶劝退是常见失败模式。

| 闸门 | 检查 | 实现 | 从哪开始 |
| --- | --- | --- | --- |
| `spec-integrity` | 规格文件未被改动 | SHA-256 对照清单 | T1 步 1 |
| `caps` | 能力声明合法（去重、依赖可达、证据绑定） | `kiln lint-caps` | T1 步 1 |
| `fmt` | 格式统一 | `cargo fmt --check` | T1 步 2 |
| `clippy@N` | lint 阶梯（下表） | `cargo clippy`（测试目标单独放宽） | T1 步 2 |
| `no-todo` | 无 `todo!`/`unimplemented!`/`dbg!` | clippy restriction lints | T1 步 5 |
| `docs:public` | 公开项有文档**与错误说明** | `missing_docs` + `missing_errors_doc` | T1 步 5 |
| `exit-code` | 失败路径退出码非 0 且错误可读 | 子进程测试 | T1 步 4 |
| `i18n` | 译文结构完整、哈希可追踪 | `kiln i18n status` | 全课程 |
| `images` | 本步需要的镜像已就位 | 运行时探测 + 预拉检查 | T1 步 9 |
| `conformance@N` | 同一套套件跑所有实现（内存/PG） | `kiln/src/gates/conformance.rs`：按项目 `conformance.toml` 的后端列表用 `KILN_BACKEND` 逐后端重跑本步规格 | T3·3.5 |
| `mutants@阈值`（slow） | 规格测试能杀掉本步模块的变异 | `kiln/src/gates/mutants.rs`（自研，限本步参考解碰过的模块，每步最多 6 个变异体） | T3·3.20 |
| `?coverage@阈值`（slow, **should**） | 覆盖含错误分支 | `llvm-profdata`/`llvm-cov`（工具链组件）；缺失时 `blocked` + `rustup component add llvm-tools-preview` | T3·3.21 |
| `bench-budget@P`（slow） | 计数预算在课程基线的 ±P% 内 | `kiln/src/gates/budget.rs` + `course/tracks/jobq/budget/<step>.toml`（判计数，不判墙钟） | T3·3.22 |
| `sim@seed`（slow） | 确定性模拟下的分布式行为 | `turmoil` / `madsim` | T4/T5 |
| `soak`（slow） | N 分钟无泄漏、无任务堆积 | 长跑脚本 + `tokio-console` | T4 |
| `?deny`（**should**） | 许可/版本/来源策略 | `cargo-deny`；未装时 `blocked` + `cargo install cargo-deny --locked` | T2 末 |
| `unsafe-budget@N` | 无未注明 unsafe | `kiln/src/gates/unsafe_budget.rs`：源码里的 `unsafe` 计数 + 每处 `// SAFETY:` + `unsafe_code = "deny"` | T3·3.24 |
| `semver@baseline` | 公开 API 与基线兼容 | `kiln/src/gates/semver.rs`：用 `rustc` 编译课程基线见证文件（`cargo-semver-checks` 是生产里的做法，不进闸门） | T3·3.2 |
| `container-repro` | 镜像跑的是同一面二进制、且体积在预算内 | `kiln/src/gates/container_repro.rs`（重新构建 + `--check-config` 对照） | T1 步 21 |
| `links` | 文档链接有效（离线：相对目标 / 锚点 / 占位符） | 内置检查器（`kiln links`） | 每条 track 末 |

`clippy@N` 阶梯（**严格度随进度只升不降**——这是「成长感」的来源，也是课程最强的机制之一）：

- `@0` 默认 clippy。
- `@1` `-D warnings` + `todo` / `dbg_macro` / 库内 `print_stdout`。
- `@2` + `unwrap_used` / `expect_used` / `indexing_slicing` / `panic`（**仅非测试目标**；测试目标放宽）。
- `@3` + `disallowed_methods`（业务代码里的 `SystemTime::now`、库里的 `std::env::var`）+ `missing_errors_doc` + `pedantic` 白名单子集。
- `@4` + `mutants` / `deny` / `semver` / `bench-budget` 全部升为 must（T3 起）。

两条实用规则：

1. **快闸门优先**：普通步骤 `kiln run` 目标 < 60 秒；慢闸门标 `slow`，只在 `kiln verify --slow` 跑。反馈速度就是 rustlings 的全部魔力。
2. **panic 政策要讲清楚**：启动期早失败（缺配置、端口占用）允许 panic + 注释；请求路径上不允许。闸门按此实现，而不是一刀切禁止 panic，否则学员学到的是坏习惯。

还有一条关于**闸门作用域**的澄清（M0 第一段的实测反馈）：

- **持久属性**累积：`spec-integrity`、`fmt`、`clippy@N`、`no-todo`、`docs:public`、`exit-code`、`i18n`、`caps` 一旦引入就贯穿后续所有步骤（只加不减）。
- **步骤作用域属性**只附着在拥有那个产物的步骤上：`images` 只在该步确实需要容器时出现，`container-repro` 只在容器化的那一步，`links` 只在交付文档的步骤。
- 两者不矛盾的原因：`kiln verify [track]` 会把**每个步骤自己的闸门都跑一遍**，所以 1.21 之后把 Dockerfile 改坏的回归会在全量校验里被 1.21 的 `container-repro` 抓到。
  也就是说：**回归保护靠 verify 的全量执行，不靠闸门列表无限累积**。

### `container-repro` 判「镜像里就是这棵树的那面二进制」（定于 1.21，判据别放宽）

它做的是项目内测试做不到的那件事：按 `Dockerfile` **重新构建**镜像，判体积上限（25 MB，与步骤规格同一个数），然后用镜像里的二进制和**宿主二进制**答同一个问题——`--check-config`（1.4），逐行比对。实测：16.5 MB / 25 MB、七行完全相同、热态十二秒。

落地时量出来的三条（后面改这个门时别绕回去）：

1. **基础镜像名要解析，不能假设**：多个镜像源并存时同一个仓库路径会存两份（各一个架构），`podman build --pull=never` 对不属于本机的那个回 `image not known`（构建跑到第三阶段才报）。`runtime::images` 现在优先选**本机架构**的那份——`images` 门因此也报出真正能用于构建的名字。
2. **构建缓存是「可以要」，不是「强加」**：夹具挂载缓存并通过 `CARGO_TARGET_DIR` 传路径；不认这个参数的 `Dockerfile` 照常构建，认的那个得把二进制 `cp` 出挂载点。把缓存挂在默认 target 目录上（第一版就是这么做的）会让**每个朴素 Dockerfile** 的镜像里一个二进制都没有——惩罚了这一步要教的东西。
3. **构建比 runner 的默认超时更需要时间**：`run = { cmd, timeout }` 现在是元数据的一部分（取步骤声明与 `kiln --timeout` 的**较大值**，同时作用于该步的规格与闸门）。第一次构建在没构建过的机器上是分钟级，被截止时间砍掉的构建只会报「timed out」，而这一步其实在正常工作。

### `links` 判树，不判外链（定于 M0 实现 1.22 之前，别改回去）

原表里写的实现是 `lychee`。落地时的决定是**不引入它，也不引入任何联网判定**：

- **闸门离线**，只判树自己的事实：相对目标是否逐段存在（精确比对目录项，不借文件系统的大小写宽容）、`#锚点` 与 `page.md#锚点` 是否命中标题、目标是不是占位符（`TODO`/`TBD`…）或绝对路径、是否越出项目、是否指向 `target/`；项目里至少要有一页，且全树至少有一个链接（没写的页面不能被判成过关）。这些问题同棵树问两次答案一样，所以 `links` 也在 `CACHEABLE` 里（`kiln/src/cmd/mod.rs`）。
- **外链连通性只在 `kiln links --external`**（需要 `curl`），永不进闸门、永不进 `cargo xtask ci`。理由是本文件的 §1 与 §8.2：**一个 URL 今天答不答，只有它自己的服务器知道**，而「这一步做完了没有」不能挂在别人的可用性上。它只把 404/410 当失败，403/429/5xx/超时打印为 `?` 并以 0 退出——会因外部原因变红的检查，最后没人跟。
- **代价要写清**：`links` 保证的是结构（页面存在、目标有主、锚点真实），抓不到链接腐烂；腐烂要靠 `--external` 或人工回访。课程自己的正文有 110+ 个外链，正是这条决定避免让它们成为最大的不稳定源。
- 机制细节：外链那半只在 `kiln/src/cmd/links.rs`，闸门（`kiln/src/gates/links.rs`）里**没有一条能到达网络的代码路径**，所以「闸门不联网」是结构事实而不是约定。

## 7. 课程体系

完整内容见 **[docs/curriculum.md](curriculum.md)**：能力注册表、track 依赖关系、5 条 track 的全部阶段与步骤、以及「同一能力跨 track 演进」的实例对照。

概览：

| track | 项目 | 难度 | 步数 | 主要新能力 |
| --- | --- | --- | --- | --- |
| T1 | `mailer` 邮件订阅微服务（题材参考《Zero To Production in Rust》）**已交付** | 入门→中 | 22 | 服务骨架、类型化错误、迁移、后台 worker、可观测基线、容器发布 |
| T2 | `relay` Webhook 投递服务 | 中 | 24 | 至少一次投递、顺序与幂等、DLQ、HMAC 签名、SSRF 防护、限流、多租户 |
| T3 | `jobq` 通用任务队列（库 + 服务 + CLI） | 中高 | 26 | 库设计、事务与 `SKIP LOCKED` 深化、cron、变异/覆盖率/基准闸门、在线 schema 演进 |
| T4 | `lakelet` 遥测摄取与查询 | 高 | 24 | WAL 与崩溃恢复、列式存储与压缩、查询引擎、批处理与写放大、性能剖析与 soak |
| T5 | `meshkv` 分布式 KV / 调度（可选） | 最高 | 22 | 共识与成员变更、确定性模拟测试、线性一致读、mTLS、集群故障演练 |

默认学习路径是 T1→T2→T3→T4；T5 可选。每条 track 自成一个交付，前一条的代码不被后一条复用（§3 D6）。

## 8. 编写规范

完整规范见 **[docs/authoring.md](authoring.md)**（步骤模板、能力声明规则、i18n front-matter、scaffold patch 规则、检查清单）。核心 7 条：

1. 步长 30–120 分钟，超了就拆；单步新概念 ≤ 3 个。
2. 每步必须一条命令可验证，默认离线（联网只用于拉 crate/镜像）。
3. 快路径 < 60 秒；慢检查标 `slow`。
4. TRAP 必填：写不出常见错解，说明这步还没想清楚。
5. 参考解必须过本步全部 must 闸门，且被 `kiln selftest` 持续验证。
6. scaffold 只新增文件、只在标记块内追加，绝不改学员可能编辑过的行（否则 patch 必然冲突）。
7. 闸门作用于目标类型：库代码严格、测试代码放宽，例外必须写进课程文档而不是藏进 `allow`。

## 9. 环境适配（实测出来的约束）

本机实测：`docker` 是 **Podman 6.1.0 的 shim**（`docker info` 返回 podman 结构、socket 在 podman machine 路径），
`docker compose` 插件**不可用**（转发到不存在的 `/usr/local/bin/docker-compose`）；镜像缓存里只有来自国内镜像源的
`alpine:3.20` / `debian:bookworm-slim` / `rust:1.88-bookworm`，**没有 postgres/redis**；`busybox` 拉取 120 秒未完成；
系统无 `timeout`（无 coreutils）。因此：

1. **容器运行时必须抽象**：`kiln` 提供一个 `ContainerRuntime` 探测层（podman / docker / lima 兼容 socket），优先 `podman`，回退 `docker`；步骤需要容器时通过它执行，不直接用 `docker` 字面量。
2. **不硬依赖 `docker compose`**：同时支持 `podman compose` / `podman-compose` / `docker compose` / 直接 `run`，由 `doctor` 报出哪条路可用（本机只有直接 `run` 可用）。
3. **镜像预拉**：步骤声明 `images = [...]`，`kiln images` 报告本地有没有、`kiln images pull` 一次性预拉并在 `doctor` 里报告；支持镜像前缀映射（本机在用 `docker.1ms.run` / `docker.m.daocloud.io`）通过 `KILN_REGISTRY_MIRROR` 覆盖。两条解析规则（实现在 `kiln/src/runtime.rs`，学员侧的测试装置照抄同一套规则）：
   - 声明的名字是 Docker Hub 名字（`postgres:17-alpine`），除非它自己点了 registry（`ghcr.io/…`、`localhost:5000/…`）；走镜像时官方镜像要加 `library/`，即 `docker.1ms.run/library/postgres:17-alpine`。
   - **本地存在性按仓库路径判断**：引擎里存的是当初拉下来时的名字，所以声明 `postgres:17-alpine` 在 `docker.1ms.run/library/postgres:17-alpine` 下也算就位；`myorg/postgres:17-alpine` 不算（那是另一个镜像）。本机 `KILN_REGISTRY_MIRROR` 没设、镜像名带前缀，靠的就是这条。
4. **闸门不得依赖 GNU userland**：不用 `timeout`、不用 GNU-only 参数；超时由 Rust 侧实现。
5. **容器里的 Rust 版本不靠镜像**：课程钉 `rust-toolchain.toml`，容器只当底座（`rust:1.88` ≠ 本机 1.98 正是这个坑）。
6. crates.io 可用但镜像拉取不稳定 → 文档里给可选的 `cargo` 源镜像/`vendor` 方案，由 `doctor` 检测后提示。

## 10. 判定器自身的质量与 CI

- `kiln` 是学员一定会读的代码，所以它自己必须过同一套闸门：同样的 lint 阶梯、`missing_docs`、类型化错误、集成测试覆盖 `run/check/watch/verify/plan/caps/i18n` 的行为。
- `kiln selftest`（课程防腐烂，等价于文档测试）：
  1. 对每一步应用参考解 → 全部 must 闸门必须绿；
  2. 对本步的 kill patch / 等价变异 → 规格测试必须失败（证明测试非空测）；
  3. `spec/` 哈希清单与实际一致；
  4. `lint-caps` 全仓通过（无重复教学、依赖可达、出口对齐）；
  5. i18n 结构完整（漂移只告警）。
- 课程 CI 与本地闸门同源：CI 只调 `cargo xtask ci`，避免「本地绿 CI 红」。CI 跑容器用 service container；本地跑容器走 §9 的运行时抽象。
- **间隔重复**（自学场景的关键补丁）：118 步跨越数月，早先学的能力必然生疏。state 里为每个能力记录 `level` 与 `last_verified`，`kiln review --due`（默认 45 天）把这些能力变成 3–10 分钟的 `probe` 小题重新抽查，并更新 `last_verified`。
- 工具链钉死 + 一条「依赖刷新」维护流程，避免课程随 crate 漂移腐烂。
- **外部工具缺失当成一等公民**：本机只装了 `cargo clippy`/`fmt`/`watch`/`expand`，`nextest`/`llvm-cov`/`mutants`/`deny` 全未装。闸门必须先探测工具，缺失时报「跑 `cargo install cargo-mutants`」这类可执行指令，而不是抛退出码；`kiln doctor` 一次性列出全部缺失项与安装命令。
- **`must` 慢闸门不许需要未安装的工具**（M2 定下，plan §6 的表按此实现）：`mutants`/`bench-budget`/`semver`/`unsafe-budget`/`conformance` 都由 `kiln` 自己用编译器、课程目录里的基线文件与确定性计数判定；`cargo-mutants`/`cargo-semver-checks`/`cargo-llvm-cov`/`cargo-deny` 是生产里的做法，写进正文与 `doctor`，但不是判据。缺口工具能改变的只有一件事：`coverage` 与 `deny` 是 `should`，缺工具时 `⚠ blocked` 并给出安装指令。
  - 可复现性的三个来源：判据是**编译器与计数**（没有一项看墙钟时间或网络）；取样是**确定性**的（变异体按源序等距取前 6 个，预算基线是课程文件，后端列表是项目文件）；**工具缺失不改判**（`must` 不依赖外部工具，`should` 缺失时只警告）。
- **慢闸门的分工与耗时**（M2 实测口径）：`mutants` 每步最多 6 次规格运行（在复用同一份沙箱复制品上增量重编）；`bench-budget` 一次 harness；`conformance` 每个后端一次规格运行；`coverage` 在工具链没有 llvm-tools 时零成本跳过。它们在 `kiln run`/`check` 这类快路径上**一次也不跑**（`slow:` 前缀由 runner 跳过），回归保护靠 `verify --slow` 与 `selftest`。

### 依赖版本基线（已核对 crates.io，2026-10）

`sqlx 0.9`、`axum 0.8`、`tokio 1.53`、`tower 0.5`、`tower-http 0.7`、`tokio-util 0.7`、`figment 0.10`、`utoipa 6`、
`proptest 1.11`、`insta 1.48`、`divan 0.1.21`、`loom 0.7`、`testcontainers 0.28`、`argon2 0.6`、`subtle 2.6`、`hmac 0.13`、
`governor 0.10`、`moka 0.12`、`object_store 0.14`、`arrow`/`parquet 60`、`datafusion 55`、`openraft 0.9`、`turmoil 0.7`、`madsim 0.2`、
`redis 1.7`、`opentelemetry 0.33`、`samply 0.13`；工具：`cargo-mutants 27.1`、`cargo-deny 0.20`、`cargo-llvm-cov 0.9`、
`cargo-nextest 0.9`、`cargo-semver-checks 0.50`、`cargo-dist 0.32`、`cargo-fuzz 0.13`。
T1 已按这份表钉过一次精确版本（在 `app/mailer/Cargo.toml` 里，每个依赖的注释写明「哪一步引的、为什么」；
`Cargo.lock` 一并提交）。T2+ 新增依赖时照做：钉精确版本，注释写清引入它的那一步。

## 11. 里程碑（以内容完整度验收，不以工期压缩）

| 里程碑 | 内容 | 验收标准 |
| --- | --- | --- |
| **M0 垂直切片** | `kiln` 骨架（list/run/check/watch/hint/state/verify + i18n 回退 + images/doctor）+ 12 个闸门 + **T1 `mailer` 全部 22 步真实内容** | 新克隆一条 `kiln` 能一路做完 T1；`selftest` 对 T1 全绿；`kiln --lang ja` 优雅回退；watch 存盘重跑；容器步骤在 podman 下跑通 |
| **M1 去重机制 + T2** | 能力台账、`lint-caps`（§3 D4 六条规则）、deepen/transfer 模板与 RECALL 卡片、`kiln plan --fast`/`probe`、间隔重复；**T2 `relay` 全部 24 步** | `lint-caps` 全仓通过；T2 的**6** 个 deepen/transfer 步（1 transfer + 5 deepen，见 `track.toml` 与 curriculum §7）无 WHY 段、RECALL 点名真实存在的更早步骤；`plan --fast` 能正确跳过 T1 已证明能力；`review --due` 有真实 probe 题 |
| **M2 T3 + 慢闸门** | **T3 `jobq` 26 步**；mutants/coverage/bench/semver/unsafe 接入；conformance 套件 | 故意引入的劣化能被 mutants 与 bench-budget 抓到；`semver` 能拦住破坏性改动 |
| **M3 T4 + 容器与性能基建** | **T4 `lakelet` 24 步**；镜像预拉与运行时抽象打磨；soak/剖析流程 | 无网环境（镜像已预拉）能跑完 T4 的快路径；soak 能发现故意注入的任务泄漏 |
| **M4 T5 + 分布式基建** | **T5 `meshkv` 22 步**（可选 track）；`turmoil`/`madsim` 确定性模拟；集群故障演练 | 同一 seed 重复跑结果一致；注入网络分区后系统行为可预测且断言明确 |
| **M5 收尾** | 中文全量翻译（或首批核心步骤 + 术语表）、文档站、`kiln new-track` 指南、年度维护流程 | 用指南在 1 小时内起一条 3 步的玩具 track；`i18n status` 能报出漂移 |

进度（2026-10）：**M0 已结束**——T1 的 22 步全部写完，冻结在 `c08f33a` 的验收是
`cargo xtask ci` 486s / 最后一行 `22/22 step(s) hold the contract`、`kiln verify --json mailer`
205s / 22 passed，逐条证据在 [handoff-m0.md](handoff-m0.md) §1。里程碑表里 M0 那一行自此是「已完成」。

**M1 已交付**（2026-10）：去重机制的运行时部分已经落地——`state.toml` 为每个能力记 `level`/`last_verified`/
`evidence`，`kiln probe`（`course/probes/`，`--check` 进 CI）、`kiln review --due`、`kiln plan --fast`
与 `kiln skip`（跳过只认**通过的 probe**），`lint-caps` R5 增加「RECALL 必须点名真实存在的更早步骤」。
T2 `relay` 的 24 步**全部交付**（正文、规格、scaffold、参考解、闸门接线；`kiln verify relay` 在最终树上
`24/24 pass`，6 个 exit 能力全部 verified），另有 19 道 probe 题支撑 `review --due` 与 `plan --fast`。
交付物一览与实测数字在 [t2-relay.md](t2-relay.md)，机制、命令与继续往下做的约定在
[handoff-m1.md](handoff-m1.md)。

顺序上的硬约束：**M0 必须先出 T1 的 22 步真实内容，再补框架功能**。
反面教材是「先写完美框架、内容放最后」——判据格式只有写真题时才能定型，而 T1 正好同时也是框架的压力测试。

### 内容体量（真实成本，供你判断）

118 步 × （正文 + 规格测试 + 闸门接线 + scaffold/参考解 + 自测）≈ 每步 2–4 小时，
合计约 **250–450 小时**的课程内容工时（不含框架本身）。这是这门课真正的成本项；
如果中途想收口，收口点在 T1/T2（已是完整的「生产级入门」课程），T3–T5 是加长线。

## 12. 风险与取舍

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| 内容体量（118 步） | 半成品课程 | 垂直切片优先（M0 = T1 完整）；每条 track 自成一个可交付的课程；T5 可砍 |
| 去重规则过严 → 作者用 `reteach` 绕过 | 机制形同虚设 | `reteach` 必须写理由并在 `selftest` 输出汇总；`kiln caps` 可查所有 `reteach` |
| 5 条独立 track 的维护成本 | 半年后某条编译不过 | 各自钉工具链 + `selftest` 全覆盖 + 年度依赖刷新 |
| 容器/镜像不稳定（实测本机就是 podman + 国内镜像源 + 拉取超时） | 学员卡在环境 | 运行时抽象 + 镜像预拉 + `doctor` 前置诊断 + 早期步骤不依赖容器 |
| 翻译漂移 | 中英不一致误导 | `source_hash` 漂移检测；英文为唯一真源；术语表 |
| 单项目递进导致落队 | 学习者放弃 | 每步参考解可应用可回退；`plan --fast` 跳过已证明能力；`review --due` 防遗忘 |
| 闸门过严打击信心 | 前期挫败 | must/should 两档 + lint 阶梯从宽到严 + `should` 给改进提示 |
| 慢闸门拖慢反馈 | 学习者不用它 | 快路径 < 60 秒硬约束；slow 只在 `verify --slow` |
| 时间估计偏差 | 交付遥遥无期 | M0 先做完 T1 标定「每步真实工时」，再重估后续里程碑 |
