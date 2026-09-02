# Build notes

## The gateway does not currently build (pre-existing)

`cargo build` in `gateway/` fails inside the vendored `livekit-0.7.24` crate, which is resolved
against a newer `libwebrtc` than it was written for:

```
error[E0063]: missing fields `key_derivation_algorithm` and `key_ring_size` in initializer of
              `libwebrtc::native::frame_cryptor::KeyProviderOptions`
  --> livekit-0.7.24/src/room/e2ee/key_provider.rs:54
error[E0639]: cannot create non-exhaustive struct using struct expression
  --> livekit-0.7.24/src/room/mod.rs:376
```

Every error is in third-party source. Fixing it means pinning `libwebrtc` to the version
`livekit 0.7.24` expects, or moving to a `livekit` release built against the current one.

### Host prerequisites, if you get past that

`webrtc-sys` is particular, and its errors name the requirement:

- **clang 21 or newer.** libwebrtc.a is built against Chromium's hermetic libc++, whose
  `trivial_abi` annotations GCC ignores — which silently breaks the calling convention for
  `unique_ptr`/`shared_ptr`. Ubuntu 24.04 ships clang 18; use `apt.llvm.org/llvm.sh 21`, then
  build with `CC=clang-21 CXX=clang++-21`.
- **glib** — `apt-get install libglib2.0-dev pkg-config`.

## `bud-auth` builds and tests independently

This is why it is a separate crate. The auth plane is security-critical and needs a test cycle
measured in seconds, not one that rebuilds a WebRTC stack — and, as above, one that currently
cannot be rebuilt at all.

```bash
cd bud-auth && cargo test                 # 101 unit + contract tests
BUD_AUTH_TEST_REDIS_URL=redis://127.0.0.1:6399/9 cargo test   # + 5 live-server tests
cargo clippy --all-targets -- -D warnings
```

`gateway/src/auth/bud_mode.rs` is the only gateway-side file in the integration. It cannot be
compile-verified in place until the livekit issue above is resolved; it is checked in isolation
against the `bud-auth` API, which is what `Cargo.toml`'s path dependency exercises.

## Update — the gateway now builds

The failure above is fixed. Root cause: **`libwebrtc` broke compatibility inside its own 0.3.x
line**, so cargo resolved forward past what `livekit 0.7.24` was written against.

The pin cascades, because each crate decides what the one below it downloads:

| Crate | Resolved to | Pinned to | Declared by |
|---|---|---|---|
| `libwebrtc` | 0.3.46 | **0.3.19** | `livekit 0.7.24` |
| `webrtc-sys` | 0.3.43 | **0.3.16** | `libwebrtc 0.3.19` |
| `webrtc-sys-build` | 0.3.19 | **0.3.11** | `webrtc-sys 0.3.16` |

`webrtc-sys-build` is the one that catches people out: it selects the prebuilt libwebrtc
artifact, so a mismatch downloads successfully and then fails compiling C++ headers that
reference a `cricket` namespace the newer artifact no longer has.

`libwebrtc` and `webrtc-sys` are pinned in `Cargo.toml` as direct dependencies — nothing in this
crate calls them, but a pin in the manifest is visible, reviewable, and survives a fresh
checkout.

> **`gateway/Cargo.lock` is gitignored, and that is why this happened.** The gateway is an
> application, not a library: committing its lockfile is the standard practice precisely so a
> fresh checkout cannot silently resolve a different dependency graph. The manifest pins above
> cover the crates known to break; committing the lockfile would cover the ones nobody has hit
> yet. Worth doing.

### Host prerequisites

```bash
curl -fsSL https://apt.llvm.org/llvm.sh | bash -s 21   # clang 18 is not enough
apt-get install -y libglib2.0-dev libva-dev libdrm-dev pkg-config
CC=clang-21 CXX=clang++-21 cargo build
```

When changing any of the pins above, clear the stale prebuilt artifact first or the errors will
make no sense:

```bash
rm -rf target/debug/build/scratch-* target/debug/build/webrtc-sys-*
```
