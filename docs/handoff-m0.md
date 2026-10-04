# M0 交接简报（一条命令走通 T1）

> M0 已结束。这份文件是**压缩后的上下文**：目标与验收、本机实测的环境事实、工作约定，以及去哪找细节。
> T1 交付物本身的说明在 [t1-mailer.md](t1-mailer.md)；分段的进度、实测数字与踩过的坑在
> [handoff-m0-log.md](handoff-m0-log.md)；机制与取舍在 [plan.md](plan.md) §3–§6、§9、§11；
> 写作/翻译规范在 [authoring.md](authoring.md) §3–§7。规划已定稿并提交；除非发现设计错误，
> **不要再改机制设计**，直接落地。

## 1. M0 的目标与验收

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
| 链接（离线 + 外链） | `kiln links` 29 页 167 链 0 findings；`kiln links --external` **115s**：`112 answered · 7 unclear · 0 gone` | 那 7 个 `?` 都是本机当时连不上的主机（docs/hub.docker.com、en.wikipedia、research.google、web.dev、sre.google），不是 404；**当时的读法把机器上的 `app/mailer` 也算进了页数**，现在 `kiln links` 只报仓库自己的页面（30 页 199 链 119 外链，且 CI 会跑它），见 [../README.md](../README.md) 的 Status 段与日志第十七段 |
| 无译文 locale | `kiln --lang ja list` exit 0，打印 `content locale: ja — 0/22 step bodies, falling back through ja -> en` | 回退链在报告里可见，不静默 |
| 容器步 | 1.9 / 1.19 / 1.21 在本机 podman 下跑通（1.21 另有 `container-repro` 门：16.5 MB / 25 MB、7 行 `--check-config` 与宿主逐行相同） | `kiln selftest` 里每步都在跑 |

两条不算“人工干预”的准备动作，写在验收清单旁边以免被当成缺口：三个容器步先跑 `kiln images pull`（`images` 门与每个夹具都会直说）；
`KILN_REGISTRY_MIRROR` 在 Docker Hub 不可达的机器上要设（`kiln doctor` 会提醒）。

## 2. 本机已实测的环境事实（不要再假设）

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

## 3. M0 没做的事（留给后面的里程碑）

- 不写 T2–T5 的内容：台账里 96 个格位有计划，`course/tracks/` 里还没有第二条 track。
- 不做慢门：`mutants` / `coverage` / `bench` / `loom` / `soak` / `sim`，以及目录里占位的 `conformance`（M1–M4）。
- 不做能力去重的运行时部分：`kiln plan --fast` / `probe` / `review --due` / 间隔重复（M1）；台账检查
  （`caps` 门与 `kiln lint-caps`）已在 M0 就位。
- 不做 `new-track` 生成器、文档站、中文翻译（回退链与漂移检测已就位，无译文）。
- 不做在线判题、不做防作弊。

## 4. 去哪找什么

| 要看的 | 在哪 |
| --- | --- |
| T1 交付物：22 步一览、门阶梯、项目最后的形状、测试装置、可照抄的模式、实测数字 | [t1-mailer.md](t1-mailer.md) |
| 命令与闸门清单、安装/运行 | [../README.md](../README.md) 的 Status 段、`kiln --help` |
| 每一步的施工过程、当时的实测数字与坑 | [handoff-m0-log.md](handoff-m0-log.md)（按段检索） |
| 机制、门目录、里程碑、风险 | [plan.md](plan.md) |
| 能力台账（97 能力 / 134 格位）与 T1–T5 的步骤表 | [curriculum.md](curriculum.md) |
| 正文模板、scaffold 规则、规格规则、检查清单 | [authoring.md](authoring.md) |
| 实测过的设计草稿（1.14 / 1.21） | [drafts/](drafts/) |

一个命令的入口是裸跑 `kiln`：它给出下一步的 id、正文路径、项目目录，以及该敲的两条命令。

## 5. 工作方式约定

- 提交粒度：一个步骤 = 一个提交（正文 + 规格 + scaffold + 参考解 + 接线），提交信息写清 `track/step`；框架改动单独提交。
- 每完成 5–6 步把 `curriculum.md` 里对应步骤的状态（若新增了能力或深度）与 `plan.md` §11 的验收项同步一次。
- 遇到与规划冲突的现实（crate API 变了、某步实测超 120 分钟、某闸门不可行），**改规划文档并在提交信息里说明**，不要默默偏离。
- 需要产品决策时（例如某步拆不拆、是否改 track 名）先问，不要自行扩大课程范围。
