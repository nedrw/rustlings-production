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
> **1.13 起按序写在 1.12 的树上；1.13 的设计已定，见本节末尾，直接写代码即可。** 从 1.11 起，一个
> 用到 SQL 的步骤开工顺序是：把这一步要用的语句写全 → 用一个跑着的空库
> `cargo sqlx prepare`（起 `docker.1ms.run/library/postgres:17-alpine`，8 秒就绪；`sqlx migrate run` 建表，
> `DATABASE_URL=… cargo sqlx prepare`）→ 把新生成的 `.sqlx/query-*.json` 作为 scaffold 内容发布，正文里给出
> 语句原文。1.13 不新增语句（它没有数据库工作）。`container-repro`（1.21）与 `links`（1.22）两个闸门仍未实现；
> **`kiln watch`（M0 §1/§2.6 的目标）还没写**，现在是这条 track 之外剩下的唯一框架缺口。
> 别用 `podman images` 的**空输出**判断镜像是否还在：本段开工时它一次返回空、第二次就列全了。

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
> ### 2026-10 进度（M0 第六段 · 做到 1.11）
>
> T1 1.11（注册）已完成并提交（`121caf8`）：`src/signup.rs`（`SignupRequest` 三个原始字符串 +
> `subscribe(pool, request)` + `hash`）、`Error::AlreadySubscribed { email }`（退出码 2，409）与
> `Error::PasswordHash { source }`（1，500）、`http.rs` 的 `Error → StatusCode` 表（与 `Error::exit_code`
> 同一条规则：调用方能修的 4xx、部署的 5xx，且 5xx 从不带原因）、`POST /signup` 的 201/400/409/503。
> `cargo xtask ci` 全绿 3m06s，`kiln selftest` 对 **1.1–1.11 共 11 步**满足契约（1.11：8/8 must），
> `kiln verify --json mailer` = 11 passed / 0 failed / 11 not authored（exit 3）。另有一轮**空 `app/` 的
> 全新走查**：1.1 → 1.11 每步「scaffold 必败 → 参考解全过 → 每个 must 闸门全绿」，1.11 是
> `0 passed; 6 failed` → `6 passed` → 全绿。
>
> **这一段实测的事实（不要再花时间验证）**
>
> - **`sqlx` 0.9 的 `query!`/`query_scalar!` 只吃字符串字面量**（`expand_query!(source = $query)` 内部
>   按 `LitStr` 或 `"a" "b"` 拼接解析）。所以设计里那个「scaffold 给 `pub const INSERT_SUBSCRIBER`」是行不
>   通的：语句写在**正文**里，参考解用字面量（可以 `\` 换行，`syn` 解出来的值与一行写法逐字相同）。
> - **`.sqlx` 的键是语句文本的 SHA-256**（`hash_string(&input.sql)`），宏与 `cargo sqlx prepare` 用同一
>   个函数；`prepare` 的做法是 `cargo check` + `SQLX_OFFLINE_DIR=<cache>` + `SQLX_TMP=<target/sqlx-tmp>`，
>   由宏把 JSON 写进临时目录再搬到 `.sqlx/`，所以它需要**能连库或已有缓存**且 `cargo check` 成功。缓存
>   目录按 `SQLX_OFFLINE_DIR` → `<manifest_dir>/.sqlx` → `<workspace_root>/.sqlx` 找；`DATABASE_URL`
>   未设时自动走离线；`prepare` 会先删掉缓存目录里的旧 `query-*.json`（所以语句改了会自动清理）。
> - **`argon2 0.6` 的 API 与 0.5 不同**：`Argon2::default().hash_password(bytes)` 自带 `getrandom` 取盐
>   （默认 features = `alloc`+`getrandom`+`password-hash`，不需要 `SaltString`/`OsRng`），返回
>   `password_hash::phc::PasswordHash`，`to_string()` 就是 PHC 串；`password_hash::Error` 实现了
>   `core::error::Error`，可以直接当 `#[source]`。`getrandom 0.4` 的函数是 `getrandom::fill(&mut [u8])`，
>   `getrandom::Error` 同样实现了 `core::error::Error`，加 `getrandom = "0.4"` 不会引入新下载。
> - **并发场景的规格写法**（1.11/1.12 都用）：`TestApp` 自动是 `Sync`，所以
>   `std::thread::scope` + `std::sync::Barrier::new(N)` 就够了，不需要 `Arc`；N 取 4 比 2 更容易把
>   「先读后写」的实现打出来。
> - **反向验证是这一步的常规动作**：把参考解改成那个阱的形状，看规格真的红（1.11：`[201, 500, 500, 500]`；
>   1.12：`[200, 200]`）。
> - 本机 `podman images` 有一次返回空、下一次才列全；镜像没了就
>   `podman pull docker.1ms.run/library/postgres:17-alpine`（8 秒）。
>
> ### 2026-10 进度（M0 第七段 · 做到 1.12）
>
> 能力 `sec/token-lifetime@apply`；hint 是「expired / replayed / tampered 三种失败都要设计」。
> 落地结果：`src/confirmation.rs`（`Token`、`TOKEN_BYTES=32`、`CONFIRMATION_WINDOW_SECS=86400.0`、
> `parse`/`issue`/`as_str`/手写 `Debug(<redacted>)`、零依赖的 `hex`），`GET /confirm?token=…`，
> 过期由库的 `now()` 写与比，一次性由 UPDATE 条件里的 `status = 'pending'` 保证，令牌不清空（清空就分不出
> 重放和伪造），三个新的 4xx 变体 + `TokenGeneration`（500/退出 1），
> `SignedUp` 改名 `SubscriberState`。scaffold 只新增文件（`src/confirmation.rs` 里 `issue` 返回 64 个 0、
> `.sqlx/` 三条新语句的元数据、`Cargo.toml` 的 `getrandom`），`time_hint` 仍是 90m，闸门列表未变。
> 1.12 的参考解**删掉了 1.11 那条 INSERT 的 `.sqlx` 条目**（语句改了，`cargo sqlx prepare` 也会删），
> 这是解法可以删文件、scaffold 不能的第一次使用，正文里写明了。
>
> **1.12 实测的规格口味（下一步照抄）**：`signed_up()` 用 `psql` 直接读 `confirmation_token`（服务永远不
> 把它给调用方，规格要当数据库的读者）；过期用一条 `UPDATE … SET confirmation_expires_at = now() -
> interval '1 minute'` 造出来，不睡觉；重放除了断 409，还断言 `confirmed_at` **没有被第二次请求挪动**。
> 反向验证同样做了：把 `confirm` 改成「先读后写」，`two_confirmations_at_once_confirm_once` 报
> `[200, 200]`；把 `issue` 留成 scaffold 的常量，第二次注册 500（`confirmation_token UNIQUE`）。
> `cargo xtask ci` 全绿 3m30s，`kiln selftest` 对 **1.1–1.12 共 12 步**满足契约（1.12：8/8 must），
> `kiln verify --json mailer` = 12 passed / 0 failed / 10 not authored（exit 3）。
>
> ### 2026-10 进度（M0 第八段 · 做到 1.13）
>
> T1 1.13（邮件端口 + 假实现）已完成并提交（`c1bd05d`）：`src/email.rs`（`Message`/`SendError`/
> 装箱 future 的 `EmailSender`/`Logging`/两个常量）、`tests/support/email.rs`（`Recording` 假实现，
> `fail_with`/`clear_failure`/`attempted`/`delivered`）、`AppState` 的 `Arc<dyn EmailSender>` 与
> `start_with_sender`/`sender()`（二进制仍走 `start`，未改一行）、`signup::subscribe(pool, sender,
> request)` 在插入成功后才发信、发失败只 `warn!` 不改 201；`tests/spec_email_port.rs` 五条。
> `cargo xtask ci` 全绿 3m45s，`kiln selftest` 对 **1.1–1.13 共 13 步**满足契约（1.13：8/8 must），
> `kiln verify --json mailer` = 13 passed / 0 failed / 9 not authored。反向验证：把发送挪到插入**之前**，
> `an_address_that_is_already_subscribed_gets_no_second_email` 报 `left: 2, right: 1`。
>
> **落地时与设计的两处不同（下一段照现状写）**：① `confirmation_message` 放在 `src/signup.rs`（不在
> `src/email.rs`）——端口模块因此不知道「订阅」这个概念，而 1.14 的 worker 要重发时正好需要它；
> ② 规格里的夹具叫 `App`（不是 `AppHarness`），并且只实现了 `post`（本步所有 HTTP 断言都是 POST /signup）。
> 另：这一步的 scaffold 没有「故意写错的函数」，它错在**缺缝**——scaffold 上规格是**编译不过**的
> （`AppState::start_with_sender` 不存在），`kiln selftest` 把编译错误计为「scaffold 失败」，这符合
> authoring §6.2；正文的 TASK 因此把那条签名写清楚了。
>
> ### 2026-10 进度（M0 第十二段 · 做到 1.20）
>
> T1 1.19（真实邮件服务器容器，`a141cd8`）、1.20（metrics + SLO 草稿，`29d522f`）已完成。1.19 把 SMTP 适配器
> 变成可配置的（`email.smtp_host`/`email.smtp_port`），**没配置时仍然是 `Logging`**（前面所有子进程规格都靠这一
> 点，1.14 更是明文断言 `an email would be sent`）；`tests/support/mailpit.rs` 是一容器一测试的邮件服务器夹具；
> 它的第三条规格是**第一次把 1.14 的 drain 截止时间从进程外判下来**（SIGTERM → 10s → 退 1）。1.20 加了
> `/metrics`（手写 Prometheus 文本格式）、按**匹配到的路由**计数的中间件、以及 `deliveries_total{outcome}` 的
> 三种结局；规格里有一条 `# HELP` 家族快照（重命名即红）。
>
> **这一段的四个环境/工具教训（很重要，别重踩）**：
>
> 1. **容器夹具必须带 `-v` 删卷**（`7ed238b`）：postgres 镜像把数据目录声明成 VOLUME，每个测试容器都会留一个匿名
>    卷，而 `podman rm -f` 不删卷。本 session 累积到 2022 个卷后引擎报 `exceeded num_locks (2048)`，**所有**容器
>    规格一起崩，症状离原因极远（当时看起来像两个 `perf(kiln)` 提交导致的回归）。两个夹具的 `Drop` 现在都是
>    `rm -f -v`，注释写明了原因。**如果以后又出现“所有容器测试同时失败”，先看
>    `podman volume ls | wc -l`，再 `podman volume prune -f`。**
> 2. **课程 patch 不能追加到骨架 `Cargo.toml` 末尾**（`aa4f6e6` 回退）：那样会让 `1.2` 的 solution 在
>    `git apply` 下失败（kiln 自己的 applier 容忍，git 不容忍）。骨架的每一行都是后面 patch 的上下文。
> 3. **共享构建目录是真正的性能开关**（`62436a6`）：`kiln process` 现在接受一个进程级 `CARGO_TARGET_DIR`，
>    `selftest` 指向 `$XDG_CACHE_HOME/kiln/target`（或 `$HOME/.cache/...`）。单步 selftest 从 1m30s → **25-57s**，
>    `cargo xtask ci` 从 7m30s（基准）→ **6m46s**，而 `kiln verify` 里每步的同一批门现在只算一次（`perf(kiln)` 的
>    树指纹缓存）。`verify` 剩下的时间（~2m30s）几乎全是 spec 自己：每个测试各起一个容器（这是刻意的隔离，
>    也是 1.19 教的 order independence），不要再为了快把它去掉。
> 4. **两个工作流建议已落地**：长验证交给子 agent（scout，冻结的树 + 只回报结论行/耗时/失败诊断前几行）；
>    在验证跑着的时候不动文件。这一轮三次验证都是这么做的，上下文省很多。
>
> **一处未复现的红（留档，别当成已修）**：`2d0f004` 上的 `kiln verify --json mailer` 报 `mailer/1.3-ci` 失败
> （它的 `spec_ci` 测试会在 `app/mailer` 里重跑项目自己的 `cargo xtask ci`）。**同一棵树、同一条命令手工跑是绿的**：
> `cd app/mailer && KILN_CI_NESTED=1 cargo xtask ci` → “xtask ci: every check passed”；
> `cargo test -q -p mailer --test spec_ci` → 2 passed in 70s。同一次 `cargo xtask ci`（selftest 的沙箱副本）里 1.3
> 也是绿的（scaffold 失败 / solution 通过 / 5/5 门）✓。所以红只出现在 `verify` 那条路径上，且当时刚跑完 6 分钟的
> `ci`（机器有负载）。`verify` 全程只用了 2m29s，说明 `spec_ci` 是**很快**失败的（不是超时），但报错行被 kiln 截成
> 一行（`exit code 101: error: test failed …`），看不到内部是哪一步。
>
> **已经量过了**：`4810e6a` 上连空跑三遍 `kiln verify --json mailer` → 三次都是 **passed 20 / failed 0 /
> not-authored 2**（exit 3 = 内容未写完），1.3 三次全绿，五个门全 passed。所以它不是确定性的失败。
> **但这不等于排除负载**：这三遍跑在已经被预热过的 `app/mailer/target` 上，内部那次 `cargo xtask ci` 每次都是
> 增量构建；原来出问题时的条件（冷构建 + 刚跑完 6 分钟 ci 的机器）没有被复现。结论：**最可能是负载/冷构建相关，
> 不是 app 状态相关、也不是确定性失败** —— 下一次再遇到就按这条走：
> 1. 保留 `kiln verify --json mailer` 的完整 JSON（对 1.3 的 `gates` 数组和失败行，别只看控制台那一行）；
> 2. 顺手看一眼 `podman volume ls | wc -l`（容器类 spec 的并发量是内部 `cargo test --workspace` 的产物）；
> 3. 冷启动复现：`rm -rf app/mailer/target` 后带负载重跑那条路径，而不是只重跑 `verify`。
> **不要用重跑把它盖过去**——这正是 1.14 段记录过的同类问题（那时也是靠“带负载才出现”才难抓）。

> **T1 还剩三步 + 一件框架事（下一段直接接）**：
>
> - **1.21-container**（120m，`release/container-image@apply`，spec `spec/image.rs` → `tests/spec_image.rs`，
>   `run.cmd = cargo test -p mailer --test spec_image`）：多阶段 Dockerfile + 探针 + “先迁移后收流量”；它的 gates
>   里已经有 **`container-repro`**，而这个门在 `kiln/src/gates/registry.rs` 里仍是
>   `GateStatus::Pending("arrives with T1 step 21's content")` ✗ **必须先实现这个门**（建议：构建镜像、跑起来、
>   打 `/healthz`、用同一面 binary 的 `--check-config` 对一下；名字叫“repro”就应该验证“镜像跑的是同一面
>   binary”）。镜像基础建议用 `docker.1ms.run/library/rust:1.98-slim` + `debian:bookworm-slim`（本机
>   `docker.m.daocloud.io/library/debian:bookworm-slim` 已经在库里），并把它们加进
>   `images = [...]` 与 1.21 的 `images` 门。注意 spec 的超时是 300s，镜像构建很可能超——建议 spec 只做
>   “构建一次 + 跑一次 + 探针 + 二进制比对”，并把依赖层缓存写进 Dockerfile。
> - **1.22-runbook**（120m，design 步骤，`ops/runbook-drill@apply`）：两条线（演练规格 + 人读的 runbook），
>   它的 gates 里有 **`links`**，同样是 Pending ✗ **必须先实现**（需要 `lychee` 或一个手写的链接检查器：
>   只查课程文档里的 URL，离线可跳过）。
> - **`kiln watch` 已做**（`kiln/src/cmd/watch.rs`）：按内容指纹轮询项目（`target/` 不算，同字节重写也不算），
>   首轮立即跑，之后每变一次重跑一次 `kiln run <step>`；实测“runs: 2 · reruns: 1”。M0 验收里的
>   “reruns on save”因此已满足。
>
> ### 2026-10 进度（M0 第十一段 · 做到 1.18）
>
> T1 1.16（会话/认证，`16b5404`）、1.17（newsletter 事务 + fan-out，`a651074`）、1.18（授权边界，
> `69cf287`）已完成。1.18 是**第一个没有 scaffold 的步骤**（§5.2）：route + 受会话限定范围的查询 + 一个错误
> 变体，1.16 已经把每种形状都演示过了，`track.toml` 里写明了为什么删掉 `scaffold` 键。到 1.18 为止：
> `cargo xtask ci` 全绿 7m11s（18/18 步满契约），`kiln verify --json mailer` = 18 passed / 0 failed /
> 4 not authored。
>
> **这一段新落地的机制（后面几步直接用）**：
>
> 1. **`tests/support/service.rs`（1.16 的 scaffold 给的）**：一个“真进程 + 能发 header 的客户端”夹具
>    （`Service::start(env)`、`call/get/get_with/post`、`log_lines`、`stop/wait_for_exit`）。1.13 的进程内夹具
>    只能发 body，cookie 与 `Authorization` 这类东西它发不了——认证类规格必须用这个。**日志断言要在 env 里
>    加 `("MAILER__LOG__FORMAT", "json")`**（默认是 pretty，1.7 的规格也是这么做的）。
> 2. **会话凭据**：`sha2`（新增依赖，scaffold 往标记块里插的）对 token 做 SHA-256——“高熵秘密用快哈希，
>    可猜的秘密用慢哈希”是这一步的课；cookie 属性固定为 `HttpOnly; SameSite=Lax; Path=/; Max-Age=…`，
>    **没有 `Secure`**（本机无 TLS），正文 TRAP 第 7 条写明这是决定而非疏漏。
> 3. **1.17 的表结构**：`issues`（两个 CHECK：非空 subject/body）+ `issue_deliveries`，后者带
>    `UNIQUE (issue_id, subscriber_id)`（投递身份是“哪一期的哪一份”），claim/mark_sent/release 与 1.15 同形。
>    事务 + **commit 之后**才 fan-out；audience 是 `INSERT … SELECT … WHERE status = 'active'`（不是 Rust 循环）。
>    队列满时那些“没排上去”的副本就留在表里——真正的 relay 属于 T3/T4，正文明写了这个缺口。
> 4. **`Job` 现在是 enum**（`Job::confirmation(subscriber)` / `Job::issue(delivery)`），worker 按种类分派；
>    两种循环长得像但不抽象化（正文解释了为什么）。任何依赖 `Job` 形状的规格都会因此打破——目前没有。
>
> **1.19（真实邮件服务器容器）动手前必须解决的三个限制**（这三条是被前面步骤“记了账”的）：
>
> - 二进制一直用 `Logging` 适配器，所以 **1.14 的 drain 截止时间、1.15/1.17 的失败路径在子进程规格里都
>   无法观测**（1.14 提交信息与 1.15 正文都写了这件事）。1.19 要把适配器**可配置**（SMTP），并且
>   **在没配置时仍然默认 `Logging`**（1.7 起的子进程规格都隐含依赖这一点，1.14 更是明文断言了
>   `an email would be sent`）——这样新规格可以指向容器里的邮件服务器，旧规格一字不改。
> - 镜像：`axllent/mailpit`。Docker Hub 直连在本机拉不动，用 `docker.1ms.run/axllent/mailpit` 缓存；
>   `images` 门与 `tests/support/` 里的镜像判定按“仓库路径”而不是字符串相等（1.9 定下的规则）。
> - SMTP 客户端：`lettre`（新增依赖，插在标记块里；不带 TLS 的 feature 组合，mailpit 的 1025 是明文）。
>
> ### 2026-10 进度（M0 第十段 · 做到 1.15）
>
> T1 1.15（幂等投递）已完成并提交（`32c5489`）：迁移 `0002_confirmation_delivery.sql`（`subscribers` 加
> `confirmation_claimed_at`/`confirmation_sent_at`/`confirmation_attempts`）、新模块 `src/deliveries.rs`
> （claim/mark_sent/release/outstanding 四个状态机函数）、`Job` 改成携带订阅者 id（不再携带 `Message`）、
> worker 的重试改成“过一次数据库”（claim → 发一次 → 记账或释放）、`AppState` 的 `Option<Queue>`（没数据库
> 就没有投递记录，也就没有队列）、重复注册会重新“报一次”仍欠的投递（答案仍是 409）、`main.rs` 排空可选队列。
> 规格四条：一次注册一封邮件且行里有记录；同地址再注册（两种写法）仍只有一封；持续失败时 attempts 到
> `MAX_ATTEMPTS`、释放占用、之后再注册能送出且只送一次；八个并发注册 → 一个 201、七个 409、一封邮件。
> `cargo xtask ci` 全绿 5m42s（15/15 步满契约），`kiln verify --json mailer` = 15 passed / 0 failed /
> 7 not authored。
>
> **这一步定下/发现的四件事**：
>
> 1. **`.sqlx` 离线元数据的做法（后续改 SQL 的步骤照抄）**：起一个 Postgres 容器 →
>    `DATABASE_URL=postgres://postgres:postgres@127.0.0.1:<port>/postgres cargo sqlx migrate run --source migrations`
>    → `cargo sqlx prepare --workspace`（在 `app/mailer` 里跑）。Docker Hub 拉不动，用
>    `docker.1ms.run/library/postgres:17-alpine`。**scaffold 自带它自己那几个查询的 `.sqlx` 文件**（否则学员
>    declare 模块后离线编译不过），solution 用新的哈希文件替换旧的（旧的要删）。
> 2. **课程 patch 不能有 rename 段**：`git diff` 把 `.sqlx` 改名写成 `rename from/to`，而 `kiln` 的 applier
>    只会删/增/改，会把它当“修改旧路径”应用——症状是三步以外的“run `cargo sqlx prepare`”。已修（`5f3fe9d`）：
>    applier 现在直接报错并告诉你用 `git diff --no-renames`。**以后生成 patch 一律加 `--no-renames`**。
> 3. **1.9 的规格改成可扩展的**（同一提交）：它原来断言“迁移恰好一条、列恰好九个”，任何后续步骤都不可能
>    满足。现在断言 1.9 承诺的那些列**存在**（子集），以及第二次启动不改变已应用迁移的集合。1.17 再加迁移
>    （0003）时不会再碰它。
> 4. **`confirmation_attempts` 的口径**：第一版把“claim 次数”当 attempts，而 worker 在**一次** claim 里重试
>    发送 → 列里读完是 1（规格当场报“10s 内没能给出放弃的证据”）。现在是“一次 claim 一次尝试”，重试循环穿过
>    数据库；这样计数才是“这条消息被真正递给邮件服务器的次数”，重启后不会归零、多个 job 也会累加。
>
> **留给后面的两处已知缺口**：① 进程在 claim 与 send 之间死掉，会留下一个永远“被占用”的订阅者——需要
> reaper（`claimed_at < now() - interval …`）或 outbox，属于 T3/T4；正文第 9 条 TRAP 已写明。② 目前只有一种
> 消息，所以状态用 `subscribers` 的列表达；**1.17 的 newsletter 是第二种消息**（“某个订阅者的某一期”才是投递
> 身份），那时该上 `deliveries` 表 + `UNIQUE (subscriber_id, kind)`——迁移文件里的注释已经把这层关系写下了。
> 另：1.18 的授权要用 1.16 的会话；1.19 必须保留“未配置邮件服务器时用 `Logging`”（见第九段第 3 条）。
>
> ### 2026-10 进度（M0 第九段 · 做到 1.14）
>
> T1 1.14（后台投递队列）已完成并提交（`4acf2f1`）：`src/delivery.rs`（有界 `tokio::sync::mpsc` +
> one worker、`try_reserve`/`Permit` 当「座位」、`watch` 停机信号、`tokio::time::timeout` 包裹的
> `drain`、full jitter 用 `getrandom`、五条 `start_paused` 单测）；`AppState` 持有队列（`sender()`
> 删掉——发送端现在只住在 worker 里）、handler 在 INSERT **之前**拿座位、`serve_with_shutdown` /
> `serve_on_with_shutdown`（`axum::serve(...).with_graceful_shutdown`）、`main.rs` 等 SIGTERM/ctrl_c
> → 排空 → 退 0/1、`QueueFull` 在状态表里是 503、`tokio` 加 `test-util`。规格三条：答案不等发送
> （挂死的 sender 下仍 201，且队满 503 且**没写行**）、失败一次的发送由重试送到、SIGTERM 排空 + 日志
> + 退出码 0。`cargo xtask ci` 全绿 4m06s（14/14 步满足契约），`kiln verify --json mailer` = 14
> passed / 0 failed / 8 not authored，`spec_worker` 4 条 11.7s。反向验证：`drop(seat)` →「34 个注册全被
> 接受、一个都没被拒」；`MAX_ATTEMPTS=1` →「10s 内 ada 的邮件没到」。
>
> **这一步定下的、后面几步要照做的三件事**：
>
> 1. **队列容量是常量 `QUEUE_CAPACITY = 32`**（原本设计里想放 `config.rs`，本段缩范围改成常量）。
>    32 而不是 64：规格靠「一个接一个的注册」把队填满，而每个注册都要在 debug 下算一次 argon2
>    （约 230ms），64 会让这一步自己的 spec 跑 18s。把它变成设置属于 1.19/1.21 的部署工作。
> 2. **规格不能依赖 `Queue` 自己的 API**：1.15 的幂等投递几乎一定要让 job 带上订阅者身份（重发、
>    去重、状态机），`Queue::start/reserve/drain` 的形状会变。所以这一步的规格只从 HTTP 与日志看
>    行为（`QUEUE_CAPACITY`/`MAX_ATTEMPTS` 两个常量除外）——`verify` 会在 1.22 的树上重跑它。
> 3. **1.19 必须保留「没配置邮件服务器时用 `Logging` 适配器」这个默认**：1.14 的子进程规格断言
>    SIGTERM 前有 `an email would be sent`，而 1.7 起所有「二进制直接跑起来」的规格也隐含这一点。
>
> **一处「没抓到」的记录（写进提交信息了）**：把 `main.rs` 里显式 `drain` 删掉，SIGTERM 规格**仍然
> 通过**——丢掉队列会关掉 channel，tokio 的 runtime drop 又会让 worker 把手上那条做完。也就是说
> **drain 的截止时间在「适配器永不阻塞」的二进制里无法观测**，所以正文把它归给单测（TRAP 第 7 条明说）。
> 若要真正判它，得让 1.19 的适配器可注入或可配置成慢速——那时可以把这条规格补强。
>
> ### 1.14 设计（已按此实现，留档）
>
> **调研结论（外部事实，别再花时间查）**
>
> - **`mpsc::Sender::send` 在 `select!` 里被取消会丢消息**（tokio 1.53 文档原话：取消 `send` 会丢掉队列里的
>   位置，要拿 `Permit` 再发才是 cancel-safe）。生产写法因此是 **`reserve()` / `try_reserve()` +
>   `permit.send(msg)`**：容量先拿到手，消息永不丢——这正好也是「先占坑再干活」的写法。
> - **队列原语选 tokio 自带的 `mpsc`（有界）**，不引 kanal/flume/crossbeam：第三方在微基准上快 2–10 倍，
>   但本步负载是每秒几十到几百封、瓶颈在邮件服务器与 argon2，吞吐差异没有意义；而 tokio mpsc 与
>   `select!`/`axum::serve`/`tokio::time` 共享同一套取消与时钟语义，零新依赖，停机链路不用自己发明。
> - **停机用 axum 自带的 `Serve::with_graceful_shutdown(fut)`**（停止 accept、等在途请求结束），而不是
>   自己写 accept 循环。生态里 `tokio_util::CancellationToken` + `TaskTracker` 是通用替代；**本课不引
>   tokio-util**，用 `watch`/`JoinHandle` 表达同样的两件事（一个信号 + 一组能 join 的任务）。
> - **重试用 full jitter**：`sleep = random(0, min(cap, base·2^attempt))`（AWS《Exponential Backoff And
>   Jitter》三种里最简单、界最清楚、可断言的那种；decorrelated jitter 在他们模拟里总耗时略优，放进
>   FURTHER）。抖动值用已经有了的 `getrandom`，**不要拿时钟当种子**。
> - **队列不是真相来源**：内存队列在进程被 kill 时丢掉它持有的东西。正文必须写明这条边界——`subscribers`
>   这张表才是「谁还没确认」的真相，1.15 的幂等键、1.17 的事务、以及将来 T3/T4 的 outbox + 认领才是
>   「一定送到」；1.14 只教有界内存队列本身。
>
> **接口与顺序（决定）**
>
> - 新文件 `src/delivery.rs`（scaffold 给形状）：`pub const MAX_ATTEMPTS: u32 = 3`、
>   `pub const DRAIN_DEADLINE: Duration = 10s`、`pub struct Job`（装 `Message` 就够：重发不需要查库）、
>   `pub struct Queue`（`mpsc::Sender<Job>` + `Sender::try_reserve` 的包装）、一个纯函数
>   `pub fn backoff(attempt: u32, base: Duration, cap: Duration) -> Duration`（可单测：上界与单调封顶）。
> - **请求里的顺序就是课程**：`let permit = queue.try_reserve()?` → 解析/哈希/INSERT → `permit.send(msg)`。
>   队满 → 新变体 `Error::QueueFull` → **503**（按 1.5 的表：部署方的问题 → 退出码 1），而且它发生在
>   **写行之前**——客户端重试不会撞上 409 死胡同；`Permit` 还顺手解决了 `send` 的取消安全问题。
> - **停机链路**：`main.rs` 用 `signal()`（`ctrl_c` + `#[cfg(unix)]` SIGTERM）→ `serve(...)
>   .with_graceful_shutdown(...)` → 之后 `queue.drain(DRAIN_DEADLINE)`。干净排空退 **0**（课程验收就是这么
>   写的），超时 `error!` 记下没发出去的条数并退 **1**。
> - **配置**：新增 `delivery.queue_capacity`（默认 64）与 `delivery.retry_base_ms`（默认 250），按 1.4 的
>   defaults/file/env/flags/report 机制加（**学员手改 `config.rs`，没有标记块**；这也是第一次让学员自己
>   加设置，19 的 `public.base_url` 照抄）。`MAX_ATTEMPTS`/`DRAIN_DEADLINE` 先用常量。
>
> **已知问题（两次观测，未修）**：`cargo xtask ci` 里的 `cargo test --workspace` 偶尔在
> `-p kiln --test gates` 上红一次（慢/负载高时），单独重跑那条测试 14/14 全绿、整套 ci 重跑也绿。两次都
> 只在机器同时跑着学员项目的编译/容器时出现，怀疑是几条 gate 测试各自起 `cargo`（编译临时工程）互相
> 抢锁/抢 CPU 造成；测超时值本身不紧（300s/60s，`kiln/tests/gates.rs`），所以不是单纯调大超时能修。
> **已修一部分（`29cbf7f`）**：闸门报告只截 cargo stderr 的前 12 行，而冷/争用时的前 12 行全是 cargo 在
> 讲自己——`Blocking waiting for file lock on package cache` + 一长串 `Compiling …`：学员拿到「没有失败的
> 失败报告」，而断言报告里有 lint 的闸门测试也会因为与 lint 无关的原因红——这正是「负载下偶发」的形状。
> 现在 `process::is_progress` 成了 `pub(crate)`、被 `clip` 统一用于过滤，并且学会忽略缩进（cargo 的旁白是
> 缩进的，而报告保留缩进，所以这个谓词以前**只匹配过第 0 列**的行——过滤器看着在工作，实际什么都没滤），
> `clip` 在没有诊断时回退到原始行首，报告永不空白；回归测试就是那个 fixture（11 行旁白 + 一行
> `error[E0308]`，截 12 行必须包含错误）。**但要说清**：带负载连跑 6 遍没有复现原来那次偶发，所以这不是
> 「偶发已经消失」，只是把一类会伪装成它的真实缺陷去掉了。再遇到就按上面的复现办法抓具体那条测试。
>
> **1.14 的施工清单（本段已经推演出细节，照这个写，别重新设计）**
>
> 1. `src/delivery.rs`（新）：`Job` 直接用 `Message`；`MAX_ATTEMPTS=3`、`RETRY_CAP=30s`、
>    `DRAIN_DEADLINE=10s`；`pub fn backoff(attempt, base, cap) -> Duration`（full jitter，`getrandom::fill`
>    取 8 字节转 `u64`/`u64::MAX`，**不要用索引切片**——`--lib --bins` 禁 `indexing_slicing`，也不要
>    `unwrap/expect/panic`）；`Queue { sender: mpsc::Sender<Message>, worker: Arc<Mutex<Option<JoinHandle<
>    Result<usize, Error>>>>> , retry_base }`（`Clone` 靠 `Arc`），`Queue::start(sender, capacity,
>    retry_base)` 里 `tokio::spawn` worker，worker 循环 `rx.recv()` → 逐个 `deliver()`（失败按 `backoff`
>    重试到 `MAX_ATTEMPTS`，最后 `error!`），返回成功投递条数。
> 2. **`subscribe` 的签名改成 `(pool: &PgPool, seat: Permit<'_, Message>, request)`**：哨位由 handler 在
>    **插入之前** `state.delivery().reserve()?` 拿到（队满 → `Error::QueueFull` → 503，且发生在写行之前），
>    插入成功就 `seat.send(confirmation_message(email.clone(), &token))`（不 await，容量已保证），失败/已订阅
>    分支直接 drop 哨位（容量自动还回去）。类型本身就强制了「先占坑再干活」。
> 3. `http.rs`：`serve`/`serve_on` 各加一个带 shutdown future 的孪生函数
>    (`serve_with_shutdown`/`serve_on_with_shutdown`) 用 `axum::serve(...).with_graceful_shutdown(...)`，
>    原函数委托它们并传 `std::future::pending()`——**这样 `tests/support/app.rs` 一个字都不用改**。
> 4. `main.rs`：`shutdown()` = `#[cfg(unix)]` SIGTERM + `ctrl_c`（`signal(...)?` 走 anyhow，bin 里禁
>    `expect`）；`serve_with_shutdown(state.clone(), &id, async { … }).await?` 之后 `drop(state)` 并
>    `delivery.drain(DRAIN_DEADLINE)`：干净排空 `info!(delivered)` 退 0，超时 `Error::DrainTimeout` →
>    `error!` + 退 1（1.5 的表：部署的问题）。
> 5. `config.rs`：`delivery.queue_capacity`（默认 64）与 `delivery.retry_base_ms`（默认 250），照 1.4 的
>    defaults/file/env/flags/report/validate 五处各加一行；`Error::QueueFull` 与 `Error::DrainTimeout` 进退出码
>    表的 `1` 一行。
> 6. 规格：进程内那条（用 1.13 的 `support/app.rs` 夹具 + 规格自己写的 `Hanging` 发送器，容量设 1）——
>    **断言写成「至少一次 503」+「行数 == 201 的条数」**（worker 何时取走第一条是竞态，别断确切分布）；
>    子进程那条（`spec_http.rs` 套路）发 SIGTERM → 退出码 0、日志里有 `email would be sent` 与
>    `the delivery queue drained`；时间相关的放 `#[tokio::test(start_paused = true)]` 单测（尝试次数、退避
>    上界、抖动非常量、drain 超时）。
> 7. **1.13 的规格已经先改好了**（`e0b3545`：`wait_for` 轮询 + 「至少一次尝试」/「grace 收到」这类跨树断言），
>    所以异步化之后 1.13 的规格在最终树上仍然成立——这一条本来是本步最大的坑，已经拆掉。
>
> **落地情况（M0 第九段已实现，上面三条「后面几步要照做」在 §5 的进度小节里）**：`src/delivery.rs` 与
> 单测、`Error::QueueFull`/`DrainTimeout`/`WorkerLost`、`subscribe(pool, seat, request)`、`AppState` 的
> 队列与 `serve_*_with_shutdown`、`main.rs` 的信号与排空、`spec/1.14/worker.rs` 三条，都已落地；
> 与下面的施工清单的两处差异：容量与重试基数**是常量**（不是在 `config.rs` 加设置），且规格的进程内
> 部分用「一个挂死的 sender」而不是「容量设 1 的设置」。
>
> **规格怎么做（决定，含一个要改旧文件的地方）**
>
> - 时间相关的断言放**单测**（`src/delivery.rs` 内，`#[tokio::test(start_paused = true)]` + 假发送器）：
>   尝试次数 = `MAX_ATTEMPTS`、退避有界、抖动不是常数、超过 `DRAIN_DEADLINE` 时 drain 返回「没排空」。这样
>   不为测试引入设置，也不睡觉。
> - **「队满 → 503 且没写行」必须在进程内做**（子进程里没有假发送器）：把 1.13 规格里那个 `App` 夹具**搬到
>   `tests/support/app.rs`**（scaffold 可以新增文件），1.13 与 1.14 的规格都 `#[path = "support/app.rs"]
>   mod app;`——代价是 1.13 的规格被改动 ⇒ 要重新 `kiln spec manifest` 并对 1.13 重跑 selftest（允许，
>   课程内容是自己维护的）。夹具要加一个「挂在第一个 job 上不返回」的发送器（`Hanging`），配合
>   `MAILER__DELIVERY__QUEUE_CAPACITY=2` 就能用 4 个请求把队填满。
> - **SIGTERM → 排空 → 退出码 0** 用子进程做（`spec_http.rs` 的套路：起二进制、发信号、等退出码、读 JSON
>   日志）。
>
> ### 1.13 的原始备注（留档）
>
>
> 1.14 是 120m 的异步步（`async/graceful-shutdown@apply` + `async/backpressure@aware`，概念：有界队列、
> 停机排空、带抖动的重试），需要 `cfg/layered-config@apply` 与 `test/fake-external@apply`（都已具备）。
> 动手前先把这两处读一遍，因为它们决定 scaffold 能不能只新增文件：
>
> - **`src/main.rs` 现在的停机路径**：1.7 起 `serve_on` 用 `tokio::select!` 收一个 oneshot，二进制里
>   `tokio` 的 `signal` feature 已经开着，但 SIGTERM→排空的完整链路要按现状确认；停机截止时间、队列
>   上限这类设置要进 `config.rs`（**没有标记块，学员手改**，跟 1.4 的 defaults/file/env/flags/report
>   机制一起；1.19 之前 `public.base_url` 也该在这时补上）。
> - **`async/backpressure@aware` 的闸门怎么判**：`gates` 里目前没有能观察「队列满时请求怎么答」的闸门，
>   这一步的规格（`spec/1.14/worker.rs`）要么用假发送器把队列吃满（把 `Recording` 改造成「每次 send 都
>   等一个信号」的慢发送器），要么断言 HTTP 答复（429/503）；后者更接近课程的一贯口味。
>
> ### 1.13 的设计原文（已按此实现，留档 · 与上面那条备注同源）
>
> 能力 `test/fake-external@apply`；60m → 建议改 **90m**（见下）；requires 已满足（1.5 的错误链、1.12 的
> 令牌）。**这一步的关键约束是注入缝**：`tests/support/mod.rs`（1.8 的 app 工厂）没有 scaffold 标记块，
> 新 scaffold 只能新增文件，所以 1.13 的规格**自己写一个能注入发送器的 app 夹具**，不碰 `TestApp`。
>
> **端口**：新文件 `src/email.rs`（scaffold 内容，`Cargo.toml` 不需新增依赖）：
> `pub struct Message { pub to: Email, pub subject: String, pub body: String }`（收件人用 1.10 的领域类型，
> 不是 `String`）；`pub struct SendError(Box<dyn Error + Send + Sync>)`（1.5 的规矩：保留 cause，实现
> `Display`/`Error`）；`pub trait EmailSender: Send + Sync + 'static`，方法签名
> `fn send(&self, message: Message) -> Pin<Box<dyn Future<Output = Result<(), SendError>> + Send + '_>>`
> ——**必须装箱**，因为 `AppState` 要拿 `Arc<dyn EmailSender>`，而 AFIT 在 1.98 还不能 dyn 分发；正文里
> 写明这一次分配是 `dyn` 的代价。第一个适配器 `pub struct Logging`（`send` 记一条
> `info!(to = %message.to, subject = %message.subject, "an email would be sent")` 并返回 Ok）：1.13 没有
> SMTP，真实的适配器是 1.19 的 mailpit。链接由 `pub fn confirmation_message(to: Email, token: &Token) ->
> Message` 造，基址是 `pub const CONFIRMATION_BASE_URL`（**本步先不做成配置项**：`config.rs` 没有标记块，
> 加设置要学员手改 1.4 那套 defaults/file/env/flags/report 机制，把它留给真正需要它的 1.19/1.21；正文
> TRAP 里说明这一点）。规格只断言 body 里有 `/confirm?token=<行里的令牌>`，不钉基址。
>
> **注入缝**：`AppState` 增加 `sender: Arc<dyn EmailSender>` 与
> `pub async fn start_with_sender(config, id, sender: Arc<dyn EmailSender>) -> Result<Self, Error>`
> （`start` 内部用 `Logging`），二进制继续用 `start`。这条缝是**公开的**，因为测试装置在库看不见的地方，
> 正文里写明理由（「注入是构造参数，不是 mock 框架」）。
>
> **用法**：1.11 的 `subscribe` 在插入成功、拿到 token 之后发一封信（`confirmation_message(email, &token)`）
> ——`subscribe` 需要拿到 token，所以它的返回值要从 `Email` 变成 `(Email, Token)`？**不要**：让
> `subscribe` 自己发（`pool` 之外再收 `&dyn EmailSender` 或 `&Arc<dyn EmailSender>`），返回值不变。
> **发送失败不改变答复**：行已经写好、订阅者已经在，信是投递问题不是注册问题 → 仍然 201，`warn!` 记一条
> （1.14 的 worker 与 1.15 的幂等重投是它的续集；这也是 1.14「把发送挪出请求路径」的引子）。已被订阅的地址
> **不发信**（409 之前不能发）。`Logging` 的失败注入不需要：假的 `Recording` 才需要。
>
> **假实现**：scaffold 新增 `tests/support/email.rs`：
> `pub struct Recording { sent: Mutex<Vec<Message>>, failure: Mutex<Option<String>> }`，
> `new()`、`fail_with(&str)`（之后每次 send 都返回 `Err(SendError::…)`）、`clear_failure()`、
> `sent() -> Vec<Message>`；`impl EmailSender for Recording` 先查失败再记录。规格用
> `#[path = "support/email.rs"] mod email;` 引入（1.9 的 postgres.rs 同一套路），夹具是规格里的
> `AppHarness`：`AppState::start_with_sender` + `std::net::TcpListener::bind("127.0.0.1:0")` +
> 自己的线程跑 `http::serve_on`，请求用 spec_http 那种裸 TCP 写法（不要复制 TestApp 的整份代码，
> 只要 get/post 两个方法）。
>
> **规格**（`spec/1.13/email_port.rs`，五条）：① 注册成功 → 假发送器**恰好**收到一封信、收件人是规整后的
> 地址、body 里有 `/confirm?token=<行里的令牌>`（表里的令牌与信里的令牌必须一致）；② 同一地址第二次
> → 409 且**没有第二封信**；③ `fail_with` 之后注册 → 仍然 201、行仍然是 `pending`、信没发出去（订阅者不
> 因为邮件服务器挂了而消失）；④ 恢复（`clear_failure`）后注册第三个地址 → 又是一封信（失败不粘住）；
> ⑤ 端口本身的一致性：`Logging` 与 `Recording` 都是 `EmailSender`，对「可投递的消息」都返回 Ok
> ——这可以用 `let senders: Vec<Box<dyn EmailSender>> = vec![Box::new(Logging), Box::new(Recording::new())]`
> 在一个测试里跑同一段断言表达出来（`test/fake-external` 的「假实现要与真实现同契约」）。
>
> **正文 TRAP 至少覆盖**：`send` 失败把注册变成 500（订阅者已存在，这是投递问题）；给已被订阅的地址也发
> 信（钓鱼/骚扰面）；把链接基址写死在正文示例里而正文又说它该是配置项（要么是常量要么是设置，不能是
> 两者）；`AppState` 里存具体类型而不是 `Arc<dyn _>`（测试没法注入）；用 `async fn` 直接写 trait 方法
> （dyn 分发编不过）；假实现只存在于测试文件里却断言真实现的行为（假货会漂移）。正文 DONE WHEN 里要
> 能复制的 `kiln run/check` 两条命令，并加上 `cargo test --lib`。

## 6. 工作方式约定

- 提交粒度：一个步骤 = 一个提交（正文 + 规格 + scaffold + 参考解 + 接线），提交信息写清 `track/step`；框架改动单独提交。
- 每完成 5–6 步把 `curriculum.md` 里对应步骤的状态（若新增了能力或深度）与 `plan.md` §11 的验收项同步一次。
- 遇到与规划冲突的现实（crate API 变了、某步实测超 120 分钟、某闸门不可行），**改规划文档并在提交信息里说明**，不要默默偏离。
- 需要产品决策时（例如某步拆不拆、是否改 track 名）先问，不要自行扩大课程范围。
