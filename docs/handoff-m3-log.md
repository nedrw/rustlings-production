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
| 3 换掉合并顺序 | 用 `HashMap` 迭代顺序替换一处合并 → `slow:sim` 红 → 回滚绿 | 等 4.6/4.8（`sim@3` 的下一批用户；4.4 会先跑通 `sim` 门） |
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
