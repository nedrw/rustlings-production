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
> **规格里必须给自己设截止时间。** 1.9 的「库连不上必须退出」测试用 `Child::try_wait` + 10 秒期限 +
> `kill`，不能用 `Command::output()`：在 scaffold 状态下桩实现会照常启动并一直服务，没有期限就会挂住
> （本次就是因此耗掉了最后一段预算）。
>
> 2026-10 进度（M0 第四段 · 做到 1.9）：T1 1.9（Postgres + 迁移）已完成并提交——正文、规格、scaffold（`sqlx`
> 依赖 + `src/db.rs` 接口 + `migrations/0001_subscribers.sql` 占位 + 容器/`psql` 测试装置 `tests/support/postgres.rs`）、
> 参考解（连接池·迁移·就绪 + `AppError` 两个变体 + `AppState::start` 的启动顺序 + `/readyz` + 工厂同序建状态）。
> `cargo xtask ci` 全绿，`kiln selftest` 对 **1.1–1.9 共 9 步**满足契约（scaffold 必败 / 参考解必绿 / must 闸门全绿，
> 1.9 是 9/9，多出来的是新实现的 `images` 闸门）。
>
> ### 本段新落地的机制
>
> **`images` 闸门与 `kiln images`。** 步骤声明 `images = ["postgres:17-alpine"]`，闸门问引擎本地有没有
> （`podman image inspect` 级别的探测），没有就给出带镜像前缀的 pull 命令；没引擎算 `blocked`，不算过。
> `kiln images` 报告全课程声明的镜像，`kiln images pull` 一次拉齐。解析规则调了两条（写进 plan.md §9.3）：
> 声明的是 Docker Hub 名字（除非自带 registry），走镜像时官方镜像加 `library/`；**本地存在性按「仓库路径」
> 判断**，所以 `docker.1ms.run/library/postgres:17-alpine` 算 `postgres:17-alpine` 就位，而 `myorg/postgres:17-alpine`
> 不算。本机 `KILN_REGISTRY_MIRROR` 根本没设、镜像却带着前缀，靠的就是第二条——否则闸门会因为名字不同而误报。
>
> **patch 解析器的规则修了。** hunk 里的一行**以它自己的第一个字符为准会被骗**：删掉的 SQL 注释到 patch
> 里长成 `--- 0001 …`，被当成文件头，于是 1.9 的参考解「hunk 声明 8 行却带 0 行」而拒应用。新规则：hunk 带
> 多少行由 `@@ -a,b +c,d @@` 的计数决定（和真 diff 一样），行的首字符只是内容。回归测试在 `patch.rs`。
>
> **1.6 的规格之前只在它自己的树上成立（已修）。** 它用 `Command::output()` 等日志进程自己退出；1.7 加上
> 监听后那个进程永不退出，于是 `kiln verify`（拿最终树判每一步）挂了 40 分钟，项目自己的 `cargo xtask ci`
> （`spec_ci` 会启动它）也挂。现在规格用「空闲端口 + 读到启动结束就停手」的写法，退出码只在确实有退出时才
> 断言；这条已写进 authoring.md §6.8（“一条规格要在它会被拿去判的每一棵树上都成立”）。1.3 的规格也因此从
> 55 秒跑完（它嵌套跑 `cargo test --workspace`，现在包含 1.6 与 1.9 的规格）。
>
> **1.9 的一处偏离规划（已改文档）。** 标题原本写「compile-time checked SQL」，但编译期校验的
> `query!`/`.sqlx` offline 需要编译期能连库或 `.sqlx/`（本机没有 `sqlx-cli`），所以 1.9 用运行时的
> `sqlx::query(..).fetch_optional(..)` 保证「没有数据库也能编译」，宏与离线元数据跟着 1.11 的第一条真实查询一起上。
> 步骤标题改为「an empty database, up to date before serving」，curriculum §4.2/§6 已同步。
>
> ### 下一步
>
> 1.11 起按序写在 1.10 的树上；**1.11 的设计已定，见下面一节，直接写代码即可**。前置已就绪：`sqlx-cli 0.9.0`
> 已装（`cargo sqlx` 在 `~/.cargo/bin`），`argon2 0.6.0` 已拉到本地缓存。`container-repro`（1.21）与 `links`（1.22）
> 两个闸门仍未实现；**`kiln watch`（M0 §1/§2.6 的目标）还没写**，现在是这条 track 之外剩下的唯一框架缺口。
> 另：本机的 `kiln-pg` 容器是早先手工试验留下的（不影响测试，测试用 `kiln-pg-<pid>-<n>` 并自行清理）。
>
> ### 2026-10 进度（M0 第五段 · 做到 1.10）
> T1 1.10（领域类型）已完成并提交：`src/domain.rs`（`Email`/`SubscriberName`/`Password` + 五个限额常量，
> scaffold 里的三个 `parse` 什么都不查、`Password` 带着 derive 出来的 `Debug`，参考解把它们变成解析器 + 手写的
> 遮蔽 `Debug`）、`Error::InvalidInput { field, expected }`（**不带值**——值可能是密码），`pub mod domain;`。
> `cargo xtask ci` 全绿，`kiln selftest` 对 **1.1–1.10 共 10 步**满足契约（1.10：8/8 must），
> `kiln verify --json mailer` = 10 passed / 0 failed / 12 not authored（exit 3）。
>
> **这一段的写法约定（下一段照抄）**：一个步骤如果需要库 API 而 scaffold 又不该改 `lib.rs`，就用
> **探针 crate**：spec 自己写一个 `Cargo.toml`（`mailer = { path = <项目> }`）、`src/lib.rs` 和 `tests/*.rs`，
> 在一个临时目录里编译并运行（同 1.5 的做法；`CARGO_TARGET_DIR` 指向项目下的独立目录，第二次起几秒）。
> 好处：spec 本身不引用还得由学员加入的模块，所以它在 scaffold 上**编得过、跑得过、只失败在断言上**；探针能直接
> `matches!`、能写 property，失败信息就是编译错误或断言行。代价：冷启动一次性多花约 20 秒。
>
> ### 1.11 已定的设计（下一段直接写代码，不要再调研）
>
> 前置已就绪：`sqlx-cli 0.9.0` 已 `cargo install`（`cargo sqlx` 可用，`~/.cargo/bin`）；`argon2 0.6.0` 从 crates.io 一
> 秒拉到（缓存已热）。步骤名/能力不变：`1.11-signup`，`db/unique-as-concurrency@apply` + `sec/password-hash@apply`。
>
> **接口。** `POST /signup`，body `{"email": …, "name": …, "password": …}`。成功 **201** + `{"status":"pending","email":"<规整后的地址>"}`（响应里永不出现密码）；
> body 不是 signup（缺字段/不是 JSON）→ **400**（用 axum 的 `JsonRejection`，handler 收 `Result<Json<SignupRequest>, JsonRejection>`）；
> 领域拒绝（`Error::InvalidInput`）→ **400**，body 用错误自己的话（它只说字段与期望，不重复值——直接复用 1.10 的约定）；
> 地址已被订阅（唯一约束）→ **409**。`http.rs` 新增一张 `Error → StatusCode` 映射表，和 1.5 的退出码表同源
> （2 那边的分类变 4xx，部署问题变 5xx）。
>
> 新错误变体：`Error::AlreadySubscribed { email: String }`（退出码 2，409）。body 格式：`Body` 的 `status` 从 `&'static str`
> 改成 `String`（400/409 要带上具体句子；1.7 的规格只断言它非空，不会碎）。
>
> **scaffold 内容。** `Cargo.toml` 标记块加 `argon2 = "0.6"`；新文件 `src/signup.rs` 只有三样：
> `SignupRequest { email: String, name: String, password: String }`（**原始字符串**，解析是学员的事）；一个
> `pub const INSERT_SUBSCRIBER: &str`（见下）；一个 `hash(password: &Password) -> Result<String, Error>`，
> scaffold 里直接返回明文（正好是那个阱：“密码存成它自己”）；新目录 `.sqlx/`（由 `cargo sqlx prepare` 生成、作为 scaffold 内容发布）。
>
> **那条被给定的语句**（`.sqlx/` 的缓存按它逐字哈希，所以正文给出、参考解逐字使用；学员重写就按 TRAP 里说的重新 prepare）：
>
> ```sql
> INSERT INTO subscribers (email, name, password_hash) VALUES ($1, $2, $3) ON CONFLICT (email) DO NOTHING RETURNING id
> ```
>
> `ON CONFLICT DO NOTHING` + `RETURNING id` + `fetch_optional` 就是本步的并发原语：约束做判定，应用从不先读后写。
> 学员写的是：`subscribe(pool, request) -> Result<Email, Error>`（解析→哈希→插入→`None` 变 `AlreadySubscribed`）与路由。
>
> **哈希。** `argon2 0.6` 的 `Argon2::default()` + `PasswordHasher`，盐每个订阅者随机（`SaltString::generate(&mut OsRng)`），
> 存 PHC 字符串（`$argon2id$…`）。1.11 **不提供** verify——校验哈希的调用方是登录（1.16），本步的规格只断言：形状是
> `$argon2id$`、不等于明文、同一密码两次注册得到不同哈希、响应里没有密码。
>
> **规格**（`spec/1.11/signup.rs`，用 1.10 的 `TestApp` + 容器装置，不需探针）：① 合法注册 201、行里 email 已规整、
> status=pending、hash 形状、响应无密码；② 同地址第二次 409 且只一行；③ **两个线程同时**投同一地址：恰好一个 201、
> 一个 409、一行（这就是能力名）；④ 域名拒绝 400 且 body 指名字段、不重复值、没插入行；⑤ 缺字段/非 JSON 400。
>
> **准备 `.sqlx/` 的命令**（我自己跑一次，产物进 scaffold）：容器起一个空库 → `sqlx migrate run --database-url …`
> → `DATABASE_URL=… cargo sqlx prepare`（在 `app/mailer` 里，带 `.sqlx/` 覆盖写入）→ 把 `.sqlx/*.json` 放进 scaffold。
> 之后 `cargo test`/`clippy` 在无库环境下必须仍然编译得过（这正是离线元数据的意义，也是本步要讲的东西）。

## 6. 工作方式约定

- 提交粒度：一个步骤 = 一个提交（正文 + 规格 + scaffold + 参考解 + 接线），提交信息写清 `track/step`；框架改动单独提交。
- 每完成 5–6 步把 `curriculum.md` 里对应步骤的状态（若新增了能力或深度）与 `plan.md` §11 的验收项同步一次。
- 遇到与规划冲突的现实（crate API 变了、某步实测超 120 分钟、某闸门不可行），**改规划文档并在提交信息里说明**，不要默默偏离。
- 需要产品决策时（例如某步拆不拆、是否改 track 名）先问，不要自行扩大课程范围。
