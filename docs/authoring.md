# 编写规范

> 配套：[规划与工程决策](plan.md)、[课程体系与能力模型](curriculum.md)。
> 面向课程作者与翻译者。这里写的是**硬规则**；违反规则的课程条目应当在 `kiln selftest` 里失败。

## 1. 命名与目录

| 对象 | 约定 | 例 |
| --- | --- | --- |
| 步骤 id | `<track>/<phase>.<index>-<slug>` | `mailer/2.15-shutdown-leases` |
| 正文（英文源） | `docs/steps/en/<track>/<phase>.<index>-<slug>.md` | `docs/steps/en/mailer/1.14-worker.md` |
| 译文 | `docs/steps/<locale>/<track>/…`（同名文件） | `docs/steps/zh-Hans/mailer/1.14-worker.md` |
| 规格测试 | `spec/<phase>.<index>/<name>.rs` | `spec/1.14/worker.rs` |
| scaffold patch | `scaffold/<phase>.<index>-<slug>.patch` | `scaffold/1.14-worker.patch` |
| 参考解 patch | `solutions/<phase>.<index>-<slug>.patch` | `solutions/1.14-worker.patch` |
| 能力 id | `<domain>/<name>`，小写连字符 | `async/graceful-shutdown` |
| probe / drill | `probes/<capability>.rs` / `drills/<slug>.rs` | `probes/async-graceful-shutdown.rs` |

slug 用英文、连字符、动词优先（`rate-limit-by-tenant` 优于 `tenant-limit`）。

## 2. 元数据的唯一真源

- **步骤元数据只在 `track.toml`**（`kind`/`origin`/`teaches`/`requires`/`run`/`spec`/`gates`/`hint`/`time_hint`）。
- **正文 front-matter 只放文档自身的字段**，避免两处真源漂移：

```toml
+++
id = "mailer/1.14-worker"
title = "Background delivery worker"
locale = "en"           # 译文改成本地化标签
# 仅译文需要下面两个字段
# source_hash = "sha256:…"
# translator = "…"
+++
```

- 正文里的 `DONE WHEN` 必须列出**确切命令**（`kiln check mailer/1.14-worker`），不能写「通过测试」这种含糊话。

## 3. 每步的规模与必填内容

1. 步长 30–120 分钟（`time_hint` 只是难度提示，不作为验收）；单步新概念 ≤ 3 个，超了必须拆。
2. 每步必须**一条命令可验证**，且默认离线（联网只用于拉 crate/镜像）。
3. 快路径（`kiln run`）< 60 秒；需要 mutants / loom / soak / bench / 模拟的检查标 `slow`。
4. TRAP 段必填，且必须是**具体的失败现象**，不是泛泛的忠告：
   - 好：`Only awaiting ctrl_c inside select! drops the other branch's future — the in-flight job vanishes with no log line.`
   - 差：`Be careful with cancellation.`
5. FURTHER 段给可点击的源码/文档（tokio、axum、sqlx 的具体文件或章节），不给「看官方文档」。
6. 正文英文、祈使句；代码标识符与生态术语保持英文；中文译文沿用 `glossary.toml` 的固定译法。

## 4. 三种正文模板

### 4.1 `origin = "teach"`（首次）

```md
# Background delivery worker

## WHY
A signup confirmation that never arrives is indistinguishable from a broken service…
（< 200 词，讲清生产里的代价，不讲语法）

## TASK
- Drain in-flight deliveries on SIGTERM; stop accepting new ones first.
- Bound the queue; define the overflow policy.
- Retry with jitter, capped attempts, and record attempts on the row.

## DONE WHEN
- `kiln run mailer/1.14-worker` — spec passes
- `kiln check mailer/1.14-worker` — all must gates green

## TRAP
1. Only awaiting `ctrl_c` inside `select!` → the other branch's future is dropped…
2. Asserting drain with `sleep(1s)` → flaky in CI; the test becomes the next bug.

## FURTHER
- `tokio::signal::unix::signal` — how the signal stream is registered
- `tokio_util::sync::CancellationToken` — drop semantics and child tokens
- Kubernetes `terminationGracePeriodSeconds` — the deadline you are racing
```

### 4.2 `origin = "deepen"`

```md
# Shutdown with in-flight leases

## RECALL
`async/graceful-shutdown@apply` — `mailer/1.14`: stop accepting, drain in-flight, force-exit on deadline.

## DELTA
Here an in-flight task holds a database lease. Dropping it does not lose a delivery —
it makes the row invisible for the lease duration. The deadline you choose now trades
recovery latency against duplicate work, and clock skew decides who wins.

## TASK / DONE WHEN / TRAP
（同 4.1）
```

### 4.3 `origin = "transfer"`

同 `deepen` 的模板，但两句必须写实：
- `RECALL` 写清**上次那步的 id**与深度（`kiln lint-caps` 会校验指向存在）；
- `DELTA` 只写「新项目里的坑」，并说明本步**闸门更严、提示更少**。

## 5. scaffold patch 规则

scaffold 会把新模块的骨架与依赖加进学员项目。它必须是**加性**的，否则必然与学员的编辑冲突：

1. **只允许两种操作**：新增文件；在标记块内插入行。
2. 标记块长这样（`Cargo.toml`、`lib.rs` 的模块注册、路由注册同理）：

```toml
[dependencies]
# kiln:scaffold:begin
sqlx = { version = "0.9", features = ["runtime-tokio", "postgres"] }
# kiln:scaffold:end
```

3. **绝不允许**修改标记块之外的任何行（包括格式化结果）。
4. scaffold 决定**函数签名**：规格测试只依赖 scaffold 里声明的签名，学员改签名就会编译失败——
   这是明确反馈，不是玄学。`kiln reset <step>` 恢复签名与骨架，但保留学员的实现文件内容（冲突时保留学员版本并告警）。
5. scaffold patch 必须在**参考解之前**应用：`selftest` 的顺序是 `scaffold → 期望失败 → 应用参考解 → 期望全绿`。

## 6. 规格测试规则

1. 规格测试**只读**（`spec/`，哈希入清单），且不得包含 `#[ignore]`。
2. **必须能在 scaffold 上失败**（空测零容忍）：`selftest` 会先在未实现状态下跑一遍，全部通过 = 该步的规格测试写废了。
3. 一个步骤可以有多个规格文件，但 `run.cmd` 只跑一个测试目标，避免「跑哪几个」变成隐性知识。
4. 不能用 `sleep` 做同步；用事件/通道/轮询条件（`wait_for`）等待。时间相关用注入时钟。
5. 测试失败信息要能指向 TRAP：断言消息里写「what to check next」，而不是只写 `assert!(x == y)`。

## 7. 闸门

`track.toml` 里 `gates = [...]` 使用极小的字符串 DSL：

| 写法 | 含义 |
| --- | --- |
| `spec-integrity` | must 闸门，无参数 |
| `clippy@2` | must 闸门，参数 `2`（lint 阶梯） |
| `mutants:80` | must 闸门，参数 `80`（阈值） |
| `?pedantic` | **should** 闸门：告警不阻塞 |
| `slow:mutants` | 慢闸门：只在 `kiln verify --slow` / `selftest` 跑 |

规则：

1. 每个 `teaches` 条目至少要有一个 must 闸门或规格测试作为证据（`lint-caps` R4）。
2. 新增闸门必须同时提供**缺失工具时的可执行提示**（`cargo install cargo-mutants` 这类），不允许只抛退出码。
3. 闸门不得依赖 GNU userland（无 `timeout`、无 GNU-only 参数）；超时在 Rust 侧实现。
4. 闸门只对**目标类型**生效：库代码严格、测试代码放宽。放宽必须在正文里写明理由，不能藏进 `allow`。

## 8. i18n

1. 英文是唯一真源；译文缺失时回退，不报错。
2. 译文 front-matter 必须带 `locale`、`source_hash`、`translator`。
3. 翻译**不得**改动代码块、命令、能力 id、闸门名——只翻散文。
4. 标题结构必须与英文一致（缺段 = 结构错误，CI 报错；`source_hash` 不符 = 漂移，只告警）。
5. 术语查 `glossary.toml`；新术语先加进词表再翻译。
6. 加一门语言 = 新建目录 + front-matter，**不改任何代码**；`kiln --lang <locale>` 必须优雅回退。

## 9. 作者检查清单

- [ ] 步骤只教 ≤3 个新概念，`time_hint` 在 30–120 分钟之间
- [ ] `teaches`/`requires` 声明完整，`kiln lint-caps` 通过
- [ ] 正文模板与 `origin` 匹配（teach 有 WHY；deepen/transfer 有 RECALL+DELTA，无 WHY）
- [ ] TRAP 写的是具体失败现象，不是忠告
- [ ] `DONE WHEN` 里是可复制的确切命令
- [ ] 规格测试在 scaffold 上失败、在参考解上全绿
- [ ] scaffold 是加性的（只新增文件 / 只改标记块）
- [ ] 快路径 < 60 秒；慢检查标了 `slow`
- [ ] 每个 `teaches` 都有 must 闸门或规格测试兜底
- [ ] 英文正文；译文（若有）带 `source_hash` 且未改代码块

## 10. 翻译者检查清单

- [ ] 从英文源复制结构，删掉不相关的段落，不新增章节
- [ ] 只翻散文；代码、命令、能力 id、闸门名、路径保持原样
- [ ] 术语与 `glossary.toml` 一致
- [ ] front-matter 补 `locale` / `source_hash` / `translator`
- [ ] `kiln i18n status` 无结构错误（漂移由作者处理，不静默修哈希）
