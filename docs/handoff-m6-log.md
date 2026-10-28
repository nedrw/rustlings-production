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
