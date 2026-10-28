# Capstone：自己造一个，课程只给门与清单

> M10 的交付物（[plan.md](plan.md) §11）。它**不是一条 track**：没有脚手架、没有参考解、没有规格
> ——这三样东西的前提是课程知道你要做什么，而 capstone 的全部意义是你自己决定做什么。

## 0. 为什么它是一张清单而不是一步

一条 track 的每一步都钉死在一个文件、一份只读规格、一份参考解上，`selftest` 才能说"脚手架必败、
参考解必绿"。capstone 没有这些，也不该有：**一个替你决定题材的课程，会产出一门照着抄完、却什么都
没自己设计过的课**。

课程能给的只有两样，而且都是它自己的东西：

- **同一套门**：`kiln capstone` 用课程已有的通用阶梯判你的项目——判据和每一步一样（fmt、clippy、
  no-todo、docs、exit-code），只是对象从"这一步的文件"变成"你的项目"；
- **一张评审清单**：机器判不了"这个设计对不对"，人能判。清单在 §3，模板在 §4。

所以"做完"在这里的定义也不同：**门全绿 + 清单每一条都有答案 + 两个模板填过**。

## 1. 题目

从下面选一个，或自带一个等价的：

- 一个把 **T6 的缓存**放在 **T7 的消息流**前面的读侧服务（读多写少、允许陈旧、要 SLO）；
- 一个用 **T8 的 RPC 契约**给 **T3 的作业引擎**套上的网关（契约先行、deadline 传播、流式进度）；
- 一个用 **T9 的探针 / 发布 / 预算**把 T1 的服务真正上线的部署仓库（清单、演练、回滚）。

**可判定的形状要求**（门会查前两条，剩下的评审看）：

- 一个库 + 一个薄二进制——`exit-code` 门要有个能跑的二进制；
- `Cargo.toml` 里有 `[lints.rust] unsafe_code = "deny"`；
- 至少涉及 **6 个能力**，其中至少 **2 个是 `operate` 深度**（用台账的语言写进 README：哪个能力、
  哪一步教的）；
- README 三节：**问题**（为谁解决什么）、**架构**（一张图或一张表）、**如何验证**（一条命令）；
- 一份 **ADR**（[模板](templates/adr.md)）记录一个你**放弃**的方案；
- 一次 **演练记录**（[模板](templates/incident-review.md)）——注入故障的演练即可，不必是真事故。

## 2. 门：`kiln capstone [PATH]`

默认目录 `app/capstone`。它跑的是课程的通用阶梯，外加一条形状检查：

```sh
cargo run -p kiln -- capstone app/capstone
# kiln capstone · app/capstone
#   ✓ unsafe-deny    `[lints.rust] unsafe_code = "deny"` is in Cargo.toml
#   ✓ fmt            `cargo fmt --all --check` is clean
#   ✓ clippy@1       clippy@1 is clean (…)
#   ✓ no-todo        no clippy::todo, … anywhere in the step's code
#   ✓ docs:public    every public item is documented (…)
#   ✓ exit-code      demo exits non-zero on `--kiln-exit-code-probe` and names the argument it refused
```

退出码：`0` 全绿；`1` 有检查红了（**包括所有检查全红**——一个完全红掉的项目仍然是一个项目）；
`2` 目录不存在（还没有项目）。

**它刻意不是 CI 门。** 课程仓库里没有 capstone 项目（`app/` 是学员自己的工作区），所以这条命令是
学员侧的，和 `kiln run` 一样在你自己的机器上跑。

`unsafe` 为什么是形状检查而不是门：`unsafe-budget@0` 是**按步**的（它数这一步的补丁写了哪些文件），
capstone 没有补丁，那个门只能回答"判不了"。项目级的等价物是**编译器**：这条检查用
`RUSTFLAGS=-F unsafe_code` 编一次项目。`-F` 是命令行级别，crate 里的 `#![allow(unsafe_code)]`
覆盖不了它——一个读 `Cargo.toml` 的字符串匹配做不到这一点（`allow` 旁边一个真的 `unsafe fn`
会通过），所以这里编，不读。

## 3. 评审清单（人读，一条一条有答案）

| # | 问题 | 课程里的出处 |
| --- | --- | --- |
| 1 | 未知路径与配错方法，客户端能分辨吗？（404 / 405 + `Allow`） | mailer/1.7 |
| 2 | 写路径先写存储再让缓存失效吗？重放会不会重复？ | cache/6.2、broker/7.7 |
| 3 | 重试有预算吗？幂等键在哪一层？ | relay/2.2、relay/2.8 |
| 4 | 依赖挂了，哪些功能还在？写下来了吗？ | relay/2.24 |
| 5 | 存活探针读依赖了吗？就绪探针真的检查吗？ | platform/9.1 |
| 6 | `maxUnavailable` 是多少，为什么是这个数？ | platform/9.2 |
| 7 | 迁移能在线做吗？回滚路径是什么？ | jobq/3.23 |
| 8 | 出问题时，你看哪三个数？告警的阈值是怎么定的？ | mailer/1.20、broker/7.9 |
| 9 | 秘密怎么进进程？谁能读别人的数据？ | platform/9.4、mailer/1.18 |
| 10 | 测试在哪一层最薄，为什么可以薄？ | mailer/1.19 |
| 11 | 一个新同事能在 30 分钟内跑起来吗？ | README「如何验证」 |
| 12 | 放弃的那个方案是什么，为什么放弃？ | [ADR 模板](templates/adr.md) |

清单里的每一问都指向课程里教它的那一步——这不是巧合：capstone 不引入新知识，它要求你把**已经
教过一遍的判断**放在一个自己选的题里再走一遍。

## 4. 模板

- [ADR](templates/adr.md) —— 一个决定的记录：背景、选项、决定、后果
- [事故复盘](templates/incident-review.md) —— 时间线、影响、根因、做对/做错、行动项
- [评审清单](templates/review-checklist.md) —— 一次代码评审读什么，按课程的习惯排序

## 5. 完成

- `kiln capstone` 全绿；
- §3 的十二问每一条都有答案（写进 README 或 ADR，不留在脑子里）；
- ADR 与复盘各一份，模板的每一节都填过——"没有事故"和"没有做对的地方"都不是合格的答案。

做完之后，回头看 `docs/curriculum.md` §3：你至少应该在六个能力上有 `apply` 以上的证据，其中两个是
`operate`。如果对不上，差的不是文档，是那六个能力里有一个其实没学会。
