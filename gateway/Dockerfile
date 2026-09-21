# syntax=docker/dockerfile:1
# 1.96.0, NOT upstream's 1.88.0 — and the reason is worth stating, because the obvious
# reading of the lockfile is wrong. Committing `gateway/Cargo.lock` does stop version DRIFT,
# but it does not lower the floor: the pinned `aws-types@1.3.16` itself declares
# `rust-version = 1.91.1`, as do ~12 aws-smithy-* crates beside it. A 1.88 image therefore
# fails in `cargo chef cook` with "requires rustc 1.91.1", before compiling a line of WaaV.
# cargo-chef's own tree needs 1.91 too. Verified by building at 1.88 and reading the failure,
# not by comparing version numbers — 1.3.16 looks older than the 1.5.0 this branch originally
# blamed, and it is, but both are above the floor.
#
# Upstream's Dockerfile still says 1.88.0. Since CI pins `dtolnay/rust-toolchain@stable`
# rather than that value, the mismatch does not show up in their clippy/test gates.
ARG RUST_VERSION=1.96.0
# Production feature set — must match CI's PROD_FEATURES (.github/workflows/ci.yml) so the shipped
# binary is exactly what CI gates. Ships the DAG orchestration engine (dag-routing) and the full
# neural turn-detection ensemble (turn-ensemble = smart-turn + turn-detect + silero-vad) — NOT just
# basic turn-detect. smart-turn/silero ONNX models lazy-download on first use (cached thereafter);
# turn-detect assets are pre-baked by the `init` stage below.
ARG CARGO_BUILD_FEATURES="--no-default-features --features dag-routing,turn-ensemble,noise-filter,openapi"
ARG ONNX_VERSION=1.23.2

# ==================== Chef Base ====================
FROM rust:${RUST_VERSION}-slim-bookworm AS chef
# --locked: install cargo-chef from ITS lockfile rather than re-resolving, so a future
# dependency bump in one of its transitive crates cannot raise the toolchain floor again
# and break this image with no change on our side.
RUN cargo install cargo-chef --locked
# The gateway depends on sibling crates by path (`bud-auth`, `waav-openai-audio`,
# `plugin-api`), so the build CONTEXT is the WaaV root, not gateway/. Laying the tree out
# under /app with its original shape keeps `path = "../bud-auth"` resolving unchanged --
# rewriting those paths for the image would make the Dockerfile and a local `cargo build`
# disagree about the same manifest.
#
#   docker build -f gateway/Dockerfile .        <- from the WaaV root
WORKDIR /app/gateway

# ==================== Planner ====================
FROM chef AS planner
# The sibling crates are real path dependencies: `cargo metadata` resolves them, so the
# planner fails without them. They are copied whole (they are small) rather than stubbed,
# because a stub would change the recipe and invalidate the cook cache on every edit.
COPY bud-auth /app/bud-auth
COPY waav-openai-audio /app/waav-openai-audio
COPY plugin-api /app/plugin-api
COPY gateway/Cargo.toml gateway/Cargo.lock ./
# cargo-chef needs a target file for cargo metadata; stub keeps cache stable. The declared
# [[bench]] targets need one too, or `cargo metadata` refuses to parse the manifest.
RUN mkdir -p src benches \
    && touch src/main.rs src/lib.rs \
    && for b in gateway_benchmarks simd_benchmarks; do echo 'fn main() {}' > "benches/$b.rs"; done
RUN cargo chef prepare --recipe-path recipe.json

# ==================== Builder ====================
FROM chef AS builder
ARG CARGO_BUILD_FEATURES
ARG ONNX_VERSION
ARG TARGETARCH

# Install build dependencies
# Note: webrtc-sys requires many system libraries for video/graphics support
# Using multiple mirror fallback strategies for ARM64 compatibility
RUN rm -rf /var/lib/apt/lists/* \
    && if [ -f /etc/apt/sources.list.d/debian.sources ]; then \
         sed -i 's|deb.debian.org/debian-security|security.debian.org/debian-security|g' /etc/apt/sources.list.d/debian.sources; \
       elif [ -f /etc/apt/sources.list ]; then \
         sed -i 's|deb.debian.org/debian-security|security.debian.org/debian-security|g' /etc/apt/sources.list; \
       fi \
    && apt-get update -o Acquire::Retries=5 -o Acquire::http::No-Cache=true -o Acquire::Check-Valid-Until=false \
    && apt-get install -y --no-install-recommends -o Acquire::Retries=5 \
       clang cmake pkg-config libssl-dev libzstd-dev ca-certificates curl git \
       libva-dev libdrm-dev libglib2.0-dev libgbm-dev \
       libx11-dev libxext-dev libxrandr-dev libxcomposite-dev libxdamage-dev libxfixes-dev \
    && rm -rf /var/lib/apt/lists/*

# Download ONNX Runtime (architecture-aware for multi-platform builds)
RUN case "${TARGETARCH}" in \
        amd64) ONNX_ARCH="x64" ;; \
        arm64) ONNX_ARCH="aarch64" ;; \
        *) echo "Unsupported architecture: ${TARGETARCH}" && exit 1 ;; \
    esac && \
    curl -L "https://github.com/microsoft/onnxruntime/releases/download/v${ONNX_VERSION}/onnxruntime-linux-${ONNX_ARCH}-${ONNX_VERSION}.tgz" -o onnxruntime.tgz && \
    tar -xzf onnxruntime.tgz && \
    mv "onnxruntime-linux-${ONNX_ARCH}-${ONNX_VERSION}" onnxruntime && \
    rm onnxruntime.tgz

# Build optimization flags
ENV RUSTFLAGS="-C opt-level=z -C link-arg=-s -C strip=symbols" \
    CARGO_PROFILE_RELEASE_LTO=thin \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    ZSTD_SYS_USE_PKG_CONFIG=1 \
    CC=clang \
    CXX=clang++

# Cook dependencies (cached layer) - sharing=locked prevents cache corruption
COPY --from=planner /app/gateway/recipe.json recipe.json
COPY bud-auth /app/bud-auth
COPY waav-openai-audio /app/waav-openai-audio
COPY plugin-api /app/plugin-api
COPY gateway/.cargo/config.toml .cargo/config.toml
# cargo-chef rebuilds the crate skeleton from the recipe but creates no bench sources, and
# Cargo refuses to parse a manifest whose declared [[bench]] targets are absent. Stubbing
# them keeps the cook stage dependency-only (bench sources are never copied into the image)
# without editing the manifest, which would then differ from the one that builds locally.
RUN mkdir -p benches \
    && for b in gateway_benchmarks simd_benchmarks; do \
         [ -f "benches/$b.rs" ] || echo 'fn main() {}' > "benches/$b.rs"; \
       done
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    cargo chef cook --release ${CARGO_BUILD_FEATURES} --recipe-path recipe.json

# Build application
COPY gateway/Cargo.toml gateway/Cargo.lock ./
COPY gateway/src ./src
RUN --mount=type=cache,target=/root/.cargo/registry,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git,sharing=locked \
    cargo build --release ${CARGO_BUILD_FEATURES} --locked

# ==================== Runtime Dependencies ====================
# Base image with all runtime dependencies for webrtc-sys
# Reused by both init and final runtime stages
FROM debian:bookworm-slim AS runtime-deps

# Install runtime dependencies
# Note: webrtc-sys links dynamically to several system libraries
# Using multiple mirror fallback strategies for ARM64 compatibility
RUN rm -rf /var/lib/apt/lists/* \
    && if [ -f /etc/apt/sources.list.d/debian.sources ]; then \
         sed -i 's|deb.debian.org/debian-security|security.debian.org/debian-security|g' /etc/apt/sources.list.d/debian.sources; \
       elif [ -f /etc/apt/sources.list ]; then \
         sed -i 's|deb.debian.org/debian-security|security.debian.org/debian-security|g' /etc/apt/sources.list; \
       fi \
    && apt-get update -o Acquire::Retries=5 -o Acquire::http::No-Cache=true -o Acquire::Check-Valid-Until=false \
    && apt-get install -y --no-install-recommends -o Acquire::Retries=5 \
       ca-certificates libssl3 libstdc++6 \
       libva2 libva-drm2 \
       libx11-6 libxext6 libxrandr2 libxcomposite1 libxdamage1 libxfixes3 \
       libglib2.0-0 libgbm1 libdrm2 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
ENV LD_LIBRARY_PATH=/app

# ==================== Init Stage ====================
# Runs waav-gateway init to download models (requires shell)
FROM runtime-deps AS init
ARG RUN_GATEWAY_INIT=true

COPY --from=builder /app/gateway/target/release/waav-gateway /app/waav-gateway
COPY --from=builder /app/gateway/onnxruntime/lib/*.so* /app/

ENV CACHE_PATH=/app/cache

RUN mkdir -p "$CACHE_PATH" \
    && if [ "${RUN_GATEWAY_INIT}" = "true" ]; then /app/waav-gateway init; fi

# ==================== Runtime ====================
# Final production image
FROM runtime-deps AS runtime

# Binary
COPY --from=builder /app/gateway/target/release/waav-gateway /app/waav-gateway

# ONNX Runtime shared libraries
COPY --from=builder /app/gateway/onnxruntime/lib/*.so* /app/

# Cached models from init stage
COPY --from=init /app/cache /app/cache

ENV RUST_LOG=info \
    PORT=3001 \
    CACHE_PATH=/app/cache

EXPOSE 3001
ENTRYPOINT ["/app/waav-gateway"]
