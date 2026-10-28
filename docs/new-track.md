# 加一条新 track：指南

> 这份指南对着 [plan.md](plan.md) §11 的验收标准写的：**一个没参与过这个仓库的人，照它能在
> 一小时内起一条 3 步的玩具 track，并且所有门是绿的。** 下面每一步都来自真实做过一遍的记录
> （2026-10-27，`docs/handoff-m5-log.md` 段 15），包括走错了的地方。

## 0. 先分清两类活

加一条 track 要动两类东西，**只有一类能被生成**：

| 类别 | 内容 | 谁来决定 |
| --- | --- | --- |
| **机械的** | 目录结构、`track.toml` 的头与每个 `[[step]]`、学员项目的骨架、规格文件的路径与命名、正文的入口路径、门的阶梯起点、规格哈希清单 | `kiln new-track` 一次写完 |
| **要判断的** | 这一步教哪个能力、哪个深度（台账是唯一真源，这是课程的决定）、这条 track 的退出能力、每个 `apply` 槽位的探题、规格与参考解的**内容** | 你 |

生成器**不会**替你决定第二类。一个替你决定了的生成器，会产出一门"过了自己的检查、却什么都没教"的课。

## 1. 生成骨架

```sh
cargo run -p kiln -- new-track toy \
  --title "Toy — a service that does one thing" \
  --step 1.1-first-step:repo/workspace-layout@apply \
  --step 1.2-second-step:err/typed-errors@apply \
  --step 1.3-third-step:obs/structured-logs@apply
```

它写出：

```
course/tracks/toy/track.toml          # track 头 + 每个 [[step]]
course/tracks/toy/skeleton/           # 学员的项目：能构建、有一个会失败的桩
course/tracks/toy/spec/1.1/first_step.rs   # 占位规格：它是**失败的**
course/tracks/toy/spec/manifest.toml  # 规格哈希清单
course/docs/steps/en/toy/1.1-first-step.md  # 正文模板
course/tracks/toy/{scaffold,solutions}/       # 空目录，等你填
```

它**不会**写的东西，正是第二节那张清单。

## 2. 立刻会撞上的三件事

### 2.1 这个能力已经有主人了

`new-track` 在创建时就读台账并告诉你：

```
the ledger already establishes what these steps ask for:
  1.1-first-step: `repo/workspace-layout` is already established at apply by mailer/1.1 — a new
  track either teaches a *deeper* depth, or says `reteach = "why"` on the step (and the ledger
  needs a slot for it)
```

这不是生成器的挑刺，是 R1/R2 的规定：**同一门课里，一个能力在同一深度只能被教一次**。两条出路：

- **教更深的深度**（`apply` 已被占了，就用 `design` 或 `operate`），并在 `course/capabilities.toml`
  里给它一个槽位（`apply = "mailer/1.1"` 这种；一个能力每个深度一行）；
- **有意重教**：在步骤里加 `reteach = "why"`，理由写清为什么这次重教有价值（练习轨就属于这种）。
  台账**不动**，`kiln lint-caps` 会把每一次重教**报出来**（"deliberate re-teaches"）——它是被记录的，
  不是被静默接受的。

步骤参数写成 `<id>:<capability>@<depth>[:<why>]`，`why` 就是 `reteach` 的理由：

```sh
--step "1.1-first-step:repo/workspace-layout@apply:the split is the whole point of this practice track"
```

### 2.2 规格必须在脚手架上失败

生成的规格是**失败的占位**（`panic!("the spec … is still the placeholder")`），生成的骨架里的
`version()` 也是 `todo!()`。这不是偷懒：**一个在脚手架上就通过的规格，恰恰是空测**（§6.2），而
`selftest` 会把它报成 "BROKEN STEP"。

所以你写的第一条真实规格，要断言一件脚手架**做不到**的事。写完把占位替换掉。

### 2.3 补丁的 hunk 头要数对

这是 §5.4 那条规则，第一次做新 track 的人一定会撞：

```
solution ✗ the reference solution did not apply: src/lib.rs: hunk `@@ -6,6 +6,5 @@` declares 6 old
and 6 new line(s) but carries 5 and 5
```

`@@ -a,b +c,d @@` 里的 `b`/`d` 是这一段**实际携带**的行数。写补丁时数一遍，或者用
`tools/rebuild-patches.py` 从树重新生成（§5.5）。

## 3. 剩下的清单

生成器在最后会打印这张清单；这里逐条说清要做什么：

1. **台账**：`course/capabilities.toml` 里给每个 `teaches` 一个槽位（能力 + 深度 + 这一步的
   `<track>/<phase>.<index>`）。`kiln lint-caps` 的 R0 会核对它与 `track.toml` 是否逐格一致。
2. **退出能力**：track 头的 `exit = [...]`，每一项都必须由这条 track 的某一步建立（R6）。
3. **探题**：每个 `apply` 槽位都要有一道探题（`course/probes/`，见 §11），否则
   `kiln probe --check` 会说这个槽位没有题。
4. **规格与参考解**：把占位规格换成真的，把 `solutions/<id>.patch` 写出来；`scaffold/<id>.patch`
   是第二步起的脚手架（第一步用整个 `skeleton/`）。
5. **正文**：模板里的 TODO 换成真的散文。`origin = "transfer"` 的步用 RECALL/DELTA，其余用
   WHY/TASK/DONE WHEN/TRAP/FURTHER（R5 会查）。

## 4. 跑门

```sh
cargo run -p kiln -- lint-caps          # 台账、深度、证据、退出能力
cargo run -p kiln -- selftest --track toy   # 每一步：脚手架红、参考解绿、must 门全绿
cargo run -p kiln -- i18n status        # 结构（译文可以以后再翻）
cargo run -p kiln -- links              # 页面里的链接
```

`selftest` 在一步还不完整时会**逐项点名缺什么**：

```
nothing to test yet: no step of toy is complete.
  toy/1.1-first-step:
    its reference solution is missing: solutions/1.1-first-step.patch
  toy/1.2-second-step:
    its scaffold is missing: scaffold/1.2-second-step.patch
    ...
```

## 5. 一小时的账

| 活 | 大概多久 |
| --- | --- |
| 读这份指南 + 跑一次 `new-track` | 10 分钟 |
| 台账与 `reteach` 的决定 | 15 分钟 |
| 三份规格（每份一个行为断言） | 30 分钟 |
| 三份参考解补丁（手写或从树生成） | 30 分钟 |
| 三段正文 | 45 分钟 |
| 跑门并修 | 30 分钟 |

超时了，通常卡在**规格写得太满**（一步只教一件事，规格只断言那件事）或**补丁手写**（改用
`tools/rebuild-patches.py` 从树生成）。

## 6. 一条 track 的形状速查

```toml
format_version = 1
id = "toy"                    # 等于目录名
index = 6                     # 全仓唯一
title = "Toy — …"
project = "app/toy"           # 学员的工作区
exit = ["cap/you-established"] # R6：每项都要由本 track 建立

[[step]]
id = "1.1-first-step"         # <phase>.<index>-<slug>
title = "…"
kind = "milestone"
origin = "teach"              # teach | deepen | transfer
entry = "docs/steps/en/toy/1.1-first-step.md"
time_hint = "60m"
concepts = ["…"]
teaches = ["repo/workspace-layout@apply"]
requires = []                 # 只能引用更早教过的槽位（R3）
run = { cmd = "cargo test -p toy --test spec_first_step" }
spec = ["spec/1.1/first_step.rs"]
scaffold = "skeleton"         # 第一步；之后是 "scaffold/1.2-….patch"
solution = "solutions/1.1-first-step.patch"
gates = ["spec-integrity", "caps", "i18n", "fmt", "clippy@1"]
```

门的阶梯**只升不降**（§6）：每个门由"教它那一步"引入，之后每一步都留着。
