# M0 交接简报（开工用）

> 这份文件是**压缩后的上下文**：新的对话/session 只需要读本文件 + [plan.md](plan.md) §3–§6、§9、§11，
> [curriculum.md](curriculum.md) §6（T1 逐步表），[authoring.md](authoring.md) §3–§7，就能直接开工，不必回看规划讨论。
> 规划已经定稿并提交；除非发现设计错误，**不要再改机制设计**，直接落地。

## 1. M0 的目标

让一个新克隆的人能一条命令地从零走到 T1 结束：

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

## 5. 现有资产

- `tools/lint-caps-md.py`：文档级 `lint-caps` 原型（台账 vs 步骤表对拍，当前 **0 问题**：97 能力 / 118 步 / 134 格位）。
  M0 要把它移植成 Rust 版 `kiln lint-caps`，输入换成 `capabilities.toml` + `track.toml`，规则见 plan.md §3 D4。
- 规划文档：`plan.md`（机制与决策）、`curriculum.md`（能力台账 + 118 步）、`authoring.md`（模板与规则）。

> 2026-10 进度（M0 第二段）：T1 1.4–1.6 内容完成（正文 + 规格 + scaffold + 参考解），
> `exit-code` 与 `docs:public` 两个闸门已实现，`cargo xtask ci` 全绿，`kiln selftest` 对 1.1–1.6 全绿，
> `kiln verify mailer` 里 6 步通过、16 步未写（exit 3 = 还有内容没写，见 cmd/verify.rs）。
> 本段与规划有两处出入，都已改文档：1.5 的「状态码映射」在还没有 HTTP 的这一步落成**退出码**映射
> （1.7 起同一套分类映射到状态码）；1.6 的「每次请求一个 request id」改为「一次运行一个关联 id」
> （工作单位是 run 的 span，1.7 用同一机制包住每个请求）。另外 `kiln verify` 发现 1.1 的
> `main.rs` 行数预算（20）撑不过边界自然变大（1.6 时是 58），已放宽到 100 并写清它的含义。
> 下一步从 1.7 继续，按同样的顺序：正文 → 规格 → scaffold → 参考解 → 接闸门。

> 2026-10 进度（M0 第三段 · 停在 1.9 门口）：T1 1.7（HTTP 骨架）与 1.8（app 工厂 / 测试基建）已完
> 成并提交，`cargo xtask ci` 全绿，`kiln selftest` 对 **1.1–1.8 共 8 步**满足契约（scaffold 必败 /
> 参考解必绿 / must 闸门全绿）。1.9-postgres 已经调研并设计到位，但**尚未提交**，下一段从这里接着做。
>
> ### 1.9 已经确定的事（不要再重新调研）
>
> **容器可用，走镜像前缀。** 本机 podman 6.1.2 能跑，但 Docker Hub 直连拉取挂住；`docker.1ms.run`
> 镜像源可用（`docker.m.daocloud.io`、`dockerproxy.net` 也留有缓存）：
>
> ```sh
> podman pull docker.1ms.run/library/postgres:17-alpine     # 294 MB，本地已缓存
> podman run -d --name kiln-pg -e POSTGRES_PASSWORD=postgres -p 127.0.0.1::5432 docker.1ms.run/library/postgres:17-alpine
> ```
>
> 课程侧的口径：镜像按 `KILN_REGISTRY_MIRROR` 前缀解析（`kiln doctor` 已实现该报告），步骤声明
> `images = ["postgres:17-alpine"]`，`images` 闸门（**尚未实现**）要做的就是「本地镜像库里有没有这个
> 镜像 + 没有就给带前缀的 pull 命令」。
>
> **测试用的 Postgres 由规格自己起。** 新增的 `tests/support/postgres.rs`（scaffold 创建、由规格用
> `#[path = "support/postgres.rs"] mod postgres;` 引入）按 podman→docker 顺序探测，`-p 127.0.0.1::5432`
> 拿随机宿主端口，`pg_isready` 轮询到就绪，`psql -c` 建每测试独立数据库，`Drop` 时 `rm -f` 容器。
> 容器是 per-test 的（5 秒级），符合「测试之间不共享任何东西」。
>
> **sqlx 的两个事实（实测）。** `sqlx::migrate!("./migrations")` 不需要数据库就能编译；`sqlx::query!`
> /`query_scalar!` 宏需要「编译期能连到数据库」或 `.sqlx/query-<hash>.json` 缓存（宏找不到缓存时的报错
> 是「set `DATABASE_URL` … or run `cargo sqlx prepare`」，缓存查找顺序是 `SQLX_OFFLINE_DIR` →
> crate 的 `.sqlx/` → workspace 的 `.sqlx/`）。本机**没有** `sqlx-cli`，`cargo sqlx prepare` 需要先
> `cargo install sqlx-cli --no-default-features --features postgres,rustls`（未做，耗时较长）。
> 因此 1.9 的这一条查询用运行时检查的 `sqlx::query_scalar`，编译期检查的宏留给 1.11 的第一条真实查询，
> 届时把 `.sqlx/` 作为 scaffold 内容一起发布；这一点要写进 1.9 正文与 curriculum §6 的注记。
>
> **1.9 的设计（已定，未落盘）。** `src/db.rs`：`Database { pool }` + `connect`/`migrate`/`ready`/`pool`；
> `migrations/0001_subscribers.sql` 建 `subscribers`（email 唯一、status、confirmation_token/expires、
> confirmed_at、subscribed_at，附两条 CHECK，1.10–1.18 直接在此之上加迁移）；启动顺序 = 配置 → 日志 →
> **连库 + 迁移** → 服务；`http::AppState { config, database: Option<Database> }` + `GET /readyz`（无库
> 或库不可用 → 503，liveness `/healthz` 仍 200）；**数据库可选但一旦配置就必须可用**（未配置时启动并
> 警告，配置了却连不上就退出非 0）——这是为了让 1.6/1.7/1.8 的规格在最终树上仍然成立（它们直接启动二进
> 制、没有数据库），也为 1.22 的断连演练留出「运行中失去数据库」的语义。
>
> **规格里必须给自己设截止时间。** 1.9 的「库连不上必须退出」测试用 `Child::try_wait` + 20 秒期限 +
> `kill`，不能用 `Command::output()`：在 scaffold 状态下桩实现会照常启动并一直服务，没有期限就会挂住
> （本次就是因此耗掉了最后一段预算）。
>
> ### 还没有动的
>
> 1.9 的正文/规格/scaffold/参考解（草稿与验证脚本在上一段的临时目录里，已失效；按上面的事实重做很快）、
> 1.10–1.22 全部内容（严格按序，每一步都压在前一步的树上）、`images`/`container-repro`/`links` 三个
> 闸门、`kiln watch`、以及 §4「M0 不做的事」里仍然排除的那些。

## 6. 工作方式约定

- 提交粒度：一个步骤 = 一个提交（正文 + 规格 + scaffold + 参考解 + 接线），提交信息写清 `track/step`；框架改动单独提交。
- 每完成 5–6 步把 `curriculum.md` 里对应步骤的状态（若新增了能力或深度）与 `plan.md` §11 的验收项同步一次。
- 遇到与规划冲突的现实（crate API 变了、某步实测超 120 分钟、某闸门不可行），**改规划文档并在提交信息里说明**，不要默默偏离。
- 需要产品决策时（例如某步拆不拆、是否改 track 名）先问，不要自行扩大课程范围。
