# The image this service ships as (step `mailer/1.21-container`).
#
# Two stages, and three decisions inside them.
#
# **The split.** The builder has the toolchain, the sources and the whole workspace's dependencies;
# the runtime has one binary, a trust store, and nothing that could compile another one. What that
# buys is not disk space as much as *surface*: an image with no compiler, no `cargo`, no sources and
# no root is an image where a shipped vulnerability has fewer places to live — and where "the image
# runs the same binary you tested" is a claim somebody can check, because there is nothing else in
# there to run.
#
# **Alpine, on both sides.** musl rather than glibc, and the reason is the second stage: a glibc
# binary needs a glibc distribution under it (`debian:bookworm-slim` is ~100 MB before the binary),
# while a musl binary fits in an 8 MB base. The whole image ends up around 25 MB — the size of the
# *shipped* thing, which is the number that matters; the builder image is a build-time dependency that
# never leaves this machine.
#
# **The dependency layer.** It comes first and alone, so it only changes when `Cargo.toml` or
# `Cargo.lock` does: the expensive half of a Rust build is cached across every code change. The
# spec's fixture additionally mounts a build cache for `/src/target` and `/usr/local/cargo`, which is
# what makes the *second* build of this image seconds rather than minutes.
FROM rust:1.98-alpine AS builder

# musl's headers and a linker: `rust:alpine` carries the toolchain, and this is what its `cc` needs to
# link a musl binary.
RUN apk add --no-cache musl-dev
WORKDIR /src

# A stub for both targets, so that `cargo build` has something to compile while it builds the
# dependencies. The stub is thrown away in the next layer.
COPY Cargo.toml Cargo.lock ./
COPY xtask ./xtask
RUN mkdir -p src \
    && printf '//! Stub: what the dependency layer compiles.\nfn main() {}\n' > src/main.rs \
    && printf '//! Stub: what the dependency layer compiles.\n' > src/lib.rs \
    && cargo build --release --locked

# The real sources: this is the only layer a code change invalidates. `touch` makes cargo rebuild the
# crate itself — without it the stub's binary would be newer than the sources and would ship.
COPY . .
# The binary is copied *out* of `target/` in the same step, and that is not decoration: the spec's
# fixture mounts a cache over `/src/target`, and a mount is not part of an image layer — a build that
# left its output in a mounted directory would hand the next stage an empty path.
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked \
    && cp target/release/mailer /src/mailer

FROM alpine:3.20 AS runtime

# A trust store: a deployment puts TLS in front of this service, and the mail server of step
# `mailer/1.19-test-layering` is spoken to by a client that may need to verify one.
RUN apk add --no-cache ca-certificates \
    && adduser -D -u 10001 -s /sbin/nologin mailer
COPY --from=builder /src/mailer /usr/local/bin/mailer

# A service that needs no privileges should have none. The process is pid 1 inside the container
# either way; non-root is the cheaper of the two ways to make that survivable.
USER mailer
# 8080 is the default of step `mailer/1.4-config`; a deployment overrides it (and everything else)
# with `MAILER__…` variables, which is the whole reason the configuration is layered. `0.0.0.0` is
# what a container needs and a laptop does not: the default `127.0.0.1` is "this container only",
# and a published port with nothing behind it is a service that looks broken while being healthy.
ENV MAILER__LISTEN__HOST=0.0.0.0
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/mailer"]
