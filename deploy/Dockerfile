# syntax=docker/dockerfile:1
#
# dotvault-server image.
#
#   docker build -f deploy/Dockerfile -t dotvault-server .
#   docker run -p 8080:8080 -v dotvault-data:/data dotvault-server
#
# The build context is the repository root.

# ---- build ---------------------------------------------------------------
FROM rust:1-bookworm AS builder
WORKDIR /src

# The whole workspace is copied: the CLI and core crates are members of it
# (core is also a dependency of the server), so cargo needs their manifests.
# Dependency downloads and compiled artifacts are kept between builds by the
# cache mounts below rather than by layer ordering.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Not copied on purpose: rust-toolchain.toml. It would make rustup try to
# fetch a toolchain at build time; the image already ships stable Rust.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p dotvault-server \
    && install -D target/release/dotvault-server /out/dotvault-server

# An empty data directory owned by the runtime user (uid/gid 65532, "nonroot"
# in distroless). Copying it into the final image keeps ownership, and a fresh
# named volume mounted at /data inherits it.
RUN mkdir /out/data

# ---- runtime -------------------------------------------------------------
# distroless/cc: glibc, libgcc and CA certificates; no shell, no package manager.
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=builder /out/dotvault-server /usr/local/bin/dotvault-server
COPY --from=builder --chown=65532:65532 /out/data /data

ENV DOTVAULT_DB=/data/dotvault.db \
    DOTVAULT_BIND=0.0.0.0:8080 \
    RUST_LOG=info,sqlx=warn

# SQLite database (WAL files live next to it). Back this up, or stream it with
# Litestream: it holds every account and all ciphertext.
VOLUME /data
EXPOSE 8080

USER 65532:65532
WORKDIR /data

# There is no curl in this image, so the server probes itself.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/dotvault-server", "--healthcheck"]

# Set DOTVAULT_PUBLIC_URL (shown on the share landing page) and, behind a
# reverse proxy that appends X-Forwarded-For, DOTVAULT_TRUST_PROXY=1 at run
# time. DOTVAULT_SERVER_SECRET is optional: without it a random secret is
# generated once and kept in the database.
ENTRYPOINT ["/usr/local/bin/dotvault-server"]
