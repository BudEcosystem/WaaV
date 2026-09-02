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
