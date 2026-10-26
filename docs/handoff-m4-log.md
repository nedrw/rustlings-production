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
