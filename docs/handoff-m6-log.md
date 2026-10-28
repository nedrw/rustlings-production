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
