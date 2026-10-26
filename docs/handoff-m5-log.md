# M5 施工日志

> M5 的范围在 [plan.md](plan.md) §11：中文全量翻译（或首批核心步骤 + 术语表）、文档站、
> `new-track` 指南、年度维护流程。本日志按段追加；M4 的日志在
> [handoff-m4-log.md](handoff-m4-log.md)。

## 段 1：第一性原理评审，与它落成的三件事（2026-10-27）

**评审方法**：回到 §0 的一句话（步步可验证、同能力不重教、机制强制去重）与两条派生原则——
**判定的诚实性是产品**、**机制必须被真实负载检验，而不是被自己的测试检验**——逐机制对照。

**发现与处置**：

1. **补丁链是全仓最脆的产物类**（本 session 三起手改损坏）。处置：`kiln prepare` 子命令
   （只铺脚手架、不跑规格、不留记录——learner 的"给我开局状态"与维护工具共用）+
   `tools/rebuild-patches.py`（铺链 → 可选 fixes → rustfmt → 按已提交补丁的段标签/段序重生成 →
   拒绝删行的 scaffold），`--check` 模式验证"每份已提交补丁等于它定义的树所产出的 diff"，并已接进
   `cargo xtask ci`（`patches_check`，五轨共 ~25 s）。**首轮 `--check` 的审计结果**：mailer 8 份、
   lakelet 10 份、jobq 3 份历史补丁不满足该性质——全部是同一类真问题（scaffold 里带着 rustfmt
   会改写的行；fmt 门只判解题后的树，所以从未被抓到），已重生成修复；`lakelet/4.18` 的补丁文件名
   与步骤 id 不一致（`4.18-cardinality.patch`）属元数据漂移，工具改为按声明路径读取。
2. **i18n 机制从未装过真翻译**——回退链、结构检查、漂移检测全是未负载检验的机制。处置：
   zh-Hans 首批三步（mailer 1.1–1.3），`kiln i18n status` 结构 0 error、`--lang zh-Hans` 时正文
   解析到 `course/docs/steps/zh-Hans/…`。机制给翻译定了两条硬规则，实测才显形：**标题行必须逐字
   保持英文**（H1 是结构锚点；本地化标题住在 frontmatter 的 `title`）、代码块逐字节不动。
3. **门的作用域曾是隐式的**（`unsafe-budget` 收尾时的按步化是第一次显式化）。处置：规则写进
   [plan.md](plan.md) §6——"这一步的工作对不对"的门按本步补丁的作用域判，"项目合不合格"的门
   按整树判；`selftest`（逐步树）与 `verify`（尾巴树）靠这个分工同时成立。

**评审中顺带修掉的**：`MissingDependency` 的提示把 track id 重复了一遍（`jobq/jobq/3.1-…`）。

**明确不做的（记下理由）**：README 里易腐的计数（步数/题数）不做"文档断言"检查——`lint-caps-md.py`
已覆盖课程表，散文数字的维护成本低于检查器的构建成本；文档站与 `new-track` 指南按 plan §11 属
M5 后段，不与翻译并行开工。

## 段 2：zh-Hans 首批（mailer 1.1–1.3）

三份文件，frontmatter 四件套（`id`/`locale`/`source_hash`/`translator`），正文四段全译、
`TRAP` 内的代码标识符与规格输出保持原文。实测：`i18n status` → `structure: 0 error(s)`；
`kiln hint mailer/1.1-workspace --lang zh-Hans` → `body: course/docs/steps/zh-Hans/mailer/
1.1-workspace.md (zh-Hans)`；`kiln links` 144 页 0 findings。术语表暂不做——前三步的词汇量
（workspace、toolchain、CI）没有产生需要仲裁的译法，第一个真正的候选是 1.4 起的 config 分层。

## 段 3：第三个 flake——`4.1` 的跨连接竞态

CI 的 lakelet selftest 在 4.1 挂了一次（12 个测试并行时 3/4 复现）：`a_frame_that_is_not_one…` 等
的是**垃圾连接**的拒绝日志，而它断言的记录在**另一个连接**的落盘里——两个事实互相独立，守护进程
晚一点读第二个 socket，规格就在落盘前打开了 spool。修法是等自己那个事实：第二个连接自己的
`accepted=1 rejected=0` 收线日志。规格改动重钉 manifest（24 份），修后五次并行全绿。三个 flake
（1.21 容器、这个、以及 review 轮的容器门）的共同教训已经一致：**测试要等它断言的那个事实的发生
信号，而不是另一个并发方的信号。**
