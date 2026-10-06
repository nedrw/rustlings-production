# M3 施工日志（分段追加）

> 计划：[m3-plan.md](m3-plan.md)。收尾后压缩成 [handoff-m3.md](handoff-m3.md)。
> 每一段记：做了什么、原始结论行、实测墙钟、下一步。

## 段 1：三条新慢门与它们的共同管线（B0a）

**做了什么**：`kiln/src/gates/{harness,sim,soak,fuzz}.rs`。`harness` 是四个计数门（含 `budget`）
共用的部分：行前缀 `<track>-<kind>`、严格的 `<name>=<value>` 解析、课程侧参数文件读取、
以及“重跑的套件为什么没通过”的那一句话（从 `conformance` 里搬过来）。注册表里 `sim`/`soak`
从 `Pending` 变 `Ready`，新增 `fuzz`；`plan.md` §6/§10 补上三行。

**为什么前缀要带 track**：旧 `budget` 把 `jobq-budget` 硬编码在门里——T4 的 harness 将被迫打印 T3 的
track 名。现在 `budget` 的前缀由 `track.id` 派生，`jobq/3.22` 的 harness 不用改。

**原始证据**：`cargo test -q -p kiln` → **315 passed · 0 failed**；`cargo clippy --workspace --all-targets -- -D warnings` → 干净；
`cargo fmt --all --check` → 干净。真正的回归证据是拿**真课程**的步骤跑一遍：

```
$ /private/tmp/kiln-main-target/debug/kiln selftest jobq/3.22-bench-budget --track jobq
    scaffold ✓ the spec fails as it must (exit code 101: error[E0432]: unresolved imports …)
    solution ✓ reference patch applies
    solution ✓ the spec passes
    gates ✓ 12/12 must green
1/1 step(s) hold the contract …
real 97.97
```

（`slow:bench-budget@10` 读的就是 T3 harness 打印的 `jobq-budget` 行，经新前缀逻辑重走了一遍。）

## 段 2：容器与剖析基建（B0b）

**做了什么**：`kiln images` 按声明引用去重（附“哪些步骤要它”）、末尾一行“declared N: M present”；
`runtime::smoke` 真起一个容器（实测 `✓ containers can start (podman \`alpine:3.20\`, 0.2s)`，整条 `kiln images` 2.51s）；
`doctor` 增 profilers 一节，并且是**跑一下**而不是找一下：`dtrace` 存在（`/usr/sbin/dtrace`）
但非 root 会拒绝，所以 `cargo flamegraph` 被报成“装了但在这台机器上跑不了”（+1 warning，doctor 从 9 变 10）；
`kiln skip --track <id>`（`--all` 也认）——M2 的验收演练要铺 15 步只能按路径序逐个 skip，T4 要铺 24 步。

**一处计划修正**：m3-plan.md §1 早先写“`dtrace` 不存在”，实测是“存在但非 root 拒绝”，计划文件已改。

## 段 3：T4 的元数据与骨架（B0c）

**做了什么**：`course/tracks/lakelet/track.toml`（24 步：29 个槽位、16 teach + 1 transfer + 7 deepen、
门阶梯、exit 12 项），`course/tracks/lakelet/skeleton/**`（`Cargo.toml` + `src/{lib,error}.rs` +
`src/protocol/{mod,frame,batch}.rs` + `src/engine/{mod,spool}.rs` + `src/service/{mod,config,logging,ingest}.rs` +
`src/bin/{lakeletd,lakelet-load}.rs` + `tests/support/{mod,service}.rs`）。

**骨架的验收方式**（M2 §3.5 的坑：`tests/support/*` 只有被 `mod` 引到时才会编到）：把它拷到
`/private/tmp/skel-check`，加一个 `tests/compile_check.rs` 带 `mod support;`，跑
`cargo build` + `cargo test --no-run` + `cargo fmt`，再把格式化后的文件拷回骨架（骨架现在与
“编得过、fmt 干净的那棵树”逐字节相同）。

**结论行**：

```
$ (in /private/tmp/skel-check) CARGO_TARGET_DIR=/private/tmp/skel-check-target cargo build -q    → 无输出
$ … cargo test -q --no-run                                                                      → 无输出
$ … cargo fmt                                                                                    → 无输出
$ kiln lint-caps → 97 capabilities · 136 slots · 119 ledger positions · 97 authored steps · no issues.
$ kiln probe --check → 41/41
```

**下一步**：`lakelet/4.1-ingest-protocol`（正文 + 只读规格 + 参考解 patch），然后 `selftest lakelet/4.1-ingest-protocol`。

## 段 4：第一个真步骤 `lakelet/4.1-ingest-protocol`

**做了什么**：正文（transfer 模板）、`spec/4.1/ingest_protocol.rs`（12 个用例）、
`solutions/4.1-ingest-protocol.patch`（`proto/frame.rs`、`proto/batch.rs`、`engine/spool.rs`、`service/ingest.rs` 四个实现 + `docs/protocol.md` 一页）、
`spec/manifest.toml`，以及写规格时被逼出来的**三处骨架修复**（见下）。

**原始结论行**：

```
$ kiln selftest lakelet/4.1-ingest-protocol --track lakelet
    scaffold ✓ the spec fails as it must (exit code 101: test a_corrupted_payload_fails_its_checksum… FAILED)
    solution ✓ reference patch applies
    solution ✓ the spec passes
    gates ✓ 9/9 must green
1/1 step(s) hold the contract … · real 7.03
$ kiln lint-caps      → 97 capabilities · 136 slots · 119 ledger positions · 97 authored steps · no issues.
$ kiln i18n status    → structure: 0 error(s) · not authored yet: 23 step body/bodies
$ kiln probe --check  → 41/41
$ kiln links          → 91 page(s) · 473 link(s) · 283 external target(s) · 全解析
$ course 的 200 个 .rs 逐个 rustfmt --check → 0 not formatted
```

**三处骨架修复（都是实测出来的，不是猜的）**：

1. **夹具的日志不能用 channel 读**：规格每 50 ms 轮询一次时，`mpsc::Receiver::recv_timeout` 路径下
   三秒内一条新行都没看到；同一份规格改成 `sleep` 后再取，每一行都在。改成“读线程往 `Arc<Mutex<Vec<String>>>`
   里追加”后两者一致——读日志不能取决于“什么时候看了一眼”。（夹具同时把 daemon 的 **stderr** 也收进来：
   一个 panic 的 daemon 应该在失败报告里看得见。）
2. **颜色只能给终端**：daemon 让 `tracing` 给字段名上色后，`accepted=3` 实际是
   `accepted\x1b[0m\x1b[2m=\x1b[0m3`——人读起来一样，`grep` 与测试读不出来。
   `logging::init` 现在用 `std::io::IsTerminal` 决定，且夹具把日志格式钉成 `text`（JSON 会把同一个字段写成
   `"accepted":3`，那是另一种读法）。
3. **`src/engine/mod.rs` 的文档注释里写了 `crate::service`**，而 3.1 的结构断言读的是**文本**——
   与 handoff-m2 §3 记下的同一个陷阱（T3 作者也踩过）。已改成“the service layer”。

另外 `src/service/config.rs` 少一个 `;`（`clippy::semicolon_if_nothing_returned`，`@3` 起就红了）、
`lakeletd.rs` 的信号处理用 `.expect`（`@2` 起禁止在生产目标用），两者都已在骨架里改掉。

**下一步**：B1（4.2 fuzz / 4.3 ADR / 4.4 WAL 恢复），逐步派子 agent；三条新门在 B1 里各自第一次被真步骤用上。

## 段 5：`lakelet/4.2-parser-fuzz`（B1 的第一步）

**做了什么**：`test/fuzzing@apply` 这一步由子 agent 交付，11 个文件、1203 行新增（提交 `2132813`），
我复核后在它之上补了三处一致性修正（见下）。这一步的机关是**骨架里种的一个 panic**：

- `scaffold/4.2-parser-fuzz.patch`（纯新增：5 个新文件 + `src/protocol/mod.rs` 标记块里插 3 行）：装置
  `tests/fuzz_protocol.rs`（把 `mutate::walk` 的每个输入同时喂给 `frame::decode` 与 `sniff::ranges`，重放
  `tests/corpus/**`，打印 `lakelet-fuzz iterations=<n>` / `corpus_cases=<n>`，踩到 panic 时**先把输入按语料格式
  打出来**再 `resume_unwind`）；语料读取器 `src/protocol/corpus.rs`；`mutate.rs` 的契约 + `todo!()`；以及
  **埋着 bug 的扫描器** `src/protocol/sniff.rs`——它只确认“头里声明的 *payload* 在窗口里”，然后按
  `OVERHEAD + length`（整帧）去 `split_at`，于是“payload 到齐、校验和差 1–4 字节”的 capture 会 panic；
- `solutions/4.2-parser-fuzz.patch`：手写 xorshift64* 的变器（截断 / 翻位 / 改长度字段 / 插垃圾）、一行的修法
  （比帧自己的 `size`）、以及最小化语料条目 `tests/corpus/parser/frame-cut-short-of-its-checksum.txt`（`0000000001`）；
- `spec/4.2/parser_fuzz.rs` 7 个用例 + `fuzz/4.2-parser-fuzz.toml`（3 种子、`iterations_min = 20000`、`corpus_cases = 1`）。

**原始结论行**（本机 rustc 1.98.1，我在自己的构建上重取）：

```
$ CARGO_TARGET_DIR=/private/tmp/kiln-verify-target cargo build -q -p kiln                     → exit 0
$ kiln selftest lakelet/4.2-parser-fuzz --track lakelet
    ✓ lakelet/4.1-ingest-protocol (setup)
    scaffold ✓ the spec fails as it must (exit code 101: test the_same_seed_walks_the_same_inputs … FAILED)
    solution ✓ reference patch applies · solution ✓ the spec passes · gates ✓ 10/10 must green
  1/1 step(s) hold the contract … · real 34.2s
$ kiln lint-caps      → 97 capabilities · 136 slots · 119 ledger positions · 97 authored steps · no issues.
$ kiln links          → 92 page(s) · 473 link(s) · 283 external target(s) 全解析
$ kiln i18n status    → structure: 0 error(s) · not authored yet: 22 step body/bodies
$ kiln probe --check  → 41/41
```

慢门与演练（在 scratch 副本 `/private/tmp/m3-check` 里做，理由见下）：

```
$ kiln check lakelet/4.2-parser-fuzz --slow
    ✓ slow:fuzz@3    3 seed(s) ran, 60000/60000/60000 mutation(s) each, 1 of 1 regression input(s) replayed (the walk took 1s)
    ✓ slow:?coverage@80 194 of 211 line(s) of this step's module are covered (91% ≥ 80%)
  10/10 must gate(s) passed · 1 should gate(s) run · real 16.8s

把 bug 种回去（把参考解的那一行反向打回 app/lakelet）：
$ kiln run lakelet/4.2-parser-fuzz        → test result: FAILED. 5 passed; 2 failed
$ kiln check lakelet/4.2-parser-fuzz --slow
    ✗ slow:fuzz@3  3 of 3 seed(s) did not pass: seed 1 ✗ (exit code 101: panicked at src/protocol/sniff.rs:89:32:), seed 7 ✗ (…), seed 20260101 ✗ (…)
回滚后：
$ kiln run   → test result: ok. 7 passed · $ kiln check --slow → 10/10 must gate(s) passed
```

这条“种回去→红，回滚→绿”就是 plan §9 的**演练第 4 条**（“注入一个未分类的截断帧 → fuzz/spec 红；回滚 → 绿”），
在这一段里先做掉了一次。

**这一段钉下来的四件事**：

1. **语料的落点被推翻了，方向与直觉相反**：`m3-plan.md` §4.3 原稿把“fuzz 找到过的那个输入”钉成规格里的字节常量
   （理由：怕目录方案逼出一个“课程→项目”的复制机制）。实测下来目录方案**不需要**复制机制——骨架本来就要把读取器与
   README 送进项目，*条目*由学员跑搜索时自己写出来；而把出事的形状写进规格（学员必须读的契约）等于把答案摊开，
   与这一步的过关条件（“fuzz 找到过至少一个 panic 并修掉”）冲突。回归网改由**规格把装置走出来的每一个输入都喂给
   扫描器** + “解码器三个答案都要出现”承担（实测：种回 bug 后规格两条用例红）。随之把三处“语料钉在规格里”的措辞
   改掉：`docs/m3-plan.md` §4.3 与开头那行状态注、`docs/plan.md` §6 的 `fuzz@N` 一行、以及
   `kiln/src/gates/fuzz.rs` 的门头/字段文档/报告行（`pinned input(s)` → `regression input(s)`）——
   **闸门现在只认条数（装置报的重放数 vs 课程侧地板），不认语料在哪、也不认它的形状**，别的 track 的 fuzz 步
   可以把语料放在自己挑的地方。
2. **`cargo test -p kiln` 里有两条自 B0c 起就红的快照断言**，是 B0c 的漏项，不是本步引入的：
   `caps::lint::tests::the_repository_course_is_clean` 断言 `authored_steps == 22 + 24 + 27`（现在 97，
   实得 `left: 97, right: 73`）、`info::tests::the_repository_course_loads` 断言 `tracks().len() == 3`（现在 4）。
   已改成 `22 + 24 + 27 + 24` 与 `4`，并把 lakelet 的 `24 步 / app/lakelet / index 4` 断言补进 info 的那条测试。
   **教训**：这两条测试读的是课程树，所以**每一批收尾都要跑 `cargo test -p kiln`，即使那一批没碰 `kiln/**`**。
   （`cargo xtask ci` 会跑到它们，所以收尾那一关本来也会红——但红了半条 track 才发现。）
3. **慢门验证不能在活的工作树里做**：`kiln` 会从子进程环境里清掉 `CARGO_TARGET_DIR`（M2 §5.2 的有意行为，实测过），
   于是 `kiln run/solution/check` 把构建落在**工作树的 `app/lakelet/target/`**（4.2 那次落了 387 MB）。工作树里
   不许出现 `target/`（Delta VFS 会卡），所以慢门验证一律在 scratch 副本里做；`kiln selftest` 是例外，它自己在
   `/var/folders/...` 建沙箱，在工作树里跑是安全的。已写进 [handoff-m3.md](handoff-m3.md) §1。
4. **骨架补了 5 个空的 `kiln:scaffold:` 块**（`src/lib.rs`、`src/{protocol,engine,service}/mod.rs`、
   `tests/support/mod.rs`）：`kiln` 拒绝在标记区之外插入，而 lakelet 骨架原来一个块都没有——这一轨后面 22 步
   每次新增模块都会撞同一堵墙（4.4 的 `tests/support/tier.rs`、4.10 的 `src/cli/**` 都要）。骨架仍然
   `cargo build` + `cargo test --no-run` + `cargo fmt` 后逐字节不变（4.1 的 selftest 作为 setup 复跑过）。

**下一步**：B1 的后两步——`4.3-write-path-adr`（`store/wal-recovery@aware`：fsync 策略作为数据 + ADR + 崩溃点表）
与 `4.4-wal-recovery`（`store/wal-recovery@apply`：torn-write 恢复 + `slow:sim@3`，turmoil 第一次上场），
然后给开发者一份 B1 检查点报告。

## 段 6：为 4.4 提前做的 `sim` 探针（turmoil 0.7 的种子能力）

**为什么先做**：`slow:sim@3` 的判据里有一条是“至少两个种子给出**不同**的 transcript”。4.4 是这条门第一次
被真步骤用上，而“turmoil 的调度真的会随种子变吗”是设计的前提——如果它不变，4.4 的场景就只能靠**场景自己**
的随机（自研 RNG）来造多样性，写法完全不同。所以先拿一个一次性探针在 `/private/tmp/simprobe` 里测掉
（不是课程内容，不进仓库）。

**探针是什么**：4.4 场景的缩影——一个 store 宿主在模拟网络上应答，3 个 writer 各自往返 4 次，
transcript 记下每个事件的观察顺序与**模拟**时钟；用 `turmoil::Builder::new().rng_seed(seed).enable_random_order()`
建场景，同一场景跑两遍（同种子）与三个种子各跑一遍。

**原始结论行**（`/private/tmp/simprobe`，`cargo test -q --offline -- --nocapture`，0.02 s）：

```
same seed: 94288898bf44b93b == 94288898bf44b93b
--- transcript under seed 1 (first run) ---
store 100
store 200
writer2 200 acked 200 at 28ms
writer1 100 acked 100 at 31ms
…（共 24 行）
seed 1 vs seed 7: 24 vs 24 line(s), first difference at Some(1)
  seed 1: store 200
  seed 7: store 0
digests: [94288898bf44b93b, 84a6e3eaa0081a1d, 6c741d597ac261f3] · distinct: 3
```

三个种子给出三个不同的 transcript（≥2 满足），同一个种子逐字节重放（transcript 的**顺序**就是种子决定的）。

**探针测出来的四件事（写进 4.4 的作业书）**：

1. **种子入口是 `Builder::rng_seed(u64)`**（`turmoil 0.7.2`，`src/builder.rs`）：它同时决定模拟网络的消息
   延迟（默认 0–100 ms 的指数分布）与**tokio 运行时的任务顺序**（`src/rt.rs` 把同一个种子交给
   `tokio::runtime::RngSeed`）；`enable_random_order()` 再把“多节点按随机顺序推进”打开。两者都要，否则
   transcript 可能对种子不敏感。
2. **用 `turmoil::net::{TcpListener, TcpStream}`，不要用 `tokio::net::*`**：用 tokio 的类型必须先
   `enable_tokio_io()`，而那之后就落到真地址上——探针实测 `10.0.0.2:7000` → `AddrNotAvailable`、
   `0.0.0.0:7000` → `AddrInUse`。turmoil 的写法是宿主 `bind("0.0.0.0:PORT")`、客户端按**宿主名**连
   （`TcpStream::connect(("store", 7000))`），名字由模拟 DNS 解析。
3. **transcript 只能用“模拟量”拼**：`tokio::time::Instant` 的 `Debug`/`Display` 里带着**真实**单调钟的基准，
   所以 `format!("{now:?}")` 让同一个种子产出两份不同的 transcript（实测
   `14487320894099136627` vs `17704257869642287446`）；改成 `Instant::now() - started`（模拟的 `Duration`）
   之后同种子逐字节一致。**这正是 `slow:sim@3` 的“同种子重放”要抓的东西**，4.4 的 TRAP 该点名它。
4. **装置要打印 `<track>-sim …` 行**（`lakelet-sim flush segment=7 records=128`）：`kiln` 的 transcript 就是
   这些行**按顺序**（`harness::transcript` 按前缀筛、去掉尾部空行），digest 是它的前 8 个十六进制字符。
   顺序就是判据，别排序、别只打印汇总数。

**下一步**：4.3 交付后按这份契约写 4.4 的作业书（`slow:sim@3` + `slow:mutants@80` + `slow:?coverage@80`）。

## 段 7：`lakelet/4.3-write-path-adr`（B1 的第二步）

**做了什么**：`store/wal-recovery@aware` 这一步由子 agent 交付（提交 `167c1f0`），我复核后把它退回去
修了一轮（提交 `cc75a0c`），两轮一共 6 个文件、约 1800 行新增。交付物：`src/engine/wal/policy.rs`
（`Policy`/`Losses`/`ALL`/`DEFAULT`/`name`/`describe`/`parse`）、`src/engine/spool/durable.rs`（scaffold 新建的
契约文件：六个方法签名 + `todo!()`）、`docs/adr/0001-write-path.md`、`docs/crash-points.md`、正文（teach 模板）
与 14 个用例的规格；4.1 的 `Spool` 因此获得“可持久字节数”这个概念（`durable_offset`）。

**这一步的核心是实测出来的数字，不是口味**：

```
$ mkdir -p /private/tmp/fsync-probe && cd /private/tmp/fsync-probe   # ADR 里贴了完整程序
$ CARGO_TARGET_DIR=/private/tmp/fsync-probe-target cargo run -q --release
append + sync_all, one record at a time: n=2000 mean=4173us p50=4075us p95=5045us p99=5369us
append + sync_data, one record at a time: n=2000 mean=4152us p50=4074us p95=4980us p99=5132us
append + fsync, one record at a time: n=2000 mean=52us p50=45us p95=86us p99=158us
append 20 records, then one sync_all: n=100 mean=5051us p50=5041us p95=6932us p99=7080us
```

一次持久写 **4.173 ms** → `per-record` 的天花板约 **240 条/s**（这不是遥测存储）；裸 `fsync(2)` **52 µs**——
差八十倍，所以“量了 `fsync` 就说这是刷盘成本”会给出一个错方向的数。区间按 ADR 里写明的规则推出
（“设备最多占墙钟的十分之一” → `I ≥ 41.7 ms` → **50 ms**），`Policy::DEFAULT = interval(50ms)`。
子 agent 把 `sync_all == F_FULLFSYNC` 标成“推断”，我读了工具链源码把它变成引用：`rustc 1.98.1` 的
`library/std/src/sys/fs/unix.rs` 里 `os_fsync` 与 `os_datasync` 在 `#[cfg(target_vendor = "apple")]` 下
**都是** `libc::fcntl(fd, libc::F_FULLFSYNC)`（1413/1418 与 1427/1432 行）——这同时解释了它报告里
“为什么 `sync_data` 也是 4.15 ms”的疑点。`docs/crash-points.md` 六条（进程死 / 机器死 / 撕裂写 /
目录项未落 / flush 失败 / 干净停机其实不 flush），每条都写“设备上有什么、恢复看到什么、哪个策略丢什么、
运维看到什么”——这就是 4.4 的地图。

**我重取的验收行**（本机 rustc 1.98.1，自己的构建）：

```
$ kiln selftest lakelet/4.1-ingest-protocol --track lakelet → scaffold ✓ / solution ✓ / gates 9/9 · hold the contract
$ kiln selftest lakelet/4.2-parser-fuzz --track lakelet      → scaffold ✓ / solution ✓ / gates 10/10
$ kiln selftest lakelet/4.3-write-path-adr --track lakelet   → scaffold ✓（test …FAILED，不是编译错误）/ solution ✓ / gates 9/9
$ kiln lint-caps    → no issues.（97 · 136 · 119 · 97；R5 检查 76/97 篇）
$ kiln links        → 93 page(s) · 475 link(s) 全解析
$ kiln i18n status  → structure: 0 error(s)
$ kiln probe --check → 41/41
$ cargo test -q -p kiln → 317 + 21 + 1 passed
在 scratch 副本里走学员流程：
$ kiln run 4.3（在**解完的树**上重跑）→ note: …durable.rs is yours, so it was left untouched ×4；
                                              test result: ok. 14 passed; 0 failed
$ kiln check 4.3 → 9/9 must gate(s) passed（links: 9 link(s) in 4 page(s)）
只铺 scaffold（不铺参考解）的副本：cargo test --no-run → exit 0（无 E0599）；
                                     cargo test --test spec_write_path → FAILED. 1 passed; 13 failed
```

**退回修的一件事，以及它逼出来的一条规则**：第一轮交付的 scaffold **没有声明规格调用的六个 `Spool`
方法**，于是 scaffold 态是编译红（`E0599`）。子 agent 引 `t2-relay.md:72` 的先例接受了它，但
`docs/authoring.md` §5.3/§5.4 写的是“标记块由骨架写定、签名由 scaffold 决定”——真正的缺口是**骨架的
`spool.rs` 从来没拿到标记块**（4.2 补过五个同类文件，漏了它），而它正是这条 track 后面每一步都要改的文件。
修的过程中又发现一条**框架级的**事实（已写进 `docs/authoring.md` §5 新增的规则 6）：**scaffold 插入的行
必须一直是学员树里的那几行**——`kiln` 靠这个判断“已应用”，所以“签名由课程给、语义由学员写”的桩
**不能靠插入**：学员就地改写过的 `todo!()` 体不再是那几行，退回按上下文找 hunk 时插入块又把
`begin`/`end` 之间的上下文挪了位，于是**在解完的树上重跑 `kiln run` 会报课程侧的补丁错误并责怪学员**。
实测两轮（`patch does not apply to src/engine/spool.rs: hunk 1 does not fit`）之后，形状改成
“scaffold 新建 `src/engine/spool/durable.rs` 放六个签名 + `todo!()`，插入只留不会变的一行 `mod durable;`”，
重跑就是上面那条 `left untouched` + 14 passed。这也是 4.2 的 `mutate.rs` 早就用对的形状，只是当时没有
把理由写下来。

**留下一件给 4.4 的决定**（不阻塞）：`bin/lakeletd.rs` 的文档注释写着停机时“stop accepting, **flush**,
log `draining`”，但代码里没有任何地方 flush——`Never`/`interval` 成为默认之后这句话从空转变为假承诺。
子 agent 没动 4.1 的文件，把它记成了崩溃点清单的第 6 条（“干净停机其实不 flush”）。4.4 定“acknowledged
= `durable_offset`”时顺手把它决定掉：要么停机真 flush，要么页面写清停机与机器死的区别。

**下一步**：`lakelet/4.4-wal-recovery`（`store/wal-recovery@apply`：校验和 + 部分写 + 重放，
`slow:sim@3` 的第一次真跑，契约见本文件段 6 的探针结论），然后给开发者 B1 检查点报告。

## 段 8：验收演练的进度（边做边记）

plan §9 要六条腿，每条要“原始输出 + 回滚对比”。做不到的都记在这里，做到的记原始行——收尾时直接搬进
[handoff-m3.md](handoff-m3.md) §4，不用回头翻提交。

| 腿 | 判据 | 状态 |
| --- | --- | --- |
| 1 无网快路径 | 预拉镜像 + 私有 `XDG_CACHE_HOME` + `CARGO_NET_OFFLINE=true` → `kiln verify lakelet`（不带 `--slow`）绿 | **部分已做**：整个 T4 依赖集能无网构建（下面那行）；`kiln verify lakelet` 要等 24 步齐 |
| 2 注入泄漏 | `slow:soak` 红并点名计数 → 回滚绿 | 等 4.17（`slow:soak@N` 第一次上场） |
| 3 换掉合并顺序 | 用 `HashMap` 迭代顺序替换一处合并 → `slow:sim` 红 → 回滚绿 | **已做**（段 11，4.6）：写者拿到帧的顺序换成 `HashMap` 迭代 → `✗ slow:sim@3 the same seed produced two different transcripts: seed 1 came out as 5da24ff3 and 45197a26`；回滚 → `✓ … transcripts 4b891aaf/59a2e06b/59d4f669 — seed 1 replayed identically` |
| 4 未分类的截断帧 | 注入 → fuzz/spec 红 → 回滚绿 | **已做**（段 5）：bug 种回去 → `kiln run` `FAILED. 5 passed; 2 failed`、`✗ slow:fuzz@3 3 of 3 seed(s) did not pass … panicked at src/protocol/sniff.rs:89:32`；回滚 → `7 passed` / `10/10 must gate(s) passed` |
| 5 拿掉 4.14 的剪枝 | `bench-budget` 红 → 回滚绿 | 等 4.14 |
| 6 改一个协议向量 / 弄坏 Dockerfile | `spec-integrity` 红 / `container-repro` 红 | 前半每步的 selftest 都在跑；后半等 4.22 的 Dockerfile |

腿 1 的无网那一半，4.3 上实测（冷目标目录 + 全新 `XDG_CACHE_HOME`，只复用已缓存的 registry 源码）：

```
$ CARGO_NET_OFFLINE=true XDG_CACHE_HOME=/private/tmp/xdg-offline43 kiln selftest lakelet/4.3-write-path-adr --track lakelet
    scaffold ✓ the spec fails as it must · solution ✓ the spec passes · gates ✓ 9/9 must green
1/1 step(s) hold the contract … · real 17.95s
```

4.4 起 `turmoil` 进项目（dev-dependency），它的 48 个包在这台机器上也能离线构建（段 6 的探针就是
`--offline` 跑的），所以这条腿在 4.4 之后要重取一次。

**收尾时要补齐的两条命令**（plan §9 的最后两行）：`kiln verify lakelet --slow --json` 要给出
`{passed: 24, failed: 0, blocked: 0, not_authored: 0, clean: true}`，`cargo xtask ci` 要 exit 0 且末行是
`24/24 step(s) hold the contract`。两条都必须从**scratch 副本**里跑（`kiln` 会把项目命令的
`CARGO_TARGET_DIR` 清掉，工作树里不许出现 `target/`，见 handoff-m3 §1 第 1 条）。

腿 1 的依赖面又多测了一个：4.7 要引入的 `zstd`（新依赖，带 C 源码的 `zstd-sys`）在**冷目标目录 + `--offline`**
下 4.25 s 构建并跑通一次压缩往返（9 个 crate 编译，`zstd-sys v2.1.0+zstd.1.5.7` 走 `cc`）：

```
$ cd /private/tmp/zstdprobe && CARGO_TARGET_DIR=/private/tmp/zstdprobe-cold cargo run --offline
   Compiling zstd-sys v2.1.0+zstd.1.5.7 … Compiling zstd v0.13.3 … Finished `dev` profile in 4.25s
zstd round trip: 65536 byte(s) -> 270 byte(s) (0.4% of the original)
```

（`crc32fast` 4.1 已经在用，`turmoil` 见段 6：两个 registry 源都在这台机器的缓存里，所以到 4.7 为止的
依赖集不需要网络。）

## 段 9：`lakelet/4.4-wal-recovery`（B1 的第三步，`slow:sim@3` 第一次真跑）

**做了什么**：`store/wal-recovery@apply` 由子 agent 交付（提交 `f4f066a`，3191 行新增）——**第一次派发因
provider 连接错误失败、没留下任何东西**（worker 是隔离工作树、只在成功时合并），第二次成功。交付物：
记录格式（`LKLT` + 版本头、`[length][payload][seal]`）、`wal::{checksum,segment,replay}.rs`、
`src/engine/spool/recover.rs`（scaffold 新建的契约文件，`Spool::recovered`）、四种死法的 turmoil 装置
`tests/sim_wal.rs` + `sim/4.4-wal-recovery.toml`、14 个用例的规格（含两个穷举扫描）、正文（deepen 模板）、
以及骨架的 `[dev-dependencies]` 标记块与 `Service::restart` 夹具。

**规格的四条不变量**：① 记录自描述且校验和覆盖**长度**（只覆盖载荷的话，被改的长度会把记录末端挪到
seal 还盖得住的地方，恢复就会把别人的字节当成记录交出来）；② 恢复只回放前缀、在第一个无法证明的记录处
**停下**（跳过比短更糟）；③ `durable_offset` 就是承诺的边界，开库即恢复并把拒绝的尾巴切掉，下一次 append
从恢复到的末端继续；④ **每个截断点**（0 字节到整文件）都恢复成记录的前缀——两个扫描用例，一个是
`segment::scan`（20 条记录的全部前缀），一个是 `Spool::open`（一个 store 的全部切点）。

**原始结论行**（我在自己的构建与 scratch 副本上重取）：

```
selftest：4.1 9/9 · 4.2 10/10 · 4.3 9/9 · 4.4 10/10 must green（scaffold ✓ / solution ✓ / hold the contract）
check 4.4 --slow：✓ slow:sim@3  3 seed(s) ran, transcripts 1fcbf0d8/9938dcf4/a4486a02 — seed 1 replayed identically (2s)
                  ✓ slow:mutants@80 5/6 mutant(s) killed (83%, needs ≥80%) (10s)
                  ✓ slow:?coverage@80 142 of 151 line(s) (94% ≥ 80%)
                  11/11 must gate(s) passed · 1 should gate(s) run · real 44.5s
                  （这一行是撤掉 `links` 之前取的；撤掉后 selftest 是 `10/10 must green`，见下面第 3 条）
sim（app/lakelet 里）：seed 1 跑两遍 → 54 行 `lakelet-sim` 逐字节相同；seed 7 → 第 1 行起就不同
  lakelet-sim recovered kind=process records=12 bytes=593 discarded=0 stop=end
  lakelet-sim recovered kind=torn    records=13 bytes=642 discarded=24 stop=torn
  lakelet-sim recovered kind=rewrite records=14 bytes=691 discarded=49 stop=corrupt
  lakelet-sim recovered kind=machine records=16 bytes=789 discarded=0 stop=end
  lakelet-sim done records=17 flushes=7 deaths=4
演练（把“停下”改成“跳过”：信任长度继续走）：
  spec → FAILED. 12 passed; 2 failed（点名 a_flipped_byte_ends_the_replay_at_the_record_it_changed 与
         a_record_the_replay_cannot_prove_is_not_skipped_over）
  sim  → 红：`restart` was refused: kind=rewrite: … the replay says it proved every record (0 discarded, `end`)
  回滚 → spec 14 passed；sim 1 passed；check --slow 回到全绿
全局：lint-caps no issues · links 94 page(s)/476 link(s) 全解析 · i18n 0 error · probe 41/41 ·
      cargo test -p kiln 317+21+1 · clippy --workspace --all-targets -D warnings exit 0 · 工作树无 target/
```

**这一段钉下来的五件事**：

1. **4.3 的两个开口都按“让承诺变真”收口**：停机路径真的 flush（任何策略）并记
   `flushed the store written=… durable=…`，开库记 `store recovered records=… bytes=… discarded=…`；
   `docs/crash-points.md` 第 6 条（原先写“干净停机其实不 flush”）被改写，而不是留一个没人关的窗口。
   实现放在 `Spool::recovered()`（引擎侧）而不是引擎的日志器，4.1 的“引擎不碰服务层”边界保持成立。
2. **四种死法，第四个是实测逼出来的**：只有截断类死法（进程/机器/撕裂）时，“跳过无法证明的记录”这个
   缺陷对 `slow:sim@3` **不可见**（子 agent 实测：spec 红、三个种子全绿）——因为截断总是让扫描在
   “长度/载荷不全”处就停下（`Stop::Torn`），根本走不到 seal 检查。加上 `rewrite`（整写落盘但一个字节
   变了）之后三个种子都会红。我复核了这个方向：那种“跳过”注入确实同时点亮 spec 两条用例与 sim。
3. **梯子回到计划**：作业书里我把 4.4 的门写成“九个快 + 三个慢、期望 12/12”，子 agent 因此补了 `links`。
   但 `m3-plan.md` §6 明写“`links` 只写在真交页面的步骤上”，4.4 不在名单里——它不交新页面，只是**编辑**
   4.3 的 `crash-points.md`。那页的链接由 4.3 的门在**最终树**上看着（实测：4.3 的 `links` 在 4.4 铺完后
   报 `12 link(s) in 4 page(s)`，此前是 9），所以把 `links` 从 4.4 的清单里撤掉、selftest 回到
   `10/10 must green`（提交 `c01cdc2`）。**教训是我的作业书要对着计划核门数，不能凭上一步的清单推**。
4. **一个只有课程作者才会踩的坑**：`kiln run <step>` 不会覆盖学员已有的文件，所以课程自己改装置
   （`tests/sim_wal.rs` 这类 scaffold 送进项目的文件）之后，老 `app/<track>/` 里那份**旧的**还在，门判的是
   旧装置。子 agent 实测：digests 从 `5dd009a0/eddf61b3/fd32e3d5` 变成 `1fcbf0d8/9938dcf4/a4486a02`，
   中间只删了 `app/lakelet/tests/sim_wal.rs` 再 `kiln run`。已写进 handoff-m3 §1。
5. **一个不可杀的变异体**：`src/engine/spool.rs:125` 的区间判断（`>=` → `>`）只有在 `elapsed() == millis`
   恰好相等时才不同，真实时钟上做不出来（4.3 的规格本来就拒绝断言“区间什么时候 flush”，authoring §6.4
   禁止靠 sleep 同步）。83% 过线，记在提交正文里，没有用 flaky 用例去凑。

**CI 预算的第一个实测数**：4.4 的 `--slow` 全程 **44.5 s**（mutants 10 s、sim 2 s，其余是 coverage 的插桩
构建）。计划里按 T3 外推的“mutants 60–90 s/步、12 步 12–18 分钟”因此偏悲观——`kiln` 自己实现变异、每步
最多判 6 个、复用一份副本，12 步的实测预估 ≈ 2 分钟。§6 的那行已按实测改。

**下一步**：B1 到此结束（4.1–4.4）。给开发者 B1 检查点报告；随后 B2 从 `4.5-batch-writes` 开始
（`io/batching@design`、`slow:bench-budget@10` 第一次上场，车道是计数不是时间：`<track>-budget <name>=<value>`
+ 课程侧 `budget/4.5-batch-writes.toml` 的 `[metric]` 基线，`_max` 结尾的只许变小）。

## 段 10：`lakelet/4.5-batch-writes`（B2 的第一步，`bench-budget` 第一次上场）

**做了什么**：`io/batching@design` 由子 agent 交付（提交 `68983ed`，1853 行新增）：12 个用例的规格、正文
（teach 模板，173 行）、`budget/4.5-batch-writes.toml`（41 行）、scaffold（新建 `flush/{mod,batcher}.rs`、
`spool/batch.rs`、课程装置 `tests/budget.rs`、`docs/write-amplification.md` 模板）与参考解。**骨架没动**
（两个插入点 `src/engine/mod.rs`、`src/engine/spool.rs` 的标记块都已经在）。

**这一步的四条不变量**：① 批量改的是**调用次数**，不是字节也不是顺序——N 个载荷写成 N 条记录、一次 open
一次 write，`records()`/`scan`/`recover` 看到的与 N 次单独 append 逐字节相同；② **确认不得越过写**：守护
进程把同一轮里到的帧一起写，但只有写完之后才答复（4.1 的承诺）；③ 计数器是这一课本身，且**可观测**；
④ 写放大是个数字（`bytes_per_payload`），**这一步没有任何判据是时间**（M2 §1.1）。

**计数器的契约**（`flush::Stats`，`Spool::stats()` 读；只有 `pub(crate) charge`/`charge_sync` 能改，
调用方无法虚报）：

| 存取器 | 计什么 |
| --- | --- |
| `records()` | 这条写路径写过的记录数 |
| `bytes()` | 写下去的字节（含 framing 与首记录的 5 字节头） |
| `calls()` | **每次 open 与每次 write 各算一次**（一个批量 = 2） |
| `syncs()` | 设备**答应了**的刷盘次数 |
| `batches()` | 真写了东西的批次（空批次不算） |

守护进程停机时多一行 `store write cost records=… bytes=… batches=… calls=… syncs=…`（打印在停机自己那次
flush **之前**，所以 `syncs` 是 append 要求的次数）。

**原始结论行**（我在自己的构建与 scratch 副本上重取）：

```
selftest：4.1 9/9 · 4.2 10/10 · 4.3 9/9 · 4.4 10/10 · 4.5 10/10 must green（五份都 hold the contract）
check 4.5 --slow：✓ slow:bench-budget@10 11 metric(s) judged against the course baseline
                    (8 within ±10%, 3 at or below an upper bound): records_64 64, calls_64 2,
                    bytes_64 4613, records_128 128, calls_128 2, bytes_128 9221, bytes_per_payload 1.1262,
                    calls_per_record_max 0.0312, calls_ratio_max 1, fsyncs_per_batch 1, appends_calls_max 128
                  10/10 must gate(s) passed · real 11.6s
演练（把批量写改回 4.1 的形状：一条记录一次 open+write，字节与顺序故意不动）：
  ✗ slow:bench-budget@10 4 of 11 metric(s) moved out of budget：calls_64 is 128, 126 of the baseline 2
    (more than ±10%)；calls_128 is 256, 254 …；calls_per_record_max is 2 and has to stay ≤ 0.034
    (it is an upper bound)；calls_ratio_max is 2 and has to stay ≤ 1.1 (it is an upper bound)
  **9/10 must gate(s) passed —— 规格全绿**：等值测试看不见“几条记录几次调用”，这正是这一门存在的理由
回滚 → ✓ 11 metric(s) · 10/10 must gate(s) passed
全局：lint-caps no issues · links 95 page(s) · i18n 0 error · probe 41/41 · cargo test -p kiln 318+21+1
```

**这一段钉下来的四件事**：

1. **`bench-budget` 的通过行说错了规则**（子 agent 报告里点出来的框架缺陷）：三个 `_max` 指标被判的是
   “≤ 上界”，而报告把 11 个指标都说成“within ±10%”。已修（提交 `1ec9258`）：
   `11 metric(s) judged against the course baseline (8 within ±10%, 3 at or below an upper bound)`，并加一条
   用例把两条规则都钉住。**这类“报告说错了判据”比判错本身更坏**——学员会照着报告去改错的东西。
2. **`calls` 的口径**：open 与 write 各计一次（一个批量 = 2）。作业书里“write calls (opens included)”有歧义，
   子 agent 选了“包含 open”并写在页面与表里；我接受这个口径——**把 open 计进去才抓得住“一条记录一次
   open”**（演练里 `calls_per_record_max` 从 0.0312 变成 2 就是这个效果）。
3. **比计划多一个模块**：`src/engine/spool/batch.rs`（计划只列了 `flush/{mod,batcher}.rs`）。理由是规则 6
   （可被改写的桩不能靠插入）+ 一个批量要动的 `len`/`last_sync`/`policy` 是 `spool.rs` 的私有字段；形状与
   4.3/4.4 的 `spool/*.rs` 一致。`flush` 拥有分组与记账，`spool/batch` 拥有那一次调用。
4. **批量的边界 = 一次读到的分片**（`READ_CHUNK`，64 KiB），而不是定时/定量的后台任务：后者会破坏“确认不得
   越过写”（除非调用方阻塞），而那正是 4.6 要处理的有界队列——页面里写明了这个交棒。

另外记一条不算问题的事实：scaffold 态会带三个 `dead_code` 警告（`todo!()` 体意味着没人调用
`append_batch`/`charge`/`charge_sync`）。`selftest` 只在解完的树上跑门，所以无害；不要靠加 `#[allow]` 去
“修”它（那是把 `clippy@4` 的下一条规则拆掉）。

**下一步**：`4.6-ingest-backpressure`（`async/backpressure@design`：`Block`/`DropNewest`/`DropOldest`/`Sample`
四种策略各带丢弃计数 + `slow:sim@3` 三种子；“谁被丢掉”是依赖调度的答案，也是 plan §9 演练第 3 条的落点）。

## 段 11：`lakelet/4.6-ingest-backpressure`（B2 的第二步，plan §9 演练第 3 条在这里完成）

**做了什么**：`async/backpressure@design` 由子 agent 交付（提交 `3aa7272`，3234 行新增）：13 个用例的规格、
正文（deepen 模板，264 行）、`service/backpressure.rs`（有界队列 + 四策略 + `Stats`/`Tally`）、骨架
`config.rs` 的两个标记块与 `queue.{capacity,policy}` 四层配置、课程装置 `tests/sim_backpressure.rs` 与
`sim/4.6-ingest-backpressure.toml`。

**两条守恒式就是这一步的骨架**：

```
offered == accepted + dropped     每个被交上来的帧要么归队列、要么被丢
accepted == written + queued      静止时：队列欠着还没写的那些
```

四个策略在边界上做什么（以及每个动哪个计数）：`Block` 等到有位置再收（从不丢，只动 `offered`/`accepted`）；
`DropNewest` 拒收**正在到达**的帧并把它交回去点名；`DropOldest` 收下新帧、丢掉**等得最久**的帧（深度不变，
绝不丢写者已经拿走的）；`Sample` 是**速率**（每 4 个收 1 个）而不是边界行为，收下的那个遇到满则按
`DropOldest` 让位。`Stats { offered, accepted, dropped, written }` 是队列的账，`Tally { written, dropped, bytes }`
是**每个连接**的账，只有队列能在帧有了结局（写掉或丢掉）时记账——包括后来被驱逐的那帧记在当初交上来的那个
连接头上。**“被丢的帧永远不会被确认”**就是这么成立的：4.1 的行 `connection closed accepted=… rejected=…
bytes=…` 在三策略下都还是真的（`accepted = tally.written()`、`rejected = 协议拒绝 + tally.dropped()`）。

**原始结论行**（我在自己的构建与 scratch 副本上重取）：

```
selftest：4.1 9/9 · 4.2 10/10 · 4.3 9/9 · 4.4 10/10 · 4.5 10/10 · 4.6 9/9 must green（六份都 hold the contract）
check 4.6 --slow：✓ slow:sim@3  3 seed(s) ran, transcripts 4b891aaf/59a2e06b/59d4f669 — seed 1 replayed identically (2s)
                 9/9 must gate(s) passed · real 12.7s
全局：lint-caps no issues（R5 检查 79/97 篇）· links 96 page(s) · i18n 0 error · probe 41/41 · cargo test -p kiln 318+21+1
```

**演练第 3 条（换掉合并顺序 → `slow:sim` 红 → 回滚绿），我亲手跑了一遍**：把写者拿到帧的顺序从队列的 FIFO
换成一个 `HashMap` 的迭代顺序（`take()` 里先把 `waiting` 收进 `HashMap<usize, Waiting>` 再遍历）：

```
✗ slow:sim@3  the same seed produced two different transcripts: seed 1 came out as 5da24ff3 and 45197a26 —
              a scenario replays exactly, and this one does not. A map's iteration order, a wall-clock read or a
              task spawned without a fixed order is the usual cause; the fix is to make the observed order an
              output of the scenario, not of the machine
8/9 must gate(s) passed · 9/9 after the rollback（transcripts 回到 4b891aaf/59a2e06b/59d4f669）
```

子 agent 的同一注入还测到规格**也会**注意到（六次跑给出的失败用例数在 1–5 之间浮动——注入的缺陷本身就是
不确定的，所以这不算规格 flake），并额外跑了 10 个种子（2、3、5、11、13、42、99、1000、7777777、20260102）
全绿作为鲁棒性证据。

**这一段钉下来的四件事**：

1. **演练账本（段 8）的腿 3 完成**——判据是“重放半部红”，门自己的报错文本已经把 `HashMap` 迭代顺序点名为
   常见原因之一（这条文本是 4.4 写 `sim` 门时留下的，这次第一次真用上）。
2. **参考解不许改写 scaffold 插入的行**（新的一课，已写进 `authoring.md` §5 规则 6）：4.6 的第一版参考解把
   插入的 `super::backpressure::Policy` 缩写成 `Policy` 并加了一条 `use`，于是 scaffold 在**解完的树上**
   不再被认作已应用（`patch does not apply to src/service/config.rs: hunk 1 does not fit`）。插入的行只能选
   “以后谁都不会想改”的写法（全路径就是这类）。这与规则 6 的桩那半条是同一个机制，现在两半都有实测例子。
3. **队列的单位是帧，不是样本**：4.1 的承诺是逐字节的（三帧进、三记录出），而 4.5 已经让帧成为写入与批量的
   单位；一个 `Record` 队列得重新编码多样本帧，字节就不再可复现。代价是“丢一帧 = 丢它带的样本”，页面与
   `Stats` 的文档都写明。
4. **`Sample` 是速率而不是边界策略**：只在满时采样等于 `DropNewest` 加戏，而“收下的那个遇到满再拒收”就不是
   “每 N 个收 1 个”了；速率让守护进程那一层的数字与调度无关（8 帧、容量 2 → 每台机器都是 `accepted=2
   dropped=6`），这也是那两条守护进程用例能确定的原因。

**下一步**：`4.7-columnar-segments`（teach，`store/columnar@apply`：先往返、再判体积 `bytes_per_sample_max`；
`unsafe-budget@0` 要求项目 manifest 里 `unsafe_code = "deny"`；`zstd` 冷构建离线 4.25 s 已测）。
