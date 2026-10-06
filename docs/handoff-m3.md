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

尚未落地：T4 的 24 步正文/规格/参考解（已交付 4.1–4.6）、探题、验收演练。

## 1. 施工中钉下来的规则（收尾时并入 §3）

四条都是 4.2 那一段实测出来的，下一个人直接照做：

1. **慢门的验证在 scratch 副本里做，不在活的工作树里。** `kiln` 会从子进程环境里清掉 `CARGO_TARGET_DIR`
   （M2 §5.2 的有意行为），所以 `kiln run/solution/check` 的构建落在**工作树的 `app/<track>/target/`**
   （4.2 那次落了 387 MB），而工作树里不允许出现 `target/`（Delta VFS 会卡）。配方：
   `rsync -a --exclude .git --exclude .delta --exclude app --exclude target --exclude .kiln ./ /private/tmp/<name>/`，
   然后在副本里 `kiln run <前置步>` → `kiln solution <前置步>` → `kiln run <本步>` → `kiln solution <本步>` →
   `kiln check <本步> --slow`（都带私有 `XDG_CACHE_HOME`）。**`kiln selftest` 是例外**：它在
   `/var/folders/...` 建砂箱，在工作树里跑安全（但它只跑 must 门，不带 `--slow`）。
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
