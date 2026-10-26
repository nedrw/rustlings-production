# M4 施工日志（T5 `meshkv`）

> 计划与决策在 [m4-plan.md](m4-plan.md)；交接简介（`t5-meshkv.md`）在收尾时写成。
> 按批次追加；每条都留原始行。

## 段 1：B0 基建 + 5.1 第一个真步骤

**C1 跨 track 依赖（plan §3 D6 的唯一例外）——已落地**（`da0c803`）：

- `track.toml` 的 `requires_track = "jobq"`；`Course::load` 校验存在、非自指、无环（三条拒绝各有一条单测）。
- `prepare`（`run`/`check` 的公共前半）在依赖项目缺失时返回 `MissingDependency`（exit 3，与“自己的项目还没建”同族），
  消息点名缺的是哪条 track 的项目。
- `verify` 把该步报成 `blocked` 并给出同一条原因；`lay_reference_tree`（骨架+全部 scaffold+全部 solution，
  不同步规格、不跑 cargo）给 selftest 的砂箱铺依赖树；依赖未写完的 track 拒绝铺半棵树。
- 单测 335 → 338（+3），`cargo test -p kiln` 全绿、clippy/fmt 干净。

**C2/C3 环境**（实测）：

```
$ rustup component add miri
error: component 'miri' for target 'aarch64-apple-darwin' is unavailable for download for channel '1.98.1-...'
$ ls ~/.cargo/registry/cache/*/ | grep -c '^loom'
1
```

miri 在钉住的 1.98.1-aarch64 上不可用 → 走 m4-plan §10 预留的降级：5.19 用 loom（已入缓存）+ `unsafe-budget@N`
+ 手工模型，miri 只在正文与 FURTHER 里点名。turmoil 0.7.2 已在缓存（离线路成立）。

**C5 元数据**（`b44ca33`）：22 步、23 槽逐格对齐台账（R0 干净），门阶梯从 T4 终点起，`unsafe-budget@0` 全程；
5.1 的 `requires` 在 R5 第一次打红后按 RECALL 补成五项（`repo/workspace-layout@apply`、
`err/typed-errors@design`、`domain/parse-dont-validate@apply`、`async/graceful-shutdown@operate`、
`cli/contract@apply`）——R5 对 transfer 步的判据是“RECALL 指的每一步都真建立了它”，空 `requires` 过不了。

**5.1 落地**（本段）：

- `app/meshkv` 骨架：一个包（库 + `meshkvd`/`meshkvc` 两个 bin，`default-run = "meshkvd"`）、
  `rust-toolchain.toml`（1.98.1）、`unsafe_code = "deny"` 从第一步就在 manifest 里（unsafe-budget@0 的判据）、
  四处标记块；`src/{lib,error,cluster,node}.rs` 与 `src/cli/mod.rs` 的契约桩（`todo!`）。
- 5.1 正文（transfer：RECALL/DELTA/TASK/DONE WHEN/TRAP/FURTHER）。
- `spec/5.1/multi_node_topology.rs`：11 个用例（顺序与 canonical 渲染、重名、非 `host:port`、名字字母表、
  未知名字、非成员 bind、三节点互认、错身份拒绝、异版本握手拒绝且 listener 继续服务、一行握手的双向读法、
  CLI 与库读同一份规格）。
- `solutions/5.1-multi-node-topology.patch`：18 个 hunk，三个文件（`cluster.rs`/`cli/mod.rs`/`node.rs`）。

**原始结论行**：

```
$ kiln spec manifest --track meshkv
1 spec file(s) pinned — commit the manifest with the specs it pins.

$ kiln selftest --track meshkv 5.1-multi-node-topology
kiln selftest · meshkv · a throwaway copy at /var/folders/...
  laid jobq's reference tree first (meshkv builds on it, plan §3 D6)

✓ meshkv/5.1-multi-node-topology (assert)
    scaffold ✓ the spec fails as it must (exit code 101: test the_topology_keeps_the_order_it_was_written_in_and_renders_it_back ... FAILED)
    solution ✓ reference patch applies
    solution ✓ the spec passes
    gates ✓ 9/9 must green

1/1 step(s) hold the contract: the spec fails on the scaffold, passes on the reference solution, and every must gate is green

$ kiln lint-caps
97 capabilities · 136 slots · 119 ledger positions · 119 authored steps
skipped: R5 prose structure: checked 98 of 119 step bodies, 21 not authored yet (`en` is the source locale)
no issues.
```

**这一段落下的规矩**：

1. **`tokio::join!` 不是 future**（它在宏里 await）。两个 await 的握手要在测试里同时发生，就把 listener 那一半
   `tokio::spawn` 成任务（`Arc<Node>`），调用的一半留在测试里；把它包进 `within(..)` 会得到“tuple 不是 future”。
2. **规格里的格式化参数也要过 `uninlined_format_args`**（`-D warnings` 对所有 target 生效）。`format!("{}", x)`
   连测试也红；`env!(..)`、方法调用、算术表达式这类不能内联的位置才允许保留 `{}`。
3. **握手失败是连接的错，不是节点的错**：`Node::accept` 返回 `Err(Protocol)`，`meshkvd` 打印一行继续服务。
   规格里用“坏连接 → accept 红 → 好连接照常”把这条钉住。
4. **骨架的 import 只写桩用得着的东西**（否则 `kiln run` 时满屏 unused warning）；参考解把 `Error` 等补进 import，
   这是 patch 的第一批 hunk。

## 段 2：B1 第一步——5.2 `dist/consensus@aware`（规则是值，页面是同一批规则的英文）

**5.2 做了什么**：`src/raft/mod.rs`（`Term`/`Entry`/`Quorum`/`Log`/`committable` 五个契约，加上 `RULES`
与 `FAILURES` 两组常量）、`scaffold/5.2-replication-rules.patch`（新建模块 + `lib.rs` 标记块里插入
`pub mod raft;` + 新建 `docs/consensus.md` 的骨架）、`solutions/5.2-replication-rules.patch`（填函数体、
写页面内容）、`spec/5.2/replication_rules.rs`（11 用例）、正文（teach，WHY/TASK/DONE WHEN/TRAP/FURTHER）。

判据是 4.3 那种“代码里的值 ↔ 页面上的表”双向核对：`RULES` 的四句话必须原样出现在页面里，
`FAILURES` 的五行必须与页面的 failure 表逐格相同。页面把规则写成**小写字面量**而不是大写加粗，
理由就写在这一步的 TRAP 里：spec 比的是逐字相等，强调标记会让“同一句话”变成两句话。

**原始结论行**：

```
$ kiln spec manifest --track meshkv
2 spec file(s) pinned — commit the manifest with the specs it pins.

$ kiln selftest --track meshkv 5.2-replication-rules
✓ meshkv/5.1-multi-node-topology (setup)
✓ meshkv/5.2-replication-rules (assert)
    scaffold ✓ the spec fails as it must (exit code 101: test a_term_moves_only_forward ... FAILED)
    solution ✓ reference patch applies
    solution ✓ the spec passes
    gates ✓ 9/9 must green

1/1 step(s) hold the contract
```

**踩到并修掉的一处**（第一版脚手架）：`committable` 的签名跨行，桩生成脚本按“签名首行 + `{ todo!() }`”
替换，产出的是 `pub fn committable( {`——**脚手架编译不过**。selftest 当时给了 `scaffold ✓`，
但附了一句 `authoring.md §5.4` 的提醒（“it does not compile on the scaffold”）；这正是 §5.4 的判据
（脚手架要能编译、规格在运行时失败）在起作用。修法：桩按**完整签名块 + 函数体**替换，重新生成两个 patch。
教训（第 5 条）：**脚手架“失败”必须是运行时的失败**，selftest 的 `✓` 旁边那句 note 要读。

## 段 3：B1 的三步 —— 5.1 拓扑、5.2 规则、5.3 教学版 Raft

**三步的关系**：5.1 给出"集群是什么"（身份、拓扑、一行握手），5.2 给出"日志的规则"（四个值 + 一页必须与代码逐字一致的说明），5.3 让规则动起来（选举、复制、提交）——而且 5.3 的判据里第一次有了**确定性模拟**。

**5.3 的形状**（`dist/consensus@apply`）：

- `src/raft/message.rs`：四种消息 + 严格文本编码（解码失败必须引用原行；条目载荷走 hex，写里带换行也不会破行）。
- `src/raft/state.rs`：**纯值状态机**——没有 socket、没有钟、没有随机。状态只因"收到消息"或"有人提案"而变。
- `tests/sim_raft.rs`：**课程内容**（脚手架交付的仪器）。turmoil 跑三台主机，虚拟钟 + 虚拟网络，
  seed 决定消息投递顺序；选举超时由 seed 抽；客户端说自己的话（`client propose …`，不是 Raft 消息）。
- `sim/5.3-teaching-raft.toml`：三个 seed（1 / 7 / 20260101），`replay = 1`。

**原始结论行**：

```
$ KILN_SIM_SEED=1 cargo test -p meshkv --test sim_raft -- --nocapture
meshkv-sim leader=n3
meshkv-sim committed=1
meshkv-sim entries=2
$ KILN_SIM_SEED=7 …        → leader=n2        （种子换了，胜者就换：门要的就是这个）
$ KILN_SIM_SEED=1 （连跑三次）→ 三次逐字节相同   （replay 的那一半）

$ kiln selftest --track meshkv
3/3 step(s) hold the contract
  ✓ meshkv/5.1-multi-node-topology   9/9 must green
  ✓ meshkv/5.2-replication-rules      9/9 must green
  ✓ meshkv/5.3-teaching-raft         11/11 must green   （含 slow:sim@3 与 slow:mutants@80）
```

**这一段钉下来的六条**：

1. **`turmoil` 的地址是 `192.168.0.0/16`，只许绑 `0.0.0.0`/`::`/localhost，连接按名字走**
   （`TcpStream::connect((name, port))`）；主机用 `sim.host(name, …)` 注册，DNS 自动分配 IP。
   第一次写 `1.1.1.1` 会得到 `AddrNotAvailable: 1.1.1.1:1234 is not supported`。
2. **`turmoil` 的 host 闭包是 `Fn`（可重启）**，所以它捕获的东西只能在闭包**里面**克隆——直接 `move` 一个
   `Vec`/`Arc` 进去会得到 `cannot move out of ... a captured variable in an Fn closure`。
3. **`tokio::join!` 不是 future**（它在宏里 await）；两个 await 的握手要把 listener 那一半
   `tokio::spawn` 成任务，调用的一半留在测试里。
4. **桩必须用掉自己的参数**（`let _ = (term, index);`），否则学员第一次构建就看到一堆他改不掉的
   warning，而 `clippy@4` 会因 `unused_variables` 变红；桩里未被用到的私有助手加
   `#![allow(dead_code)]`（课程自己的做法：4.21/4.24 的脚手架都这么写），参考解里去掉。
5. **改 patch 的内容必须同步改 hunk 头**（authoring.md §5.4）：本段第一次踩到——往"新建文件"的 hunk 里
   加行而忘了改 `+1,195` 这个数，`git apply` 居然 exit 0，而交付出去的文件在中间就断了。
   正解不是手改计数，是**重建两棵树再 `diff -u` 重生成**（规则 12）。
6. **桩的生成器要跳过闭合大括号那一行**（少跳一行就会多出一个 `}`；`lstrip('mut ')` 是按**字符集**剥，
   会把 `term` 削成 `erm`）。本段把生成器写成 `stubgen.py`（在 `/private/tmp`），按签名取参数名、
   保留 `Type::method` 的标记形态、给每个桩函数补 `let _ = (...)`。

## 段 4：5.4 快照与日志压缩（B1 收尾）

**5.4 做了什么**：`src/raft/snapshot.rs`（`Snapshot`：index + term + 不透明载荷）、
`Log` 学会基点（`base()`/`compact_to()`/`starting_at()`，并且 `at`/`append`/`truncate_from` 都按基点算）、
`Raft` 学会取快照/压实/装入（`take_snapshot`/`compact`/`install_snapshot`/`needs_snapshot`）、
`InstallSnapshot` 消息与四个臂、`docs/snapshot.md`、`tests/budget_snapshot.rs`（`bench-budget` 的仪器）。
7 个规格用例 + 5 个预算计数。

**原始结论行**：

```
$ kiln selftest --track meshkv
4/4 step(s) hold the contract
  ✓ 5.1  9/9 must green
  ✓ 5.2  9/9 must green
  ✓ 5.3 11/11 must green
  ✓ 5.4 11/11 must green   （含 slow:bench-budget@10 与 links）

$ kiln lint-caps → checked 101 of 119 step bodies · no issues
$ kiln links      → every target resolves
$ cargo test -p kiln → 338 + 21 + 1
```

**这一段钉下来的五条规矩（都很贵）**：

1. **后一步要扩展前一步的文件，前一步就得留标记区**。5.4 要给 `Log` 加方法、给 `Raft` 加字段/方法/匹配臂、
   给 `Message` 加变体——全是"插入"。所以 5.2/5.3 的**已发布文件**被改成带标记区的形态：`Log` 的 impl 内、
   `Raft` 的字段与 impl 内、`Message` 的枚举内、`term()`/`encode()`/`on_message`/`refuse()` 的 match 内。
   教训：**写一个"会被后人扩展"的类型时，先把挂载点留好**，否则只能回去改已发布的步骤（本段就这样做了，
   并且 5.2/5.3 的补丁全部重新生成、重新 selftest）。
2. **match 里的标记区不是空注释，是"臂的挂载点"**。空标记区让 match 在那一点"没有臂"，编译仍穷尽；
   5.4 的脚手架往四个 match 各插一条臂，文件照样编译、规格照样红在 todo 上。
3. **catch-all（`_ => ...`）解决不了这个问题**：它在"变体还没加"时是 unreachable（要 `#[allow]` 才不报警），
   而且加了变体之后它把真臂吞掉。标记区才是正解。（本段先试了 catch-all，回滚。）
4. **补丁生成必须在"格式化之后"**：先 `cargo fmt --all` 再 `diff -u`，否则交付的树第一道 `fmt` 门就红。
   本段的 `build_b1.py` 因此有 `--patches-only` 模式：重建 → 格式化 → 只重生成补丁。
5. **跨行签名会骗过桩生成器**（`pub fn new(\n  a: A,\n) -> Result<Self> {` 的首行不以 `{` 结尾）。
   第一版 `stubgen.py` 把整个 `Snapshot::new` 的实现留在了桩里，于是桩引用 `Error` 而 import 被剥掉，
   脚手架编译失败。修法：按 `fn 名字(` 识别函数起点，再往下收签名直到 `{`。

## 段 5：5.5 成员变更（B1 收口）——联合共识

**5.5 做了什么**：`src/raft/membership.rs`（`Config`、`Membership::{Stable, Joint}`、`encode`/`decode`）、
`Raft::membership()` / `begin_membership()`、把三处"谁说了算"接到 membership 上（竞选、计票、提交），
提交时采纳配置（leader 与 follower 都采纳）、`peers()` 在变更期是两套并集、`tests/sim_membership.rs`
（纯值集群 + 种子决定超时与投递序）、`sim/5.5-membership.toml`。9 个规格用例。

**原始结论行**：

```
$ KILN_SIM_SEED=1   → elect leader=n1   write index=2 committed=1   change voters=5 joint_before=1 joint_after=0   write index=4 committed=1 joined=1
$ KILN_SIM_SEED=7   → elect leader=n3   （同三条不变）
$ KILN_SIM_SEED=20260101 → elect leader=n2
$ KILN_SIM_SEED=1 连跑两次 → transcript 逐字节相同（sha 前 16 位一致）

$ kiln selftest --track meshkv
5/5 step(s) hold the contract
  ✓ 5.1 9/9 · 5.2 9/9 · 5.3 11/11 · 5.4 11/11 · 5.5 11/11
```

**这一段钉下来的四条**：

1. **"只让一个节点超时"是确定性模拟里唯一诚实的选举起点**。第一版让三个节点同时 `campaign()`，
   结果都投自己、无人过半（真实 Raft 靠各自的超时错开，而值层模型没有钟）。改成 **seed 决定谁的超时先到**：
   一个候选、两个应答，胜者随 seed 变——这既复现了真实机制，又让 `sim` 门的"种子必须起作用"成立。
2. **配置的采纳是"已提交日志"的性质，不是本地 `pending` 的性质**。第一版要求 `pending == 该配置` 才采纳，
   于是"新加入的节点收到已提交的配置条目"什么都不会发生。改成：只要扫描到本段新提交的配置条目就采纳；
   规格里用"未提交不变、提交即变"两条钉住。
3. **`peers()` 在 joint 期必须是并集**，否则要离开的节点收不到它还没提交的那条配置，变成永远差一个人的死锁。
4. **`-D clippy::expect_used` 会命中"不可能失败"的 `expect`**（`Config::new(self.members.clone()).expect(...)`）。
   正解不是 allow，是给内部调用一个**免校验的构造器**（`pub(crate) fn from_voters`），并把"为什么这里不用检查"
   写在它的文档里。

## 段 6：5.6 线性一致读与租约读（B2 第一步）

**5.6 做了什么**：`src/raft/read.rs`（`Lease`、`ReadAnswer::{Served, NotLeader, NeedsQuorum}`、
`LEASE_MILLIS = 400`、`ELECTION_TIMEOUT_MILLIS = 1000`、`is_safe_lease`）、`Raft` 的读路径
（`confirm_read` 记住 read index 并发心跳、`complete_read` 多数派确认后才授租约、`serve_read`
按角色/租约作答）、以及"失去领导权就丢租约"（`campaign`/`step_down`/同 term 的 `on_append`
三处）。9 个规格用例。

**判据的核心不是代码，是时间的关系**：`is_safe_lease(lease, timeout)` 必须严格小于，
课程自己的两个常量必须满足它——规格里有一条专门检查这件事。

**原始结论行**：

```
$ kiln selftest --track meshkv
6/6 step(s) hold the contract
  ✓ 5.1 9/9 · 5.2 9/9 · 5.3 11/11 · 5.4 11/11 · 5.5 11/11 · 5.6 9/9
$ kiln lint-caps → no issues      $ kiln links → every target resolves
```

**这一段钉下来的两条**：

1. **"现在"必须是参数，不能是调用**。租约的每个函数都收 `now`（毫秒）：自己读钟的函数既没法测
   （测试会和窗口赛跑），也没法被 `5.13` 的虚拟钟驱动。这条与 `time/clock-injection@apply — jobq/3.8`
   是同一件事，但在分布式读路径上它是**正确性**问题而不是可测性问题。
2. **元数据里的 `spec` 路径来自步骤名**（`5.6-lease-reads` → `spec/5.6/lease_reads.rs`，测试目标
   `spec_lease_reads`）。我按 `read.rs` 命名文件，`missing_content` 判成"还没写正文"，
   selftest 于是报 `0/0`——**`0/0 held` 这个输出本身就是"这一步还没被作者化"的信号**，要读它。
