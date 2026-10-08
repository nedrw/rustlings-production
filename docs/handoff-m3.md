# M3 交接简报（T4 `lakelet` 的 24 步 + 三条新慢门 + 容器与性能基建）

> **状态：施工中。** 计划与要拍板的决策在 [m3-plan.md](m3-plan.md)（已拍板：六个决策点全部按推荐）。
> 分段施工记录在 [handoff-m3-log.md](handoff-m3-log.md)，按批次追加。
> M3 收尾时这份文件会像 [handoff-m2.md](handoff-m2.md) 一样补全：范围与验收（§0）、三条新门的实测契约（§1）、
> T4 的项目形状（§2）、施工中钉下来的决定（§3）、实测数字与验收演练（§4）、缺口（§5）。

## 0. 已落地的部分（截至本次提交）

| 批次 | 提交 | 内容 |
| --- | --- | --- |
| B0a | `kiln(gates): the three slow gates T4 needs…` | `slow:sim@N`、`slow:soak@N`、`slow:fuzz@N` 三条 must 慢门 + 四门共用的 harness 管线（前缀带 track、严格 `k=v` 解析、课程侧参数文件、重跑失败原因） |
| B0b | `kiln: the container and profiling infrastructure T4 leans on` | `kiln images` 按引用去重 + 一行离线结论、容器**运行**探测（`✓ containers can start (podman \`alpine:3.20\`, 0.2s)`）、`kiln skip --track`、`doctor` 的 profilers 一节 |
| B0c | `course(lakelet): …` | T4 的 24 步元数据（槽位/origin/门阶梯）+ `app/lakelet` 骨架（库 + 两个 bin + 夹具） |
| B1a | `course(lakelet/4.1-ingest-protocol): framing, its edges, and the store behind it` | 4.1 正文（transfer 模板）+ 12 用例规格 + 参考解 patch + 由它逼出来的三处骨架修复 |
| B1b | `course(lakelet/4.2-parser-fuzz): a seeded fuzzer, the corpus it leaves, and the panic it finds` | 4.2 正文（teach）+ 7 用例规格 + `tests/fuzz_protocol.rs`/`src/protocol/corpus.rs`/埋着 bug 的 `sniff.rs`（骨架）+ 变器与一行的修法（参考解）+ `fuzz/4.2-parser-fuzz.toml` + `slow:fuzz@3` 第一次真跑 |
| B1c | `course(lakelet/4.3-write-path-adr): …`（`167c1f0` + `cc75a0c`） | 4.3 正文（teach）+ 14 用例规格 + `wal/policy.rs`（fsync 策略作为数据）+ `spool/durable.rs`（scaffold 新建的六个签名契约）+ ADR（实测 4.173 ms/次持久写 → `interval(50ms)`）+ 六条崩溃点；`Spool` 获得 `durable_offset` |
| B1d | `course(lakelet/4.4-wal-recovery): a log is a prefix, and opening a store is what replays it`（`f4f066a` + `c01cdc2`） | 4.4 正文（deepen）+ 14 用例规格（两个穷举扫描）+ `wal/{checksum,segment,replay}.rs` + `spool/recover.rs` + 四种死法的 `tests/sim_wal.rs` + `sim/4.4-wal-recovery.toml`（`slow:sim@3` 首次真跑）；骨架获得 `[dev-dependencies]` 标记块与 `Service::restart`；4.3 的两个开口收口（停机 flush、开库记恢复） |
| B2a | `course(lakelet/4.5-batch-writes): the write path writes a batch, and the counters say what it cost`（`68983ed`） | 4.5 正文（teach）+ 12 用例规格 + `flush/{mod,batcher}.rs` + `spool/batch.rs` + 课程装置 `tests/budget.rs` + `budget/4.5-batch-writes.toml`（基线＝参考解实测）+ `docs/write-amplification.md`；`bench-budget@10` 首次真跑（`flush::Stats` 五个计数器成为学员契约） |
| B2b | `course(lakelet/4.6-ingest-backpressure): overload is a decision, and every one of them is counted`（`3aa7272`） | 4.6 正文（deepen）+ 13 用例规格 + `service/backpressure.rs`（有界队列 + 四策略 + `Stats`/`Tally`）+ 骨架 `config.rs` 两个标记块与 `queue.{capacity,policy}` 四层配置 + `tests/sim_backpressure.rs` + `sim/4.6-ingest-backpressure.toml`；`slow:sim@3` 第二次真跑，plan §9 演练第 3 条在此完成（`HashMap` 顺序注入 → 重放半部红） |
| B2c | `course(lakelet/4.7-columnar-segments): a segment is columns, and the page writes the layout down`（`0813393`） | 4.7 正文（teach）+ 14 用例规格（含 123 字节金向量 + 全部截断/翻转扫描）+ `segment/{mod,encode,decode,compress}.rs` + `docs/format.md` + `budget/4.7-columnar-segments.toml`（10 计数）+ 装置 `tests/budget_segments.rs`；`unsafe-budget@0` 首次真跑（scaffold 落 `unsafe_code = "deny"`），`mutants@80` 6/6 |
| B2d | `course(lakelet/4.8-compaction): a merge is a value, and a reader never sees half of one`（`4a946e0`） | 4.8 正文（teach）+ 14 用例规格 + `compaction/{mod,plan,snapshot}.rs`（合并策略作为值、五种 stage、退休给最后一个读者）+ 装置 `tests/{sim,budget}_compaction.rs` + `budget/4.8-compaction.toml`（13 计数）+ `sim/4.8-compaction.toml`；`kiln verify lakelet` 首次真跑（`8/8 judged · 0 failed · 16 without content`） |
| B3a | `course(lakelet/4.9-retention): old data leaves in whole buckets, and a reader never sees it go` | 4.9 正文（teach，15 用例）+ `retention/{mod,partition}.rs`（scaffold 新建的契约文件）+ manifest v2（每段时间范围 + 持久 floor + `u64::MAX` 哨兵）+ `SegmentInfo::spanning` 与三参 `new` 并存（§1 第 10 条）+ `Snapshot::records` 按本 epoch 的 floor 过滤 + `tests/budget_retention.rs` 与 `budget/4.9-retention.toml`（19 计数，含 `bytes_freed_on_disk` 与 `bytes_rewritten_max 0`）；`slow:mutants@80` 5/6（83%，噪声见 §5） |
| B3b | `course(lakelet/4.10-downsampling): a window is a bucket of the absolute grid, and an answer answers only the range it was asked` | 4.10 正文（teach，17 用例）+ `query/{mod,downsample}.rs`（窗口/范围/五种聚合/`Bucket`/`Answer`/`TOLERANCE`/`agree`，窗口复用 4.9 的 `Partition`）+ `src/cli/mod.rs` 与 `src/bin/lakeletq.rs`（`lakeletq query` 落地，jobq/3.15 的 CLI 契约 + `MAX_WINDOWS`）+ 装置 `tests/budget_downsample.rs` 与 `budget/4.10-downsampling.toml`（14 计数，含 `gaps`、`invented_windows_max 0`、`mean_gap_max`）；框架修复：`bench-budget` 的 blocked 消息不再硬编码 `tests/budget.rs` |
| B3c | `course(lakelet/4.11-query-engine): the grammar is a subset, the refusals are typed, and the plan is printed` | 4.11 正文（teach，18 用例）+ `query/{series,lex,parse,plan,exec}.rs`（全函数系列解析、按位置拒绝的扫描器、四类拒绝带类别、`ts BETWEEN` 必填且半开、`OR` 拒绝、计划只从 manifest 裁剪）+ `Snapshot::records_of` + `--sql` 一半的 `cli/mod.rs` + 写成的 `docs/query.md`（`links`）；**顺带修好 4.10 scaffold 缺标记块**（`cli/mod.rs` 末尾补块，4.19/4.23 的前置条件） |
| B3d | `course(lakelet/4.12-aggregation-correctness): a sum is not +, and a day is not 24 hours` | 4.12 正文（teach，17 用例）+ `query/agg.rs`（Kahan–Neumaier 的 `Sum` + 写成数字的 `BOUND`/`error_bound`、刻意保留的 `naive`、顺序无关性、`Zone` 自带转换表且表外回答 `None`）+ `downsample.rs` 的 `downsample_zoned` + `--zone`（只对 1d 窗口生效，本地日 23/25 小时）+ `exec.rs` 的 `execute_in`；`slow:mutants@80` 5/6（噪声，见 §5）、`?coverage` 177/177（100%） |
| B3e | `course(lakelet/4.13-model-based-tests): a model that knows nothing about segments, and a walk that has to agree with it` | 4.13 正文（teach，18 用例）+ `engine/rng.rs`（xorshift64\*，金向量 + 差异性）+ `tests/support/model.rs`（只有“记录 + floor”的参考模型）+ `tests/support/walk.rs`（夹具：操作字母表、时钟、transcript、收缩）+ `tests/sim_model.rs`（读 `KILN_SIM_SEED` 的装置）+ `sim/4.13-model-based-tests.toml`；`slow:sim@3` transcripts `85c499a1/903bdf8e/a8efae13`（种子 1 逐字节重放）、`slow:mutants@80` 6/6 |
| B3f | `course(lakelet/4.14-index-pruning): the index is beside the store, and pruning is a number` | 4.14 正文（teach，20 用例）+ `index/{mod,zone,bloom}.rs`（`<store>/index/<ordinal>.lki` 的侧车索引：zone map + 词级布隆；缺失/损坏/过期只损失剪枝）+ 装置 `tests/budget_pruning.rs` 与 `budget/4.14-index-pruning.toml`（18 计数，含 `pruning_rate`）；计划的第二级剪枝（`blocks_pruned`）与 `Outcome` 的新计数器；`spec/4.11` 的键数断言放宽成 `>=`（rule 10 的正解）并重钉。**plan §9 腿 5 完成**：去掉索引查询 → `pruning_rate 95 → 50`、`blocks_pruned 11 → 0`、`7/10 must`；回滚 `10/10 must` |
| B4a | `course(lakelet/4.15-budget-regression): the engine counts work per phase, and the profile names the phase that moved` | 4.15 正文（deepen，15 用例）+ `trace/{mod,counters}.rs`（七相位计数与恒等式：计数工作不计数时间）+ `query/exec.rs` 按相位计费 + `Outcome` 带 `trace`（`EXPLAIN` 的键不动——rule 10）+ 装置 `tests/attribution.rs` 与 `budget/4.15-budget-regression.toml`（36 计数）+ 项目页 `docs/perf.md`（`links`）；框架修复：`bench-budget` 不再静默忽略基线里没有的计数 |
| B4b | `course(lakelet/4.16-profiling): a profile is evidence, and the page carries the run that produced every number` | 4.16 正文（teach，13 用例、8 条 TRAP）+ `lakelet-load --profile`（`--cycles` 可重复、`--seconds` 按钟停）+ 项目页 `docs/profile.md`（顶部两符号 = 每次 `seal` 的四次 `sync_all`，**七个相位没有一个计到 flush**——正文据此建议段路径也走 4.3 的 `wal::Policy`）+ 规格**逐行重跑并比对页面上的那段输出**；骨架改了 17 加/2 删（补两个标记块、改单线程 runtime，实测八条 parked worker 线程会淹掉 profile）；框架无改动 |
| B4c | `course(lakelet/4.17-soak): the gate judges counters, and the harness fails on the counter that moved` | 4.17 正文（teach，17 用例）+ 五个活计数器（`open_tasks`/`open_handles`/`store_files`/`store_bytes`/`records`）+ 装置 `tests/soak.rs`（稳态断言、`LAKELET_SOAK_MINUTES` 长形态）与 `soak/4.17-soak.toml`（`cycles_min = 4000` 下界 + 全上界）+ 项目页 `docs/soak.md`（`links`）。**plan §9 腿 2 完成**：在 `ingest::serve` 注入“句柄不放”的泄漏 → `✗ slow:soak@4000 … panicked at tests/soak.rs:356 (after 149s)`（装置点名 `open_tasks 2003 → 4002`）、`7/11 must`；回滚 → `open_tasks 2`、`11/11 must`。计时：soak 门 ~146 s |
| B4d | `course(lakelet/4.18-cardinality-cost): cardinality is the bill, and the cap is a counted policy`（`dc8f159`；框架 `2d1c585` + `0c0300d` + `76b755f`，文档 `f4fd31c`） | 4.18 正文（teach，16 用例）+ `cardinality/{cap,mod}.rs`（带上限的准入、按系列计价、从日志与侧车索引重建的账本、`offered = admitted + rejected`）+ `lakeletq inspect series` + `docs/cardinality.md` + `soak/4.18-cardinality-cost.toml`（复跑 4.17 的界，未动）；**变异门的三次修正**：位点按文本锚定（`76b755f`）、抽样 12（`0c0300d`）、范围＝本步写下的行（`2d1c585`），并因此给 4.14/4.17 各补了三条真缺口的用例 |
| B4e | `course(lakelet/4.19-backup-restore): a copy is a log, a restore is an ordered write` | 4.19 正文（teach，19 用例）+ `backup/{mod,manifest,restore}.rs`（副本清单＝4.4 的日志格式、只取清单点名的段与日志已证前缀、先全验后写且最后写 `segments.manifest`）+ `cli/backup.rs`（`backup`/`restore`/`drill` 三命令）+ 装置 `tests/sim_backup.rs`（完整，695 行）+ `sim/4.19-backup-restore.toml` + `docs/dr-drill.md`（RTO/RPO，带一块可逐行复跑的确定性输出）；`slow:sim@3` `5b740412/aacc05b0/b52d27f8`、`slow:mutants@80` 12/12 |
| B4f | `course(lakelet/4.20-capacity-model): the model and the comparison, judged in one lane` | 4.20 正文（teach，14 用例）+ `engine/cost.rs`（`Model`/`Usage`/`Estimate`/`Measured`/`Predicted`/`Drift`/`Agreement`：整数微单位、无浮点、不读任何东西）+ 完整装置 `tests/budget_cost.rs` + `budget/4.20-capacity-model.toml`（35 计数）+ `docs/capacity.md`（模型的 25 行块 + 保留期表，规格逐行/逐格重算）；`slow:bench-budget@10` 实测：模型与店之间的漂移 23/0/0/2/12/24 ‰；**演练**：把 `bytes_per_sample` 4→5（20%）→ 35 个计数里 10 个越界（点名 `drift_segment_bytes_permille_max 181 > 40`、`within_max 1 > 0`）、`9/10 must`；回滚 `10/10` |
| B5a | `course(lakelet/4.21-cold-tiering): the tier is a port, the client signs, and the suite runs twice` | 4.21 正文（deepen，**27** 用例）+ `tier/{mod,http,s3,filesystem}.rs`（blob 端口 `put/get/delete/分页 list`、`Policy`、`sweep`（先 put 后删本地）与 `hydrate`）+ 完整装置 `tests/s3_stub.rs`（进程内 S3 形状端点，带请求日志与故障注入）+ `tests/support/tier.rs`（`KILN_BACKEND`）+ `conformance.toml`；`slow:conformance@2` `local ✓ s3 ✓`、`slow:mutants@80` 10/12（83%，两个不可见位点见 §5）；SigV4 用 AWS 自己的 get-vanilla 向量钉住；演练：把顺序倒过来（先删本地）→ `the_window_between_the_object_and_the_delete_is_readable_from_both_sides` 红 |
| B5b | `course(lakelet/4.22-reproducible-build): the same source, the same bytes`（+ `course(lakelet): the project names the default binary a bare cargo run means`） | 4.22 正文（deepen，16 用例）+ 项目七个文件（两阶段 `Dockerfile`、项目自己的 `rust-toolchain.toml`、`.github/workflows/ci.yml`、`deny.toml`（从 lockfile 用 `cargo metadata` 推导）、`scripts/repro.sh`（两次构建逐字节比较，只印计数与名字）、`docs/build.md`、`docs/supply-chain.md`）；实测 `scripts/repro.sh` → `artifacts_compared=4 identical=4 differing=0 verdict=reproducible`（34 s）；**`images` 绿**（两个声明镜像在本机），**`container-repro` 红——但是引擎的**（见 §5）；骨架补 `default-run = "lakeletd"`（否则门问宿主用 `cargo run --package lakelet` 根本得不到答案），并补一条用例钉住它 |

尚未落地：T4 的 24 步正文/规格/参考解（已交付 4.1–4.22；还差 4.23–4.24）、探题、验收演练。**plan §9 腿 6 的第二半仍欠：容器引擎坏着**（见 §5）。

## 1. 施工中钉下来的规则（收尾时并入 §3）

四条都是 4.2 那一段实测出来的，下一个人直接照做：

1. **慢门的验证在 scratch 副本里做，不在活的工作树里。** `kiln` 会从子进程环境里清掉 `CARGO_TARGET_DIR`
   （M2 §5.2 的有意行为），所以 `kiln run/solution/check` 的构建落在**工作树的 `app/<track>/target/`**
   （4.2 那次落了 387 MB），而工作树里不允许出现 `target/`（Delta VFS 会卡）。配方：
   `rsync -a --exclude .git --exclude .delta --exclude app --exclude target --exclude .kiln ./ /private/tmp/<name>/`，
   然后在副本里 `kiln run <前置步>` → `kiln solution <前置步>` → `kiln run <本步>` → `kiln solution <本步>` →
   `kiln check <本步> --slow`（都带私有 `XDG_CACHE_HOME`）。**`kiln selftest` 是例外**：它在
   `/var/folders/...` 建砂箱，在工作树里跑安全。**但别以为它只跑快门**：`cmd/selftest.rs` 调用的是
   `session.gates(step, true)`，也就是 `slow:mutants@80`、`slow:bench-budget@10` 都会跑
   （4.9 实测它打印的就是 `gates ✓ 10/10 must green`，含两条慢门）。
2. **每批收尾都跑 `cargo test -p kiln`，即使那一批没碰 `kiln/**`。** 里面有几条断言读的是**课程树**
   （`caps::lint::tests::the_repository_course_is_clean` 的 `authored_steps`、
   `info::tests::the_repository_course_loads` 的 `tracks().len()`），一次 track 元数据落地就要同步它们——
   4.2 这一段发现它们**自 B0c 起就是红的**（B0c 往 `course/tracks/` 里加了 T4）。
3. **fuzz 的语料是**项目里的 `tests/corpus/**`（一行一个十六进制输入，`.txt`，读取器由那一步的骨架送来），
   闸门只认“重放条数 ≥ 课程侧文件的 `corpus_cases`”，不认语料在哪也不认它的形状——理由与推翻过程见
   [m3-plan.md](m3-plan.md) §4.3。别再把语料字节钉进规格：那会把“fuzz 找到的东西”先告诉学员。
4. **骨架里凡是要被后步扩展的模块文件，都得带一个 `kiln:scaffold:begin`/`end` 块**（`kiln` 拒绝在标记区
   之外插入；4.2 之前 lakelet 的 5 个模块文件一个块都没有，第一批 scaffold 就撞了墙）。`Cargo.toml`、
   `src/lib.rs`、各 `mod.rs`、`tests/support/mod.rs` 都已经有；新加的模块文件同理。`Cargo.toml` 的
   `[dependencies]`、`[dev-dependencies]`、`[lints.rust]` 与 `[workspace] members` 四处都有块。
5. **改课程自己的装置/文件之后，`app/<track>/` 里那份旧的还在**：`kiln run <step>` 永不覆盖学员已有的文件
   （这是对的），所以作者改了 `tests/sim_wal.rs` 这类 scaffold 送进项目的文件之后，门判的是旧副本——实测
   （4.4）：digests 从 `5dd009a0/eddf61b3/fd32e3d5` 变成 `1fcbf0d8/9938dcf4/a4486a02`，中间只做了“删掉
   `app/lakelet/tests/sim_wal.rs` 再 `kiln run`”这件事。**骨架改过之后旧 `app/<track>` 同样作废**（4.6 踩过：
   从“未 rustfmt 的骨架”建出来的副本会让 scaffold 以与步骤无关的理由失败）——本地复验前删掉那一份，
   或换一棵 scratch 树。
6. **写 RECALL 前先查 `course/capabilities.toml`**：每个能力按深度写着是哪一步建立的
   （`[capability."async/backpressure"]` → `apply = "relay/2.14"`），所以 `requires` 里每一项对应的
   `<能力>@<深度> — <track>/<phase>.<index>` 一查即得，不用逐个 `track.toml` 翻。**规范形式是编号**
   （`lakelet/4.5`，handoff-m2 §3.5）；带 slug 的形式（`lakelet/4.5-batch-writes`）R5 也接受，4.3/4.4 的正文
   用了后者且都过了——两种都行，但同一个 track 里别混着写。
7. **课程拥有的文件要被后步追加内容，就得在建它的时候留一个标记块**。4.5 的 `tests/budget.rs` 没有块，而
   scaffold 只能在标记区内插入，所以 4.7 只能新建自己的装置 `tests/budget_segments.rs`（课程侧文件的 `command`
   指过去）。两边都行，但要选得明白：**要共享就留块，不留块就每步一份**。
8. **作业书里的门数一律对着 `track.toml` 数，不要推**。4.4（写 12、实际 11）与 4.7（写 11、实际 12）各错一次，
   两次都是凭邻近步骤的清单估的。
9. **作者工作一律在 `/private/tmp`，不在仓库里留未跟踪的暂存目录**。4.8 实测：在工作树里建一个未跟踪的作者
   目录之后，工作树的终端停止物化（“the checkout kept changing”），只能把它删掉、把作者工作挪到 `/private/tmp`。
   （与规则 1 同一族：工作树只放会被提交的东西。）一个**空**目录同样算——`git status` 看不见它，VFS 看得见。

10. **早期规格只能通过“早期树上已经存在的东西”接纳后来的契约。** 4.9 要给 `SegmentInfo` 加时间范围时，
    最自然的写法是把 `SegmentInfo::new` 从三个参数扩成五个——那是**课程级回归**：
    `kiln run lakelet/4.8-compaction` 会把**当前**规格同步进学员树，于是顺序学习的学员在 4.8 拿到的规格
    编译不过，`xtask ci` 的“每一步都守约”也永远红。判据是**每一步的规格必须在自己那棵树上通过**
    （`kiln selftest <该步>`），所以后来的契约要么是加一个**新**构造函数/新方法（4.9 的做法：
    `SegmentInfo::new` 保留三参、新增 `spanning`），要么是早期树本来就有的形状。改早期规格文本等于同时
    承认那条红线——能避免就别改。

11. **`slow:mutants@80` 会抽到不可能被杀的位点，这是仪器的噪声，别为它改门。** 4.9 实测：六个判定位点里
    有一个是 `src/engine/compaction/mod.rs` 里 `Vec::with_capacity(HEADER + len * ENTRY + …)` 的 `+` →
    `-`；把它手工打进去，15 条规格用例与 19 个预算计数全绿——这不是规格的缺口，是那个算术改不出行为。
    曾实现过一条“容量参数不是位点”的规则并**回滚**了，理由写进 §5：它会重排**每一步**的抽样
    （jobq 3.20–3.27 与 lakelet 的十二步都在内），而它自己的前提也不严格成立（容量参数更大可能在溢出时 panic）。
    结论：83% ≥ 80% 是过门，噪声接受。

12. **派活前先重建基线树并 `diff` 核对，再把路径写进作业书**。4.10 的作业书里写着“`/private/tmp/l4b3/base`
    是 `through 4.9`”，实际那份是 **`through 4.8`**（我在 4.9 落地前建的）——子 agent 自己找到了
    正确的那棵树才没出错。做法：`skeleton` + 逐步 `scaffold`/`solution`（4.1…前一步）重建一份，
    然后 `diff -r -x .git -x target -x Cargo.lock -x 'spec_*.rs'` 与手头的候选树比；只差 `kiln`
    生成的 `Cargo.lock` 与 `tests/spec_*.rs` 才是同一棵树。**作业书也写到磁盘**
    （`/private/tmp/l4b3-book-<step>.md`），派活时只说“先读那份文件”：worker 死掉时重派只要一段短提示，
    不必重贴整份契约（实测：4.10 的第一个 worker 因“terminal 建不了临时目录”而死，草稿只有
    拾起来才知道——那次是同一个 worker 的继承风险，也是第 11 条的同族问题）。

## 5. 施工中记录、收尾时并入 §5 的缺口

- **变异门会抽到改不出行为的位点（D1，实测，决定不改门）**：`src/engine/compaction/mod.rs:261`
  `Vec::with_capacity(HEADER + self.segments.len() * ENTRY + checksum::LEN)` 的 `+` → `-` 被抽中过；
  手工应用后发现 15/15 规格用例与 19/19 预算计数全绿，于是一个判定位点无人能杀，4.9 的比值为 5/6 = 83%。
  候选修法是“`with_capacity`/`reserve`/`reserve_exact` 的参数不是位点”，已实现并**回滚**：
  ①它会重排每一步的抽样（jobq 3.20–3.27 与 lakelet 十二步的已记录数字全部作废，需重新测量）；
  ②前提不严格——容量参数变大仍可能在溢出时 panic，所以位点不是“绝不可见”的。
  留着的东西：`kiln/src/gates/budget.rs:215` 在 main 上一直是 fmt 脏的（B2b 的预算报告修复带来），
  已用 `cargo fmt --all` 修好（只重排了一条断言）。
- **4.7 的规格有一处未被覆盖的边界（抽样时发现，未修）**：`src/engine/segment/decode.rs:422` 的
  `if shift > 63` 改成 `>=` 后，读取器会在 `bits > 1` 检查之前拒绝 `shift == 63` 的那个字节，也就是
  拒绝一个需要第十个字节的 varint——所以一条“往返一个差值大到需要第十个字节的时间戳”的用例能钉住它。
  4.7 记录的 `mutants 6/6` 是那次抽样的运气，不等于这条边界被覆盖；收尾时可考虑补一条用例。
- **变异门抽的是“文件当前的内容”，而不是“本步写的那些行”（实测；已修，见 §1 第 17 条）**：候选位点来自本步
  参考解补丁碰过的 `src/**.rs` 在**学员树当下**的内容，于是后来者在同一个文件里加的代码会落进**更早一步**
  的抽样，而更早那一步的规格没有义务覆盖后来者的功能。实测：4.11 往 `src/cli/mod.rs`（4.10 的补丁碰过
  它）加了 `render_plan`，4.10 的抽样就抽到 `src/cli/mod.rs:763 if !json {` 并让它存活（4.10 的规格里
  根本没有 `EXPLAIN`）——而 4.11 自己的规格能杀掉它（用例 9 同时断言两种形状）。同样的抽样移位把 4.10
  从 6/6 变成 5/6（83%，仍过门）。候选修法：抽样只取“本步补丁新增/改动的行”（行号可以从 hunk 里读），
  代价是全部变异比值要重测；在那之前，比值低于 100% 要先看它是“本步的真缺口”还是“后来者的代码被抽中”。
  对照：4.7 的 `decode.rs:422 if shift > 63` 确实是 4.7 自己写的行，那是**真缺口**（一个需要第十个字节的
  varint 没被覆盖），与仪器无关。
- **`compaction` 没有只读的 manifest 读入口（4.19 发现，未修）**：唯一的门 `Segments::open` 会**写**（删孤儿、
  删 `segments.manifest.pending`、sync 目录），所以 4.19 的 `copy` 打开它的源就是“一次会写的读”，源在只读挂载上时
  连打开都会失败——而线上备份工具不该改它的源。修法：给 `compaction` 一个只读的 `Manifest::load(directory)`
  （`decode` 早已存在，只是没暴露），4.19 的 `copy` 改用它；代价是 4.19 的解补丁与规格要重跑一次。
- **恢复后的账本窗口与源不同（4.19 发现，未修）**：`Registry::of_store` 的窗口来自活着的侧车索引（整段的 zone），
  恢复出来的店没有 `index/`，窗口只来自日志记录。4.19 的第 12 条用例断言了成立的那个方向（系列与计数相同）、
  并在用例里写明理由，但 `docs/cardinality.md`（4.18 的项目页）没提这个差别——收尾时可补一句。
- **正文与规格点名了一个参考解删掉的常量（4.19 发现，未修）**：4.19 的 scaffold 在 `cli/backup.rs` 顶部给了
  `DRILL_LINES`（27 行接口声明），正文（TASK 段）与规格的失败消息都点它的名，而参考解重写这个文件时把它删了
  （名字活在 drill 自己的 `(name, Measurement)` 列表里）。两种修法：参考解保留常量（并可加一条“打印的名字就是它
  声明的那些”的用例），或把正文与消息改成指“接口”而不指常量。
- **店的日志从不截断，所以它吞掉了大部分字节（4.20 发现，未修）**：`tests/budget_cost.rs` 实测 `log_share_permille 885`
  对 `sample` 的 `113` —— 容量模型若不把日志算进去只会定价磁盘的十分之一（4.20 的 TRAP 8 与 `docs/capacity.md`
  都写了）。真正的修法是一个“段封装之后截断日志”的机制，而 4.15 的相位计数里没有任何一项计到它；那是 4.20 之后
  的步骤或 M4 的活。
- **侧车索引的“每系列开销”是一个除法，不是一个常数（4.20 发现，已在页面写明）**：每段一个定长索引文件，它的大小
  不随段内系列数变，所以 4.20 的 `series_overhead_bytes` 在高基数上会**高估**几个数量级；页面写了它会往哪边错，
  规格没有为此发明一个新输入。收尾时可考虑在 `docs/cardinality.md` 里点一句与它的关系。
- **`SegmentInfo` 的字段可见性不一致（4.20 顺带发现，未修）**：`bytes`/`records` 是 pub 字段，而时间范围是私有
  字段 + 访问器。无实害，但“计划自己算出来的数”与“清单里读出来的数”被两种方式读。
- **没有只读的 manifest 入口（4.19 发现，4.21 再次撞上，仍未修）**：4.21 的 `hydrate` 因此只能接一个**活着的**
  `&Segments`（`Segments::open` 是唯一的门且会写），于是“一个已停进程的冷店根本无法打开”的后果只能写进页面与用例
  （`a_stopped_store_whose_segments_are_cold_is_not_openable_at_all`、`the_way_back_for_a_stopped_store_is_one_copy_per_segment`），
  key 恰好就是店自己的路径，所以手工回来的办法是“每段一次 copy”。**三个证人之后，这笔修正值得单独一批**：给
  `compaction` 一个只读的 `Manifest::load(directory)`（`decode` 已存在，只是没暴露），让 4.19 的 `copy` 与 4.21 的
  `hydrate` 都改用它——代价是两步的解补丁与 lane 要重跑（已向用户确认是否放进收尾）。
- **`mutants` 只跑一遍 `run.cmd`（不带 `KILN_BACKEND`），与 `conformance` 的后端轮询不一致（4.21 发现，未修）**：
  见 §1 第 20 条；当步的弥补是让每次运行都跑到 S3 那一半。框架侧两选一：走查也按后端跑（代价大），
  或用 `conformance.toml` 里第一个声明的后端（代价小、语义清楚）。
- **scaffold 状态的警示与 `tests/support/mod.rs` 的耦合（4.21 提醒，未修）**：4.21 的 scaffold 里有 31 条未使用警告
  （`todo!()` 之后才被调用的私有助手、未用 import），学员在写完之前一直看着；另外 `tests/support/**` 被每个规格目标
  编译，所以新增一个 support 模块会进入**所有**规格的编译面（一处坏掉就红一片）。两条都是课程造题的已知代价，记着即可。
- **容器引擎坏了，plan §9 腿 6 的第二半还欠着（4.22 期间实测，等用户修）**：`podman run --rm alpine:3.20 true`（包括
  `debian:bookworm-slim`、强制 arm64/amd64）均报 `creating overlay mount …: input/output error`，`podman images` 只列出
  `<none>:<none>`；而 `kiln images` 仍能找到声明的镜像，所以 `images` 门是绿的。后果：4.22 的 `container-repro`
  报 `the image did not build, so there is nothing to judge … input/output error`（`podman build` exit 125，< 1 s），
  步骤因此是 `10/11 must`。引擎恢复后要做的：在 lane 里重跑 `check --slow`（应 `11/11`，带镜像大小与 `--check-config`
  比对行），并补跑腿 6 的 drill（把 `FROM ${RUNTIME} AS runtime` 改成 `${BUILDER}` → 尺寸检查先跑，应该红在
  “the image is NNN MB, over the 25 MB this step allows”）。**镜像构建需要暖的 registry**：门把
  `$XDG_CACHE_HOME/kiln/container-registry` 挂进 builder；`~/.cache/kiln/container-registry` 是暖的（144 MB），
  而一个全新的私有 `XDG_CACHE_HOME` 会递给 builder 一个空缓存（有网时它去取，离线则建不出来）。
- **`container_repro.rs` 只把 `MAILER__` 前缀的环境变量带进宿主运行（4.22 发现，未修，暂时潜伏）**：镜像是
  `ENV` 里的任何 `LAKELET__*` 都会让门报“the image does not run the binary this tree builds”。4.22 的 Dockerfile
  因此一个 `ENV` 都不写（页面写明理由）。正解：前缀应当从项目/ track 推出来，或那次比较忽略镜像的环境——收尾时一并。
- **`kiln` 的补丁器忽略 `new file mode`（4.22 发现，未修）**：`src/patch.rs` 不设文件模式，所以 `scripts/repro.sh`
  在学员的树里是 `-rw-r--r--`（`kiln run` 后直接 `./scripts/repro.sh` 会 EACCES）。4.22 的正解是让规格跑 `sh
  scripts/repro.sh` 并查 `#!` 行，页面写 `chmod +x`；修 `patch.rs` 可以以后做（代价是要重验所有含可执行文件的补丁）。
  第三个实测点（4.12）：活下来的位点是 `src/cli/mod.rs:678 if windows > MAX_WINDOWS`，而 `MAX_WINDOWS`
  在 4.12 的参考解补丁里出现 **0 次**（4.10 有 3 次、4.11 有 5 次）——更早两步的行被 4.12 的规格判了。
  第四个实测点（4.14）：它的补丁**必须**碰 `cli/mod.rs`（4.11 的 JSON 键断言逼出来的），于是抽样抽到
  4.10 的 `if windows > MAX_WINDOWS`，作者一开始只有 66%，补了一条命令行边界的用例才回到 83%。
  **一个步骤只要碰到共享文件，就会“继承”那个文件里没被覆盖的行**——把小步骤的比值当成“本步规格好坏”
  的读数之前，先看那些行是不是本步写的。
- **`mutants@80` 的样本只有 6 个、比值阈值 80%**：5/6 = 83% 只高出阈值 3 个百分点，
  抽样换一位就会翻盘。这是计划 §6 D-1 定下的形状（“12 步、每步最多 6 个”），没改；
  但如果以后再出现“某个步骤因仪器原因卡在 83%”，值得重新讨论样本数（每再多判一个位点约 +1–2 s）。

13. **每棵树一个 `CARGO_TARGET_DIR`，没有例外**。4.13 的作者在作者侧踩了与砂箱侧同一个坑的新版本：两棵
    副本共用一个 target 目录，cargo 把**旧的** `lakelet` 产物交给了新树（那棵树里 `rng.rs` 是被改坏的），
    于是一条规格为一个树里并不存在的理由而红，差点去追一个假的 bug。砂箱侧的版本写在 `handoff-m2` §5.2，
    作者侧同样成立：**每个 scratch 树、每个 dev 树、每次演练都用自己的 target 目录**（名字里带上用途）。

14. **工作树里的文件只用文件工具创建，不要用终端写**。4.14 的作者实测：用 shell 往工作树的**新未跟踪目录**
    里写文件，Delta 报 `<skipped-untracked-directory>` 并让整棵树的终端停摆（“the checkout kept changing”）；
    恢复要把那棵树还原到 Delta 认识的样子（撤掉 shell 那次编辑、删掉未跟踪目录），然后**用文件工具建文件、
    再用 `cp` 把内容拷进去**。第 9 条的姊妹条：第 9 条说作者工作放 `/private/tmp`，这条说工作树里建文件的方式。

15. **交付的文件要对哈希或行数核对，并且先别删 worker 的副本**。4.16 实测：作者用文件工具分块转录的 730 行
    规格（因为 shell 写新目录会被跳过，见第 14 条）到我这边只剩约 200 行且 rustfmt 脏——是 scratch 副本车道里
    的 `spec-integrity`（“manifest says sha256:65e71040…, the file is sha256:934d4cb5…”）与 `fmt` 把它抓出来的，
    从作者自己的副本拷回后才 16/16。做法：每一步合并后**先比 `spec/manifest.toml` 里钉的哈希**（或至少比行数），
    worker 的 scratch 树在核对通过前不要清理。

16. **给计数门写的装置，一行只放一个 `name=value`**。`kiln` 的 `harness::measurements` 是严格的：一行里写
    两个（`open_tasks=2 open_handles=0 …`）会被判成“不是数字”并让门 **blocked**（4.17 的作者第一次
    `selftest` 就撞上）。五个计数就写五行；最后的 `cycles=`/`refused=` 同理各占一行。

17. **变异门抽的是“本步补丁写下的行”**（2026-10 修）。`mutants` 的候选位点现在是
    `sources::production_lines`（每个被补丁碰过的 `src/**.rs`，取补丁**新增/改写**的新行号），不再是整个文件：
    一个文件里更早或更晚的步骤写的行不属于本步，本步的规格没有义务覆盖它。修前的四次实测（4.10 的
    `MAX_WINDOWS` 被 4.12 判、4.11 的 `render_plan` 被 4.10 判、4.14 被迫碰 `cli/mod.rs` 后抽到 4.10 的行、
    4.12 在尾巴树上掉到 66%）写在 §5；修之后**每个变异步的抽样都会变**，所以修这一类必须配一次全量重测
    （lakelet 全部 + jobq 3.20–3.27）。副作用是好的：把镜头对准本步自己的行之后，4.14 与 4.17 各自暴露了
    真缺口，已补用例修掉。**同时把 `MUTANT_CAP` 从 6 提到 12、`ATTEMPT_CAP` 提到 3×**：抽样收窄之后
    每个存活位点更重，而本步自己的管道里总有几行任何测试都看不见（`OpenOptions` 的一个布尔旗标、
    只决定仪器自身内存的 `Weak` retain、两个分支在相等时答案一样的比较）——实测：4.17 抽到 8 个里
    3 个这种位点，读到 50%。12 个之后同样三个只占四分之一。代价：每个步骤的走查约慢一倍（15–20 s → 30–40 s）。
    **锚定**：位点按**文本**在当下的树里找，不按行号——行号是“写补丁时那棵树”里的行号，后来者一改同一个
    文件，行号指向的就是别人的代码（实测：尾巴树上 4.9 的走查在 `compaction/mod.rs` 上变异了两行
    4.14–4.18 移动过的代码，因为它**不是自己写的**而红）。`Written { line, text }`：行号给读者，文本给走查。
    **例外只有一个**：4.17 的 `slow:mutants@70`——它**只有 8 个可判位点**，其中两个经实测任何测试都看不见
    （`fsync` 与 `F_FULLFSYNC` 在 `O_RDONLY` 描述符上都成功），所以 6/8 = 75% 就是它的天花板；
    阈值写在元数据里、理由写在日志里。**第二次实证（4.19）**：抽样收窄到本步自己的行之后，本步的
    “管道”里仍有几行没有用例看得见——4.19 抽到 drill 的旗标循环两行与两处守卫，作者补了第 19 条用例
    （drill 的路径处理）才把 12 个位点全杀。教训：**“本步写下的每一行都要有一条用例抵达”不是自动成立的**；
    抽样存活时先问“谁该看见它”，再谈噪声。

18. **页面上那些门判不了的数字，要有一个可复跑的确定性块**（4.16 立、4.19 收）。4.16 的 `docs/profile.md` 与
    4.19 的 `docs/dr-drill.md` 都带一段“命令 + 逐行输出”，规格**重跑那条命令并逐行比对**——页面上的数字是量出来的，
    让它保持真实的唯一办法是让产出它的命令被判（4.19 实测：把页面上 `copy_bytes=569` 改成 `570`，
    `the_page_carries_the_drill_the_command_prints` 立刻红）。**墙钟不进那块**：RTO/耗时写在正文里、机器名写清楚，
    需要打印时放在环境变量后面（4.19 的 `LAKELET_DRILL_TIMED=1`），否则同一台机器两次运行就会给出不同字节，
    而 `slow:sim@N` 的重放会因此失败。

19. **scaffold 的桩有两种形状，第二种是 `const fn` 逼出来的**（4.20 实测，4.12 也撞过）。普通方法用
    `todo!("lakelet/<step>: …")`；**`const fn` 里的 `todo!()` 是编译错误**（`E0015`：带格式化的 panic 不是
    const 上下文允许的），所以那些方法用单字面量的 `panic!("lakelet/<step>: …")`，行为一样是运行时失败、
    能过 `no-todo`门，但形状不同。**推论**：如果一个 `const`（如 4.20 的 `Usage::DEFAULT`）调用了某个函数，
    那个函数不能是桩——否则 scaffold 连编译都过不了，而契约要求 scaffold “能编译、跑测试失败”而不是“编译失败”。

20. **`mutants` 与 `conformance` 对“这个套件跑几遍”的意见不一致——同时上两道门的步骤要自己弥补**（4.21 实测）。
    `conformance@N` 会把 step 的 `run.cmd` 按 `conformance.toml` 的每个后端各跑一遍（带 `KILN_BACKEND=<name>`），
    而 `mutants` 只跑**一遍、不带这个变量**——于是“走在非默认后端才被覆盖的那些行”变异门根本走不到，会成批存活。
    4.21 的实测：第一轮 6 个 S3 侧位点全数存活（`if !scheme.eq_ignore_ascii_case("http")`、`class() == Retryable`、
    payload 哈希、读循环）。**当步的做法**：让**每一次**运行都跑 S3 形状的那一半（进程内端点每次开，`KILN_BACKEND`
    只决定端口用例走哪个实现），于是走查的单次运行也能看到——这是“更多 conformance”而不是更少。
    **框架侧的正解**（未做）：走查应当像 conformance 一样按后端跑，或至少用第一个声明的后端——已记入 §5。

21. **多个 bin 的项目必须用 `default-run` 说出“默认跑哪个”**（4.22 实测并已修：骨架 + 一条用例）。
    `container-repro` 问宿主的那半是 `cargo run --package <包> -- --check-config`；一个有三个 bin
    （`lakeletd`/`lakeletq`/`lakelet-load`）而没有 `default-run` 的项目会直接 `exit=101`
    （“could not determine which binary to run”），于是门在“宿主答不出”上变红/阻塞——**与镜像无关**。
    修法（已落地）：骨架的 `[package]` 里写 `default-run = "lakeletd"`（守护进程才是部署启动的那个），
    并让规格用一条用例钉住“默认二进制就是镜像入口点跑的那个”；验证：重建 base22 后
    `cargo run --quiet --package lakelet -- --check-config` → 配置报告 + `exit=0`。以后的 track 同理。
