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
