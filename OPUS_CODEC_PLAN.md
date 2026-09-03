# D8 — Full-duplex Opus codec (paired gateway + SDK)

Last perf/resilience item (Tier-1 `941aba3`, Tier-2 `1a1cddb`). User chose **full-duplex**:
uplink (SDK encode → gateway decode → STT) **and** downlink (gateway transcode TTS→opus → SDK
decode). Today both `/ws` audio directions are raw linear16.

## Why opus
Mobile/bad-network mic uplink at 16–24 kbps + DTX (vs ~256 kbps linear16@16k) and a comparable
downlink saving; opus also has built-in PLC the decoder applies on a dropped packet. This composes
with the Tier-1/Tier-2 jitter buffer + scheduled player (decode is a front-stage before them).

## Load-bearing principle — negotiate + graceful-degrade, NEVER break
The codec is **optional on both ends**. A client may request opus but the gateway binary may be
built **without** the codec (default), or the browser may lack an opus encoder. So:

1. Client sends desired codecs in the config envelope:
   - `stt_config.audio_in_codec`  : `"linear16"` (default) | `"opus"`
   - `tts_config.audio_out_codec` : `"linear16"` (default) | `"opus"`
2. Gateway **negotiates** against what it can actually do (`cfg!(feature = "opus-codec")`) and
   **echoes the EFFECTIVE codecs in the `ready` message**:
   `ready.audio_in_codec` / `ready.audio_out_codec`.
3. Client uses whatever `ready` reports. If it asked opus and `ready` says `linear16`, it sends/decodes
   linear16. A `config_warning` explains the downgrade. **No path ever errors on an opus request.**

This is the same canonical mapper/graceful-degrade pattern as emotion/lang/voice standardization.

## Architecture — cargo feature `opus-codec` (default OFF)
Mirrors the existing `dag-routing` / `turn-detect` / `noise-filter` / `openapi` features.

- **Default build:** ZERO new deps, ZERO build tooling, ZERO risk. Only the negotiation layer compiles;
  opus requests degrade to linear16. Fully unit-testable with no libopus.
- **`--features opus-codec` build:** pulls `opus = "0.3"` (→ cached `audiopus_sys`, vendored libopus
  built offline via CMake — verified: cmake 3.28 + cc + 210-file source all present, no headers/network
  needed). Real encode/decode wired into the seams.

## Wire framing — one opus packet per WS binary frame (both directions)
WebSocket preserves message boundaries, so each binary frame carries exactly one opus packet. No Ogg
container. Encoder/decoder are **one per connection, continuous** (opus is a streaming codec; barge-in
just stops feeding — no reset). Symmetric on uplink and downlink.

### Constraints (opus is picky; validate + coerce at config parse, warn on coercion)
- Sample rate ∈ {8000, 12000, 16000, 24000, 48000}. Uplink uses `stt_config.sample_rate`; downlink uses
  `tts_config.client_playback_rate` (default **48000** when opus + unset). Non-opus rate → warn + nearest.
- Frame size ∈ {2.5,5,10,20,40,60} ms. Downlink reuses `audio_out_chunk_ms` (default **20**); the
  existing `OutputChunker` already emits exact `chunk_ms` PCM16 chunks → one chunk = one opus frame.
- Mono only (channels=1, already the default).

## Ingress seam (uplink: opus → PCM16 → STT)
`core/voice_manager/manager.rs::receive_audio` (~521-608) — currently forwards raw bytes to STT at
~601-607. Insert: if effective in-codec == Opus, `OpusStreamDecoder.decode(frame) -> PCM16 bytes` at
`stt_config.sample_rate`, then proceed exactly as today (VAD f32 convert + STT send see PCM16). The
decoder is held in the VoiceManager. `audio_in_codec` is a GATEWAY transport codec, distinct from the
provider `encoding` field (which some STT providers — Alibaba/Reverie — use to accept opus directly);
they must not be conflated.

## Egress seam (downlink: TTS PCM16 → opus → client)
`handlers/ws/config_handler.rs` egress path: `EgressAudio::convert()` normalizes to PCM16 @ client rate,
then `OutputChunker` emits 20ms PCM16 chunks, each sent as `MessageRoute::Binary(DroppableAudio)`.
Insert the encode **after the chunker, per emitted chunk**: encode the PCM16 chunk → one opus packet →
send as the binary frame. Utterance-end `flush_remainder` may emit a short tail → **zero-pad to a full
opus frame** before encoding (a few ms trailing silence, inaudible). If the audio at the seam is NOT
linear PCM16 (a provider that emits mp3/ogg — sniffed), opus egress **falls back to passthrough** for
that stream + warns (no in-gateway container decode this phase). Encoder held per-connection alongside
the chunker state.

## SDK side — optional / dynamically-loaded, respect `ready`
- **TypeScript + widget:** opus via **WebCodecs** `AudioEncoder`/`AudioDecoder` (codec `'opus'`),
  `supportsWebCodecsOpus()` feature-detect, **dynamically imported** so the default bundle stays lean.
  - Encode runs on the **main thread** (AudioWorkletProcessor can't host WebCodecs): the capture worklet
    already posts 20ms Int16 frames to the main thread → encode each → send. 20ms aligns to one opus frame.
  - Decode is a **front-stage before the jitter buffer + scheduled player**: opus packet → PCM16 → existing
    playout. Opus's own PLC complements the player's underrun concealment.
  - The client only SETS `audio_in/out_codec: opus` when `supportsWebCodecsOpus()` AND the user opted in,
    and always conforms to the `ready` echo (downgrade to linear16 if the gateway says so).
- **Python:** defer (server/headless client; lower value, needs a system libopus). Negotiation-aware
  (will send linear16, parse the `ready` echo) but no encode/decode this phase.

## Validation
- **Default build (no feature):** negotiation coerces opus→linear16, `ready` echoes linear16, a
  `config_warning` is emitted; gateway lib suite + clippy green. (live, zero-risk)
- **`opus-codec` feature:** round-trip unit test — encode N frames then decode → PCM16 matches the input
  within opus lossy tolerance (energy/correlation, not bit-exact); rate/frame coercion tests; ingress
  decode + egress encode wired-path tests. Build the feature (`cargo build --features opus-codec`) to
  prove the vendored libopus compiles.
- **SDK:** `supportsWebCodecsOpus()` detect; encode→decode round-trip via WebCodecs where available
  (jsdom lacks WebCodecs → seam tested with an injected fake encoder/decoder, like the Silero seam);
  config sets opus only when supported; `ready`-echo downgrade honored.
- **Live e2e (browser):** real opus round-trip through the gateway `/ws` — the final manual gate
  (documented; needs a WebCodecs browser + an `opus-codec` gateway build).

## Commit sequence
1. Gateway negotiation scaffold (config fields + `core/audio/codec.rs` + `ready` echo + degrade) — default build, tested.
2. Gateway `opus-codec` feature (encode/decode wired into both seams) — built + tested under the feature.
3. SDK opus (TS + widget WebCodecs path, optional/dynamic) — tested; Python negotiation-aware.
