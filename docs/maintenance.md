# 年度维护：一年后回来做什么

> 写给一年后的维护者（也可能是你自己，且已经不记得任何细节）。M5 的四件事之一（plan.md §11）。
> 贯穿全文的一条原则：**机器判得了的，都在 `cargo xtask ci` 里；这份文档只写机器判不了的。**

## 0. 一年里什么东西会坏

按"它坏的时候谁先叫"排，从最吵到最静：

| 会坏的东西 | 谁先叫 | 修在哪 |
| --- | --- | --- |
| 工具链（`rust-toolchain.toml` 是唯一真源） | `cargo xtask ci` 的 `fmt`/`clippy`/`test` | 改钉版 + 修树 |
| 新 rustfmt 的格式化结果 | `patches_check`（树重排后补丁不再等于树） | `tools/rebuild-patches.py` 重新生成 |
| 依赖（版本写在**补丁里**，见 §2.3） | 构建失败，或某条规格因上游行为变化而红 | 修树，再重新生成补丁 |
| 容器镜像（`images = [...]`） | `kiln images` / 容器门 | `track.toml` 与代码里的常量一起改 |
| 规格文件被改动而清单没重钉 | `spec-integrity` | `kiln spec manifest --track <t>` |
| 能力台账与 `docs/curriculum.md` §3 不一致 | `ledger_matches_curriculum`（`cargo test`） | 台账与文档同时改 |
| 探题引用的能力/深度已经变了 | `kiln probe --check` | `course/probes/` |
| 正文里的外部链接腐烂（330 个） | **没人**，除了 `kiln links --external` | 正文 |
| 一行正文以 `<` 开头（渲染成原始 HTML，段落断在半句） | `kiln site --check`（在 ci 里） | 重排那一行 |
| 英文正文改了、译文没跟上 | **没人**，除了 `kiln i18n status` 的漂移告警 | 译文（或词表） |
| 新语言的站点标签、track 名没翻 | **没人**（缺键就回退英文，永远不报错） | `locales/<locale>/ui.toml`、`[localized_titles]` |
| 正文里的数字（148 步、127 道探题…） | **没人** | 人肉改散文 |
| 译文读起来是否通顺 | **没人** | 人读 |
| 题还分不分得出"会"与"不会" | **没人** | 人做一遍 |

最后四行是这份文档存在的理由：它们永远不会变红。

## 1. 一年一次的顺序

先读 `git log --oneline -40` 和 [handoff-m5-log.md](handoff-m5-log.md) 的末两段——那里记着上一次收工时
已知的坑（容器 flake、miri 不可用、registry 镜像的地址）。

1. **环境自检**：`kiln doctor`。它说三件事：工具链与 `rust-toolchain.toml` 是否一致、容器能不能起、
   哪些慢门的工具没装（`cargo-semver-checks`、`cargo-fuzz`、`samply` 在这台机器上一直是 ✗，它们是
   should，不是 blocker）。`!` 行是警告，`✗` 行是缺东西；两类都别当成"要修"，先看它是不是本来就这样。
2. **升工具链**：只改 `rust-toolchain.toml` 的 `channel`（全文唯一一处，`workspace.package.rust-version`
   与 `rust:1.98-alpine` 镜像标签要跟着看）。然后跑 `cargo xtask ci`——第一次通常红在 `fmt`：
   新 rustfmt 会把课程材料排成另一种样子，而课程补丁是**树 diff 出来的**，树一变补丁就不再等于树。
3. **修树的固定动作**（不是手改补丁，见 authoring.md §5.5）：

   ```sh
   cargo fmt --all                      # 仓库自己的 crate
   # 课程材料（规格、探题）由 ci 的 format_course_rust 覆盖；补丁里的树要重新生成：
   python3 tools/rebuild-patches.py --track mailer --fixes /tmp/fixes.py   # 需要时
   python3 tools/rebuild-patches.py --track mailer --check                 # 验证：补丁 = 树
   ```

   `--check` 全绿只说明"补丁与它定义的树一致"，不说明树本身是对的——树错只会在下一步的脚手架或
   某条规格里露出来（工具自己的注释写了它看不见什么）。
4. **升依赖**：这在别的仓库是 `cargo update`，在这里是**改树**。每条 track 是一个独立的 cargo
   workspace（`course/tracks/<t>/skeleton/Cargo.toml` 起头，后续依赖由 scaffold 补丁插进
   `kiln:scaffold:` 标记块），版本号因此躺在补丁里：`axum = "0.8.9"`、`sqlx = "0.9.0"`。升级 = 改
   fixes 模块里的那几行 + `rebuild-patches.py` 重新生成；`cargo update` 只对仓库根的两个 crate
   （`kiln`、`xtask`）有意义。
5. **升镜像**：`kiln doctor` 会把每条 track 声明的镜像列出来（当前四个：`postgres:17-alpine` 35 步、
   `axllent/mailpit:v1.21` 1 步、`rust:1.98-alpine` 2 步、`alpine:3.20` 2 步）。改的时候三处一起动：
   `track.toml` 的 `images = [...]`、**scaffold 里那个 `const IMAGE`**（例如
   `mailer/scaffold/1.9-postgres.patch` 的 `tests/support/postgres.rs`）、以及会碰这个镜像的正文
   （`grep -rn "postgres:17" course/docs`）。然后 `kiln images pull` 再用 `kiln images` 确认。
6. **外部链接**：`kiln links --external`。它要网络，所以永远不是一个门；330 个链接里每年坏几个是
   常态，坏掉的换掉或删掉（`links` 的离线那一半仍要全绿）。
7. **慢门与容器**：`kiln verify --slow`（按 track 跑，能在 `kt` 里分 track 定位）。它跑
   `slow:` 前缀的门——mutants、coverage、soak——这些 `cargo xtask ci` 不跑。容器相关的 track
   （mailer 1.9 起、jobq、relay、lakelet 1.21/4.22）第一次跑之前 `kiln images pull`。
8. **站点**：`kiln site` 把整门课渲染进 `site/`（317 页、两种语言、一个搜索索引）；它先渲染到
   旁边的临时目录、全部写成后才替换 `site/`，所以删掉/改名的页面不会继续发布，失败的构建也不会
   留下半更新的一棵树。
   `cargo xtask ci` 跑的 `kiln site --check` 会渲染到一个临时目录再删掉，并检查**生成页面的每个
   站内链接都落在写出来的文件上**——这是 `kiln links` 看不见的那一半（它判 markdown，不判站点）。
   正式发布就是 `kiln site` 然后把 `site/` 交给任意静态托管。
9. **文档里的数字**：`grep -rn "126\|148\|165\|127" docs/ course/docs` 逐处核一遍。它们是散文，
   没有检查器（capacity 台账与 curriculum 有测试，散文没有）。改完记得把这一轮的结论追加到
   `handoff-m5-log.md`——下一个人读的是它。

## 2. 改东西的落点（省得又踩一遍）

| 你想改 | 落点 | 之后必须跑 |
| --- | --- | --- |
| 参考代码 / 学员项目的代码 | 树（`rebuild-patches.py` 的 fixes） | `--check`，然后 `kiln selftest --track <t>` |
| 规格断言 | `course/tracks/<t>/spec/<n>/<name>.rs` | `kiln spec manifest --track <t>`，再 selftest |
| 英文正文 | `course/docs/steps/en/<t>/<step>.md` | `i18n status`（会报中文漂移），`links` |
| 中文正文 | `course/docs/steps/zh-Hans/...` | `i18n status`（形状 + 术语漂移） |
| 术语表 | `course/glossary.toml` | `i18n status`——**先补词表，再统一正文** |
| 能力台账 | `course/capabilities.toml` + `docs/curriculum.md` §3 | `cargo test -p kiln`（那份等价性测试）与 `lint-caps` |
| 加一个门 | `xtask/src/main.rs`（唯一入口）+ plan §6 的阶梯 | 从引入它的那一步起，后面每一步都要带上 |
| 加一道探题 | `course/probes/<capability>.<depth>.rs` | `kiln probe --check` |
| 一门新语言 | `docs/steps/<locale>/`、`course/locales/<locale>/ui.toml`、各 `track.toml` 的 `[localized_titles]` | `i18n status`、`kiln site --check` |
| 正文的排版（换行、代码跨度） | `course/docs/steps/<locale>/…` | `kiln site --check`、`i18n status` |
| 加一条 track | 见 [new-track.md](new-track.md) | 那份指南自己的验收 |

## 2.1 加一门语言：三处，不改代码

1. `course/docs/steps/<locale>/`：正文，front matter 五个字段（`id`/`title`/`locale`/`source_hash`/`translator`）。
2. `course/locales/<locale>/ui.toml`：`welcome`、`final_message` 与 `site.*` 的标签（authoring.md §8.1）。
   缺哪个键就保留英文——部分目录不是错误。
3. 每条 `track.toml` 的 `[localized_titles]`：track 名。步骤标题不用动：它在正文的 front matter 里。

判据：`kiln --lang <locale> list <track>` 与 `kiln site --locale <locale>` 读起来是这门语言，
`kiln i18n status` 结构 0 error、漂移 0 warning。

## 3. 一年里不要做的事

- **不要为了让某个门变绿而把它降成 `should`。** 门的级别是课程的设计（plan §6），红的时候要么修
  树，要么认为这门判错了并写进 plan——不是调开关。
- **不要手改补丁。** authoring.md §5.5 里那三次实测事故，最短的一次也花了半天。
- **不要为了让 `i18n status` 变绿而"对齐结构"。** 英文多了一节就是内容变了：译文要重写那一节，
  不是把标题塞回去。
- **不要升级这台机器上本来就不能用的工具**（miri、flamegraph 驱动的 dtrace）：它们从来不是门，
  装上也只是多一行 `!`。
- **不要在没跑 `kiln images pull` 的机器上判断容器步骤"坏了"**：`kiln images` 先说话。

## 4. 成本

一次 `cargo xtask ci` 在 M0 收尾时是 **486 s**（22 步）；现在它逐条走完九条 track 的 148 步
（`selftest` 会重新构建每一步的脚手架与参考解），一次完整运行本机实测 **1 小时 33 分**（2026-10-29，
离线、复用 `target/` 缓存）；评审修复后的第二次实测 **2 小时 20 分**（同日）——时长主要由慢闸门决定
（jobq 的 mutants 约 30 分、lakelet 的 soak+mutants 约 50 分），不随 track 数线性增长，CI 每次 push 都跑。年度维护真正花时间的是三件慢活：`verify --slow`、`links --external`、
以及"人读一遍"——前两件各几十分钟，最后一件没有上限，也不该有。
