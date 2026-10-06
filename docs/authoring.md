# 编写规范

> 配套：[规划与工程决策](plan.md)、[课程体系与能力模型](curriculum.md)。
> 面向课程作者与翻译者。这里写的是**硬规则**；违反规则的课程条目应当在 `kiln selftest` 里失败。

## 1. 命名与目录

| 对象 | 约定 | 例 |
| --- | --- | --- |
| 步骤 id | `<track>/<phase>.<index>-<slug>` | `mailer/2.15-shutdown-leases` |
| 正文（英文源） | `docs/steps/en/<track>/<phase>.<index>-<slug>.md` | `docs/steps/en/mailer/1.14-worker.md` |
| 译文 | `docs/steps/<locale>/<track>/…`（同名文件） | `docs/steps/zh-Hans/mailer/1.14-worker.md` |
| 规格测试（真源） | `spec/<phase>.<index>/<name>.rs` | `spec/1.14/worker.rs` |
| 规格哈希清单（生成物） | `spec/manifest.toml` | 由 `kiln spec manifest` 重写，不手改 |
| scaffold patch | `scaffold/<phase>.<index>-<slug>.patch` | `scaffold/1.14-worker.patch` |
| track 骨架 | `skeleton/`（整条 track 一份，第一步用它） | `skeleton/src/main.rs` |
| 参考解 patch | `solutions/<phase>.<index>-<slug>.patch` | `solutions/1.14-worker.patch` |
| 能力 id | `<domain>/<name>`，小写连字符 | `async/graceful-shutdown` |
| probe 题 | `probes/<domain>-<name>.<level>.rs`（头部写 `probe = "<能力>@<深度>"`） | `probes/delivery-idempotency.apply.rs` |
| probe 参考答案 | `probes/solutions/<同名>.rs` | `probes/solutions/delivery-idempotency.apply.rs` |
| drill | `drills/<slug>.rs` | `drills/async-select-cancellation.rs` |

slug 用英文、连字符、动词优先（`rate-limit-by-tenant` 优于 `tenant-limit`）。

规格文件会被同步进学员项目：`spec/<phase>.<index>/<name>.rs` 变成 `tests/spec_<name>.rs`，也就是
`run.cmd` 的 `--test spec_<name>` 能启动的那个 target。这条映射是一条规则（不是命名建议）：
`kiln` 在 `spec-integrity` 里对照它，`cargo xtask ci` 也会检查 `track.toml` 里的 `spec` 与 `run.cmd`
是否对得上。所以**名字必须小写下划线**，且一个步骤至少要有一个 `run.cmd` 指向的目标。

## 2. 元数据的唯一真源

- **步骤元数据只在 `track.toml`**（`kind`/`origin`/`teaches`/`requires`/`run`/`spec`/`gates`/`hint`/`time_hint`）。
- **`run` 可以带 `timeout`**（秒）：`run = { cmd = "cargo test -p mailer --test spec_image", timeout = 1800 }`。它是个**下界**——取步骤声明的与 `kiln --timeout` 的较大值，同时作用于这一步的规格与闸门。只有「命令本身贵得出格」的步骤该写它（目前只有 1.21：镜像里要编译一棵依赖树，第一次构建在没构建过的机器上是分钟级；被默认 300s 砍掉的构建只会报 “timed out”，而这一步其实在正常工作）。
- **正文 front-matter 只放文档自身的字段**，避免两处真源漂移：

```toml
+++
id = "mailer/1.14-worker"
title = "Background delivery worker"
locale = "en"           # 译文改成本地化标签
# 仅译文需要下面两个字段
# source_hash = "sha256:…"
# translator = "…"
+++
```

- 正文里的 `DONE WHEN` 必须列出**确切命令**（`kiln check mailer/1.14-worker`），不能写「通过测试」这种含糊话。

## 3. 每步的规模与必填内容

1. 步长 30–120 分钟（`time_hint` 只是难度提示，不作为验收）；单步新概念 ≤ 3 个，超了必须拆。
2. 每步必须**一条命令可验证**，且默认离线（联网只用于拉 crate/镜像）。
3. 快路径（`kiln run`）< 60 秒；需要 mutants / loom / soak / bench / 模拟的检查标 `slow`。
4. TRAP 段必填，且必须是**具体的失败现象**，不是泛泛的忠告：
   - 好：`Only awaiting ctrl_c inside select! drops the other branch's future — the in-flight job vanishes with no log line.`
   - 差：`Be careful with cancellation.`
5. FURTHER 段给可点击的源码/文档（tokio、axum、sqlx 的具体文件或章节），不给「看官方文档」。
6. 正文英文、祈使句；代码标识符与生态术语保持英文；中文译文沿用 `glossary.toml` 的固定译法。

## 4. 三种正文模板

### 4.1 `origin = "teach"`（首次）

```md
# Background delivery worker

## WHY
A signup confirmation that never arrives is indistinguishable from a broken service…
（< 200 词，讲清生产里的代价，不讲语法）

## TASK
- Drain in-flight deliveries on SIGTERM; stop accepting new ones first.
- Bound the queue; define the overflow policy.
- Retry with jitter, capped attempts, and record attempts on the row.

## DONE WHEN
- `kiln run mailer/1.14-worker` — spec passes
- `kiln check mailer/1.14-worker` — all must gates green

## TRAP
1. Only awaiting `ctrl_c` inside `select!` → the other branch's future is dropped…
2. Asserting drain with `sleep(1s)` → flaky in CI; the test becomes the next bug.

## FURTHER
- `tokio::signal::unix::signal` — how the signal stream is registered
- `tokio_util::sync::CancellationToken` — drop semantics and child tokens
- Kubernetes `terminationGracePeriodSeconds` — the deadline you are racing
```

### 4.1.1 `teach` 步的可选段：`## RECALL` / `## DELTA`

`teach` 步可以**额外**带 `## RECALL`/`## DELTA` 段（R5 只看必需段是否齐全、以及非 teach 不许有
WHY）：当这一步的能力与更早的某项能力名字相近或相邻时，用两段短路标点名那一项与那个步骤，比塞进
WHY 更好读。`relay/2.9-ordering` 是样本（它同时指向 `relay/2.6`、`relay/2.7` 与 `mailer/1.14`）。

## 4.2 `origin = "deepen"`

```md
# Shutdown with in-flight leases

## RECALL
`async/graceful-shutdown@apply` — `mailer/1.14`: stop accepting, drain in-flight, force-exit on deadline.

## DELTA
Here an in-flight task holds a database lease. Dropping it does not lose a delivery —
it makes the row invisible for the lease duration. The deadline you choose now trades
recovery latency against duplicate work, and clock skew decides who wins.

## TASK / DONE WHEN / TRAP
（同 4.1）
```

### 4.3 `origin = "transfer"`

同 `deepen` 的模板，但两句必须写实：
- `RECALL` 写清**上次那步的 id**与深度（`kiln lint-caps` 会校验指向存在）；
- `DELTA` 只写「新项目里的坑」，并说明本步**闸门更严、提示更少**。

### 4.4 `kind = "design"` 的正文

文档型步骤（每条 track 末尾）交付两样东西，正文必须分开写：

- **可执行的演练**：`run` 指向一个规格测试（断言「无丢单」这类不可观测承诺）；
- **人要读的页面**：runbook / 演练手册，受 `links` 闸门检查（plan §6 里写了为什么它是离线的）。
  闸门会读项目里的每一页 markdown，因此页面的硬规则是：
  - 一页都不能「只是占位」：项目里至少一页，且全树至少有一个链接；`TODO`/`TBD` 这类占位目标算不过；
  - 相对目标要在项目里真的存在（逐段精确比对，大小写不符也算错）；`#锚点` 与 `page.md#锚点` 要命中该页标题；
  - 不能写绝对路径、不能越出项目（`../..`）、不能指向 `target/`；外链只被列出、不被抓取；
  - scaffold 的 stub 页用占位目标把清单写出来：学员 `kiln run` 看到的红就是他还没填的那几项。

正文里把两者写成一件事（「把 runbook 写成你会重放的演练」），而不是「写一份文档」。

## 5. scaffold patch 规则

scaffold 会把新模块的骨架与依赖加进学员项目。它必须是**加性**的，否则必然与学员的编辑冲突：

1. **只允许两种操作**：新增文件；在标记块内插入行。`kiln` 自己执行补丁，并在应用前就拒绝
   含删除行的 scaffold，所以违反这条的课程文件不会被悄悄应用。
2. 标记块长这样（`Cargo.toml` 用 `#`，Rust 文件用 `//`；`kiln` 只找行里的
   `kiln:scaffold:begin` / `kiln:scaffold:end` 记号，不关心注释语法）：

```toml
[dependencies]
# kiln:scaffold:begin
sqlx = { version = "0.9", features = ["runtime-tokio", "postgres"] }
# kiln:scaffold:end
```

3. **绝不允许**修改标记块之外的任何行（包括格式化结果）。插入点必须在既有标记块**内部**：
   scaffold 不能自己新建一个标记块，所以标记块由骨架或更早的创建文件写定 —— 学员因此能一眼看出
   课程以后会改哪些区域。
4. scaffold 决定**函数签名**：规格测试只依赖 scaffold 里声明的签名，学员改签名就会编译失败——
   这是明确反馈，不是玄学。
5. scaffold patch 必须在**参考解之前**应用：`selftest` 的顺序是 `scaffold → 期望失败 → 应用参考解 → 期望全绿`。

`kiln run <step>` 与 `kiln check <step>` 都会先把这个步骤准备好（同步规格 + 应用 scaffold），两步都是幂等的：

- 文件已经存在且内容一致 → 什么都不做；
- 文件已经存在但内容不同 → **保留学员的版本**并说明（scaffold 永不覆盖学员代码）；
- 插入行已经在位 → 视为已应用。

### 5.1 第一步的 scaffold 就是 track 骨架

一条 track 的第一步没有“在什么之上打补丁”，所以它的 `scaffold` 直接写 `"skeleton"`：把
`course/tracks/<track>/skeleton/` 里缺少的文件复制进 `app/<track>`。骨架同样绝不覆盖已有文件（对比
track 骨架的规则与 patch 的规则一样）。

这也是 track 决定自身“每个 track 是独立 workspace”（plan §3 D6）的地方：骨架里就有 `[workspace]`，
而仓库根 `Cargo.toml` 用 `exclude = ["app"]` 把 `app/` 挡在课程 workspace 之外，两边不会互相干扰。

### 5.2 有些步骤没有 scaffold

只改学员自己文件的步骤（例如 `mailer/1.2-toolchain`：工具链钉版、风格设置、lint 策略都是学员要做的决定）
不声明 scaffold。`kiln` 不会抱怨：没有 scaffold 的步骤只做规格同步。给这种步骤硬塞一个 scaffold，等于把学员
该做的决定替他做了。

### 5.3 scaffold 与参考解的对称性

- **参考解**是普通 patch：它可以改写、删除、新增，并且在树的形状与它预期不符时**拒绝应用**（不猜）。
  它作用于 scaffold 之后的那个状态，所以 `selftest` 才能验证“scaffold 必败 / 参考解必全绿”。
- **`kiln reset <step>`** 把 scaffold 的加值收回去：去掉仍在原位的插入行，删掉没人动过的创建文件；
  学员编辑过的文件保留并提示。reset 之后 `kiln run <step>` 会把 scaffold 重新放回去。reset 不会删除
  任何不是 scaffold（或参考解）创建的文件。

## 6. 规格测试规则

1. 规格测试**只读**（`spec/`，哈希入清单），且不得包含 `#[ignore]`。
2. **必须能在 scaffold 上失败**（空测零容忍）：`selftest` 会先在未实现状态下跑一遍，全部通过 = 该步的规格测试写废了。
3. 一个步骤可以有多个规格文件，但 `run.cmd` 只跑一个测试目标，避免「跑哪几个」变成隐性知识。
4. 不能用 `sleep` 做同步；用事件/通道/轮询条件（`wait_for`）等待。时间相关用注入时钟。
5. 测试失败信息要能指向 TRAP：断言消息里写「what to check next」，而不是只写 `assert!(x == y)`。
6. **优先行为断言，而不是“文件存在”**。能观察的行为：编译器的看法（`cargo check --lib` 告诉你有
   没有 library target）、程序跑起来的输出与退出码、编译失败本身（拿一个故意的坏文件让被禁的 lint 顶回
   来）、`cargo xtask ci` 在干净树上绿、在坏树上红。做不到行为断言时（例：黑盒规格无法调用一个
   可能还不存在的 library），就退回去量一个结构事实，**并在正文里写明为什么**。
7. 规格文件也会进 fmt/clippy 的 `--all-targets`，所以它自己必须 rustfmt 干净、没有 warning：
   课程不能要求学员遵守自己都不遵守的 fmt。`rustfmt --edition 2024 --config-path rustfmt.toml --check \
  course/tracks/*/spec/**/*.rs` 可以本地验证（`kiln spec manifest` 之后记得重生成清单）。
8. **一条规格要在它会被拿去判的每一棵树上都成立**，不只是它自己那一步的树：`selftest` 用本步的树
   （scaffold 后 / 参考解后），而 `verify` 与「学员做到后面再回头跑早期步骤」用的是**最终树**。所以规格
   不能依赖“此刻服务还没有 HTTP 层”这类事实。反例（已修）：1.6 的规格用 `Command::output()` 等日志进程
   自己退出——1.7 加了监听后它永远不退出，`verify` 与项目自己的 `cargo xtask ci`（`spec_ci` 会启动它）
   都挂死。写法：给它一个空闲端口（`--listen-port`）、读到「启动结束」的标点（进程结束 / 说出 `listening`
   / 那个端口开始应答）就停手，再断言行内容；退出码只在确实有退出时才断。

## 7. 闸门

`track.toml` 里 `gates = [...]` 使用极小的字符串 DSL：

| 写法 | 含义 |
| --- | --- |
| `spec-integrity` | must 闸门，无参数 |
| `clippy@2` | must 闸门，参数 `2`（lint 阶梯） |
| `mutants:80` | must 闸门，参数 `80`（阈值） |
| `?pedantic` | **should** 闸门：告警不阻塞 |
| `slow:mutants` | 慢闸门：只在 `kiln verify --slow` / `selftest` 跑 |

规则：

1. 每个 `teaches` 条目至少要有一个 must 闸门或规格测试作为证据（`lint-caps` R4）。
2. 新增闸门必须同时提供**缺失工具时的可执行提示**（`cargo install cargo-mutants` 这类），不允许只抛退出码。
3. 闸门不得依赖 GNU userland（无 `timeout`、无 GNU-only 参数）；超时在 Rust 侧实现。
4. 闸门只对**目标类型**生效：库代码严格、测试代码放宽。放宽必须在正文里写明理由，不能藏进 `allow`。
   具体分工（`clippy@N` 按这个实现，也决定了 workspace lints 表里能写什么）：

   | 目标 | 闸门看什么 |
   | --- | --- |
   | `--all-targets` | `-D warnings`、`todo`、`dbg_macro`（测试里的 `todo!()` 一样是未完成工作） |
   | `--lib` | `print_stdout`、`disallowed_methods`：库不许直接写终端、不许直接读时钟/环境 |
   | `--lib --bins` | `unwrap_used`/`expect_used`/`indexing_slicing`/`panic`：二进制也是生产代码 |

   所以 `[workspace.lints]` 里**只能**放对所有目标都成立的东西（`todo`/`dbg_macro`/
   `missing_docs`/`unsafe_code`）；放 `unwrap_used` 或 `print_stdout` 会让第一条测试断言就编译不过。
5. **闸门有两种作用域**（见 plan.md §6）：持久属性（`spec-integrity`/`fmt`/`clippy@N`/`no-todo`/`docs:public`/`exit-code`/`i18n`/`caps`）一旦引入就贯穿后续；
   步骤作用域属性（`images`/`container-repro`/`links`）只写给拥有那个产物的步骤。回归保护靠 `verify` 跑遍每步自己的闸门，不靠列表无限稀释。
   **引入它的那一步就是它开始生效的那一步**（`?coverage@N` 从 T3·3.21 起）：想提前某个门，先读 plan.md §6 里
   “门的起点是能力，不是日程表”那段——那里写了为什么提前是内容工作而不是配置。
6. 闸门与 `selftest` 同步演化：`cargo xtask ci` 会跑 `kiln selftest`，所以在 CI 变绿之前，每一步都必须满足「scaffold 上必败 / 参考解上必全绿」。
   本机新增工具链依赖时（`cargo-mutants` 这类），先让 `kiln doctor` 能报出来，再把闸门写进 `track.toml`。

## 8. i18n

1. 英文是唯一真源；译文缺失时回退，不报错。
2. 译文 front-matter 必须带 `locale`、`source_hash`、`translator`。
3. 翻译**不得**改动代码块、命令、能力 id、闸门名——只翻散文。
4. 标题结构必须与英文一致（缺段 = 结构错误，CI 报错；`source_hash` 不符 = 漂移，只告警）。
5. 术语查 `glossary.toml`；新术语先加进词表再翻译。
6. 加一门语言 = 新建目录 + front-matter，**不改任何代码**；`kiln --lang <locale>` 必须优雅回退。

## 9. 作者检查清单

先看 [t1-mailer.md](t1-mailer.md) §4「测试装置」与 §5「可以照抄的模式」——T1 是已交付的参考实现，
夹具、探针 crate、design 步、`.sqlx` 元数据这些形状都有现成样本，不必重新发明。

- [ ] 步骤只教 ≤3 个新概念，`time_hint` 在 30–120 分钟之间
- [ ] **先查台账再看正文**：这一步要建立的 (能力, 深度) 是否已被别的步骤建立（`grep` 台账或
      `kiln caps`）；若在别的 track 已建立**同深度**，本步只能是 `transfer`（把它写进 `requires`，
      **不写进 `teaches`**）——`lint-caps` R0/R1/R2 会拦，但作者要在设计前就想清楚
- [ ] **相邻能力的边界要写在正文里**：新能力与已教过的东西**同名或近义**时（`delivery/idempotency`
      与 `api/idempotency-key`、T1 的投递 claim 与 `db/atomic-claim`、`delivery/idempotency` 与
      `delivery/at-least-once`），正文必须点名**那一项能力与那个步骤**，并写清「本次不重复的是什么、
      多出来的是什么」。这条对 `teach` 步同样成立（`deepen`/`transfer` 的 RECALL 由 R5 机械校验，
      `teach` 步靠这句人工规则：相似的名字会让学员以为在重讲，也会让后续作者把边界越写越模糊）
- [ ] `teaches`/`requires` 声明完整，`kiln lint-caps` 通过
- [ ] 正文模板与 `origin` 匹配（teach 有 WHY；deepen/transfer 有 RECALL+DELTA，无 WHY）
- [ ] TRAP 写的是具体失败现象，不是忠告
- [ ] `DONE WHEN` 里是可复制的确切命令
- [ ] 规格测试在 scaffold 上失败、在参考解上全绿
- [ ] scaffold 是加性的（只新增文件 / 只改标记块），或本步不需要 scaffold
- [ ] 快路径 < 60 秒；慢检查标了 `slow`
- [ ] 每个 `teaches` 都有 must 闸门或规格测试兜底
- [ ] 规格文件本身 rustfmt 干净（它会被当成学员的 test target 编译）
- [ ] 英文正文；译文（若有）带 `source_hash` 且未改代码块
- [ ] `cargo xtask ci` 绿（它含 `spec-integrity`、`lint-caps` 与 `kiln selftest`）
- [ ] `deepen`/`transfer` 步：`RECALL` 点名了更早的步骤（`track/<phase>.<index>`）、点了深度，
      且那一步真的建立过本步的 `requires` 或要深化的槽位（`lint-caps` R5 会查）
- [ ] `apply` 及以上的槽位：有对应的 probe（`course/probes/`），且 `kiln probe --check` 通过——
      没有 probe 的槽位要在正文或 handoff 里写明理由（例如需要真实数据库的网络型能力）

## 10. 翻译者检查清单

- [ ] 从英文源复制结构，删掉不相关的段落，不新增章节
- [ ] 只翻散文；代码、命令、能力 id、闸门名、路径保持原样
- [ ] 术语与 `glossary.toml` 一致
- [ ] front-matter 补 `locale` / `source_hash` / `translator`
- [ ] `kiln i18n status` 无结构错误（漂移由作者处理，不静默修哈希）
