# M3 交接简报（T4 `lakelet` 的 24 步 + 三条新慢门 + 容器与性能基建）

> **状态：施工中。** 计划与要拍板的决策在 [m3-plan.md](m3-plan.md)（已拍板：六个决策点全部按推荐）。
> 分段施工记录在 [handoff-m3-log.md](handoff-m3-log.md)，按批次追加。
> M3 收尾时这份文件会像 [handoff-m2.md](handoff-m2.md) 一样补全：范围与验收（§0）、三条新门的实测契约（§1）、
> T4 的项目形状（§2）、施工中钉下来的决定（§3）、实测数字与验收演练（§4）、缺口（§5）。

## 0. 已落地的部分（截至本次提交）

| 批次 | 提交 | 内容 |
| --- | --- | --- |
| B0a | `kiln(gates): the three slow gates T4 needs…` | `slow:sim@N`、`slow:soak@N`、`slow:fuzz@N` 三条 must 慢门 + 四门共用的 harness 管线（前缀带 track、严格 `k=v` 解析、课程侧参数文件、重跑失败原因） |
| B0b | `kiln: the container and profiling infrastructure T4 leans on` | `kiln images` 按引用去重 + 一行离线结论、容器**运行**探测（`✓ containers can start (podman \`alpine:3.20\`, 0.2s)`）、`kiln skip --track`、`doctor` 的 profilers 一节 |
| B0c | `course(lakelet): …` | T4 的 24 步元数据（槽位/origin/门阶梯）+ `app/lakelet` 骨架（库 + 两个 bin + 夹具） |

尚未落地：T4 的 24 步正文/规格/参考解（从 4.1 开始）、探题、验收演练。
