# M6 施工日志

> M6 的范围与验收在 [m6-plan.md](m6-plan.md)。本日志按段追加；M5 的日志在
> [handoff-m5-log.md](handoff-m5-log.md)。

## 段 1：计划落定，第一批四道探题（2026-10-28）

**为什么先做探题。** 空白分析（用户提问触发的三次盘问）给出了起点：97 能力 / 136 槽位 / 119 步 /
83 探题，深度 `apply 79 / design 36 / operate 13 / aware 8`。缺的东西分两类——地图的横向几块
（缓存、broker、SQL 索引与复制、gRPC/长连接、编排/身份）和路径本身的洞（**24 个 apply 槽位没有
探题**、operate 薄、没有 capstone）。两类里**只有探题是纯加法**：不动台账、不动 track、不动补丁，
而它直接决定 `plan --fast` 能跳什么、`review --due` 会提醒什么——间隔重复这条复利现在只覆盖 61%。

计划写在 [m6-plan.md](m6-plan.md)：M6.0 探题（六批 × 4 道）→ M6.1 缓存 T6 → M7 broker T7 →
M8 RPC T8 → M9 platform T9 → M10 capstone。顺序的理由、每阶段的验收、成本口径都在那份文件里，
本日志只记施工。

**第一批（A）四道**，每道都是"练习 + 参考解，只差函数体"，并且单独验证过两遍（参考解 6/6 或
7/7 通过；练习文件在 `todo!()` 上全挂）：

1. **`db/transactions@apply`（教它的步：`mailer/1.17`）**——`Ledger::transfer` 必须"先决定、后写
   两边"。测试的重点是**拒绝路径不留部分结果**：目标账户已关闭时，借记不能已经落地。另有一条
   200 次确定性转账的不变量测试（总额不变、没有账户超过总额）。
2. **`api/pagination@apply`（`relay/2.4`）**——`page_after(cursor, limit)`。测试的重点是
   **两页之间插入新行**：偏移分页会重复或跳过，游标分页不会；以及 `next` 是"下面还有一行"的
   承诺，不是"这一页没满"。
3. **`async/graceful-shutdown@apply`（`mailer/1.14`）**——先关门再排水。预算用"能跑几个任务"
   而不是墙钟（探题必须确定性）；测试钉住"每个接下的任务恰好落在 completed 或 dropped 之一"，
   以及第二次 drain 什么都不做。
4. **`sec/password-hash@apply`（`mailer/1.11`）**——记录的形状：每条一个盐、成本写进记录、
   比较不停在第一个不同字节、旧成本记录能验证且要求重哈希。密码学原语是玩具（文件里写明了），
   检验的是它外面那层记录。

**顺手加的一行**：`kiln probe --check` 现在打印覆盖率（`59/79 apply slot(s) have a probe`）并列出
缺的槽位，指向这份计划的批次表。它现在是信息而不是失败——一红就是 20 道题的量；当最后一道落地时，
把这条改成失败（一行）。这是 M6.0 的验收从"人肉数"变成"机器数"的那一步。

**验收（本机实测）**：`kiln probe --check` 87/87 参考解通过、覆盖率 59/79；`cargo test -p kiln`
在本轮末尾跑（见提交信息）；`cargo fmt`/`clippy` 只改了 `kiln/src/cmd/probe.rs` 与四对探题文件。

**下一批（B）**：`repo/workspace-layout`（1.1）· `repo/toolchain-pinning`（1.2）· `ci/pipeline`（1.3）·
`sec/secrets-management`（1.4）——这一批全是"工程骨架"类，写法与 A 不同：题目给一个错误的工作区/
工具链/流水线，让人判断它错在哪、改成什么，而不是补一个函数体。

## 段 2：第二批四道（工程骨架类，2026-10-28）

B 批和 A 批的题型不同：A 是"补一个函数体"，B 是**判断**——给一个工作区/工具链/流水线/日志，
找出它错在哪。四道都写成"输入是字符串或小结构，输出是违例列表"，因为探题不能开 cargo、不能起 CI、
不能连 rustup；每道都把"替换了什么、为什么不是假的"写在开头：

1. **`repo/workspace-layout@apply`（1.1）**——给一组包（lib/bin + 依赖），报告两件事：库依赖了
   二进制（方向错），以及强连通的一组（环，一组一个违例，不是一条边一个）。环用 Tarjan，放在
   `check` 的函数体里（探题的约定是"只有函数体不同"）。
2. **`repo/toolchain-pinning@apply`（1.2）**——给若干文件内容，扫描 `channel =` / `toolchain:` 两种
   钉法，报告"没钉"、"钉了两次"（版本相同也是两次）与"两处不一致"。`scan` 是给定的（机械部分），
   `audit` 才是题。
3. **`ci/pipeline@apply`（1.3）**——给一份 workflow 文本和任务运行器跑的检查列表，报告"没有调用
   `cargo xtask ci`"以及"哪一行自己又跑了一遍 `cargo fmt/test/clippy`"。`#` 之后是注释（与 YAML
   一致），所以"任务行后面注释里提到 cargo test"不会误报。
4. **`sec/secrets-management@apply`（1.4）**——`redact`：按值、最长优先、空条目忽略（空串会匹配
   每个位置，把整行抹掉，比泄漏更糟）、大小写敏感（密码是字节串）。测的是"最后一道防线"。

**两处测试自身的毛病，都当场修了**：工具的 `both_syntaxes…` 原本只测给定的 `scan`，在练习文件上
会通过——探题的约定是**每条测试在 `todo!()` 上都要挂**，改成同时断言 `audit`；`redact` 的
连接串期望值写错了位置（`postgres://` 后面才是被替换的那段），参考解自己把它抓出来了。

**验收**：`kiln probe --check` **91/91** 参考解通过、覆盖率 **63/79**；每道练习文件在 `todo!()` 上
全挂（9/7/7/7 条）。`cargo fmt`/`clippy`/`test -p kiln` 在本轮末尾跑。

**下一批（C）**：`obs/structured-logs`（1.6）· `http/service-skeleton`（1.7）· `test/app-factory`（1.8）·
`db/migrations`（1.9）——偏"协议与契约"：日志事件的字段、404/405 的诚实、装置与生产的同一性、
迁移的不可编辑性。

## 段 3：第三批四道（协议与契约，2026-10-28）

C 批的题型是"把一条约定写成一个可判定的判断"：

1. **`obs/structured-logs@apply`（1.6）**——`line`：`event=… run.id=…` 之后按给定顺序拼字段，所有
   值过 `render_value`（给定）。测七条：没有字段也带 run.id；字段顺序；空格/引号/空值/`=` 的引号规则；
   以及**值里有换行时一行仍是一行**（转义成 `\n`）。
2. **`http/service-skeleton@apply`（1.7）**——`Router::handle`：未知路径 404 且 `allow` 为空；
   已知路径配错方法 405 且 `allow` 是该路径注册的方法（不是全局的）；`HEAD` 就是没有正文的 `GET`。
   最后一条测试特意用了只有 `POST` 的路由：对它发 `HEAD` 必须 405，不能"因为是 HEAD 就放行"。
3. **`test/app-factory@apply`（1.8）**——`start`：部件顺序固定（router → database → mail），端口来自
   `Ports::next` 而**从不**读 `config.port`。测的是"固定端口被忽略"与"两个应用不会拿到同一个端口"，
   后者正是段 1 里那个"共享 target 目录"事故的同一类错误。
4. **`db/migrations@apply`（1.9）**——`plan`：先查历史（已应用的迁移不能缺失或改动），再算待应用，
   最后拒绝"编号插进历史中间"的新迁移。四条错误路径各一条测试，外加"已是最新 = 空计划"。

**验收**：`kiln probe --check` **95/95**、覆盖 **67/79**；四道练习在 `todo!()` 上全挂（7/7/6/7）。

**下一批（D）**：`test/fake-external`（1.13）· `sec/authn-session`（1.16）· `test/integration-layering`
（1.19）· `release/container-image`（1.21）——偏"替身与发布"：端口后面的假实现、会话的有效期与撤销、
测试之间的资源隔离、镜像的基线钉版与非 root。

## 段 4：第四批四道（替身与发布，2026-10-28）

1. **`test/fake-external@apply`（1.13）**——端口、假实现与用例三分：`Mailer` trait 是端口，
   `FakeMailer` 记录被要求发送的消息、可以被设定成失败（**失败不记账**），`send_confirmation`
   收 `&mut dyn Mailer` 而不是假实现本身。最后一条测试用第二个实现（计数器）走同一个用例，
   这就是"用例不认识假实现"的编译期证明。
2. **`sec/authn-session@apply`（1.16）**——`authenticate` 的三种拒绝与顺序：未知 → 已撤销 →
   已过期（撤销是人的决定，过期是时间流逝，两者同时成立时报前者）。截止时刻是**开区间**：
   `now == expires_at` 已经过期。
3. **`test/integration-layering@apply`（1.19）**——审一个测试清单：任意两个测试共用一个资源
   （`real:` 开头的算真实依赖，其余是夹具）都要报，`Unit` 测试用真实依赖另报一条。假实现被两个
   测试共用也是冲突——"只是个假实现"正是上一个测试的收件箱变成下一个测试初始状态的方式。
4. **`release/container-image@apply`（1.21）**——读 Dockerfile：每个 `FROM` 必须钉版（非
   `latest` 的 tag，或 digest；`scratch` 天然钉死），**最后一个** stage 必须选非 root 的 `USER`
   （builder 的 `USER` 不算）。

**验收**：`kiln probe --check` **99/99**、覆盖 **71/79**；四道练习在 `todo!()` 上全挂（6/7/7/8）。
D1 写的时候差点留下一条"只测给定代码"的测试（`FakeMailer::new().sent()` 本来就是空），按 probe
的约定换成了走用例的 token 原样插入测试。

**下一批（E）**：`ops/runbook-drill`（1.22）· `http/middleware-layering`（2.1）·
`api/idempotency-key`（2.2）· `api/contract-testing`（2.3）——偏"接口与演练"。

## 段 5：第五、六批，覆盖率改为失败，M6.0 完成（2026-10-28）

**E 批（接口与演练）**：`ops/runbook-drill`（1.22，程序里每一步的前置条件必须由**更早**的步骤
建立；自己提供自己的前置条件不算）、`http/middleware-layering`（2.1，logging 最外层、限流在鉴权
之前、鉴权在处理函数之前；缺的层不审计）、`api/idempotency-key`（2.2，同键同载荷重放、同键不同
载荷冲突、无键不记录）、`api/contract-testing`（2.3，删端点/删请求或响应字段是破坏性，加不是）。

**F 批（容量与降级）**：`async/backpressure`（2.14，容量算"排队 + 运行中"，拒绝的任务不入队）、
`test/chaos-injection`（2.17，脚本化的故障按顺序消费，延迟按 tick 预算判超时）、
`ops/rolling-upgrade`（2.22，每个仍在线上的版本只能提及 schema 里存在的列）、
`ops/degradation-modes`（2.24，只按特性自己的依赖判断，fallback 决定剩下什么）。

**测试自身又抓出三处**：E1 的"完整 runbook"夹具里有一个没人提供的前置事实；E4 的排序断言把
`FieldRemoved` 排在 `Removed` 前面（枚举声明顺序才是排序依据）；这些都在参考解运行时暴露并修好。
F 批没有再出现。

**覆盖率从信息变成失败**：`kiln probe --check` 的覆盖行还在，但缺一道现在会进 `problems`，退出码 2。
负向验证做过——临时把 `db-transactions.apply.rs` 移走，输出 `✗ 1 apply slot(s) have no probe:
db/transactions@apply` 并失败；移回来 79/79 绿。同时新增 `kiln/tests/probes_cover_apply.rs`，让
`cargo test` 不启动 cargo 也能说同一句话。

**M6.0 完成**：107 道练习 / 107 个参考解 / 覆盖 **79/79**。`plan --fast` 现在能为每一个 apply 槽位
提供跳过依据，`review --due` 能重新检查每一个 apply 能力——间隔重复从 61% 到 100%。六批共 24 道，
每道都两侧验证（参考解全过、练习在 `todo!()` 上全挂）。

**下一步（M6.1）**：缓存 track T6 的 8 步（计划 §4 有步表草稿）。它要动台账、curriculum、规格、
树与补丁、正文与译文、每步一道探题——按 `new-track.md` 的流程走，不是加一批文件那么简单。

## 段 6：T6 `cache` 前四步交付（2026-10-28）

按 `new-track.md` 的流程起了一条新 track，落地即完整（台账、curriculum、规格、树与补丁、双语正文、
每步探题、门阶梯），`kiln selftest --track cache` → **4/4 step(s) hold the contract**。

**四步**：6.1 读穿缓存与存活时间（`cache/read-through@apply`）；6.2 写路径先写存储再删缓存
（`cache/invalidation@apply`）；6.3 一个键一个填充者（`cache/single-flight@apply`）；6.4 过期是
一个区间、按键确定（`cache/expiry-jitter@apply`）。

**项目形状与草稿的差别**（写进 m6-plan §4）：草稿是一个 `BTreeMap + Mutex` 的"共享缓存"，实际做成
**时钟是参数**的同步库。理由：这门课每一步的判定都是单线程语义，`Mutex` 只会让测试变慢、把失败
变成调度；并发那一半（单飞）用"进行中标记"的状态机表达，正好是希望学员重写一遍的东西。`Store`
计数读次数，于是"缓存真的服务了这次"是断言而不是感觉。

**结构决定**：每一步的新 API 都落在**新文件**里（`invalidation.rs` / `single_flight.rs` /
`expiry.rs`），脚手架只做两件事——在 `lib.rs` 的标记区里插一行 `pub mod …;`、创建那个文件里的桩。
这正是 authoring.md §5 教的形状：插入的行是"以后谁都不会想改"的一行，桩本身在新文件里，参考解
就地改桩不会让 scaffold 认不出自己。

**这一步踩到的坑（都在过程中修掉了）**：

1. **`[localized_titles]` 又放错位置**：写在 `project`/`exit` 之前，TOML 把后面两个键吞进表里，
   `track.toml` 直接解析失败。规则很硬：所有顶层键之后、第一个 `[[step]]` 之前。
2. **规格文件不过 rustfmt**：`fmt` 门在每一步都红（`cargo fmt --all --check` 看整个项目）。
   跑 `cargo fmt --all` 之后重钉 manifest。
3. **词表贪心**：给 `stale` 加了词条，结果 lakelet/meshkv 的既有译文里用的是别的说法，一次冒出十几条
   漂移警告。按词表自己的规则撤掉了——`stale` 是普通形容词，不是课程术语；保留 `cache`、`invalidation`、
   `TTL`、`thundering herd` 四条。
4. **术语被强调拆开**：`**确定**性` 里夹了 `**`，词表匹配不到；改成纯文本。
5. **闭包借用**：探题的计数器用 `let mut calls = 0` 被闭包可变借用，`assert_eq!(calls, 1)` 在两次
   调用之间读不了；换成 `Cell<u32>`（共享借用）。
6. **一个白送的测试**：`releasing_a_key_nobody_holds_is_false` 只调用给定的 `release`，在练习文件上
   会通过；按探题约定改成同时断言 `begin` 仍然可以填。

**同步的计数**：curriculum §3/§11 加了 4 个能力与 4 步（97→101 能力、136→140 槽位、119→123 步），
`ledger_matches_curriculum` 与两个仓库级测试（`caps::lint::tests::the_repository_course_is_clean`、
`info::tests::the_repository_course_loads`）跟着改；`probe --check` 覆盖 83/83。

**验收（本机实测）**：`selftest --track cache` 4/4；`probe --check` 111/111、覆盖 83/83；
`lint-caps` `101 capabilities · 140 slots · 123 ledger positions · 123 authored steps`，no issues；
`i18n status` 123/123、结构 0 error、漂移 0；`links` 干净；`site --check` 261 页；
`rebuild-patches.py --track cache --check` 全部 identical；`cargo test -p kiln` 379 + 21 + 1 + 1 + 1。

**未完**：6.5–6.8（版本化键、热点两层、命中率 SLO、缓存故障降级）按计划表待写。

## 段 7：T6 后四步，八步齐（2026-10-28）

6.5 版本化键（`cache/versioned-keys@apply`）、6.6 两层缓存（`cache/two-level@apply`）、
6.7 命中率与 SLO（`obs/cache-effectiveness@operate`）、6.8 缓存故障的熔断（`ops/cache-failure@operate`）。
`kiln selftest --track cache` → **8/8 step(s) hold the contract**。

**一处设计返工（6.5）**：第一版把 `Versioned` 写成包着 6.1 的 `Cache` 的壳，`get` 里派生一个
`namespace#version:key` 交给内层——然后规格立刻红了三处：`Cache::get` 会用**同一个键**去问存储，
而存储只知道原始键，于是每个 miss 都返回 `None`。这恰恰是 6.5 的题眼：**被版本化的是缓存键，
存储的键不变**。改法是让 `Versioned` 自带条目表与 ttl（几十行、不再复用 `Cache`），`get` 用原始
键取数、用派生键存。补丁与树全部重新生成，`rebuild-patches.py --check` 八份全 identical。

**两处探题自身的毛病**：`fetch(&calls, "v1")` 的闭包同时借用 `Cell` 和字面量的 `&str`，`impl
FnMut + '_` 说不清借的是谁；把字面量改成 `String` 传进去（只借 `calls`）。另一处是
`cache-two-level` 的探题用"谁回答"的枚举代替计数器——同一个事实的另一种观察方式，比复制课内的
计数器更值得重写。

**同步的计数**：curriculum §3/§11 与台账到 **105 能力 / 144 槽位 / 127 步**，四个仓库级测试同步；
探题库 113 道、覆盖 **85/85** apply 槽位。

**验收（本机实测）**：`selftest --track cache` 8/8；`probe --check` 113/113、覆盖 85/85；
`lint-caps` `105 capabilities · 144 slots · 127 ledger positions · 127 authored steps`，no issues；
`i18n status` 127/127、结构 0 error、漂移 0；`links` 干净；`site --check` **269 页**；
`rebuild-patches.py --track cache --check` 全部 identical；`cargo test -p kiln` 379 + 21 + 1 + 1 + 1；
`fmt`/`clippy` 干净。

**M6.1 完成**：T6 八步、双语正文、每步规格与补丁；6.1–6.6 六个 apply 槽位各有一道探题，
6.7/6.8 是 operate 深度——按现行政策（`aware` 不出题、`apply` 全覆盖）不出题，`plan --fast` 不会
因此跳过它们。下一步按计划是 **M7 broker**（T7）。

## 段 8：T7 `broker` 前四步（2026-10-28）

第七条 track，按 `new-track.md` 落地前四步：7.1 分区路由（`broker/partitioning@apply`）、7.2 提交
偏移（`broker/consumer-offset@apply`）、7.3 范围再平衡（`broker/rebalance@apply`）、7.4 lag
（`broker/lag@apply`）。`kiln selftest --track broker` → **4/4 step(s) hold the contract**。

**项目形状**：`app/broker` 是一个库 + 薄二进制，每个分区就是一个 `Vec`，没有 broker 进程也没有网络
——所有语义在一次调用里完成，确定性来自"没有时钟、没有线程"。`partition_for`（FNV-1a）是给定项，
因为哈希不是题；题是路由、偏移、分配与 lag 这四个承诺。

**这一步最值钱的教训（补丁链）**：第一次生成的 7.2–7.4 脚手架补丁全在 `kiln prepare` 上失败——
因为我用**骨架文本**当每一棵树的 `lib.rs` 基座，于是 `diff(上一棵树, 这一棵)` 里出现了"删掉 7.1
参考解的 `append`"这种段，脚手架拒绝删行。正确的模型是**脚手架链**：第 N 步的准备状态 =
骨架 + 解 1 + 脚手架 2 + 解 2 + … + 脚手架 N；因此
`scaffold_N = diff(full_{N-1}, scaffold_N)`、`solution_N = diff(scaffold_N, full_N)`。
改成这个模型后四步一次过。这条已经写进 [[repo-workflow-conventions]]。

**另外两处小毛病**：规格里 `use broker::{Consumer, Topic}` 少了模块段（类型在
`broker::consumer::Consumer`）；`i18n status` 抓到三处术语漂移（`claim` 这里是"主张"不是"认领"、
`deterministic` 要写"确定性"、`backlog` 要写"积压"），都按词表裁决后改文本。

**同步的计数**：curriculum §3.9 加四个能力、§12 加 T7、探题库与组装两节顺延到 §13/§14（引用同改）；
台账到 **109 能力 / 148 槽位 / 131 步**；探题库 **117 道、覆盖 89/89** apply 槽位。

**验收（本机实测）**：`selftest --track broker` 4/4；`probe --check` 117/117、覆盖 89/89；
`lint-caps` `109 capabilities · 148 slots · 131 ledger positions · 131 authored steps`，no issues；
`i18n status` 131/131、结构 0 error、漂移 0；`links` 干净；`site --check` **279 页**；
`rebuild-patches.py --track broker --check` 七份全 identical；`cargo test -p kiln` 379 + 21 + 1 + 1 + 1；
`fmt`/`clippy` 干净。

**未完**：7.5–7.9（重放、跨分区顺序、恰好一次、事件模式演进、lag 告警）在计划 §5 的表里。

## 段 9：T7 后五步，九步齐（2026-10-28）

7.5 重放（`broker/replay@apply`）、7.6 排序分组（`broker/ordering@design`）、7.7 恰好一次
（`broker/exactly-once@design`）、7.8 事件模式演进（`broker/schema-compat@apply`）、7.9 lag 告警
（`ops/lag-alert@operate`）。`kiln selftest --track broker` → **9/9 step(s) hold the contract**。

**五步各自的那条判断**：重放是"读区间不移动任何提交"（`new` 把 `to` 夹到日志，`next_record` 只走到
`end`）；排序分组是"排序约束的连通分量一个键"（并查集，组的键取最小名，答案与规则顺序无关）；恰好
一次是"效果与偏移同一张表"（`process` 先查再应用，重放零新增、崩溃补一段不重复）；模式演进是
"加法兼容、减法破坏，必填新增也是减法"；lag 告警是"末尾连续段 + 严格高于阈值"，一次低谷重置。

**踩到的两处**：7.9 的 front matter 标题里用了 ASCII 双引号，TOML 字符串直接坏掉（改成「」）；7.8 的
译文漏了术语 `step` 的中文（词表要求"步"），`i18n status` 报一条漂移后补上。

**同步的计数**：curriculum §3.9 加五个能力、§12 的步表到九行；台账 **114 能力 / 153 槽位 / 136 步**；
探题库 **119 道、覆盖 91/91** apply 槽位。

**验收（本机实测）**：`selftest --track broker` 9/9；`probe --check` 119/119、覆盖 91/91；
`lint-caps` `114 capabilities · 153 slots · 136 ledger positions · 136 authored steps`，no issues；
`i18n status` 136/136、结构 0 error、漂移 0；`links` 干净；`site --check` **289 页**；
`rebuild-patches.py --track broker --check` 十七份全 identical；`cargo test -p kiln` 379 + 21 + 1 + 1 + 1；
`fmt`/`clippy` 干净。

**M7 完成**。下一步按计划是 **M8 rpc**（T8：契约、deadline、流式背压、协议演进）。

## 段 10：T8 `rpc` 五步交付（2026-10-28）

第八条 track，按 `new-track.md` 一次落地五步：8.1 线路字段号（`rpc/field-numbers@apply`）、
8.2 绝对 deadline（`rpc/deadline@apply`）、8.3 规范状态码（`rpc/status-codes@apply`）、
8.4 流式窗口（`rpc/server-streaming@apply`）、8.5 多路复用（`rpc/multiplexing@design`）。
`kiln selftest --track rpc` → **5/5 step(s) hold the contract**。

**项目形状**：`app/rpc` 是库 + 薄二进制，没有网络也没有代码生成——契约是数据（`Field` 的号码与类型）、
deadline 是一个整数、流是一个窗口。每条判断因此都能在一次调用里复现。

**五步各自的题眼**：号码是线路的身份（改名兼容，删号与复用号破坏；号码搬家算删除）；deadline 是绝对
的时间点（每跳从同一份预算里付，恰好花光算完成，装不下的跳**跑之前**失败）；状态码是 API 的一部分
（内部失败 → 规范码，可重试只含瞬时）；流是窗口不是缓冲（在途到窗口即停，确认后继续）；多路复用靠
每流窗口 + 每轮一项的 pump（一条阻塞的流被跳过，不拖住别人）。

**踩到的两处**：`call` 返回 `Result` 却没有 `# Errors` 一节，`clippy@1` 门（`-D missing_errors_doc`）
在第 8.2 步红了；`deadline` 的译名——译文本着"gRPC 里就叫 deadline"写了英文，词表要求出现"截止时间"，
在第一处加了括注。

**同步的计数**：curriculum §3.10 加五个能力、§13 加 T8，探题库与组装两节顺延到 §14/§15（引用同改）；
台账 **119 能力 / 158 槽位 / 141 步**；探题库 **123 道、覆盖 95/95** apply 槽位。

**验收（本机实测）**：`selftest --track rpc` 5/5；`probe --check` 123/123、覆盖 95/95；
`lint-caps` `119 capabilities · 158 slots · 141 ledger positions · 141 authored steps`，no issues；
`i18n status` 141/141、结构 0 error、漂移 0；`links` 干净；`site --check` **301 页**；
`rebuild-patches.py --track rpc --check` 九份全 identical；`cargo test -p kiln` 379 + 21 + 1 + 1 + 1；
`fmt`/`clippy` 干净。

**M8 完成**。下一步按计划是 **M9 platform**（T9：K8s 清单与探针、rollout、HPA/PDB、外部 secrets、
OIDC）。

## 段 11：T9 `platform` 前五步（2026-10-28）

第九条 track。9.1 探针接线（`k8s/probe-wiring@apply`）、9.2 滚动发布算术（`k8s/rollout@apply`）、
9.3 中断预算（`k8s/disruption-budget@apply`）、9.4 外部 secrets（`sec/external-secrets@operate`）、
9.5 OIDC（`sec/oidc@apply`）。`kiln selftest --track platform` → **5/5 step(s) hold the contract**。

**项目形状**：`app/platform` 是库 + 薄二进制，没有集群——清单是数据、发布是算术、令牌是结构体。

**五步的题眼**：存活探针失败会重启进程（所以它不能读依赖），就绪失败只是摘端点（所以它必须真检查）；
滚动发布是两个预算的算术（下限与在途上限，`maxUnavailable: 0` 需要 surge，一个副本 + 一个不可用
预算会把服务打挂——这是写进正文的已知代价，规格也这么断言）；中断预算的边界（零次即挡排干、封顶、
不为负）；secret 是引用（解析缺失要点名，清单里出现值就是泄漏）；OIDC 先验算法（`alg: none` 是最
出名的洞），再看签发方、受众、未生效、过期。

**踩到的三处**：`validate` 返回 `Result<&str, _>` 时两个 `&str` 输入让省略的生命周期有歧义
（E0106），给返回加 `<'a>` 绑定到 claims；9.2 有一条规格把"单副本 + 不可用预算 1"断言成"不能走"，
与正文自相矛盾——按真实语义改成"可以走"，并把"pod 不存在才走不了"单独测；四处术语漂移（`event`/
`timer`/`secret`/`deadline`）按词表裁决后改文本。

**同步的计数**：curriculum §3.11 加五个能力、§14 加 T9，探题库与组装两节顺延到 §15/§16（引用同改）；
台账 **124 能力 / 163 槽位 / 146 步**；探题库 **127 道、覆盖 99/99** apply 槽位。

**验收（本机实测）**：`selftest --track platform` 5/5；`probe --check` 127/127、覆盖 99/99；
`lint-caps` `124 capabilities · 163 slots · 146 ledger positions · 146 authored steps`，no issues；
`i18n status` 146/146、结构 0 error、漂移 0；`links` 干净；`site --check` **313 页**；
`rebuild-patches.py --track platform --check` 九份全 identical；`cargo test -p kiln` 379 + 21 + 1 + 1 + 1；
`fmt`/`clippy` 干净。

**未完**：9.6 HPA、9.7 滚动发布演练；之后是 M10 capstone。

## 段 12：T9 收尾，M10 capstone（2026-10-28）

**T9 后两步**：9.6 自动伸缩（`k8s/autoscaling@design`，公式 + 容差带 + 快上慢下）、9.7 发布演练
（`k8s/rollout-drill@operate`，逐步核两个预算）。`selftest --track platform` → **7/7**。踩到两处：
9.7 的规格把两个违规的顺序写反（按枚举声明顺序排序才是仓库约定）；9.6/9.7 的正文写完之前 selftest 只
报 5/5（它跳过没有正文的步骤），补上正文才真正跑起来。台账到 **126 能力 / 165 槽位 / 148 步**。

**M10 capstone**：按 plan §11 的"不是 track"落地：

- **`kiln capstone [PATH]`**（默认 `app/capstone`）跑课程的通用门——`fmt`、`clippy@1`、`no-todo`、
  `docs:public`、`exit-code`——外加一条形状检查 `[lints.rust] unsafe_code = "deny"`。它用一份
  合成的 track/step 元数据喂 `GateContext`，所以**没有新增门、也没有改门**；退出码 0/1/2。
- **为什么没有 `unsafe-budget@0`**：那个门是**按步**的（数这一步补丁写的文件），capstone 没有补丁，
  它只能回答"判不了"。项目级的等价物就是 deny lint，编译器免费执行——这一条写进了 capstone.md。
- **`docs/capstone.md`**：题目的三种形状、可判定的形状要求（库 + 薄二进制、deny unsafe、至少六个能力
  其中两个 operate、README 三节、ADR、演练记录）、门的用法、**十二问评审清单**（每一问指向课程里教它
  的那一步）、完成定义。
- **三个模板**：`docs/templates/adr.md`（背景/选项/决定/后果，后果里"变坏的"不许空）、
  `incident-review.md`（影响/时间线/根因/做对/做错/行动项，行动项要能变绿）、`review-checklist.md`
  （按课程顺序的评审清单）。

**验收**：`kiln capstone /tmp/cap-demo` 全绿、退出 0；一份故意坏掉的副本（去掉 deny、加 `todo!()`）
红三道、退出 1；不存在的目录退出 2。`cargo test -p kiln` 里两个新单测（阶梯解析、deny 检查）通过。

**至此 M6–M10 全部交付**。计划里的"可选"项（SQL 索引深度、DB 复制/切换演练、批流 ETL、搜索）仍是
可选：它们各是一条新 track 的体量，不在"M6–M10"的范围内。
