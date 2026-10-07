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

尚未落地：T4 的 24 步正文/规格/参考解（已交付 4.1–4.10）、探题、验收演练。

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
- **`mutants@80` 的样本只有 6 个、比值阈值 80%**：5/6 = 83% 只高出阈值 3 个百分点，
  抽样换一位就会翻盘。这是计划 §6 D-1 定下的形状（“12 步、每步最多 6 个”），没改；
  但如果以后再出现“某个步骤因仪器原因卡在 83%”，值得重新讨论样本数（每再多判一个位点约 +1–2 s）。
