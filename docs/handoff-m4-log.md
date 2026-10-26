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

## 段 7：5.7 客户端路由与重定向（B2 第二步）

**5.7 做了什么**：`src/client.rs`（`Answer::{Served, Redirect, NoLeader}`、`Transport` trait、
`Client`（跟随重定向、环路检测、跳数上限 `MAX_HOPS`、记住 leader）、`leader()`/`hops()`）、
`Error::NotLeader { leader: Option<String> }`（跟随者拒绝写入时点名 leader）、`propose` 改为返回该错误。
8 个规格用例。

**原始结论行**：

```
$ kiln selftest --track meshkv
7/7 step(s) hold the contract
  ✓ 5.1 9/9 · 5.2 9/9 · 5.3 11/11 · 5.4 11/11 · 5.5 11/11 · 5.6 9/9 · 5.7 10/10
```

**这一段钉下来的五条**：

1. **错误枚举里的 `use` 不能放在变体区**（`use` 是 item，不是变体）——脚手架一插就被编译器骂。
   而"脚手架只能写进标记区"又意味着**不能**在文件头插 `use`。所以变体里带的是 `Option<String>`
   （错误消息要的是名字，不是类型）；类型化的重定向在 `client.rs` 的 `Answer` 里。
2. **改骨架就是改所有后续步骤的交付物**：给 `error.rs` 加标记区之后，5.1–5.6 的树都带着它，
   `fmt` 门先红（我加的空行不符合 rustfmt）。**改骨架之后要把所有树同步并重跑整条轨道的 selftest**，
   不能只看新步骤。
3. **桩生成器会被字符串里的花括号骗**：`format!("{current}")` 里的 `{` 让朴素的括号计数失衡，
   生成器把整个文件后半段吞进了桩里（第一版把 `Client` 结构体也吃掉了）。
4. **桩生成器会跨过 trait 的方法声明**：`fn call(...);` 以 `;` 结尾、没有函数体，生成器继续往下找 `{`，
   于是把下一个 item（一个结构体）当成了它的函数体。修法：签名以 `;` 结尾就跳过。
5. **脚手架"只能写进标记区"这条规则会在你违反它时立刻抓住你**——本段第一次撞上它是在给 5.7 插
   `use crate::cluster::NodeId;` 时，报错原话就是"outside a `kiln:scaffold:begin`/`kiln:scaffold:end`
   block: scaffolds may only write inside marked regions (authoring.md §5)"。

## 段 8：5.8 幂等会话（B2 第三步）

**5.8 做了什么**：`src/idempotency.rs`（`Session` + `key_for`、`Answers`（键 → 答案 + 时刻）+ `expire`、
`Ledger::apply`（见过的键不再应用、闭包不调用、原答案返回）、`default_window`）。8 个规格用例。

**原始结论行**：

```
$ kiln selftest --track meshkv
8/8 step(s) hold the contract
  ✓ 5.8 9/9 must green（scaffold 红在 "a retry is answered with the original answer and is not applied again"）
```

**这一段钉下来的四条（三条是工具链的，都很隐蔽）**：

1. **`diff` 在"有差异"时退出码是 1**——用 `&&` 串「生成三个补丁」的命令会在第一个 diff 处断掉，
   于是补丁只写了一半（lib.rs 的段没了），而**文件看起来是生成了的**。分隔符必须是 `;`。
2. **补丁必须在真实补丁链上验证**：单独对"上一棵树"能应用不代表对"轨道累积出来的树"能应用。
   做法（本段起成为规矩）：建一个 scratch 目录，从骨架开始按 5.1→5.x 逐笔 `git apply` 骨架与解，
   再 `git apply --check` 新的一对。"文件在规划后被改过"这类错误只有这条链能提前抓到。
3. **桩生成器的两个真 bug**（`out[:-len(signature)]` 会吃掉紧邻的文档行；参数切分用了行里**最后一个**
   `)` 而不是与左括号配对的那个）。症状分别是：桩丢了 `# Errors` 段（`docs:public` 门红）、
   参数变成 `(key, ) -> Result<()>`。**生成器要先在一个 9 行的最小例子上验证再跑全量。**
4. **文件系统会对特定路径偶发 `NotFound`**（`readdir` 列得出来、`stat` 却说不存在，同一进程里
   前后两次判断还能不同）。kiln 的 `missing_content` 因此报 `its spec file is missing`，而
   `selftest` 把它显示成 `0/0 held`。**`0/0` 的意思是"这一步还没被当成已作者化"**，先看 `verify`
   的那一行；本段的绕法是换一个规格文件名（元数据与 `run.cmd` 一起改）。

## 段 9：5.9 mTLS（B2 收口）——身份是证书证明的，不是握手声称的

**5.9 做了什么**：`src/security.rs`（`Certificate`（主题/签发者/半开窗口）、`TrustStore`、
`verify`（三条规则、每种拒绝各说各的原因）、`authenticate`（证书主题必须等于声称的名字）、
`Identity`（无公开构造器——`verify`/`authenticate` 是唯一的门）、`Rotation`（当前/下一张、
`presented(now)`、`accepts`、`earliest_safe_retirement`））、`docs/mtls.md`、`tests/sim_mtls.rs`
（轮换场景，seed 决定握手顺序）。10 个规格用例。

**原始结论行**：

```
$ KILN_SIM_SEED=1 / 7 / 20260101 → 三个不同的握手顺序，同一组 verdict（overlap 全接受、退休后只接受新的）
$ 同 seed 重放 → transcript 逐字节相同

$ kiln selftest --track meshkv
9/9 step(s) hold the contract
  ✓ 5.9 11/11 must green（含 slow:sim@3 与 links）
```

**这一段钉下来的两条**：

1. **"已证明的身份"要是一个类型**。`Identity` 没有公开构造器，所以"这个 peer 认证过"是类型事实而不是
   约定——`domain/parse-dont-validate@apply` 用在安全决策上。规格里有一条用例把这个形状写出来
   （拒绝时 `Result` 里根本没有 `Identity`）。
2. **轮换的算术要作为函数暴露**（`earliest_safe_retirement(overlap)`），而不是写成页面上的一句话：
   "新证书生效时刻 + 分发耗时"是能判的东西，而"记得留够时间"不是。

## 段 10：5.10 确定性状态机 + 5.11 客户端 SDK（B3 前两步）

**5.10**：`src/kv.rs`（`Command` set/cas/delete 严格解析、`Kv`（`BTreeMap` + applied index）、
`Applied`、`digest`（FNV-1a 按键序）），`tests/sim_kv.rs`（两台机器同日志同 digest）。10 个用例。
**关键补丁**：leader 赢得选举时写的**空条目**必须能被状态机应用（no-op、推进 index）——否则
"应用整个已提交前缀"在第一次选举后就卡住。这条是 5.11 的 in-process 后端逼出来的。

**5.11**：`src/sdk.rs`（`SdkError` 四类 + `is_retryable`、`Reply`、`Backend` port、`Sdk` 重试策略
（键只算一次、跟随重定向、`MAX_ATTEMPTS`、拒绝不重试））、`conformance.toml`
（`backends = ["scripted", "in-process"]`）、`tests/support/mod.rs`（两个实现）、`docs/sdk.md`。8 个用例 × 两个后端。

**原始结论行**：

```
$ KILN_BACKEND=scripted   cargo test -p meshkv --test spec_client_sdk → 8 passed
$ KILN_BACKEND=in-process cargo test -p meshkv --test spec_client_sdk → 8 passed
$ kiln selftest --track meshkv → 11/11 step(s) hold the contract
```

**这一段钉下来的四条**：

1. **`conformance@2` 真的会抓到东西**：scripted 后端第一次就全绿，in-process 后端暴露了三个真缺口——
   没应用 leader 的空条目、没应用"整个已提交前缀"、CAS 不匹配被当成功。**同一套规格在两个实现上跑**
   是这一步最有价值的部分。
2. **CAS 在提案前拒绝**（leader 能看见的失败不要写进日志），否则状态机会永远回答 `Mismatch` 而调用方
   分不清"被拒绝"和"写得慢"。
3. **桩生成器第三次被同一类问题咬**：多行 **trait 声明**（`fn write(...);`）既没有函数体也不能丢参数。
   规则：**声明原样保留、定义才替换**；而且 `;` 行只输出一次。
4. **`Box<dyn Trait>` 不会自动实现 trait**：SDK 要泛型于 port，就必须给 `Box<dyn Backend>` 一个转发实现，
   否则"运行时选实现"的调用方（正是 conformance 的用法）编译不过。

## 段 11：B4 收尾与 B5 —— 5.16 到 5.22（七步，T5 的 22 步全部写成）

**5.16 混沌矩阵**（`slow:sim@3`）：`src/chaos.rs` 的 `MATRIX`（四种故障 × "客户端看到什么" × "必须仍成立什么"）
与 `docs/chaos.md` 的同一张表逐行核对；`check()` 拒绝重复行、空单元、两列同话。`tests/sim_chaos.rs` 为每行
造一个剧本。

**5.17 分布式预算**（`bench-budget@10` + `links`）：`src/budget.rs` 的 `cost`/`workload`——quorum `⌊n/2⌋+1`、
fan-out `n−1`、acks `quorum−1`；预算文件以 n=5、1000 写为基线。

**5.18 集群可观测**（`bench-budget@10` + `links`，deepen）：`src/metrics.rs` 的 `NodeMetrics`（lag/选举/拒绝/提交）
与整数 `Slo::budget_left`；正文用 RECALL/DELTA（deepen 步不许有 WHY——R5 抓到了我第一版）。

**5.19 unsafe 深潜**（`links`）：`src/ring.rs` 的 SPSC 环（`MaybeUninit` + 两个原子）、三个义务各带 `// SAFETY:`
（门只在 3 行回看里认注释）、`loom` 两个模型；manifest 的 `unsafe_code = "deny"` 在这步摘掉，轨道元数据从 5.19
起改为 `unsafe-budget@5`。

**5.20 复用 jobq**（`links`，D6 的唯一例外）：`jobq = { path = "../jobq" }` + `src/reuse.rs` 的
`pub use jobq::engine::JobId` / `job_key` / `record_job`；验收是"`jobq` 零改动"。

**5.21 协议演进**（`slow:sim@3` + `links`）：`src/protocol.rs` 的 `Protocol`（major 不可混、minor 非对称兼容：
老构建认不出更新的 minor）、`accept` 的拒绝同时点名两个版本、`Upgrade::phases()`（除最后一个节点外全部 `compat`）。

**5.22 一致性审计**（`links`）：`src/audit.rs` 的 `audit(&[State]) -> Report`——共同前缀、谁发散（同前缀不同
digest）、一句可执行的 `summary()`。

**原始结论行**：

```
$ kiln selftest --track meshkv
22/22 step(s) hold the contract      （22 步，每步 scaffold 红 / 解绿 / must 门全绿）
$ kiln lint-caps
97 capabilities · 136 slots · 119 ledger positions · 119 authored steps · no issues
$ kiln links → every target resolves
```

**这一段钉下来的五条**：

1. **deepen 步不许有 `## WHY`**（R5）：5.18 的第一版写了 WHY，被"origin = deepen must not re-explain with
   `## WHY`"直接拦下；改成 RECALL/DELTA 才过。
2. **`unsafe-budget` 的 `// SAFETY:` 只回看 3 行**：多行解释会被算作"没有注释"，把说明压到两行以内。
3. **生产代码禁下标**（`clippy::indexing_slicing`）：`self.slots[i]` 换成 `.get(i)` 并处理 `None`。
4. **路径依赖的相对路径相对 crate 根**：`jobq = { path = "../jobq" }` 在 `app/meshkv` 里指向 `app/jobq`；
   scratch 树里要放一个同形状的 stand-in 才能本地编译。
5. **`drop 1`/`partition` 的单链路不是隔离**（5.13/5.14 的教训在 5.16 的矩阵里也成立）：剧本要隔离一个节点得
   切它和**每一个** peer 的链路。

## 段 12：第二轮评审与修复（参考代码、内容、框架三线）

**评审怎么做的**：两个子 agent 分别读参考解与课程内容，逐条对着正文/规格/补丁找"说了但没做"和"做了但会错"。
结论分三线：

**框架（kiln）**：`MissingDependency` 只说"缺前置 track"，不说从哪一步开始跑——补 `first` 字段（取前置 track 的
第一步）并把 `kiln verify` 的 blocked verdict 的 `fix` 指到 `kiln run <first>`；`state` 的读只把 NotFound 当默认
（权限/损坏不再静默重置）、写改为临时文件 + `sync_all` + rename、并发合并走 `.kiln/state.lock`；补丁器在
`Solution` 模式下拒绝删除一个被改过的文件，锚定改为"旧行非空才要求上下文匹配"；`Plan::apply` 出错回滚已写文件。

**参考代码（21 个 step 树）**：九处真 bug，全部是"能跑但语义错"：

1. `Log::len()` 被当成集群下标——压缩之后 `len()` 比最后下标小；新增 `Log::last_index()/last_term()`，所有位置
   计算改走它们；
2. `last_position()` 在日志被快照替换后返回 `LogPosition::empty()`——回退到快照的位置；
3. `pending` 成员配置跨任期存活——`campaign`/`step_down` 清掉；
4. `compact` 接受"提交下标处但任期不同"的快照——拒绝；
5. `matches()` 接受快照覆盖范围内的任意下标——只认快照自己那个位置（更低的下标查不了，要让 leader 发快照）；
6. `Ring::len()` 两个原子读不是同一时刻——`saturating_sub`；
7. drill 的 `isolated()` 用"切断了与每个 peer 的链路"——改成"够不到多数派"，否则健康集群也会被判成脑裂；
8. audit 只比 `min(applied)` 那一个下标——比每个至少两个节点到达的下标；
9. `is_retryable()` 把 `RetriesExhausted` 也算可重试——SDK 刚做出的决定，不该让调用方再问一遍。

**怎么落的**：手改 5.2 的补丁把 hunk 头改坏了（`git checkout` 回滚）；"重放补丁链再修"在 5.5 处因 5.4 的上下文
变化失败；最终做法是**先修树、最后生成补丁**：把修复应用到 `s52…s522` 的 stub 与 full 树，再 `diff -u` 生成
scaffold（上一步 full → 本步 stub）与 solution（本步 stub → 本步 full）。两条规则是这次踩出来的：

- **补丁段的标签与顺序以已提交补丁为模版**：`--- /dev/null` 是 kiln 判"创建文件"的唯一依据，`--- a/path` 会被当成
  "修改一个已有文件"，于是"the file does not exist"；段序与标签从 `git show HEAD:…patch` 读出来，重新生成后
  逐字节可比。
- **scaffold 只能加不能删**：5.4 的 stub 里 `last_position` 还是 5.3 的形状，把快照回退的修复加进 stub 会让
  scaffold 删一行（被 `authoring.md` §5 拒绝）——所以那处修复属于 5.4 的**参考解**，stub 保持原样；同理 5.2 的
  stub 里 `last_index/last_term` 是洞，5.3 起的 stub 才是实现（否则会把上一步的工作回退成洞）。

**内容**：5.6/5.8/5.10 的 DONE WHEN 里的测试目标名（`spec_read`/`spec_idempotency`/`spec_kv`）与实际规格文件名
（`spec_lease_reads`/`spec_duplicates`/`spec_deterministic_apply`）对齐；5.16 的 sim 头写错步骤名（`chaos-injection`
→ `chaos-matrix`）；5.16/5.21 正文承诺"`links` judges it"却没挂门，补上；5.17 的页面 7 节点 ack 数 2000 → 3000、
"per-node average falls" 改成 "rises"（模型是 `(n−1)/n`），模块文档与规格测试名一起改；5.11 的页面与规格把
`RetriesExhausted` 不可重试写成明文；m4-plan §2/§6 的门阶梯与 track.toml 对齐（mutants 4 步、sim 9 步、
bench-budget 3 步、links 12 步）；README 的 T5 状态与计数刷新。

**规格两次补钉**：5.2 增加 `the_log_reports_its_own_end`（`last_index/last_term` 是交付物——5.3 的 scaffold 会调用它们，
不写进规格，学员的树在下一步就编译不过）；5.11 增加"`RetriesExhausted` 不可重试"的断言。`kiln spec manifest` 重钉 22 份。

**第三次踩到同一条**：手插的 `isolated()` 修复没走 rustfmt，5.14 的 `fmt` 门红了一次
（`*other != node && ...` 被拆成三行）。修复应用到树之后必须 `rustfmt` 再 `diff -u`——这正是 §1 第 6 条的由来，
这次是它第三次抓到同一类疏忽。

## 段 13：BP —— T5 的 18 道探题（池 65 → 83）

按 D-6 排除 5 个槽位（`aware` 政策、四个机制不在单文件里的 design/operate，理由见 [t5-meshkv.md](t5-meshkv.md) §6），
其余 18 个槽位各一道，四批并行：

| 批 | 槽位 |
| --- | --- |
| A | `dist/topology@apply`、`dist/consensus@apply`、`dist/snapshot@design`、`dist/membership@apply` |
| B | `dist/linearizable-read@design`、`dist/client-routing@apply`、`api/idempotency-key@design`、`sec/mtls@apply` |
| C | `dist/deterministic-state-machine@apply`、`api/client-sdk@apply`、`query/consistency-levels@apply`、`test/deterministic-simulation@design` |
| D | `test/chaos-injection@design`、`perf/distributed-budget@design`、`obs/metrics-slo@operate`、`unsafe/hand-rolled@operate`、`dist/protocol-evolution@design`、`ops/rolling-upgrade@design` |

**实测**：`kiln probe --check` → `83 exercise(s) · 83 reference(s) passed`；每对文件 7–10 条用例、`diff -u` 的 hunk
2–6 个（每个 hunk 只删桩行）、`rustfmt --check` 全过、**没有 `unsafe` 关键字**（5.19 的题用安全代码重抽"下标协议"：
满/空的判定、`saturating_sub`、容量掩码——`UnsafeCell` 与别名那一半在步骤里，不在题里）。5.8 的题与已有的
`delivery-idempotency.design` 明确分工：那道是去重**窗口**，这道是**会话**——键归 SDK、重启后能认出重试、重复答复
带原来的下标。

## 段 14：BV —— 毕业树能判什么，不能判什么

尾巴树（22 步解全铺）上 `kiln verify meshkv` 只对 **5.19–5.22** 给出逐门判定：5.1–5.18 的 `unsafe-budget@0`
数到了 5.19 才写下的 `src/ring.rs`。前缀树（只铺到 5.18）又反过来编译不了 5.19+ 的规格（`verify` 会把整条
track 的规格都铺进 `tests/`）。结论写进 [t5-meshkv.md](t5-meshkv.md) §5/§6：逐门证据以 `selftest` 的 22/22 为准，
`--slow` 的慢门（sim 9、bench-budget 3、conformance 1）在毕业树上全绿，`mutants` 四步被那条 must 门挡住。
要一条命令全判绿，`unsafe-budget` 得按"本步补丁写下的文件"计数（mutants 已是这个口径）——留给 kiln，不是 M4 的欠账。

## 段 15：把 `verify` 从"只判最后四步"救回来

段 14 留下的缺口（`unsafe-budget` 按全项目计数，毕业树上 5.1–5.18 数到 5.19 的 `src/ring.rs`）按 mutants 的口径修掉：
门只数**本步 scaffold/参考解写下的文件**（新增 `sources::patched_sources`，脚手架是 skeleton 时不贡献文件）；
`unsafe_code = "deny"` 的要求只在**项目整体没有 unsafe** 时成立——后一步合法地放开了它，前一步不能被它判红。
两条规则各配一个单测（`the_files_a_step_writes_are_what_its_budget_counts`、`the_deny_is_required_only_while_the_project_holds_no_unsafe`）。

**第二个坑**：`mutants`（和 `coverage`）把项目复制进临时目录，但 `meshkv` 的 `jobq = { path = "../jobq" }` 在副本里
指向一个不存在的目录，四个 mutants 步全 `blocked`（"failed to load manifest for dependency `jobq`"）。
`sandbox::copy_project_and_dependencies` 现在把 `requires_track` 的项目复制到副本**旁边**（`Track::project` 是
`app/jobq`，副本里要用目录名 `jobq`——第一次按整条路径拼出 `app/app/jobq` 才踩到）。两个调用点（mutants、coverage）都换了。

**结果**：`kiln verify meshkv --slow --json` → `{blocked: 0, clean: true, failed: 0, not_authored: 0, passed: 22, steps: 22}`，
exit 0，42 s；慢门真跑：`mutants@80` 10/12、12/12、4/5、6/6，`sim@3` 九步各自的 transcripts，`bench-budget@10` 三步，
`conformance@2` 两个后端。M4 的验收行「`kiln verify meshkv --slow` 在尾巴树上全绿」至此成立。

## 段 16：1.21 的容器 flake

`cargo xtask ci` 第一次跑挂在 `mailer/1.21-container`：四个用例并行，两个 `podman build` 撞在同一个层缓存与两个命名
卷上，报的是 build 失败（不是超时），同一用例单独跑是绿的。修法是在 `tests/support/container.rs` 的 `build()` 里加一把
进程内互斥锁（`static BUILDS: Mutex<()>`，中毒也取回），构建因此串行；有缓存时排队的那几个是秒级。这是课程内容
（1.21 的 scaffold），补丁段用 `diff -u` 重新生成了一个新文件 hunk，`selftest mailer/1.21-container` 复验。
