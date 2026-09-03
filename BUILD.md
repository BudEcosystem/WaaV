# Building WaaV Gateway — verified procedure & gotchas

This documents the **actual** steps to get a clean build, discovered empirically (the
repo did **not** compile from a clean checkout before these fixes). See
`PRODUCTION_PLAN.md` S9/W11 for the reproducibility workstream.

## TL;DR

```bash
cd gateway
# 1. Use the committed lockfile (now tracked) so webrtc deps stay on the known-good pair.
cargo build --locked            # libwebrtc 0.3.19 + webrtc-sys 0.3.16 (matches livekit =0.7.24)

# 2. On ARM64 hosts that also have CUDA installed, disable the NVIDIA video codec:
CUDA_HOME=/tmp/nocuda cargo build --locked   # see "Gotcha 2"
```

## Why a lockfile is mandatory (Gotcha 1 — was S9, now fixed)

`Cargo.toml` pins `livekit = "=0.7.24"`, which depends on `libwebrtc = "0.3.19"` (i.e. `^0.3.19`).
With **no committed `Cargo.lock`**, a fresh `cargo build` floats `libwebrtc` to the latest
`0.3.x` (**0.3.35**), which added `key_derivation_algorithm` + `key_ring_size` to
`KeyProviderOptions`. livekit 0.7.24's code doesn't set them →

```
error[E0063]: missing fields `key_derivation_algorithm` and `key_ring_size`
error: could not compile `livekit`
```

The **entire** webrtc family must move together — pinning only `libwebrtc` is not enough,
because `webrtc-sys` and `webrtc-sys-build` will independently float to newer versions that
download a newer prebuilt webrtc whose C++ glue namespace changed (`rtc::scoped_refptr` →
`webrtc::scoped_refptr`), giving `error: 'rtc' has not been declared` in `frame_cryptor.cpp`.
livekit 0.7.24's own bundled lockfile uses **libwebrtc 0.3.19 + webrtc-sys 0.3.16 +
webrtc-sys-build 0.3.11**. Pin all three and commit the lockfile:

```bash
cargo update -p libwebrtc        --precise 0.3.19
cargo update -p webrtc-sys       --precise 0.3.16
cargo update -p webrtc-sys-build --precise 0.3.11   # selects the matching prebuilt webrtc download
cargo clean -p webrtc-sys && rm -rf target/*/build/scratch-*   # purge any stale webrtc download
git add gateway/Cargo.lock          # .gitignore now allows /gateway/Cargo.lock
```

> **Deeper finding:** even fully pinned, building the *old* livekit 0.7.24 on a *modern*
> ARM64 + GCC 13 host is fragile (old webrtc C++ vs new toolchain). The durable fix is to
> upgrade livekit to a release that tracks current webrtc-sys — but `Cargo.toml` pins
> `livekit = "=0.7.24"` for prost-0.12 / google-api-proto compatibility, so that upgrade is a
> tracked workstream (see PRODUCTION_PLAN.md risk register), not a one-liner.

## ARM64 + CUDA native build (Gotcha 2)

`webrtc-sys 0.3.16`'s `build.rs` enables the NVIDIA hardware video codec whenever
`<CUDA_HOME>/include/cuda.h` exists (default `/usr/local/cuda`) **with no architecture
guard**. On aarch64 it then tries to assemble x86-64 trampolines
(`src/nvidia/implib/libnvcuvid.so.tramp.S`, full of `call`/`jmp *%rax`) →

```
Error: unknown mnemonic `call' -- `call _libnvcuvid_so_save_regs_and_resolve'
error occurred in cc-rs: ... libnvcuvid.so.tramp.S
```

The gateway uses WebRTC for **audio**, not the NVIDIA video codec, so skip it by pointing
`CUDA_HOME` at a directory without `cuda.h`:

```bash
CUDA_HOME=/tmp/nocuda cargo build --locked   # /tmp/nocuda/include/cuda.h absent → NVIDIA block skipped
```

(For x86-64 hosts with CUDA this codec builds fine; the issue is ARM64-only. CI runs on
x86-64 ubuntu-latest without CUDA, so it is unaffected.)

## Toolchain (Gotcha 3)

Do **not** drop a `rust-toolchain.toml` pinning a numbered channel + extra targets into this
repo on a sandboxed host — rustup will attempt a fresh toolchain install that can fail with
`Directory not empty` and block `cargo` entirely. Pin the toolchain in **CI** instead
(`dtolnay/rust-toolchain@stable`, see `.github/workflows/ci.yml`). edition 2024 needs Rust ≥ 1.85.

## Feature flags (Gotcha 4 — was S2)

`default = []` ships **stub** VAD/turn-detection (returns `Ok(0.0)`/`false`). For the real
neural components build with:

```bash
cargo build --release --features dag-routing,turn-ensemble,noise-filter,openapi
```

`turn-ensemble` = `silero-vad` + `smart-turn` + `turn-detect`. `ort` downloads its own ONNX
Runtime binaries; the turn-detect model needs `cargo run --features turn-detect -- init`.

> **Gotcha 5 (was a hard build break):** `ort` is specced `2.0.0-rc.10` but `^2.0.0-rc.10`
> floats to `rc.12`, whose `Session` API made `inputs`/`outputs` private and the error type
> non-`Send`/`Sync` — `turn_detect/model_manager.rs` then fails with **96 errors** and the
> entire `turn-ensemble`/`turn-detect` feature does not compile. The committed `Cargo.lock`
> pins `ort = 2.0.0-rc.10`; if regenerating, run `cargo update -p ort --precise 2.0.0-rc.10`.

> **Gotcha 6 (ONNX Runtime at runtime):** `ort` is built with `load-dynamic`, so it loads
> `libonnxruntime.so` at RUNTIME via dlopen — `download-binaries` does NOT satisfy this. Without
> it, every ONNX feature (turn-detect/smart-turn/silero) panics at model load
> (`libonnxruntime.so: cannot open shared object file`). Provide ONNX Runtime **1.22.x** (the
> version rc.10 expects) and point at it:
> ```bash
> curl -sSL -o ort.tgz https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-linux-<arch>-1.22.0.tgz
> tar xzf ort.tgz
> export ORT_DYLIB_PATH=$PWD/onnxruntime-linux-<arch>-1.22.0/lib/libonnxruntime.so
> ```
> Turn-detect also needs the model provisioned first: `CACHE_PATH=$HOME/.cache/waav-gateway cargo run --features turn-detect -- init`.

## System prerequisites

`cc`/`g++`, `cmake`, `pkg-config`, `ld`. No system OpenSSL needed (rustls). No system
ONNX Runtime needed (`ort` `download-binaries`).
