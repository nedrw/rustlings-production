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
| 2 注入泄漏 | `slow:soak` 红并点名计数 → 回滚绿 | **已做**（段 22，4.17）：在 `ingest::serve` 注入“句柄不放”的泄漏 → `✗ slow:soak@4000 the soak did not finish: exit code 101: thread 'the_counters_hold_their_place_over_a_long_run' … panicked at tests/soak.rs:356:9: (after 149s)`、`7/11 must gate(s) passed`；回滚 → `✓ slow:soak@4000 4000 cycle(s) in 142s, 5 live counter(s) inside their bounds: open_handles 0, open_tasks 2, refused 0, store_bytes 2743, store_files 13`、`11/11 must`（作者与我各自跑了一遍） |
| 3 换掉合并顺序 | 用 `HashMap` 迭代顺序替换一处合并 → `slow:sim` 红 → 回滚绿 | **已做**（段 11，4.6）：写者拿到帧的顺序换成 `HashMap` 迭代 → `✗ slow:sim@3 the same seed produced two different transcripts: seed 1 came out as 5da24ff3 and 45197a26`；回滚 → `✓ … transcripts 4b891aaf/59a2e06b/59d4f669 — seed 1 replayed identically` |
| 4 未分类的截断帧 | 注入 → fuzz/spec 红 → 回滚绿 | **已做**（段 5）：bug 种回去 → `kiln run` `FAILED. 5 passed; 2 failed`、`✗ slow:fuzz@3 3 of 3 seed(s) did not pass … panicked at src/protocol/sniff.rs:89:32`；回滚 → `7 passed` / `10/10 must gate(s) passed` |
| 5 拿掉 4.14 的剪枝 | `bench-budget` 红 → 回滚绿 | **已做**（段 19，4.14）：把计划的索引查询去掉 → `✗ slow:bench-budget@10 6 of 18 metric(s) moved out of budget … pruning_rate is 50, -45 of the baseline 95`、`7/10 must gate(s) passed`；回滚 → `pruning_rate 95`、`10/10 must gate(s) passed`（作者与我各自跑了一遍，数字逐字一致） |
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

## 段 12：`lakelet/4.7-columnar-segments`（B2 的第三步，体量最大的一步）

**做了什么**：`store/columnar@apply` 由子 agent 交付（提交 `0813393`，2983 行新增）：14 个用例的规格、
正文（teach 模板，176 行）、`segment/{mod,encode,decode,compress}.rs`、`docs/format.md`（scaffold 模板，
学员填、规格读回）、`budget/4.7-columnar-segments.toml`（10 个计数）与课程装置 `tests/budget_segments.rs`；
`unsafe_code = "deny"` 由 **scaffold** 落进 `[lints.rust]` 标记块（规则 6 允许：这行以后谁都不会改）。

**段格式（`docs/format.md` 的原文布局）**：`LKLS` + 版本 + `COLUMNS=3` + 列块长度 + 压缩长度 +
zstd 帧 + 4 字节 seal；列块是 `u64` 记录数 + 三列（**series → timestamp → value** 的固定顺序），
每列 `u8` 标签 + `u32` 长度 + 载荷；标签 `PLAIN=0 / RLE=1 / DELTA=2 / DIC=3`。
每列**先各自选最短的那种编码**（平局取标签小的），再整块做一次 `zstd`（`LEVEL = 3`）。
选编码**不看压缩器**（否则字节就成了 zstd 版本的函数，金向量也就钉不住）。

**两条判据的纪律**：① **列块字节由规格里的金向量钉死**（`GOLDEN: [u8; 123]`，逐区域比对并点名移动的是哪一段），
这是 4.1 给线上协议立的规矩、也是 T4 不带 `semver@baseline` 的理由（plan §7 D-2）；**压缩后的字节不钉**（换一个
zstd 版本可能产出不同字节，那不是学员的错），只判往返与体积。② 陌生字节是**类型化错误**：规格对一份真段做
**全部截断点 + 全部单比特翻转**的扫描，每次都要 `Error::Protocol` 且句子非空（还检查这些句子不是同一句）。

**原始结论行**（我在自己的构建与 scratch 副本上重取）：

```
selftest：4.1 9/9 · 4.2 10/10 · 4.3 9/9 · 4.4 10/10 · 4.5 10/10 · 4.6 9/9 · 4.7 12/12 must green
check 4.7 --slow：✓ unsafe-budget@0  0 `unsafe` occurrence(s) in 31 production file(s), and `unsafe_code` is denied in …/app/lakelet/Cargo.toml
                  ✓ slow:bench-budget@10 10 metric(s) judged against the course baseline (4 within ±10%, 6 at or below an upper bound):
                    samples 1024, series 8, distinct_values 16, samples_read 2048, mismatches_max 0, column_block_bytes_max 3032,
                    segment_bytes_max 170, bytes_per_sample_max 0.166, compression_ratio_max 0.0561, flat_column_block_bytes_max 1099
                  ✓ slow:mutants@80 6/6 mutant(s) killed (100%, needs ≥80%) (6s)
                  12/12 must gate(s) passed · real 31.2s
```

**两条演练（都做了红/回滚/绿）**：

```
(a) 值列按 `==` 分组而不是 `to_bits()`（`-0.0` 吞掉 `0.0`）→ 规格 `13 passed; 1 failed`，失败信息点名
    `the first difference is Some(Record { series: "cpu,host=h0", ts_millis: -9223372036854775808, value: 0.0 })`；
    回滚 → 14 passed
(b) 每列都写成 PLAIN（编码选择被拆掉）→ ✗ slow:bench-budget@10 4 of 10 metric(s) moved out of budget：
    column_block_bytes_max 29719 ≤ 3100、segment_bytes_max 536 ≤ 200、bytes_per_sample_max 0.5234 ≤ 0.2、
    flat_column_block_bytes_max 29719 ≤ 1150；**规格同时红 5 例**（金向量钉住了编码）；
    回滚 → 12/12 must gates passed
```

**这一段钉下来的五件事**：

1. **每个步骤各自的装置**：4.5 的 `tests/budget.rs` 里没有 `kiln:scaffold:` 块，而 scaffold 只能在标记区内插入，
   所以 4.7 只能新建自己的 `tests/budget_segments.rs`（`budget/4.7-columnar-segments.toml` 的 `command` 指向它）。
   这不是权宜：课程拥有的文件若想让后步**追加**指标，就得在建它的时候留一个标记块。已写进 handoff-m3 §1
   第 7 条——“要共享就留块，不留块就各步一份”。
2. **一个“等价变异体”被删掉了**：第一次 `--slow` 是 `5/6 (83%)`，唯一幸存的是 delta 解码器里的 `if count == 0`——
   因为选编码的平局规则永远不会给 delta 列写出零记录，这个分支**不可杀**（杀掉它不改变任何行为）。子 agent 把
   “空时钟列”的判断上提到 `stamps` 的入口（一处代替一个分支），死分支消失、变异不再等价，于是 `6/6 (100%)`。
   这是 `mutants` 门的一条真知识：**门能杀的只是会改变行为的改动**，等价变异体的正解是消除等价性，不是写 flaky 用例。
3. **一个计数上界，来自“绝不 panic”这条不变量**：`decode::records` 在动手分配之前先拒 `count` 大于时钟列能描述的
   记录数（任一种编码下每条记录至少 1 字节）；否则约 30 字节的坏文件可以让库去要十亿条记录的内存——那比 panic 更糟。
4. **压缩相关的边界都留了余量**：`segment_bytes_max 200`（实测 170）与 `compression_ratio_max 0.07`（0.0561）
   随 zstd 版本浮动，所以留 ~18–25% 的松量；列块的计数则是记录的精确函数。
5. **我的作业书又算错了一次门数**（写 11，实际 12）。加上 4.4 那次（写 12，实际 11），这是第二次凭邻近步骤的清单推门数
   ——已写成 handoff-m3 §1 第 8 条：**门数一律对着 `track.toml` 数**，别推导。

**下一步**：`4.8-compaction`（B2 最后一步：合并策略作为数据、读者永远看不到半合并的库、`slow:sim@3` 第三个真用户 +
`bench-budget` 数“每移除一个字节重写了多少”）。

## 段 13：`lakelet/4.8-compaction`（B2 收官；`kiln verify` 第一次真跑）

**做了什么**：`store/compaction@design` 由子 agent 交付（提交 `4a946e0`，4103 行新增）：14 个用例的规格、
正文（teach 模板，222 行）、`compaction/{mod,plan,snapshot}.rs`、课程装置 `tests/{sim,budget}_compaction.rs`、
`budget/4.8-compaction.toml`（13 计数）与 `sim/4.8-compaction.toml`；scaffold 纯加法（0 删除行），
`src/engine/mod.rs` 的标记块里只插了 `pub mod compaction;`。**4.8 不交项目页面**，所以它的门里没有 `links`
——`links` 判的是**项目里的 markdown**，而这一步的三份“文档”是课程页、模块文档与被规格读回的常量。

**这一步的四条不变量**：① **合并计划是值**（`Policy { width, ratio }` + `&[SegmentInfo]` → `Plan`，无 I/O；
规则是“**最旧的**一段**连续**、两两大小都在 `ratio` 倍以内的 `width` 个段”，取且仅取 `width` 个——从不并一个，
也从不并整层）；② **读者看不到半合并**（`Snapshot` 认的是一个 `Arc<Set>`，被替换的文件**退休**到最后一个认它的
读者松手为止）；③ **五个阶段、两个世界**（新段写完+落盘 → manifest 写 `pending`、flush、rename、目录 flush →
旧文件退休；`planned/written/durable` 崩 → 就是合并前的库，`swapped/retired` 崩 → 新的那套；**绝不留缝、绝不重复**）；
④ 重写是个数字（`bytes_rewritten_per_byte_removed_max` 打头，`records_lost_max = 0` 是守恒式）。

**原始结论行**（我在自己的构建与 scratch 副本上重取）：

```
selftest：4.1 9/9 · 4.2 10/10 · 4.3 9/9 · 4.4 10/10 · 4.5 10/10 · 4.6 9/9 · 4.7 12/12 · 4.8 10/10 must green
check 4.8 --slow：✓ slow:sim@3  3 seed(s) ran, transcripts 21de1955/630e6a63/95af3e2e — seed 1 replayed identically (3s)
                  ✓ slow:bench-budget@10 13 metric(s) judged against the course baseline (7 within ±10%, 6 at or below an upper bound):
                    segments_sealed 16, records_sealed 2048, merges 5, segments_after 1, files_removed 15, plan_left_max 0,
                    records_kept 2048, records_lost_max 0, bytes_after_max 5700, bytes_rewritten_max 12029, bytes_removed 2412,
                    bytes_rewritten_per_byte_removed_max 4.9871, refused_max 0
                  10/10 must gate(s) passed · real 9.8s
```

**`kiln verify lakelet` 第一次真跑**（8 步有内容、16 步没有；快车道，24.5 s）：

```
$ kiln verify lakelet
8/8 judged step(s) pass · 0 failed · 0 not prepared · 16 without content
nothing failed yet — but not everything was judged either
$ kiln verify lakelet --json | tail
…,"totals":{"blocked":0,"clean":false,"failed":0,"not_authored":16,"passed":8,"steps":24},"track":"lakelet"}
```

这正是 M3 收尾那条验收命令的形状（届时要是 `{"passed":24,"failed":0,"blocked":0,"not_authored":0,"clean":true}`），
现在把 `not_authored` 从 16 往 0 推的过程有了可重放的读数。

**演练（我只做了核心那条，红/回滚/绿都取了行）**——把“退休”换成**在换 manifest 时立刻删掉旧文件**（合并对读者变成两步可见）：

```
spec → FAILED. 12 passed; 2 failed：点名 a_reader_that_took_a_snapshot_before_the_swap_answers_the_same_after_it
       与 a_merge_keeps_every_record_once_in_the_same_order
sim  → ✗ slow:sim@3 3 of 3 seed(s) did not pass：三个种子都在
       `a_merge_is_a_schedule_and_a_reader_never_sees_half_of_one`（tests/sim_compaction.rs:841）panic
回滚 → spec 14 passed；sim 回到 21de1955/630e6a63/95af3e2e；10/10 must gates passed
```

子 agent 另做了两条：把策略改成“每次合并并掉所有段”→ `✗ slow:bench-budget@10`（`merges is 16 … ≤ 5`、
`bytes_rewritten_per_byte_removed_max is 37.8109 → ≤ 7`）；在 bytes 落盘之前就让合并可见 → 五阶段用例在
`written` 处红（`the manifest says segment 4 is 196 byte(s) and …/0000000004.lks holds 98`），`sim@3` 三种子全红。

**这一段钉下来的四件事**：

1. **manifest 是“整文件 + rename”**，用 4.4 的 `checksum` 封口，逐段记 `(index, bytes, records)`；`pending` →
   flush → rename → 目录 flush 三步的顺序**就是**崩溃点。子 agent 明确拒绝了文本索引（崩在中间会读成一个**更小**
   的库＝一个洞）与“manifest 也是日志”（真实，但那是另一步的事）。
2. **退休给最后一个读者**：在换的时候删会打断跨换的读者（演练 a 实测），永不删则目录无限长；所以 `Set::drop`
   来删。快照持有的是**段集合**而不是记录（否则一次查询要把库拷一份）。
3. **装置里的死法是 `mem::forget` 而不是 drop**（进程死不会跑析构），而且**什么都不放回**：前三个阶段断言
   “索引没被动过”，而不是把 manifest 写回去——后者会把演练 (a) 的那类缺陷**盖住**。
4. **一条环境教训**：在 Delta 工作树里写一个**未跟踪的作者目录**会让工作树的终端停止物化（“the checkout kept
   changing”），子 agent 因此把作者工作挪到了 `/private/tmp/d48` 并把那个目录删掉。已写进 handoff-m3 §1 第 9 条：
   **作者工作一律在 `/private/tmp`，不要在仓库里留未跟踪的暂存目录**。

**下一步**：B2（4.5–4.8）结束，给开发者 B2 检查点报告；随后 B3 从 `4.9-retention` 开始
（“旧数据没了”是两半的断言：比 cutoff 旧的全部不可达、比它新的一个都没被碰；计数说一个保留期回收了多少字节，
测试说哪些段因为读者还握着而删不掉）。

另外，`kiln verify lakelet` 的快车道已经在 8/24 步的树上跑过一次（段 13）：`8/8 judged step(s) pass · 0 failed ·
0 not prepared · 16 without content`，JSON 的 totals 是 `{"blocked":0,"clean":false,"failed":0,"not_authored":16,
"passed":8,"steps":24}`——收尾那条验收（`passed: 24, not_authored: 0, clean: true`）的形状已经验过，剩下的只是把
`not_authored` 推到 0。

## 段 14：B3a — 4.9 `store/retention@apply`（旧数据按桶离开，读者看不到它离开）

**做了什么**：`course/tracks/lakelet/spec/4.9/retention.rs`（1391 行、15 用例）、
`scaffold/4.9-retention.patch`（879 行，`+850/−0`：新建 `src/engine/retention/{mod,partition}.rs` 与
`tests/budget_retention.rs`，只在 `src/engine/mod.rs` 的标记块里插 `pub mod retention;`）、
`solutions/4.9-retention.patch`（918 行，`+507/−52`）、`budget/4.9-retention.toml`（87 行、19 计数）、
`course/docs/steps/en/lakelet/4.9-retention.md`（233 行，teach 模板）。`track.toml` 没动（门数 10，与作业书一致）。

**这一步钉下来的五条不变量**：① **段的年龄来自索引**——manifest 进入 version 2：头部多一个 `floor`、
每条目五个 `u64`（`index, bytes, records, first_millis, last_millis`），版本字节把旧索引变成**拒绝**
而不是误读；② **cutoff 是桶边界不是时刻**（`Policy::cutoff` 用 `Partition` 向下取整，`keep = 0` 表示
全留）；③ **整段过期**（`last < cutoff` 才过期，半开：`last == cutoff` 留），**跨界段保留且绝不重写**，
它低于 cutoff 的记录改为“不再是被答案”，计数 `records_counted 1260` 与 `records_answered 1200` 的差
就是它；④ **floor 持久且单向**（`max(policy.cutoff(now), floor)`，落进 manifest，重开仍在；一次不删
任何东西的 pass **什么都不写**：epoch、字节全同）；⑤ **退休给最后一个读者**（复用 4.8 的 `Set`），
所以“哪些段删不掉”是测试说的，不是报告说的。

**一处契约形状是复审逼出来的**：最初把 `SegmentInfo::new` 从三参扩到五参，那会让 `kiln run
lakelet/4.8-compaction` 同步出的规格在 4.8 自己的树上编译失败（顺序学习的学员 + `xtask ci` 的逐步守约）。
改成 `new` 保留三参（“手工造的、只谈大小的值，没人写下它的年龄”，`range: None`）、新增
`spanning(...)`（`seal`/`merge`/`decode` 都走它），范围是 `Option`，未写下的年龄在 manifest 里写成
`u64::MAX` 两次（一个记录不可能有的毫秒），规则对它**保留且不算跨界**。4.8 的规格与哈希回到原值
（`4a644e44…`），4.9 的哈希重钉（`1e474441…`）。规则已写进 `handoff-m3.md` §1 第 10 条。

**原始结论行**（我在自己的构建与 scratch 副本上重取）：

```
$ kiln selftest --track lakelet                                    # 9/9 step(s) hold the contract（4.1…4.9）
$ kiln selftest lakelet/4.9-retention --track lakelet              # scaffold ✓ E0599 `spanning` / solution ✓ / gates ✓ 10/10 must green
$ kiln check lakelet/4.9-retention --slow                          # real 21.8s
  ✓ slow:bench-budget@10 19 metric(s) judged（13 within ±10%，6 at or below an upper bound）:
    segments_before 24, records_before 2160, bytes_before 9384, segments_expired 10, records_expired 900,
    bytes_reclaimed 3904, straddling 1, segments_after 14, bytes_after 5480, bytes_freed_on_disk 3904,
    bytes_rewritten_max 0, records_counted 1260, records_answered 1200, records_below_cutoff_max 0,
    floor_hours 11, idle_expired_max 0, segments_unaccounted_max 0, bytes_unaccounted_max 0, refused_max 0
  ✓ slow:mutants@80 5/6 mutant(s) killed (83%, needs ≥80%)（walk 12s）
  10/10 must gate(s) passed
$ kiln verify lakelet                                              # real 10.4s → 9/9 judged step(s) pass · 0 failed · 0 not prepared · 15 without content
$ kiln lint-caps → no issues · kiln links → 99 page(s) · 491 link(s) ✓ · kiln i18n status → 0 error(s) · kiln probe --check → 41/41
$ cargo test -q -p kiln → 319 + 21 + 1 passed · cargo clippy --workspace --all-targets → 干净 · cargo fmt --all --check → 干净
```

两条补丁在**纯净的** `through 4.8` 树上重放后与受测树逐字节一致（`diff -r` 只剩 `kiln` 同步出来的
`tests/spec_*.rs`），`budget/4.9-retention.toml` 的 19 个基线与门打印的 19 个数一一相等。

**演练（我按作者的配方重取了一遍）**：把读路径的过滤从 `>= floor` 改成 `>= 0`（即“不按 floor 限制
答案”）——

```
1. RED   $ cargo test -q --test spec_retention   → test result: FAILED. 9 passed; 6 failed
2. CAUGHT $ cargo test -q --test spec_retention what_the_index_stopped_naming
   panicked at tests/spec_retention.rs:748:5: the records a read returns are the records at or after the
   cutoff, once each, in the store's order — and nothing older, whoever can still find it on the disk
3. GREEN 回滚 → test result: ok. 15 passed; 0 failed
```

红的是六条：`what_the_index_stopped_naming…`、`a_straddling_segment_is_kept_and_not_rewritten`、
`a_reader_that_took_its_snapshot_before_the_pass_keeps_its_view`、`the_floor_survives_the_open`、
`the_floor_never_moves_backwards`、`a_reopened_store_adds_segments_without_reusing_an_ordinal`。

**这一段钉下来的四件事**：

1. **早期规格只能通过“早期树上已经存在的东西”接纳后来的契约**（已写进 `handoff-m3.md` §1 第 10 条）。
   判据是每一步的规格必须在自己那棵树上通过：改早期规格的文本，等于同时把红线挪过去。
2. **`slow:mutants@80` 会抽到改不出行为的位点，这是仪器的噪声，不为此改门**（§1 第 11 条、§5）：
   4.9 的 6 个判定位点里有一个是 `Vec::with_capacity(HEADER + len * ENTRY + …)` 的 `+` → `-`，
   手工打进去后 15/15 用例与 19/19 计数全绿。曾实现“容量参数不是位点”的规则并**回滚**：它会重排
   每一步的抽样（jobq 3.20–3.27 + lakelet 十二步），而且前提不严格（容量更大可能溢出 panic）。
   顺手修掉一条**早已在 main 上**的 fmt 脏点（`kiln/src/gates/budget.rs:215`，B2b 带来的）。
3. **抽样换个位点就会露出真缺口**：回滚前的抽样把 4.7 从 6/6 变成 5/6，新位点是
   `src/engine/segment/decode.rs:422` 的 `if shift > 63` → `>=`——这是 4.7 规格**没覆盖的边界**
   （一个需要第十个字节的 varint），不是仪器的噪声。记进 §5，收尾时可补一条用例。
4. **Delta 会跳过未跟踪目录里的文件**：`course/tracks/lakelet/spec/4.9/` 是新目录，第一次合并时
   `spec.9` 文件没到我的工作树（`spec/manifest.toml` 却到了），Delta 当时报的是
   `<skipped-untracked-directory path="course/tracks/lakelet/spec/4.9"/>`。形状与第 9 条同一族：
   **新建目录里的文件要么进 git 的索引、要么由本步的提交带走**，别把它当“文件已经写好了”。

**下一步**：4.10 `query/downsampling@apply`（下采样 + `lakeletq` CLI，窗口复用 4.9 的 `Partition`，
CLI 沿用 jobq/3.15 的退出码与两条流契约），随后 4.11–4.14。plan §9 的演练台账不变：腿 2（4.17 泄漏）、
腿 5（4.14 去掉裁剪）、腿 6 后半（4.22 弄坏 Dockerfile）还没做。

## 段 15：B3b — 4.10 `query/downsampling@apply`（窗口在绝对网格上，答案只答它被问到的范围）

**做了什么**：`spec/4.10/downsampling.rs`（1524 行、17 用例）、`scaffold/4.10-downsampling.patch`
（993 行、**0 删除**：新建 `src/engine/query/{mod,downsample}.rs`、`src/cli/mod.rs`、
`src/bin/lakeletq.rs`、`tests/budget_downsample.rs`，并在 `src/engine/mod.rs` 与 `src/lib.rs`
的标记块里各插一行）、`solutions/4.10-downsampling.patch`（770 行）、
`budget/4.10-downsampling.toml`（83 行、14 计数）、`course/docs/steps/en/lakelet/4.10-downsampling.md`
（277 行、12 条 TRAP）。`track.toml` 未动（门数 10，与作业书一致）。

**这一步钉下来的五条不变量**：① **窗口是绝对时间轴的桶**（`floor(ts / width) * width`，半开，
复用 4.9 的 `Partition`；锚在第一条记录上的窗口会让同一小时的两个查询给出两条阶梯，也让保留期的
桶与查询的桶没法比）；② **桶只数范围内的记录**（首尾桶是部分的，`Bucket::count` 是那个诚实的数字，
绝不用范围外的记录去填）；③ **空窗口缺席而不是零**（“没有上报”与“上报了零”是两个事实）；
④ **`series` 是精确名字，`None` 才是全部**；⑤ **容差是代码里的数字**（`TOLERANCE`/`agree`，本步
所有两数比较都走它，没有一条用例用 `==` 比 `f64`）。CLI 是 jobq/3.15 的契约换一组子命令：
`stdout` 是答案、失败时 `stdout` 为空、0/2/3/4/1 一类一码、`--json` 只是一种格式、不读 stdin；
另加 `MAX_WINDOWS`（范围与窗口之比是调用者的笔误，在开库之前按 2 拒掉）。

**原始结论行**（我在自己的构建与 scratch 副本上重取）：

```
$ kiln selftest --track lakelet                              # 10/10 step(s) hold the contract（4.1…4.10）
$ kiln selftest lakelet/4.10-downsampling --track lakelet     # scaffold ✓（1 passed; 16 failed）/ solution ✓ / gates ✓ 10/10 must green
$ kiln check lakelet/4.10-downsampling --slow                 # 10/10 must gate(s) passed
  ✓ slow:bench-budget@10 14 metric(s) judged（9 within ±10%，5 at or below an upper bound）:
    segments_sealed 4, records 400, series 4, windows 28, buckets_emitted 23, gaps 5,
    invented_windows_max 0, records_scanned 400, series_matched 100, records_in_range 88,
    records_per_bucket_max 4, records_unaccounted_max 0, mean_gap_max 0.030632411067195164, refused_max 0
  ✓ slow:mutants@80 6/6 mutant(s) killed (100%, needs ≥80%)（walk 10s）
$ kiln verify lakelet          → 10/10 judged step(s) pass · 0 failed · 0 not prepared · 14 without content（rc=3 = 还有 14 步没内容）
$ kiln lint-caps → no issues · kiln links → 100 page(s) · 496 link(s) ✓ · kiln i18n status → 83/97 · kiln probe --check → 41/41
$ cargo test -q -p kiln → 320 + 21 + 1 passed · clippy 干净 · cargo fmt --all --check 干净
```

**两条补丁在我的**（从仓库重建的）**`through 4.9` 树上重放后与受测树逐字节一致**，`budget/4.10-downsampling.toml`
的 14 个基线与门打印的 14 个数一一相等。

**演练（我按作者的配方重取了一遍）**：把范围过滤去掉（桶于是数整个窗口，也就是“部分的桶去借邻居”）

```
1. RED    $ cargo test -q --test spec_downsampling → FAILED. 12 passed; 5 failed
          同一棵树上的预算车道也红：buckets_emitted 25（应 23）、invented_windows_max 2
2. CAUGHT a_range_that_starts_and_ends_mid_window_leaves_partial_buckets
          panicked at tests/spec_downsampling.rs:514:5: the first bucket starts on the grid and not at
          the range's edge: a window is a bucket of the time axis
3. GREEN  回滚 → ok. 17 passed; 0 failed
```

**这一段钉下来的三件事**：

1. **基线树必须在派活前重建并核对**：作业书里写的 `/private/tmp/l4b3/base` 其实是 `through 4.8`
   （我在 4.9 落地前建的），子 agent 自己找到了 `through 4.9` 的那一份并使用。我随后**从仓库重建**
   （skeleton + 逐步 scaffold/solution）并 `diff` 核对，确认它的树只多出 `kiln` 生成的
   `Cargo.lock` 与 `tests/spec_*.rs`。以后每步：**先重建基线树、`diff` 核对，再把路径写进作业书**。
2. **一条门消息里的硬编码路径**（框架修复）：`bench-budget` 的 blocked 消息把 harness 写成常量
   ``tests/budget.rs``——那是 jobq/3.22 的文件，T4 的四个车道各有自己的名字（本步是
   `tests/budget_downsample.rs`），于是“被阻塞”的门把读者指向一个项目里不存在的路径。现在路径从
   基线的 `command` 里取 `--test <target>`（`kiln/src/gates/budget.rs`，+81/−10，带单测）。
3. **一个 worker 死了、另一个 pick up 了它的草稿**：上一段的环境故障留下了 `/private/tmp/l4b3/{gen,check2}`
   里的半成品；新的 worker 明确声明了这一点，逐行核对草稿、修掉四处不一致（规格头写“Sixteen cases”、
   装置注释点名一个不存在的计数、正文说“17 条全红”（实际 16 条红，那条结构用例读源码所以过）、一处
   换行）、重生成补丁并重跑全部验收。**继承的散文必须自己再验一遍**：这一步的页内数字我逐条对了预算
   车道的实测值，用例名与规格逐条对齐（17/17）。

## 段 16：B3c — 4.11 `query/engine@design`（受限语法、按类别拒绝、把计划摆出来）

**做了什么**：`spec/4.11/query_engine.rs`（2016 行、18 用例）、
`scaffold/4.11-query-engine.patch`（1166 行、**0 删除**：新建 `query/{series,lex,parse,plan,exec}.rs`
与 `docs/query.md` 的桩、在 `query/mod.rs` 的标记块里插五行）、
`solutions/4.11-query-engine.patch`（1826 行，+1138/−205：五个模块的语义、`cli/mod.rs` 的 `--sql` 一半、
`Snapshot::records_of`、写成的 `docs/query.md`）、`course/docs/steps/en/lakelet/4.11-query-engine.md`
（218 行、12 条 TRAP）。`track.toml` 未动（**10 must + 1 should**：`slow:?coverage@80` 是应该门）。

**这一步钉下来的六条不变量**：① **系列名是读出来的、不是校验出来的**——`Series::of` 是全函数
（指标名＝第一个逗号之前，`k=v` 才是标签，畸形项永不匹配），因为摄取协议接受任意字节；
② **扫描器按位置拒绝**（未闭合的引号指向开引号那一格）；③ **每个拒绝都是一个变体并带着类别**：
文本错 `2`、店里从来没有过的指标 `3`（“安静的机队”不等于“打错的指标”）、低于 floor 的区间 `2`
并在句子里点名 floor、store 拒绝 `4`、部署坏 `1`；④ **`ts BETWEEN` 是必须的且半开**（范围在库里
处处半开，包含上界会让相邻两个答案重叠一毫秒），`OR` 被拒（它是唯一会改变“计划能裁掉什么”的从句）；
⑤ **`GROUP BY time(<window>)` 是必须的**（没有窗口就没有诚实的默认），`LIMIT` 只数**系列**且必须配
`GROUP BY …, series`；⑥ **计划只从 manifest 做**（按 4.9 的每段时间范围裁剪，`EXPLAIN` 一个段都不开
——用例把“被裁掉那段的文件删掉”来证明这件事，而执行复用的是 4.10 的 `downsample`，逐系列调用）。

**原始结论行**（我在自己的构建与 scratch 副本上重取）：

```
$ kiln selftest --track lakelet                          # 11/11 step(s) hold the contract（4.1…4.11）
$ kiln selftest lakelet/4.11-query-engine --track lakelet # scaffold ✓（0 passed; 18 failed）/ solution ✓ / gates ✓ 10/10 must green
$ kiln check lakelet/4.11-query-engine --slow             # 10/10 must gate(s) passed · 1 should gate(s) run
  ✓ links              41 link(s) in 7 page(s) resolve
  ✓ slow:mutants@80    6/6 mutant(s) killed (100%, needs ≥80%)（walk 13s）
  ✓ slow:?coverage@80  620 of 635 line(s) of this step's module are covered (97% ≥ 80%)
$ kiln verify lakelet    → 11/11 judged step(s) pass · 0 failed · 0 not prepared · 13 without content
$ kiln lint-caps → no issues · kiln links → 101 page(s) · 502 link(s) ✓ · kiln i18n → 84/97 · kiln probe --check → 41/41
$ cargo test -q -p kiln → 320 + 21 + 1 · clippy 干净 · cargo fmt --all --check 干净
```

**演练（我重取了一遍）**：把计划的 `meets()` 从 `first < range.to()` 改成 `<=`

```
1. RED    $ cargo test -q --test spec_query_engine → FAILED. 17 passed; 1 failed
2. CAUGHT the_plan_prunes_from_the_manifest_and_explain_opens_nothing
          panicked at tests/spec_query_engine.rs:941:5: the one segment whose stated span meets an hour-long range
3. GREEN  回滚 → ok. 18 passed; 0 failed
```

**这一段钉下来的三件事**：

1. **修好了“创建者没留标记块”的墙（对 4.19/4.23 是前置条件）**：4.10 的 scaffold 建出了
   `src/cli/mod.rs` 却**没有标记块**（违反 §1 第 4 条），于是任何后来的 scaffold 都插不进去——
   4.11 只能让学员自己在文件里加 `--sql` 那一半（它照做了并写进正文），而 4.19 的
   `src/cli/backup.rs`、4.23 的 `src/cli/rotate.rs` 会撞同一堵墙。修法是在 4.10 的 scaffold 补丁里
   给那个新文件末尾补 5 行块（hunk 头 161→166），然后在**全新副本**里重跑 4.1…4.11 的阶梯并
   `check --slow` 两步：块落到了学员的 `app/lakelet/src/cli/mod.rs:863`，4.10 与 4.11 各自
   `10/10 must` 仍然绿。**代价要记下来**：文件行数一变，变异门的抽样就移位，4.10 的比值从
   6/6（100%）变成 5/6（83%，仍过门）。
2. **变异门的一个结构缺陷（实测，收尾时并入 §5）**：`slow:mutants@80` 的候选位点来自“本步参考解
   补丁碰过的 `src/**.rs` 文件的**当前内容**”——后来者在同一个文件里加的代码会落进**更早一步**的
   抽样里，而更早那一步的规格没有义务覆盖后来者的功能。实测：4.11 往 `src/cli/mod.rs` 加了
   `render_plan`（那是 4.10 补丁碰过的文件），4.10 的抽样就抽到 `src/cli/mod.rs:763 if !json {`
   （`!` 被去掉）并让它存活——而 4.11 自己的规格**能**杀掉它（用例 9 同时断言两种形状）。
   候选修法（留给下一轮）：只抽“本步补丁新增/改动的行”，而不是整个文件；代价是全部变异比值重测。
   与之相对，4.7 的 `decode.rs:422 if shift > 63` 确实是 4.7 自己写的行，那是一个**真缺口**。
3. **4.11 自己定的三处与作业书不同，都记录在案**：`GROUP BY time(...)` 必填（书里写成 `[...]`）、
   `execute` 返回 `Result<Outcome, Refusal>`（`Error` 表达不了类别 `3`，而 `src/error.rs` 没有标记块
   所以本步不能扩它）、类别 `3` 只在“区间碰到的段”里判定（为了不让 `rows_scanned` 变成谎言，
   4.18 才是“仓库知道自己有哪些名字”的地方）。

**后记（同一段，块的位置改了两次）**：第一版把标记块放在 `src/cli/mod.rs` 的**末尾**——它让
4.10/4.11 的补丁里“够到文件尾”的 hunk 在 `git apply` 下失配（`patch failed: src/cli/mod.rs:154`，
重建 `through 4.11` 基线树时实测；`kiln` 自己的补丁解析器容忍它，所以课程管线没事，**坏掉的是作者配方**）。
块改到 `MAX_WINDOWS` 与 `Outcome` 的文档注释之间（在两个后续补丁每个 hunk 的上下文之外）之后：
四个补丁（4.10 scaffold+solution、4.11 scaffold+solution）都能 `git apply` 到重建的 `through 4.9` 树，
产物与 `kiln` 阶梯产物逐字节一致；全新副本里 4.1…4.11 阶梯 + 两步 `check --slow` 仍然绿
（4.10 `10/10 must`、变异 5/6；4.11 `10/10 must` + 1 should、变异 6/6、覆盖 620/635）。
**教训**：scaffold 的插入点不能放在“任何一个后续补丁会够到的文件尾”，否则作者配方（`git apply`）与
课程管线（`kiln`）会分叉。

## 段 17：B3d — 4.12 `num/aggregation-correctness@apply`（和不是 `+`，一天不是 24 小时）

**做了什么**：`spec/4.12/aggregation.rs`（1277 行、17 用例）、`scaffold/4.12-aggregation-correctness.patch`
（430 行、**0 删除**：新建 `src/engine/query/agg.rs`（412 行）+ 在 `query/mod.rs` 的标记块里插 3 行）、
`solutions/4.12-aggregation-correctness.patch`（810 行：`agg.rs` 333、`cli/mod.rs` 61、`downsample.rs` 147、
`exec.rs` 39）、`course/docs/steps/en/lakelet/4.12-aggregation-correctness.md`（198 行）。
`track.toml` 未动（`9 must + 1 should`——`slow:?coverage@80` 是应该门）。

**这一步钉下来的六条不变量**：① **和不是 `+`**：`Sum` 是 Kahan–Neumaier 补偿累加器，`BOUND` 与
`error_bound(values)` 把承诺写成数字（相对“量级之和”），而 `naive` **故意留在模块里**——它是所有人
都写得出来的实现，也是用例要证明其错的那一半；② **答案不依赖读取顺序**：同一批记录打乱后答案必须落在
`error_bound` 之内（朴素求和过不了这条，这就是这个模块存在的理由）；③ **计数是整数**（`Bucket::count`
仍是 `u64`）；④ **一天不是 24 小时**：`Zone` 只带**这个构建所带的**转换表（`UTC` + 一个春季前跳、
秋季后跳的时区），表外回答 `None` 而不是猜；⑤ **`--zone` 规则刻意很窄**：只有 **1d** 窗口会变成
**本地日**（所以前跳那天 23 小时、后跳那天 25 小时），更短的窗口一个字节都不变，`--zone UTC` 与不带
`--zone` 逐字节相同；⑥ **`EXPLAIN` 不加键**（4.11 的规格数着十二行——rule 10）。

**原始结论行**（我在自己的构建与**全新** scratch 副本上重取）：

```
$ kiln selftest --track lakelet                          # 12/12 step(s) hold the contract（4.1…4.12）
$ kiln selftest lakelet/4.12-aggregation-correctness --track lakelet  # scaffold ✓（... FAILED）/ solution ✓ / gates ✓ 9/9 must green
$ kiln check lakelet/4.12-aggregation-correctness --slow   # 9/9 must gate(s) passed · 1 should gate(s) run
  ✓ slow:mutants@80    5/6 mutant(s) killed (83%, needs ≥80%)（12 次尝试里 1 次没能判：不编译或没跑完）
  ✓ slow:?coverage@80  177 of 177 line(s) of this step's module are covered (100% ≥ 80%)
$ kiln verify lakelet    → 12/12 judged step(s) pass · 0 failed · 0 not prepared · 12 without content
$ kiln lint-caps → no issues · kiln links → 102 page(s) · 502 link(s) ✓ · kiln i18n → 85/97 · kiln probe --check → 41/41
$ cargo test -q -p kiln → 320 + 21 + 1 · clippy 干净 · cargo fmt --all --check 干净
```

两条补丁在**我自己重建并核对过**的 `through 4.11` 树上重放后与受测树逐字节一致（`diff -r` 只剩 `kiln`
同步的 `tests/spec_*.rs`）；scaffold 0 删除。

**演练（我重取了一遍）**：把 `Sum::value()` 里的补偿项去掉（等于只留朴素求和）——

```
1. RED    $ cargo test -q --test spec_aggregation → FAILED. 12 passed; 5 failed
2. CAUGHT a_window_answer_is_the_accumulators_answer
          panicked at tests/spec_aggregation.rs:597:5: the window's sum is the accumulator's sum:
          a running total over the same records answers 1e16 / left: [1e16] / right: [1.0000000000001e16]
          （另外四条：补偿和、打乱顺序、bound 是承诺、窗口答案…）
3. GREEN  回滚 → ok. 17 passed; 0 failed
```

**这一段钉下来的三件事**：

1. **§5 那条仪器缺陷又添了第三个实测点**：4.12 的 5/6 里活下来的位点是
   `src/cli/mod.rs:678 if windows > MAX_WINDOWS {`（`>` → `>=`），而 **`MAX_WINDOWS` 在 4.12 自己的
   参考解补丁里出现 0 次**（4.10 有 3 次、4.11 有 5 次）——它是更早两步的行，被 4.12 的规格“判”了。
   （另一次没能判的尝试是 `agg.rs:531 - → +`，编译器报 “leading `+` is not supported”，门正确地把它
   排除在比值之外。）结论不变：比值低于 100% 先看是“本步真缺口”还是“后来者/更早者的行被抽中”。
2. **作者按规则 10 找出了三处不许动的地方**：`Refusal` **不能加变体**（4.11 的规格对它穷举匹配、没有
   通配臂——加了就让 4.11 在最终树上红），所以时区拒绝复用 `Refusal::Syntax`（类别 2）并把位置算在
   时区名里；`EXPLAIN` 不能加键；`downsample.rs` 没有标记块，所以本地日入口不能由 scaffold 声明，
   由学员自己写（正文写明形状）。这三条都记在这一段里，后面的步骤照抄。
3. **一次“先粗后细”的演练教训**：我第一次把 `Sum::add` 改成朴素加法时留下了悬空的 `total`，于是
   `cargo test` 失败的是**编译**而不是规格；换成去掉 `value()` 里的补偿项才是行为变异。**演练必须确认
   红的是规格**（编译错也红，但它什么都没证明）。

## 段 18：B3e — 4.13 `test/model-based@apply`（模型是契约的最简形式，随机走一遍必须与它一致）

**做了什么**：`spec/4.13/model_based.rs`（1005 行、18 用例）、`scaffold/4.13-model-based-tests.patch`
（1321 行、**0 删除**：新建 `src/engine/rng.rs`、`tests/support/{model,walk}.rs`、`tests/sim_model.rs`，
并在 `engine/mod.rs` 与 `tests/support/mod.rs` 的标记块里插三行）、
`solutions/4.13-model-based-tests.patch`（166 行：只有 `rng.rs` 与 `model.rs` 的函数体）、
`sim/4.13-model-based-tests.toml`（41 行，`seeds = [1, 7, 20260101]`、`replay = 1`）、
`course/docs/steps/en/lakelet/4.13-model-based-tests.md`（222 行）。`track.toml` 未动（10 must，无 should 门）。

**这一步钉下来的五条不变量**：① **自研种子 RNG 的确定性与差异性都算契约**（xorshift64\*：同种子同序列
——用金向量钉住；不同种子必须给出不同序列，`slow:sim@3` 的“至少两份不同 transcript”靠的就是它）；
② **模型不知道段、合并、manifest、文件的存在**——它只有“追加进来的记录 + floor”，因为读者看到的就是
“接受过的前缀减去已经忘记的”，**合并对读者必须不可见**；③ **每一步之后都比**（同一份 transcript 里
`model=` 与 `store=` 的摘要成对出现，不一致就走收缩）；④ **transcript 只有计数、摘要和名字**（没有墙钟、
路径、地址，也没有未掩码的浮点），因此 `replay` 种子必须逐字节可重放；⑤ **不一致时收缩**（贪心地删操作，
直到删任何一步都仍然不一致，然后打印最小序列——`op=1 append / op=2 flush / op=3 expire` 就是“最早咬人的
那次过期”的形状）。

**原始结论行**（我在自己的构建与 canonical 副本 `l4b3-check4` 上重取）：

```
$ kiln selftest --track lakelet                     # 13/13 step(s) hold the contract（4.1…4.13）
$ kiln selftest lakelet/4.13-model-based-tests --track lakelet  # scaffold ✓（2 passed; 16 failed）/ solution ✓ / gates ✓ 10/10 must green
$ kiln check lakelet/4.13-model-based-tests --slow   # 10/10 must gate(s) passed
  ✓ slow:sim@3     3 seed(s) ran, transcripts 85c499a1/903bdf8e/a8efae13 — seed 1 replayed identically（walk 2s）
  ✓ slow:mutants@80 6/6 mutant(s) killed (100%, needs ≥80%)（walk 12s）
$ kiln verify lakelet    → 13/13 judged step(s) pass · 0 failed · 0 not prepared · 11 without content
$ kiln lint-caps → no issues · kiln links → 103 page(s) · 502 link(s) ✓ · kiln i18n → 86/97 · kiln probe --check → 41/41
$ cargo test -q -p kiln → 320 + 21 + 1 · clippy 干净 · cargo fmt --all --check 干净
```

三个种子的 transcript 摘要与作者报告的一致（`85c499a1/903bdf8e/a8efae13`）；两条补丁在我重建并核对过的
`through 4.12` 树上重放后与作者的两棵工作树都逐字节一致；scaffold 0 删除。

**演练（我重取了一遍）**：把 `Model::expire` 改成空实现（floor 不再前进）——

```
SPEC   $ cargo test -q --test spec_model_based → FAILED. 15 passed; 3 failed
       （the_floor_moves_forwards_and_what_it_forgot_stops_being_an_answer、
         an_expiry_moves_the_floor_and_the_answers_shrink_with_it、a_walk_replays_what_it_was_seeded_with）
SIM    $ KILN_SIM_SEED=1 cargo test -q --test sim_model -- --nocapture
       lakelet-sim result=disagreed ops=14 seed=1 shrunk_to=3
       lakelet-sim shrink op=1 kind=append n=2 series=cpu,host=h3 · op=2 kind=flush · op=3 kind=expire
       panicked at tests/sim_model.rs:115
GREEN  回滚 → spec 18 passed · 单种子 harness ok
```

**这一段钉下来的四件事**：

1. **作业书里那几处“作者改得更好”的偏差，全都记录在案**：走步做成**夹具**（`tests/support/walk.rs`，
   因为规格需要一个“故意错的序列”来演示收缩，而测试目标之间不能互相 import）；**一致时的 transcript
   不打印种子**（打印了三个种子就会因为一个与走步无关的原因而不同，`slow:sim@3` 的反空洞判据就白设了
   ——作者的理由比我的作业书好）；**模型在 flush 把它封成段时才听到一批**（读者的答案就是 manifest 认的
   那些段）；**走步把合并比例放宽到 3**（实测：比例 2 时三个种子里有一个从不合并，本步的主张就没被验到）。
2. **一次“反过来教我一件事”的抽样**：`slow:mutants@80` 第一次抽到 `rng.rs` 的 `if span <= 0`（`<=` → `<`）
   ——**值**不变，但退化区间会**多消耗一次抽样**，于是可观察的是生成器的流位置。作者把它写成规则
   （“a span that names nothing draws nothing”）并用一条用例钉住，于是 6/6。这不是仪器噪声，是**真缺口**。
3. **每个树一个 `CARGO_TARGET_DIR`，没有例外**：作者在作者侧踩了运河对岸的同一个坑——两棵副本共用一个
   target 目录，cargo 把**旧的** `lakelet` 产物给了新树，于是一条规格为一个树里并不存在的理由而红。
   这条写进 handoff-m3 §1 第 13 条。
4. **规格与装置的分工**：规格读的是**固定种子**（环境变量 `KILN_SIM_SEED` 是给门跑的 harness 的，规格读环境
   就等于让规格判机器），而 harness 读 `KILN_SIM_SEED`/`KILN_SIM_OPS` 并打印 transcript。

## 段 19：B3f — 4.14 `query/index-pruning@apply`（剪枝是可数的，侧车索引是派生状态）

**做了什么**：`spec/4.14/index_pruning.rs`（1675 行、20 用例）、`scaffold/4.14-index-pruning.patch`
（1098 行、**0 删除**：新建 `src/engine/index/{mod,zone,bloom}.rs` 的契约桩与课程装置
`tests/budget_pruning.rs`，并在 `engine/mod.rs` 的标记块里插 `pub mod index;`）、
`solutions/4.14-index-pruning.patch`（774 加 / 75 删：三个模块的函数体 + `compaction/{mod,snapshot}.rs`、
`query/{plan,exec}.rs`、`cli/mod.rs`、`docs/query.md`）、`budget/4.14-index-pruning.toml`（123 行、18 计数）、
`course/docs/steps/en/lakelet/4.14-index-pruning.md`（257 行）。另加两处**早期契约的兼容式修改**：
`spec/4.11/query_engine.rs` 的 `assert_eq!(keys.len(), 12)` 放宽成 `>=`（在 4.11 自己的树上是 12、在最终树
上是 13，两边都过——rule 10 的正解），并重钉它的哈希；`spec/manifest.toml` 增加 4.14。

**这一步钉下来的五条不变量**：① **索引在段的旁边，不在段里**（`<store>/index/<ordinal>.lki`）：4.7 的
规格钉着 `segment::VERSION == 1` 且拒绝段尾多出的字节，4.8 的规格数 `segments/` 里的文件——所以段格式
一个字节没动，索引也不会污染 4.8 的孤儿规则；② **索引是派生状态，安全方向**：缺失、损坏、过期（manifest
不再认的 ordinal）只损失剪枝，永不改变答案，且两种“没有索引”分开计数（`index_missing` / `index_unusable`）；
③ **布隆过滤的是“词”而不只是系列名**（`SegmentIndex::of` 同时插入名字与它带的每个 `key=value`，用 4.11 的
`Series::of` 读标签），因为查询问的是**对**（`WHERE host='h0'` 也要选中 `cpu,host=h0,dc=east`）——只插名字
会给出假阴性，也就是丢记录；④ **计划两级剪枝**：manifest 的时间范围（4.11 的 `segments_pruned`）+ 索引
（新增 `blocks_pruned`），于是 `opened + pruned + blocks_pruned` 就是 manifest 认的全部段；
⑤ **可数**：18 个计数器里 `pruning_rate`（整数百分比）就是 plan §9 演练第 5 条要推动的那个数。

**原始结论行**（我在自己的构建与**全新**副本 `l4b3-check5` 上重取，阶梯 4.1…4.14 全部 `run` 红 / `solution` 绿）：

```
$ kiln check lakelet/4.14-index-pruning --slow      # 10/10 must gate(s) passed
  ✓ slow:bench-budget@10 18 metric(s) judged（12 within ±10%，6 at or below an upper bound）:
    segments_total 24, bytes_total 5896, bytes_outside_range 2945, segments_opened 1,
    segments_pruned_by_time 12, blocks_pruned 11, bytes_read 243, bytes_avoided 2708, pruning_rate 95,
    segments_unaccounted_max 0, bytes_unaccounted_max 0, rows_scanned 60, rows_answered 30,
    answer_mismatch_max 0, index_files 24, index_missing_max 0, index_unusable_max 0, refused_max 0
  ✓ slow:mutants@80 5/6 mutant(s) killed (83%, needs ≥80%)（活下来的是 §5 记的那个 with_capacity 位点）
$ kiln verify lakelet → 14/14 judged step(s) pass · 0 failed · 0 not prepared · 10 without content
$ kiln selftest lakelet/{4.7,4.11,4.14} --track lakelet → 三步各自 1/1 hold the contract（4.11 放宽后的断言在它自己的树上依旧过）
$ cargo test -q -p kiln → 320 + 21 + 1 · clippy 干净 · fmt 干净 · lint-caps no issues · links 104 page(s) · i18n 0 error · probe 41/41
```

**演练（plan §9 第 5 条，我重取了一遍）**：把计划里的索引查询去掉（`Lookup::Found` 一律当作“可能答”，
即回到 4.11 的行为）——

```
RED   ✗ slow:bench-budget@10 6 of 18 metric(s) moved out of budget: segments_opened is 12, 11 of the
      baseline 1; blocks_pruned is 0, -11 of the baseline 11; bytes_read is 2951, 2708 of the baseline
      243; bytes_avoided is 0, -2708 of the baseline 2708; pruning_rate is 50, -45 of the baseline 95;
      rows_scanned is 720, 660 of the baseline 60 —— 7/10 must gate(s) passed（✗，mutants 也 ⚠“规格没在未变异
      的树上是绿的”，因为四条规格用例同时红了）
GREEN 回滚 → ✓ slow:bench-budget@10 … pruning_rate 95 … · ✓ slow:mutants@80 5/6 · 10/10 must gate(s) passed
```

与作者的数字逐字一致（`pruning_rate 95 → 50`、`blocks_pruned 11 → 0`、`segments_opened 1 → 12`），
而**该不动的东西一个都没动**（`segments_total`、`bytes_total`、`bytes_outside_range`、
`segments_pruned_by_time`、`rows_answered`、`answer_mismatch_max`、`index_*`、`refused_max`、两条守恒式）。

**这一段钉下来的四件事**：

1. **B3 收口**：4.9–4.14 六步全部落地（`kiln verify lakelet` 14/14），plan §9 的腿 5 完成。剩下的腿：
   腿 2（4.17 的 soak 抓泄漏）、腿 6 后半（4.22 弄坏 Dockerfile）、腿 1 的完整一半（24 步齐后无网跑
   `kiln verify lakelet`）。
2. **§5 的抽样缺陷又添一个方向**：4.14 的补丁必须碰 `cli/mod.rs`（4.11 的 JSON 键断言逼出来的），于是
   抽样抽到了 **4.10 的行** `if windows > MAX_WINDOWS`，作者一开始只有 66%，补了一条命令行边界的用例才到
   83%。结论：**一个步骤只要碰到共享文件，就会“继承”那个文件里没被覆盖的行**——收尾时写进 §5。
3. **作业书里我写错的命令**：`rustfmt --config path rustfmt.toml` 不合法（要 `--config-path`）；作者纠正了。
   给下一位的提醒记在这里，别再写错。
4. **规则 9 家族的新失败模式**：用**终端**往工作树的**新未跟踪目录**里写文件，Delta 会报
   `<skipped-untracked-directory>` 并让整个工作树的终端停摆（“the checkout kept changing”）；恢复办法是
   先把那棵树恢复到 Delta 认识的样子（撤掉 shell 写的那次编辑、删掉未跟踪目录），然后用**文件工具**建文件、
   再 `cp` 内容过去。**工作树里的文件只用文件工具创建**（第 9 条的姊妹条，见 §1 第 14 条）。

## 段 20：B4a — 4.15 `perf/bench-budget@operate`（从预算到归因：把工作分到相位上数）

**做了什么**：`spec/4.15/budget_regression.rs`（1026 行、15 用例）、
`scaffold/4.15-budget-regression.patch`（652 行、**0 删除**：新建 `src/engine/trace/{mod,counters}.rs`
的契约桩、课程装置 `tests/attribution.rs`（316 行）与 `docs/perf.md` 的桩，`engine/mod.rs` +3）、
`solutions/4.15-budget-regression.patch`（566 行：`counters.rs` 的函数体、`query/exec.rs` 按相位计费、
`cli/mod.rs` 在 `--json` 里带出 `trace`、写成的 `docs/perf.md`）、
`budget/4.15-budget-regression.toml`（105 行、**36 计数**）、
`course/docs/steps/en/lakelet/4.15-budget-regression.md`（205 行，deepen：RECALL/DELTA/TASK/DONE WHEN/TRAP，
**没有 WHY**）。`track.toml` 未动（10 must：`links` 与 `slow:bench-budget@10` 在内，**没有 mutants**）。

**这一步钉下来的四条不变量**：① **相位**：`Headers / Blocks / Decompress / Decode / Filter / Aggregate /
Render`，每一笔都是引擎真的做过的计数——**计数工作，绝不计数时间**；② **恒等式**：`Blocks >= Decompress`、
`Decode >= Filter >= Aggregate`、`Headers` 等于计划看过的段数，且 `Decode == rows_scanned`（一条计数没有
用例能反驳，比没有计数更糟）；③ **profile 只能从一次运行读，不能从 `EXPLAIN` 读**（rule 10：4.11 数着它的
行数、4.14 按名字与顺序钉着十三个键——所以 `trace` 落在 `Outcome` 与答案文档里；正文把这件事讲成
“`EXPLAIN` 预测，profile 测量”）；④ **一次运行的形状**（`Trace::new()` 的 `runs == 1`，两次运行合并报 2）。

**两处作者比作业书更对的判断**：`Filter` 只是**谓词**（若把它定义成“谓词且范围”，在引擎能产生的每一次运行
上它都等于 `Aggregate`，`Filter >= Aggregate` 就成了不可反驳的恒等式——正是正文反对的东西），范围那一半
由窗口通负责；`Aggregate` 记的是**窗口通真正折进去的**（`Answer::in_range()`），不是“交给它的”，否则
`GROUP BY …, series` 会把同一条记录按系列数重复计数。

**原始结论行**（我在自己的构建与副本 `l4b3-check5` 上重取）：

```
$ kiln selftest --track lakelet                     # 15/15 step(s) hold the contract
$ kiln check lakelet/4.15-budget-regression --slow   # 10/10 must gate(s) passed
  ✓ slow:bench-budget@10 36 metric(s) judged（35 within ±10%，1 at or below an upper bound）:
    selective_headers 24, selective_blocks 1, selective_decompress 1, selective_decode 60,
    selective_filter 30, selective_aggregate 30, selective_render 1, selective_share_decode 41, …
    wide_decode 720, wide_filter 360, wide_aggregate 360, decode_saved 660, filter_saved 330,
    aggregate_saved 330, segments_saved 11, bytes_saved 2708, merged_total 1647, merged_runs 2, refused_max 0
$ kiln verify lakelet → 15/15 judged step(s) pass · 0 failed · 0 not prepared · 9 without content
$ cargo test -q -p kiln → 321 + 21 + 1 · clippy 干净 · fmt 干净 · lint-caps no issues · links 105 page(s) · i18n 0 error · probe 41/41
```

**演练（我重取了一遍）**：把 `Filter` 命中的记录再按 `Decode` 记一次（相位说谎）——

```
RED    $ cargo test -q --test spec_budget_regression → FAILED. 12 passed; 3 failed
CAUGHT a_record_the_range_kept_out_is_filtered_and_not_aggregated
       panicked at tests/spec_budget_regression.rs:707:5: the segment holds sixty records and both
       series were decoded / left: 90 / right: 60（另两条：恒等式用例、索引拿掉后相位移动的用例）
GREEN  回滚 → ok. 15 passed; 0 failed
```

**这一段钉下来的两件事**：

1. **框架修复：`bench-budget` 不再“静默忽略”基线里没有的计数**。作者的实测：装置多打印一个数、基线里没有
   它，门照样绿，于是那个数字看起来被看着、其实没有任何比较。现在通过/失败的判词里会带一句
   `N the baseline does not name so they were not judged: …`（`kiln/src/gates/budget.rs`，带单测；
   正确内容上它一个字都不多说——4.10 与 4.15 两条车道的实测行都没变）。另一面已经是对的：**基线里有而
   装置没打印**的计数会让门 `blocked` 并点名。
2. **一条给我的提醒**：作业书里我把 4.15 的慢门写成“可能有 `mutants@80`”，实际 `track.toml` 里没有——
   作者按规则 8 对着文件数并指出（并改用两次伪造演练顶替）。门数只从 `track.toml` 数，这句话我还是要
   自己再犯一次，记在这里。

## 段 21：B4b — 4.16 `perf/profiling@operate`（profile 是证据：采样、读它、说出结论）

**做了什么**：`spec/4.16/profiling.rs`（730 行、13 用例）、
`scaffold/4.16-profiling.patch`（631 行、**0 删除**：新建 `lakelet-load` 的 `--profile` 模式文件、
装置与 `docs/profile.md` 的桩，并按规则 4 给 `src/bin/lakelet-load.rs` 补了两个标记块）、
`solutions/4.16-profiling.patch`（382 行：模式的语义 + 写成的 `docs/profile.md`）、
`course/docs/steps/en/lakelet/4.16-profiling.md`（151 行、8 条 TRAP）；**骨架改了 17 加 / 2 删**
（见下）。`track.toml` 未动（9 must，无慢门）。

**两条不变量**：① **`--cycles N` 是可重复的、`--seconds N` 不是**：前者打印相位计数（4.15 教的那些）
与工作形状，后者按钟停——规格只依赖前者；② **页面上的数字是命令的输出**：`docs/profile.md` 里那一段
`lakelet-load --profile --cycles 8` 的输出，由规格**重新跑一遍逐行比对**（“从一次运行里改页面，不要凭
意图改”）。

**这一步的发现（作者没有顺着我的作业书圆谎，值得记下）**：profile 顶部两个符号是 `__fcntl` 与 `__open`
——就是每次 `seal` 的**四次 `sync_all`**（段文件、侧车索引、pending manifest、目录）——而**七个相位里
没有一个计到 flush**。正文因此诚实地说“这个进程把时间花在没有任何相位计费的地方”，并据此建议把段路径
也交给 4.3 给记录路径的那套 `wal::Policy`，而不是假装某个计数看到了它。

**骨架改动（实测而不是风格）**：`lakelet-load` 原本没有标记块（规则 4），且用默认多线程 runtime——
八条 `tokio-rt-worker` 线程停在 `__psynch_cvwait`，第一次采样的顶部就是它（22463 个样本），把单线程程序的
profile 淹了。改成 `#[tokio::main(flavor = "current_thread")]` 并补两个标记块之后：**前 15 步的补丁在
新骨架上依旧全部 `git apply` 通过**，`kiln selftest --track lakelet` **16/16**。

**原始结论行**（我在自己的构建与**全新**副本 `l4b3-check6` 上重取）：

```
$ kiln selftest --track lakelet                    # 16/16 step(s) hold the contract · real 6m57.8s
$ kiln check lakelet/4.16-profiling --slow         # ✓ spec-integrity 17 spec file(s) match · … · 9/9 must gate(s) passed
$ kiln verify lakelet                              # 16/16 judged step(s) pass · 0 failed · 0 not prepared · 8 without content
$ cargo test -q -p kiln → 321 + 21 + 1 · clippy 干净 · fmt 干净 · lint-caps no issues · links 106 page(s) · i18n 0 error · probe 41/41
```

**演练（我重取了一遍）**：把页面上那段的 `records=8000` 改成 `8001`——

```
RED    $ cargo test -q --test spec_profiling → FAILED. 12 passed; 1 failed
CAUGHT the_page_carries_the_profile_the_command_prints（tests/spec_profiling.rs:600:5）
       `docs/profile.md`'s block is not what `lakelet-load --profile --cycles 8` prints: … Edit the page
       from a run, never from an intention.（两侧列表并排打印，唯一差别是 records=8000 / 8001）
GREEN  回滚 → ok. 13 passed; 0 failed
```

**这一段钉下来的两件事**：

1. **一个 worker 交付的文件可能“到了但是残的”**：因为 shell 往**新目录**里写的文件会被 Delta 跳过
   （规则 14），作者只能用文件工具分块转录 730 行的规格；到我这边的副本是 `934d4cb5…`（**少了约 530 行、
   且 rustfmt 脏**），而 manifest 钉的是 `65e71040…`。**是 scratch 副本车道里的 `spec-integrity` 与 `fmt`
   把它抓出来的**（`0/16 judged … 16 failed`），我从作者自己的副本拷回正确文件（哈希对齐）后 16/16。
   → 新增 §1 第 15 条：**交付的文件要对哈希（或行数），并且先别删 worker 的副本**。
2. **骨架也可以在后来的步骤里改，只要验证到位**：改骨架等于改**每一步**的树——判据是“前 N 步的补丁
   是否仍能 `git apply`”+“全量 `selftest` 是否还 16/16”，两条都做了才敢留。理由与证据进正文（八条 parked
   线程会淹掉单线程程序的 profile）。

## 段 22：B4c — 4.17 `perf/soak@apply`（十个一千次循环之后，还活着的是什么）

**做了什么**：`spec/4.17/soak.rs`（920 行、17 用例）、`scaffold/4.17-soak.patch`（1059 行、**0 删除**：
新建 `src/engine/soak.rs`（302 行桩）、`src/service/soak.rs`（146 行桩）、课程装置 `tests/soak.rs`（525 行，
完整）、`docs/soak.md` 的桩（35 行），并在 `engine/mod.rs` 与 `service/mod.rs` 的标记块里各插一行）、
`solutions/4.17-soak.patch`（906 行，碰 **12 个文件**）、`soak/4.17-soak.toml`（51 行）、
`course/docs/steps/en/lakelet/4.17-soak.md`（158 行）。`track.toml` 未动（11 must）。

**这一步钉下来的四条不变量**：① **五个活计数器**：`open_tasks`（spawn 了还没被 reap 的任务——
实现为“句柄还没被 await 或 drop”，刻意不做成“直到任务跑完”，那会变成与最后一条指令的竞态）、
`open_handles`、`store_files`、`store_bytes`、`records`，每一个的**增长**都对应一个具体的事故；
② **稳态是一条断言而不是一句希望**：中点与终点的计数必须在声明的界内一致，不一致时由装置自己报出点名
计数的那句话；③ **判据是计数不是分钟**：`cycles_min = 4000` 是**下界**，`[metric]` 全是**上界**，
`LAKELET_SOAK_MINUTES=30` 是给人跑的长形态、门从不设置它；④ **一行一个计数**（`name=value`）——
门的解析器遇到 `a=b c=d` 会 **报错**而不是跳过（作者第一次 selftest 就撞上，`slow:soak@4000` 直接
`blocked`）。这条写进 §1 第 16 条。

**两处作者比作业书更准的判断**：`store_files` 数的是**manifest 认的那些文件**，不是磁盘上的一切——
4.14 **故意**留下过期 ordinal 的索引文件（它的规格钉着这件事），数磁盘会把那个“垃圾”读成每次过期漏一个
文件；被发现的是计数器不够准，而不是那个文件该删。`store_bytes` **不含日志**——这份构建从不截断日志，
把它算进去会让稳态恒不成立，那是另一件事，不是泄漏。

**原始结论行**（我在自己的构建与**全新**副本 `l4b3-check7` 上重取；阶梯 4.1…4.17 全部 run 红 / solution 绿）：

```
$ kiln check lakelet/4.17-soak --slow            # 11/11 must gate(s) passed
  ✓ spec-integrity 18 spec file(s) match · … · ✓ links
  ✓ slow:soak@4000 4000 cycle(s) in 146s, 5 live counter(s) inside their bounds:
    open_handles 0, open_tasks 2, refused 0, store_bytes 2743, store_files 13
  ✓ slow:mutants@80 5/6 mutant(s) killed (83%, needs ≥80%)（walk 21s）
$ kiln verify lakelet → 17/17 judged step(s) pass · 0 failed · 0 not prepared · 7 without content
$ kiln selftest --track lakelet → 17/17 step(s) hold the contract · real 10m30.6s
$ cargo test -q -p kiln → 321 + 21 + 1 · clippy 干净 · fmt 干净 · lint-caps no issues · links 107 page(s) · i18n 0 error · probe 41/41
```

计时：`check --slow` 里的 soak 门约 **146–157 s**（每循环三次设备等待：段、索引目录、索引文件——那是 4.8 的
持久性决定，不是本步能省的）；中点（第 2000 循环）`open_tasks=2 open_handles=0 store_files=13 records=384`，
终点（第 4000 循环）`open_tasks=2 open_handles=0 store_files=13 store_bytes=2743 records=384 cycles=4000`。

**演练（plan §9 腿 2，我重取了一遍）**：在**生产代码**里注入泄漏——`ingest::serve` 把每条连接的句柄收进
一个活到进程结束的 `Vec`，不再 `drop`：

```
RED    ✗ slow:soak@4000 the soak did not finish: exit code 101: thread
       'the_counters_hold_their_place_over_a_long_run' (…) panicked at tests/soak.rs:356:9: (after 149s)
       （装置自己的那句话点名了计数与两个数：`the soak does not reach a steady state: open_tasks was 2003
        at the halfway mark and is 4002 at the end, which is 1999 more than the 0 this step allows it`；
        门只剪贴 panic 的首行，所以这句话在测试输出里、不在门的行里）
       ⚠ slow:mutants@80 the spec does not pass on the project as it is… · 7/11 must gate(s) passed（2m42s）
GREEN  回滚（`cmp` 对齐原文）→ ✓ slow:soak@4000 4000 cycle(s) in 142s … · 11/11 must gate(s) passed（3m01s）
```

**这一段钉下来的三件事**：

1. **腿 2 完成**（计划 §9）：泄漏被 soak 抓住、点名计数、回滚变绿 ✓。台账（段 8）里腿 1 的完整一半、
   腿 6 后半仍待收尾（4.22 的 Dockerfile）。
2. **§5 的抽样缺陷又添一个方向（第五个实测点）**：本步的参考解碰了 **12 个文件**，其中大部分是 4.8/4.14
   的共享文件，于是“本步的规格”第一次跑出 **0/6**——作者把六个抽样位点逐个手工打进去量，四条死于“重开
   仓库并对盘上的东西（manifest、epoch、段长度、索引）逐项比对”的用例、一条死于合并后的 epoch，活下来的
   那条是 4.8 `discard` 的 `NotFound` 分支（不可达：`with_policy` 先建 `segments/` 再 `read_dir`）——
   规则 11 的仪器噪声。最终 5/6 = 83%（也正是这两条新用例把规格从 12–16 带到 17 条）。
3. **门对装置输出形状是硬的**：一行一个 `name=value`（第 16 条）。另外记下 `kiln check --slow` 在
   4.17 上要跑 ~3 分钟（soak 146 s + mutants 21 s + 快门），这是 T4 目前最慢的一步。

## 段 23：B4d — 4.18 `obs/cardinality-cost@apply` + 变异门的三次修正与全量重测

**4.18 做了什么**：`spec/4.18/cardinality.rs`（1220 行、16 用例）、`scaffold/4.18-cardinality.patch`
（552 行、**0 删除**：`engine/cardinality/{cap,mod}.rs` 的契约桩 + `docs/cardinality.md` 的桩 + 一行标记块插入）、
`solutions/4.18-cardinality.patch`（1340 行、碰 11 个文件）、`soak/4.18-cardinality-cost.toml`（40 行，
带 4.17 的 command 与本树的界）、正文 153 行。`track.toml` 未动（10 must）。
五条不变量：**cap 是数据**（`Cap { limit }`，`Admission::{Admitted, Rejected}`）；**账本是派生状态**
（从写路径历史重建、用侧车索引修剪，重启后能“跟踪”本进程从未见过的系列）；**守恒式**
`offered = admitted + rejected`；**拒绝在 4.6 已经数拒绝的地方可见**（连接的 `Tally`——单向协议无法回话，
正文写明）；**`lakeletq inspect series` 给留下的系列定价**（页面里的算术都是本店实测）。
`slow:soak@4000` 复跑：五个活计数器与上一步逐位相同（`store_bytes` 差 2 字节，在 4.17 的允许内），
所以**没有动任何界**；作者还实测并报告了一件与作业书预期相反的事：账本泄漏**不会**推动那五个计数器
（它们数文件/任务/记录，不数账本），抓到它的是边界与守恒用例。

**变异门的三次修正**（框架，`2d1c585` + `0c0300d` + 本次）：
1. **范围**：位点从“补丁碰过的文件的所有行”改成“补丁**写下的行**”（`sources::production_lines`）。
   修前四次实测：4.10 的 `MAX_WINDOWS` 被 4.12 判、4.11 的 `render_plan` 被 4.10 判、4.14 被迫碰
   `cli/mod.rs` 后抽到 4.10 的行、**4.12 在尾巴树上掉到 66%**（那是 M3 验收 `verify --slow` 无法通过的原因）。
2. **锚定**：行号不跟着树走——`verify --slow` 第一次跑出新红：4.9 与 4.17 在 `compaction/mod.rs` 上
   变异了**后来步骤移动过的行**。改成按**文本**匹配（`Written { line, text }`）。
3. **样本**：`MUTANT_CAP` 6 → 12、`ATTEMPT_CAP` 2× → 3×（收窄后每票更重；4.17 抽到 8 个里 3 个
   不可见位点读到 50%）。**4.14 与 4.17 因此补了三条真缺口的用例**（合并必须声明合并后的范围、
   短于自身算术的索引必须在算术溢出前被拒、被撕开的日志必须能打开并忘掉尾巴）——都是各自页面早就
   声称、只有规格沉默的事。

**原始结论行**（我在自己的构建与**全新**副本 `l4b3-check8` 上重取）：

```
$ kiln verify lakelet --slow        # real 12m01s
  18/18 judged step(s) pass · 0 failed · 0 not prepared · 6 without content   （exit 3 = 还有 6 步没内容）
  其中慢门各自跑过：4.2 fuzz@3、4.4/4.6/4.8/4.13 sim@3、4.9/4.10/4.14/4.15/4.17/4.18 的 bench/soak、
  12 个 mutants 步、4.11/4.12 的 ?coverage
$ kiln selftest --track lakelet     # real 16m46s → 18/18 step(s) hold the contract
$ jobq 3.20…3.27 八个变异步各自 selftest → 1/1 hold the contract（八条 exit=0）
$ cargo test -q -p kiln → 322 + 21 + 1 · clippy 干净 · fmt 干净
```

**这一步钉下来的三件事**：① **4.18 落地**（B4 四步完成：4.15–4.18；还差 4.19、4.20）；② **变异门修完并全量重测**
（t lakelet 18/18 + jobq 八步；`verify --slow` 第一次在尾巴树上全绿，M3 验收的形状达成）；③ **一个新的
经验**：门的“位点”必须按内容锚定，按行号锚定等于把后来者的代码算在前面的步骤头上——这条与 §5 的
“抽样范围”是同一个教训的两半。
