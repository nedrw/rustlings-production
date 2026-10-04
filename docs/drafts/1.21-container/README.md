# 1.21-container：一份能跑的镜像设计（2026-10 实测草稿，已归档）

> **这一步已经落地**（`0a7e7da`）：正文在 `course/docs/steps/en/mailer/1.21-container.md`，参考解在
> `course/tracks/mailer/solutions/1.21-container.patch`，不要再照本文件的「落地清单」执行——它只记录
> 当时打算怎么做。下面的 16.5 MB 与热构建数字与落地版本一致；三处按实测改掉了：**基础镜像要显式选本机
> 架构的那份**、**构建缓存是「可选」不是「强加」**（夹具传 `CARGO_TARGET_DIR`，Dockerfile 可以不认）、
> **1.21 的截止时间写进 `run.timeout = 1800`**。详见 `handoff-m0-log.md` 第十四段。

`app/` 不跨 worktree 复制，所以这一版放在这里：把四个文件拷回
`app/mailer/{Dockerfile,.dockerignore,tests/spec_image.rs,tests/support/container.rs}` 即可复现下面的数字。

## 实测（这台机器：podman machine，arm64，4 vCPU / 4GB）

| 指标 | 值 |
| --- | --- |
| 成品镜像（`alpine:3.20` + musl 二进制 + ca-certificates + 非 root 用户） | **16.5 MB** |
| 构建（依赖已缓存） | **1m24s** |
| 规格四条（`cargo test --test spec_image -- --test-threads=1`） | **4 passed，68s** |
| builder 基础镜像 `rust:1.98-alpine`（build-time only） | 981 MB |
| builder 基础镜像 `rust:1.98-slim`（build-time only） | 828 MB |
| runner 基础镜像 `alpine:3.20` | 8.1 MB |
| runner 基础镜像 `debian:bookworm-slim`（glibc 路线的备选） | 100 MB |

结论：**能变小的只有"被验证的那面镜像"**（16.5 MB vs glibc+debian 路线的约 130 MB）；Rust 工具链的
基础镜像在官方各变体里都是 800 MB–1 GB 量级，它只活在构建机上（不进成品镜像，可以 `podman rmi` 掉）。

## 三个只有跑过才知道的坑（都已修，别再踩）

1. **stub 层必须满足 workspace 的 lint**：`[workspace.lints.rust] missing_docs = "deny"` 会打在
   "依赖层用的 stub" 上（空 `src/lib.rs` 不过）。stub 现在带 `//!` 文档。
   —— 这也是那次 **28 分钟失败**的真凶（报错被 buildah 截成一行 `exit 101`，看起来像超时）。
2. **别把缓存挂到 `/usr/local/cargo`**：那是镜像放 **工具链** 的地方，挂上去之后构建报
   `/bin/sh: cargo: not found`（exit 127）。要挂就挂 `/usr/local/cargo/registry`。
3. **挂载不进镜像层**：`/src/target` 被缓存挂着时，`COPY --from=builder /src/target/release/mailer`
   找不到文件。所以 builder 的最后一个 `RUN` 里把二进制 `cp` 到挂载之外（`/src/mailer`），runtime 再从
   那里 `COPY`。

另外两条环境事实：镜像站会把 arm64 存成 arm64（`podman pull --platform linux/arm64` 可用，
之前 `alpine:3.20` 是 amd64、走 QEMU 模拟）；`podman build -v` 只接受**绝对路径**（不是命名卷），
夹具因此把缓存放在 `$XDG_CACHE_HOME/kiln/container-{target,registry}` 下。

## 落地清单（下一步照这个走）

1. 把这四个文件拷回 `app/mailer`；`kiln run mailer/1.21-container` 应 4 passed
   （首次构建要几分钟：deps 要编译一遍，之后是秒级）。
2. **scaffold**：给一版**单阶段** `Dockerfile`（`FROM rust:1.98-alpine` + `COPY . .` +
   `RUN cargo build --release` + `CMD`，root、无 `MAILER__LISTEN__HOST`）——它在规格上失败得很难看且很
   有教育意义：容器只监听 127.0.0.1（发布端口后面什么都没有）、镜像里带 cargo/cc、`id -u` = 0、体积超上限。
   `.dockerignore` 与 `tests/support/container.rs` 属 scaffold（新文件）；`tests/spec_image.rs` 是规格。
3. **参考解**：本文档里的 `Dockerfile`（多阶段 + musl + 非 root + `ENV MAILER__LISTEN__HOST=0.0.0.0`）。
4. **`container-repro` 门**（`kiln/src/gates/registry.rs` 里仍是 `Pending`）：建议判"镜像跑的是同一面
   binary"——`podman build` 之后在容器里 `--version`/`/healthz`，并与宿主二进制的同一路由答案对比；
   注意门的超时也要放宽（`session.timeout()`，默认 300s），或者让门复用同一份构建缓存。
5. `track.toml` 的 1.21 已经声明 `container-repro` 门；`images = [...]` 需要加
   `rust:1.98-alpine` 与 `alpine:3.20`（`kiln images pull` 会按仓库路径匹配镜像站前缀）。
6. 正文（`course/docs/steps/en/mailer/1.21-container.md`）要写：多阶段为什么、musl 为什么、
   依赖层为什么先、`0.0.0.0` 为什么、探针为什么要分活着/就绪、以及"镜像体积是规格的一部分"。
