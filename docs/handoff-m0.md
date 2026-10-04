# M0 交接简报（开工用）

> 这份文件是**压缩后的上下文**：新的对话/session 只需要读本文件 + [plan.md](plan.md) §3–§6、§9、§11，
> [curriculum.md](curriculum.md) §6（T1 逐步表），[authoring.md](authoring.md) §3–§7，就能直接开工，不必回看规划讨论。
> 规划已经定稿并提交；除非发现设计错误，**不要再改机制设计**，直接落地。
> 分段的进度、实测数字与踩过的坑在 [handoff-m0-log.md](handoff-m0-log.md)：读它是为了「别重踩」，不是必读；
> 现状看本文件 §1 的验收清单与 §5 的交付清单。

## 1. M0 的目标

让一个新克隆的人能一条命令地从零走到 T1 结束：

**状态（2026-10，M0 结束）：T1 的 22 步全部写完。** 1.21/1.22 落地时的实测与坑记在日志的第十四、十五段；
下面这份验收清单逐条对齐的结论写在紧接着的一节里（“新克隆一条命令”这条是用 `kiln selftest` + `kiln verify` 逼近的，
三个容器步仍需先 `kiln images pull`，这不是“人工干预”，是 `images` 门/夹具会直说的准备动作）。

1. `kiln` 骨架：`list` / `run` / `check` / `watch` / `hint` / `reset` / `verify --json` / `doctor` / `images` / `spec restore` / `i18n status` / `caps`。
2. 12 个闸门：`spec-integrity`、`fmt`、`clippy@N`、`no-todo`、`docs:public`、`exit-code`、`i18n`、`images`、`container-repro`、`links`、`caps`、`timeout`。
3. **T1 `mailer` 全部 22 步真实内容**（正文 + 规格测试 + scaffold + 参考解 + 闸门接线）。
4. `kiln selftest`：逐步验证「参考解全绿、规格在 scaffold 上必败、spec 哈希一致、`lint-caps` 通过」。

验收（缺一不可）：

- 新克隆 → `kiln` → 一路做到 `mailer/1.22`，中途不出现需要人工干预的报错；
- `kiln selftest` 对 T1 全绿；
- `kiln --lang ja`（或任何无译文的 locale）优雅回退英文，全流程不报错；
- `kiln watch` 存盘重跑；`kiln verify --json` 输出可机读的进度与诚实度标记；
- T1 的容器步骤（1.9 Postgres、1.19 mailpit）在本机 podman 下跑通。

### 验收实测（2026-10，冻结在 `c08f33a`，三个命令全绿）

| 验收项 | 实测 | 证据 |
| --- | --- | --- |
| 全部内容 + 契约 | `cargo xtask ci` **486s**，最后一行 `22/22 step(s) hold the contract` | fmt / clippy `-D warnings` / workspace 179+15+1 测试 / spec-integrity 22 pinned / `lint-caps` 97 caps·134 slots·118 positions·22 authored |
| 逐步规格 | `kiln verify --json mailer` **205s**：`passed 22 · failed 0 · not_authored 0 · blocked 0` | 两个容器/文档步（1.21 10/10 门、1.22 9/9 门）也在里面 |
| 链接（离线 + 外链） | `kiln links` 29 页 167 链 0 findings；`kiln links --external` **115s**：`112 answered · 7 unclear · 0 gone` | 那 7 个 `?` 都是本机当时连不上的主机（docs/hub.docker.com、en.wikipedia、research.google、web.dev、sre.google），不是 404 |
| 无译文 locale | `kiln --lang ja list` exit 0，打印 `content locale: ja — 0/22 step bodies, falling back through ja -> en` | 回退链在报告里可见，不静默 |
| 容器步 | 1.9 / 1.19 / 1.21 在本机 podman 下跑通（1.21 另有 `container-repro` 门：16.5 MB / 25 MB、7 行 `--check-config` 与宿主逐行相同） | `kiln selftest` 里每步都在跑 |

两条不算“人工干预”的准备动作，写在验收清单旁边以免被当成缺口：三个容器步先跑 `kiln images pull`（`images` 门与每个夹具都会直说）；
`KILN_REGISTRY_MIRROR` 在 Docker Hub 不可达的机器上要设（`kiln doctor` 会提醒）。

## 2. 构建顺序（先出内容，再补框架）

1. 仓库骨架：`kiln/`（clap 子命令 + manifest 解析 + state）、`course/course.toml`、`course/tracks/mailer/track.toml`（先放 3 个步骤）、`xtask/`。
2. `kiln list/run/check/reset` + state 文件；`run` 先支持 milestone（在 `app/<track>` 里跑 `cargo test --test <spec>`）。
3. `spec-integrity`（哈希清单生成子命令）+ `fmt` + `clippy@N` + `no-todo` 三个闸门；闸门缺失工具时给安装指令。
4. i18n：目录约定 + 回退链 + `i18n status`（**先做回退，译文可以不写**）。
5. 容器运行时抽象 + `images` / `doctor`（podman 优先，见 §4）。
6. `watch`（notify，沿用 rustlings 的触发与时间戳输出）。
7. `selftest`（脚手架→期望失败→参考解→期望全绿）+ `verify --json`。
8. **T1 22 步内容**：从 1.1 开始写，边写边把 format 补全（判据格式只有写真题时才定型）；
   1.1–1.6 完成后跑一次 `selftest` 与 `verify`，再继续 1.7–1.22。

硬规则（来自 authoring.md）：

- 规格测试在 scaffold 上必须失败；参考解必须过全部 must 闸门；
- scaffold patch 只新增文件 / 只在 `# kiln:scaffold:begin|end` 标记块内插入；
- scaffold 决定函数签名，规格测试只依赖这些签名；
- 快路径 `kiln run` < 60 秒；慢检查标 `slow`；
- 闸门不得依赖 GNU userland（**本机没有 `timeout`**），超时在 Rust 侧实现。

## 3. 本机已实测的环境事实（不要再假设）

| 事实 | 影响 |
| --- | --- |
| `rustc`/`cargo` 1.98.1，edition 2024 | 课程钉工具链；`rust-toolchain.toml` 是唯一真源 |
| trait 里的 `async fn` 仍**不是 dyn compatible**（实测 E0038，措辞已是 "dyn compatible"） | T2/T3 的端口设计步骤按此讲 |
| `docker` 是 **Podman 的 shim**；`docker compose` / `podman compose` **能打印版本但用不了**（`config` 交给 `/usr/local/bin/docker-compose` 后失败） | 容器层必须探测运行时；不写字面 `docker compose` |
| `podman`/`docker` 自报 **6.1.2**（不是 6.1.0）；`/usr/local/bin/docker-compose` 存在（v2.30.3）且 `config` 可用 | 只有直接 `run` 与 `docker-compose` 可用；`kiln doctor` 按功能探测并报出哪条路通 |
| 镜像缓存只有国内镜像源的 alpine/debian/rust:1.88；**没有 postgres/redis**；拉取曾 120 秒超时 | `images pull` + 镜像前缀映射（`KILN_REGISTRY_MIRROR`） |
| 系统无 `timeout`（无 coreutils） | 闸门/脚本的可移植性约束 |
| 只装了 `cargo clippy`/`fmt`/`watch`/`expand`；`nextest`/`llvm-cov`/`mutants`/`deny`/`semver-checks`/`fuzz` 未装 | 闸门探测缺失工具并给安装命令 |
| crates.io API 可用（版本基线见 plan.md §10） | 依赖选型按该表，M0 时钉一次精确版本 |

> 2026-10 复核（M0 第一段）：上面两行容器相关的事实已用 `kiln doctor` 重测。
> 差异只在于「坏在哪里」：容器运行时本身没问题（podman 6.1.2 可用），坏的是 compose 这条转发链；
> 另外 `docker-compose` 单体版确实装上了并能 `config`。规则不变：**不写字面 `docker compose`**，
> 由 `kiln/src/runtime.rs` 探测后选择可用命令。

## 4. M0 不做的事（避免范围蔓延）

- 不写 T2–T5 的任何内容（可留空目录与 `track.toml` 占位）。
- 不做慢闸门：`mutants`/`coverage`/`bench-budget`/`loom`/`soak`/`sim`（M1–M4 才接）。
- 不做能力去重的运行时部分：`plan --fast` / `probe` / `review --due` / 间隔重复（M1）；但 `caps` 闸门与 `lint-caps` 的元数据检查**要在 M0 就位**，读 `capabilities.toml` + `track.toml`。
- 不做 `new-track` 生成器、不做文档站、不做中文翻译（只留接口与回退）。
- 不做在线判题、不做防作弊。

## 5. M0 交付了什么

口径：`course/` 是唯一真源；下面的路径都能在仓库里点到文件，数量取 `kiln lint-caps` / `kiln list` 的实际输出。

- **命令**（`kiln/src/cmd/`）：`list` / `run` / `check` / `watch` / `hint` / `reset` / `solution` /
  `verify`（`--json`，带诚实度标记）/ `selftest` / `caps` / `lint-caps` / `spec restore` /
  `spec manifest` / `i18n status` / `images`（+ `pull`）/ `links`（+ `--external`）/ `doctor`。
- **闸门**（`kiln/src/gates/`）：`spec-integrity`、`caps`、`fmt`、`clippy@N`、`no-todo`、`docs:public`、
  `exit-code`、`i18n`、`images`、`container-repro`、`links` 已实现；`timeout` 刻意留成 runner 自己的
  截止时间而不是逐步门（plan.md §6）。慢路径（`mutants`/`coverage`/`bench`/`loom`/`soak`/`sim`）留给 M1+。
- **课程元数据**：`course/course.toml`；`course/capabilities.toml`（97 能力 / 134 格位）；
  `course/tracks/mailer/track.toml`（22 步，逐步接好 `gates` / `hint` / `time_hint` / `requires`）。
- **T1 内容**：正文 22 篇 `course/docs/steps/en/mailer/*.md`；规格 `course/tracks/mailer/spec/`
  （22 步 + `manifest.toml` 哈希清单）；scaffold 19 个补丁 + 骨架 `course/tracks/mailer/skeleton/`
  （1.1 用骨架、1.2 无 scaffold）；参考解 22 个 `course/tracks/mailer/solutions/*.patch`；
  1.21 的实测草稿留在 `docs/drafts/1.21-container/`。
- **工具**：`xtask`（`cargo xtask ci` 是 CI 的唯一入口）；`tools/lint-caps-md.py` 是 Rust 版
  `lint-caps` 的原型，仍在（它对拍课程文档自己的表格，所以留着）。

每一步的进度、实测数字与坑按时间顺序记在 [handoff-m0-log.md](handoff-m0-log.md)：第十四段是 1.21、
第十五段是 1.22；要重做一个已经量过的决定，先去那里搜那一步的 id。

## 6. 工作方式约定

- 提交粒度：一个步骤 = 一个提交（正文 + 规格 + scaffold + 参考解 + 接线），提交信息写清 `track/step`；框架改动单独提交。
- 每完成 5–6 步把 `curriculum.md` 里对应步骤的状态（若新增了能力或深度）与 `plan.md` §11 的验收项同步一次。
- 遇到与规划冲突的现实（crate API 变了、某步实测超 120 分钟、某闸门不可行），**改规划文档并在提交信息里说明**，不要默默偏离。
- 需要产品决策时（例如某步拆不拆、是否改 track 名）先问，不要自行扩大课程范围。
