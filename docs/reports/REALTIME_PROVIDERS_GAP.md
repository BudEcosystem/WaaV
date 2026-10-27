# Realtime / Speech-to-Speech Provider Gap

Which vendors offer an **OpenAI-Realtime-style bidirectional audio↔audio session**
(audio IN → audio OUT in one integrated API: S2S / voice-agent / multimodal-live —
*not* streaming STT or streaming TTS alone) that WaaV has **not** wired as a realtime
provider. From a 3-agent sweep of WaaV's ~70 integrated vendors + Pipecat's catalog
(2026-06-15). Doc/marketing-sourced (high confidence, corroborated across official
API refs), **not yet live-probed** — verify each message schema when wiring.

**WaaV wires only 2 realtime providers today: OpenAI `gpt-realtime` + Hume EVI.**
At least **11** more are available from vendors WaaV *already integrates* for STT/TTS,
plus **3** net-new vendors Pipecat has.

---

## A. Vendor ALREADY integrated in WaaV (STT/TTS) — realtime path NOT wired
Credentials/config plumbing already exist → highest leverage. Each needs one
`BaseRealtime` WS-client module (≈ the OpenAI realtime client) unless noted.

| # | Provider | Product | Protocol + endpoint | Status | Effort |
|---|---|---|---|---|---|
| 1 | **Azure OpenAI Realtime / Voice Live** | `gpt-realtime` on Azure; Voice Live unified agent | WS, event schema **≈ OpenAI Realtime** | GA (realtime) / preview (Voice Live) | **XS** — reuse WaaV's OpenAI realtime client, swap auth (`api-key`) + endpoint (`wss://<res>.services.ai.azure.com/voice-live/realtime?api-version=…`) |
| 2 | **Google Gemini Live** | Gemini 2.5 Flash native-audio | WS `BidiGenerateContent` (`wss://{loc}-aiplatform.googleapis.com/ws/…/BidiGenerateContent`; AI-Studio variant) | **GA on Vertex** (Dec 2025) | **M** — own event schema; also unlocks Vertex auth |
| 3 | **Deepgram Voice Agent** | voice-to-voice agent (STT+LLM+TTS+turn-taking, BYO-LLM optional) | WS `wss://agent.deepgram.com/v1/agent/converse` | GA | **M** |
| 4 | **ElevenLabs Conversational AI** | Agents Platform (ASR+LLM+TTS) | WS `wss://api.elevenlabs.io/v1/convai/conversation?agent_id=…` (+ signed-URL for private) | GA | **M** |
| 5 | **AssemblyAI Voice Agent** | "speech in, speech out" (Universal-3 + LLM + TTS) — **NOT STT-only anymore** | WS `wss://agents.assemblyai.com/v1/ws?token=…` (PCM16 in/out) | GA | **M** |
| 6 | **Speechmatics Flow** | conversational voice-agent | WS `wss://flow.api.speechmatics.com/v1/flow` | GA | **M** |
| 7 | **PlayHT / Play.ai** | Play.ai voice agents | WS `wss://api.play.ai/v1/talk/<agent_id>` (`setup`→`audioIn`/`audioStream`) | GA | **M** |
| 8 | **AWS Nova 2 Sonic** | `amazon.nova-2-sonic-v1:0` (Bedrock) | **HTTP/2 bidi event-stream** `InvokeModelWithBidirectionalStream` (NOT WebSocket) | GA | **L** — needs a new (non-WS) transport + SigV4 |
| 9 | **iFlytek 超拟人交互** | unified-net direct S2S (Spark) | WS + HMAC-SHA256 (`*.xf-yun.com`; exact path unconfirmed) | GA (CN) | **M** — verify endpoint |
| 10 | **Yandex Realtime** | SpeechKit / AI-Studio realtime voice model (RU/KK) | WS, JSON events (endpoint behind CAPTCHA) | GA? | **M** — verify endpoint/status |
| 11 | **Smallest.ai Hydra** | full-duplex S2S model | access-gated; no public direct WS endpoint surfaced (routed via Atoms) | beta/gated | **qualify first** (probe/contact before committing) |

## B. Net-new vendors (NOT in WaaV at all) — Pipecat integrates these
| Provider | Product | Protocol | Note |
|---|---|---|---|
| **xAI Grok Realtime** | Grok Voice Agent API | WS | Net-new vendor; high mindshare, frontier model |
| **Ultravox** | hosted Realtime API (`api.ultravox.ai`) | WS (`join_url`), 48 kHz | Net-new; popular low-latency speech-LLM |
| **Inworld** | Realtime API (Realtime TTS-2 + server VAD) | WS | Net-new; character/agent personas |

## C. Worth knowing (not in WaaV, not in Pipecat)
**Kyutai Moshi / Hibiki** (open full-duplex S2S; Hibiki = live S2S translation; self-hostable) · **Gradium** (commercial Kyutai spin-off; in Pipecat as STT/TTS only) · **Mistral Voxtral** (audio direction; no Mistral in WaaV).

## D. Checked and ruled OUT (stay STT/TTS — no integrated audio↔audio API)
**Cartesia** (Line = an agent SDK/framework, not a wireable S2S endpoint; Sonic is TTS) ·
**Gladia** (real-time STT only) · **LMNT / Murf / Speechify** (streaming TTS only) ·
**Resemble** ("S2S" = voice *conversion*, no LLM/turn-taking) · **Sarvam** (Samvaad is a
managed no-code platform, no S2S dev API) · **Tencent** (TRTC ConvAI is a cascade
orchestrator; Hunyuan Voice is app-only) · **Baidu / NAVER CLOVA / IBM Watson / Huawei**
(streaming STT/TTS or cascade, no single bidirectional model API).

**WaaV is AHEAD of Pipecat on Hume EVI** — Pipecat's Hume integration is TTS-only.

---

## Recommended order to add (value ÷ effort)
1. **Azure OpenAI Realtime** — XS, reuses the existing OpenAI realtime client; unlocks Azure-governed/enterprise customers immediately.
2. **Google Gemini Live** — flagship S2S, vendor already integrated; biggest reach.
3. **Deepgram Voice Agent + ElevenLabs Conversational AI + AssemblyAI Voice Agent** — all GA WebSocket, vendors already integrated, similar shape (batch a shared S2S scaffold).
4. **Speechmatics Flow + PlayHT Play.ai** — same WS S2S pattern.
5. **AWS Nova 2 Sonic** — high enterprise demand but needs a new HTTP/2 bidi-stream transport (most engineering).
6. **xAI Grok Realtime / Ultravox / Inworld** — net-new vendors (Pipecat parity); add after the existing-vendor wins.
7. **iFlytek / Yandex** — regional; verify exact endpoints first.

All implement the same `BaseRealtime` trait + `create_realtime_provider` factory + per-provider api-key map that OpenAI/Hume already use, so the per-provider cost is mostly the vendor's session-config + event-name mapping (exactly the Beta→GA work already done for OpenAI).
