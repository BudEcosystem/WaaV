# Related work: Bud WaaV interaction mode

Research notes for the arXiv-style paper and the launch video, compiled 2026-10-27.

**Conventions**
- Every number carries its source URL, either inline or in the section it comes from.
- **(SR)** marks a vendor or author self-reported number.
- **AA** = Artificial Analysis, independent.
- **FDB** = Full-Duplex-Bench.
- Citation keys in `[brackets]` refer to `refs.bib` in this folder.

**The headline correction to our own story:** since TML's May 2026 preview, the "fast front model + slow background worker" pattern has shipped in hosted, closed products:
- OpenAI GPT-Live (voice layer plus backend model, API GA Sep 2026)
- Google Gemini 3.8 Live (background tools while talking, Sep 2026)
- LiveKit async tools (Jun 2026)
- Pipecat async functions

WaaV's novelty claim should therefore rest on:
1. open source and self-hosted, end to end
2. an explicit per-turn decision by a small open front model (request, correction, stop, answer, wait)
3. agents (A2A, MCP, webhook, or an orchestrator LLM) as first-class background tasks with progress and cancellation
4. confirmation gates on side-effecting tools, plus caller controls
5. any vendor in every slot
6. reproducible numbers on public benchmarks

## 0. Comparison table

Legend:
- **Lat.** = the latency metric and value the source reports. These metrics are not mutually comparable; the metric name is always given.
- **FDB v1 turn lat.** = Smooth-Turn-Taking latency, from end of user speech to model start, from the FDB README (<https://github.com/DanielLin94144/Full-Duplex-Bench/blob/main/v1_v1.5/README.md>) unless noted.
- **BG tasks** = does the system keep talking while tools or agents run.

| System | Architecture | Open / closed | Self-hostable | Latency metric = value (source) | FD-bench numbers | Tool use / background tasks | Provider choice | Availability (as of 2026-10-27) |
|---|---|---|---|---|---|---|---|---|
| **Bud WaaV interaction mode** | Cascade: Silero VAD + Smart Turn v3.2 + LiveKit text EOT → STT → front model (Qwen3.5-2B, llama.cpp) per-turn decision → TTS; background = any LLM / orchestrator (e.g. gpt-oss-20b) / A2A / MCP / webhook agents as tasks | Open source | Yes | *To be measured; report FDB v1 turn latency, front-decision latency, ack TTFA* | *To be measured (v1, v1.5 per-scenario, v3)* | Yes: tasks with progress, confirmations for side-effecting tools, caller controls | 44 speech vendors + any LLM/agent | Available (open source) |
| TML Interaction Models (TML-Interaction-Small) | Native full-duplex, 200 ms micro-turns, 276B MoE / 12B active + async background model | Closed | No | FDB v1 turn latency **0.40 s** (SR, TML blog) | v1 lat 0.40 s; v1.5 "avg" 77.8 (TML-defined); v3 RQ 82.8 / Pass@1 68.0 (with BG agent) (SR) | Yes: background model, simultaneous tool calls (SR) | None | Research preview; limited preview "in the coming months" |
| OpenAI GPT-Live-1 | Full-duplex voice layer, interaction decisions "many times per second", delegates to backend text model | Closed | No | No absolute latency published; AA TTFA 1.34 s (Astra, medium) | "+30 pp Full Duplex Bench over GPT-Realtime-2.1" (SR; version unstated) | Yes: backend model / third-party model; async RPC | Backend: OpenAI or third-party LLM; OpenAI voices | API GA (Sep 2026), $0.05/min voice layer |
| OpenAI gpt-realtime(-1.5/-2/-2.1) | Single speech-to-speech model; gpt-realtime-2 reasoning effort minimal…xhigh | Closed | No | GPT-4o: avg 320 ms audio response (SR, 2024); AA TTFA: gpt-realtime 0.98 s, 1.5 0.81 s, 2 (min) 1.12 s | GPT-Realtime row: turn lat **1.470 s**, Pause-Syn TOR 0.010; TML table: 1.5 0.59 s, 2.0-min 1.18 s; FDB-v3 Pass@1 0.600 | Async function calling, MCP, SIP; rt-2 preambles + parallel tools | OpenAI only | GA; audio $32 / $64 per 1M tokens in / out (≈$1.15 / $4.61 per hr, AA); 60-min session cap |
| Google Gemini 3.1 Flash Live (preview) | Native audio, "based on Gemini 3 Pro", thinkingLevel minimal…high | Closed | No | FDB v1 turn lat **0.567 s (min) / 0.940 s (high)**; AA TTFA 0.96 s (min) / 2.99 s (high) | v1 above; v1.5 avg 54.3 / 45.5 (TML-defined); FDB-v3 Pass@1 0.540, lat 4.25 s | Synchronous function calling only (no async) | Google only | Legacy preview; $0.005/min in, $0.018/min out |
| Google Gemini 3.8 Live (+ Extended Thinking) | Native audio; background tools while talking | Closed | No | No latency figure (SR); AA TTFA 1.18 s / 1.35 s | Not on FDB README | Yes: NON_BLOCKING async calls default (SILENT / WHEN_IDLE / INTERRUPTED) | Google only | GA in Gemini API (Sep 2026); 15-min audio session without compression |
| Gemini 2.5 Flash native audio | Native audio | Closed | No | AA TTFA 0.63 s (Dialog) / 3.87 s (Thinking) | On FDB README (v1/v1.5 code); numbers not in table | Function calling; ComplexFuncBench Audio 71.5% (SR) | Google only | GA on Vertex |
| Kyutai Moshi | Native full-duplex, Helium 7B + Mimi 12.5 Hz, inner monologue | Open weights (CC-BY 4.0) | Yes (GPU) | 160 ms theoretical / 200 ms practical (SR) | turn lat 0.265 s; Pause-Syn TOR 0.985; BC TOR 1.000 | None | None | Released 2024 |
| Kyutai Unmute | Cascade STT → any LLM → streaming TTS, semantic VAD | Open source (MIT) | Yes (16 GB+ GPU) | "response latency is below a second" (SR) | Not reported | Not a focus | Any OpenAI-compatible LLM; Kyutai STT/TTS | Released 2025 |
| NVIDIA PersonaPlex | Moshi fine-tune + hybrid voice/role prompt | Open weights (NVIDIA Open Model License) | Yes (GPU) | FDB v1 turn lat **0.070 s** (paper) / 0.170 s (released ckpt) | Pause-Syn TOR 0.584; Intr TOR 1.000, GPT-4o 4.210 | None (future work) | None | Released Jan 2026 |
| NVIDIA NemotronLabs VoiceChat | 11B hybrid Mamba/Transformer full-duplex | Open weights (research only) | Yes (GPU, vLLM) | ~450 ms turn-taking; FDB v1 448 ms (SR) | Pause-Syn TOR 0.153; Intr 480 ms; FDB-v3 Pass@1 33% (SR) | Tool calling with "on-hold" message; **no interrupt during tool execution** | None | Research release Jul 2026 |
| Freeze-Omni | Frozen Qwen2-7B + speech enc/dec + chunk state head | Open | Yes | 745 ms avg statistical (SR) | turn lat 0.953 s; Pause-Syn TOR 0.642 | None | None | 2024 |
| dGSLM | Textless dual-tower | Open (research) | Yes | n/a | turn lat 0.352 s; Pause-Syn TOR 0.934 | None | None | 2022 |
| Sesame CSM | Conversational TTS (Llama backbone + Mimi), not duplex | CSM-1B open (Apache-2.0); product closed | 1B only | n/a | n/a | n/a | n/a | Demo + 1B weights |
| Pipecat + Smart Turn | Framework: VAD + Smart Turn audio EOT (8 MB) → STT → LLM → TTS | Open source (BSD-2) | Yes | Smart Turn 12.6 ms CPU inference (SR, v3) | Not reported | Async function calls (`cancel_on_interruption=False`); no confirmation pattern | Very broad (38 STT / 59 TTS / 32 LLM) | Available |
| LiveKit Agents | Framework: VAD + text / audio EOT models; adaptive interruption | Open source (Apache-2.0); some models cloud-only | Partly | Text EOT ~50–160 ms CPU; audio EOT v1 9.9% false cutoff at 300 ms (SR) | Not reported (used as FDB-v3 harness) | Async tools since 1.6 (Jun 2026): ack + progress, cancel, duplicate-confirm | Broad | Available; $0.01/min Cloud session |
| Vapi | Hosted orchestration | Closed | No | "<500ms" (SR) | n/a | Tool status messages; async toggle (undocumented); no confirmations | BYO providers / custom LLM | $0.05/min + providers |
| Retell AI | Hosted, proprietary turn-taking | Closed | No | turn-taking "roughly 600ms" (SR) | n/a | Not documented | Several LLMs incl. realtime | $0.07–0.31/min |
| Deepgram Voice Agent | Hosted agent API; Flux STT with built-in EOT | Closed (self-host option) | Partly | Flux "~260ms end-of-turn detection" (SR) | n/a | Function calls; no documented talk-while-running | BYO LLM / TTS | $0.050–0.075/min standard |
| ElevenLabs Agents / Eleven Realtime | Hosted cascade + new end-to-end-tuned S2S | Closed | No | Eleven Realtime TTFA 570–620 ms (SR, "measured by AA") | "Full-Duplex-Bench 95.8%" (SR; version / metric unstated) | Per-tool interruption modes; no async / confirmations documented | Broad LLM choice | $0.08/min + LLM |

## 1. Thinking Machines Lab: "Interaction Models" (11 May 2026)

Source for this whole section, unless noted: TML blog post, <https://thinkingmachines.ai/blog/interaction-models/> (DOI 10.64434/tml.20260511) [`tml2026interaction`]. Authorship is "Thinking Machines Lab", with no individual authors named. **Every number below is self-reported by TML.**

### 1.1 Problem framing (as TML states it)
- "Today's models experience reality in a single thread": turn-based models give "a narrow channel for human-AI collaboration".
- "For interactivity to scale with intelligence, it must be part of the model itself." Interactivity is trained into the model, not bolted on with VAD/turn-detector harnesses.
- This is the main philosophical contrast with Bud WaaV. TML puts interactivity inside one large trained model. WaaV builds it in a gateway from a small, fast front model plus swappable components (VAD, Smart Turn, text turn detector, any STT/TTS, any background LLM/agent).

### 1.2 Architecture
- **Two parts.** (a) A real-time **interaction model** stays in the conversation with the user. (b) An **asynchronous background model** handles sustained reasoning, tool use and longer-horizon work. Both share context. The post does not give the background model's size.
- **Model:** `TML-Interaction-Small`, a **276B-parameter MoE with 12B active**, trained from scratch.
- **Micro-turns:** continuous input and output streams are processed in **200 ms chunks** (200 ms in, 200 ms out per micro-turn). The model therefore decides every 200 ms whether to speak, stay silent, backchannel or interject.
- **Encoder-free early fusion.** Audio goes in as dMel through a light embedding layer. Video goes in as 40x40 patches through an hMLP. Audio comes out of a flow head (Mel flow). All parts are co-trained with the transformer.
- **Serving:** streaming sessions append each 200 ms chunk to a persistent GPU sequence. TML upstreamed a version to SGLang and wrote latency-optimised MoE kernels (gather+gemv). Batch-invariant kernels cost "<5%" end-to-end overhead.
- **Claimed capabilities, shown in the demos:** seamless dialogue management, verbal and visual interjections, simultaneous speech (live translation), time awareness, and simultaneous tool calls, search and generative UI.

### 1.3 Reported benchmark numbers (TML table, verbatim)
Columns: TML-Interaction-Small | GPT-realtime-2.0 (minimal) | GPT-realtime-1.5 | Gemini-3.1-flash-live (minimal) | Qwen 3.5 Omni-plus-realtime | GPT-realtime-2.0 (xhigh) | Gemini-3.1-flash-live (high).

| Benchmark (metric, modality) | TML | GPT-rt-2.0 min | GPT-rt-1.5 | Gem-3.1 FL min | Qwen3.5-Omni-plus-rt | GPT-rt-2.0 xhigh | Gem-3.1 FL high |
|---|---|---|---|---|---|---|---|
| FD-bench v1 turn-taking latency (s) ↓ | **0.40** | 1.18 | 0.59 | 0.57 | 2.14 | 1.63 | 0.94 |
| FD-bench v1.5 Average ↑ | **77.8** | 46.8 | 48.3 | 54.3 | 39.0 | 47.8 | 45.5 |
| FD-bench v3 Response Quality % / Pass@1 % (audio+tools) | 82.8* / 68.0* | 80.0 / 52.0 | 77.9 / 55.0 | 68.5 / 48.0 | 60.0 / 50.0 | 81.0 / 58.0 | 71.4 / 48.0 |
| QIVD accuracy % (video+audio) | 54.0 | 57.5 | 41.2 | 54.7 | 59.0 | 58.2 | 56.1 |
| Audio MultiChallenge APR % | 43.4 | 37.6 | 34.7 | 26.8 | n/a | 48.5 | 36.1 |
| BigBench Audio accuracy % | 75.7 / 96.5* | 71.8 | 81.4 | 71.3 | 73.0 | 96.6** | 96.6 |
| IFEval (VoiceBench) % audio | 82.1 | 81.7 | 68.1 | 67.6 | 80.3 | 83.2 | 82.8 |
| IFEval % text | 89.7 | 89.6 | 87.5 | 85.8 | 83.4 | 95.2 | 90.0 |
| HarmBench refusal % (text) | 99.0 | 99.5 | 100.0 | 99.0 | 99.5 | 100.0 | 98.0 |

\* With the background agent enabled ("For benchmarks that require reasoning or tool calls we report our results with background agent enabled"). \*\* Artificial Analysis figure for GPT-realtime-2.0 with thinking set to high. TML attributes the Audio MultiChallenge baselines to Scale AI and the BigBench Audio baselines to Artificial Analysis.

**Internal interaction benchmarks** (TML vs GPT-realtime-2.0 minimal): TimeSpeak macro-acc 64.7 vs 4.3; CueSpeak macro-acc 81.7 vs 2.9; RepCount-A off-by-one 35.4 vs 1.3; ProactiveVideoQA PAUC@ω=0.5 33.5 vs 25.0 (staying silent scores 25.0); Charades mIoU 32.4 vs 0.

**Caveats for our paper:**
- TML does not define how the "FD-bench v1.5 Average" is formed. The FDB v1.5 paper itself defines **no** aggregate score (§6.2). The 77.8 is therefore TML's own aggregation of v1.5 behaviour categories, and its weighting is unknown. Do not compare a WaaV v1.5 "average" with it unless the formula is matched, or the formula is stated and per-category numbers are reported.
- The FD-bench v3 numbers in TML's table (for example GPT-realtime-1.5 Pass@1 55.0) differ from the FDB-v3 paper's own GPT-Realtime Pass@1 of 0.600 (§6.4). The runs, model versions and harnesses differ.
- The FD-bench v1 "turn-taking latency" corresponds to FDB v1's Smooth-Turn-Taking latency (end of user speech to start of model response; §6.1).

### 1.4 Stated limitations (TML's own list)
- **Long sessions:** continuous audio and video fill the context quickly, so very long sessions need careful context management.
- **Connectivity:** low-latency streaming needs reliable connections; without one, "the experience degrades significantly".
- **Alignment and safety:** real-time interfaces raise open questions.
- **Scale:** larger pretrained models are "currently too slow to serve in this setting"; larger models are planned "later this year".
- **Background agents:** TML says it has "just scratched the surface" of how background agents work with the interaction model.

### 1.5 Availability
- The post says "Today, we're announcing a research preview of interaction models", but also "In the coming months, we will open a limited research preview to collect feedback, with a wider release later this year." On 11 May 2026 it was therefore **not generally available**: closed weights, hosted only, with a research grant "coming soon".
- **Positioning for WaaV:** WaaV interaction mode targets the same user-visible behaviour, an immediate acknowledgement while background work runs as tasks. It does so with open components, self-hosted, on any vendor, with a 2B front model rather than 276B/12B-active, and it is available today.

### 1.6 TML launch video: "Introducing interaction models"
Source: YouTube <https://www.youtube.com/watch?v=A12AVongNN4>, channel @thinkymachines, published 11 May 2026 [`tml_video_2026`].
- **Length:** 2:15 (135–136 s from YouTube's `lengthSeconds`). About 61k views on 27 Oct 2026.
- **Description:** "For AI to collaborate with people the way we do with each other, it should listen, talk, watch, show, and think at the same time… This is a preview demo of our interaction model."
- **Where the clips live:** the blog embeds no player. It shows 9 short vignette clips (animal story, slouching, danger, anger, QA, Uber, search, diet, in-ear) under its capability headings.

**Structure**, from the auto-captions and an 8-second contact sheet:

| Time | What happens |
|---|---|
| 0:00–0:16 | **Setup by instruction.** The presenter, alone at a desk, tells the model: "Every time one of [my friends] enters the frame, I need you to say 'friend'." Title cards: "THINKING MACHINES", "Introducing Interaction Models". |
| 0:19–0:37 | He explains full duplex while the model listens. |
| 0:41 | **Proactive visual trigger.** The model says "Friend." unprompted as friend 1 walks in. |
| 0:42–1:13 | **Simultaneous translation.** Friend 1 speaks Hindi and the model translates into English at the same time. Caption chip: "Simultaneous translation". |
| 1:18 | "Friend." again for friend 2. The instruction holds across speakers. |
| 1:19–1:47 | **Search while talking.** Reaction times by modality. Caption: "The model can search while talking and listening". |
| 1:48–2:13 | **Generative UI in parallel.** "Visualize them in a bar chart." While the chart renders, the presenter asks a separate "why is auditory faster" question and gets an answer. Cut to a full-screen chart ("Human Reaction Times by Modality"). Caption: "The model can produce visualization while keeping the conversation going". It ends on a close-up of the two people. |

**Visual style:**
- One continuous, locked-off wide shot in a hi-fi listening room (horn speakers, record shelf), with only one cutaway, to the chart.
- A small picture-in-picture of the laptop UI (blue gradient) sits bottom-right.
- Small white sans-serif captions on translucent grey chips, lower-left.
- **No narrator voice-over, no music bed noted, no metrics overlay. Benchmarks appear only on the blog** (two scatter plots plus the table). The video shows no numbers.
- Pacing is real-time conversational and the feel is unscripted. The demo carries the argument.

**Reception (secondary):**
- Sean Goedecke notes the video "is making a big deal about some features (like real-time simultaneous translation) that are just features of fully-duplex audio models", and observes "an uncomfortable half-second lag sometimes" (<https://www.seangoedecke.com/interaction-models/>) [`goedecke2026im`].
- Rift Dispatch: "a glimpse, not a product" (<https://riftdispatch.beehiiv.com/p/when-sci-fi-loses-the-fi>) [`rift2026scifi`].
- No Verge, TechCrunch or Wired piece was found.

**Lessons for Bud:**
1. Their format, a real room, several people, and capability captions, works without a narrator.
2. They show **no** numbers in the video. Bud can differentiate by showing *live* measured numbers on screen.
3. A visible lag was noticed by critics. Bud should show its lag honestly as a number rather than hide it.

## 2. Google: Gemini Live, the Live API, native audio and Project Astra

Vendor numbers are marked **(self-reported)**. Artificial Analysis (AA) numbers are independent. AA's TTFA is the mean time to first audio over the 1,000 Big Bench Audio items: <https://artificialanalysis.ai/speech-to-speech> [`aa_s2s_leaderboard`].

### 2.1 Project Astra (research prototype)
- **Dec 2024:** "up to 10 minutes of in-session memory". It understands language "at about the latency of human conversation", with no ms figure, and uses Search, Lens and Maps. Trusted testers only; prototype glasses. <https://blog.google/innovation-and-ai/models-and-research/google-deepmind/google-gemini-ai-update-december-2024/> [`google2024gemini2`]
- **Status now:** Astra's capabilities are flowing into Gemini Live, Search and the Live API. <https://deepmind.google/models/project-astra/> [`deepmind_astra`]
- **Architecture:** no paper or architecture disclosure.
- The 2024 "Multimodal Live API" introduced real-time audio and video input with "multiple, combined tools" (same Dec 2024 post).

### 2.2 Gemini 2.5 Flash Native Audio (Dec 2025)
Model id: `gemini-2.5-flash-native-audio-preview-12-2025`.
- **Self-reported:** ComplexFuncBench Audio 71.5%; instruction adherence 90% (from 84%).
- **Availability:** GA on Vertex AI, preview in the Gemini API. Live speech translation covers "over 70 languages and 2000 language pairs".
- Source: <https://blog.google/products-and-platforms/products/gemini/gemini-audio-model-updates/> [`google2025nativeaudio`]
- **AA (independent):** "Dialog Thinking" 91%, TTFA 3.87 s; "Dialog" 69%, TTFA 0.63 s.

### 2.3 Gemini 3.1 Flash Live (preview, 26 Mar 2026)
Model id: `gemini-3.1-flash-live-preview`.
- **Model card:** "based on Gemini 3 Pro". Inputs audio, images, video and text up to 128K; outputs audio and text up to 64K. <https://deepmind.google/models/model-cards/gemini-3-1-flash-live> [`deepmind_g31card`]
- **API docs** (<https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-live-preview>) [`google_g31doc`]:
  - 131,072 input and 65,536 output tokens.
  - `thinkingLevel` minimal, low, medium or high; the default is minimal.
  - **Async function calling not supported** (synchronous only); no proactive audio or affective dialogue.
  - Now labelled a "legacy preview model", with users told to move to 3.8 Live.
- **Self-reported** (Google eval PDF; AA ran Big Bench Audio and Scale ran Audio MultiChallenge), <https://storage.googleapis.com/deepmind-media/gemini/gemini-3-1-flash-live-model-evaluation.pdf> [`deepmind_g31eval`]:

  | Benchmark | Thinking High | Thinking Minimal |
  |---|---|---|
  | Audio MultiChallenge | 36.1% | 26.8% |
  | Big Bench Audio | 95.9% | 70.5% |
  | ComplexFuncBench Audio | 90.8% | not given |

- **Launch blog** (<https://blog.google/innovation-and-ai/models-and-research/gemini-models/gemini-3-1-flash-live/>) [`google2026g31`]:
  - Gemini Live can "follow the thread of your conversation for twice as long".
  - "lower latency", with no figure.
  - All output is SynthID-watermarked.
- **AA (independent):** High 97%, TTFA 2.99 s; Minimal 71%, TTFA 0.96 s. Priced at about $0.35/hr audio input and $1.38/hr audio output.
- **FD-bench v1 (FDB README, third party)**, minimal / high:
  - Pause Synthetic TOR 0.022 / 0.007
  - Backchannel TOR 0.727 / 0.491
  - Smooth-turn latency **0.567 s / 0.940 s**
  - Interruption latency 0.337 s / 0.651 s
  - Interruption GPT-4o 3.575 / 4.121
- **FD-bench v3** (FDB-v3 paper, "Gemini Live 3.1"): Pass@1 0.540, latency 4.25 s (fastest of those tested), take-turn 78.0%.
- **TML table:** v1 latency 0.57 s (min) / 0.94 s (high); v1.5 avg 54.3 / 45.5; Audio MultiChallenge 26.8 / 36.1.

### 2.4 Gemini 3.8 Live and 3.8 Live Extended Thinking (15 Sep 2026; the current default)
Blog: <https://blog.google/innovation-and-ai/models-and-research/gemini-models/gemini-3-8-live-gemini-3-8-live-extended-thinking/> [`google2026g38`]
- **Self-reported benchmarks:**
  - Extended Thinking is #1 on the AA Speech-to-Speech Quality Index at 82.6.
  - τ-Voice 68.6%; Sierra τ-Voice-banking 35.1%; Big Bench Audio 97.7%.
  - 3.8 Live is #2 in the Speech Agent Arena.
- **Languages:** 97, with switching mid-conversation. The API docs say 99 languages; keep both and attribute each to its source.
- **Background work: this matters for positioning.** 3.8 Live "executes tools and API calls in the background while continuing the conversation". Extended Thinking narrates progress on background tasks and acknowledges early with cues like "Let me check that…".
- **No latency figure** in the post.
- **API** (<https://ai.google.dev/gemini-api/docs/models/gemini-3.8-live>, <https://ai.google.dev/gemini-api/docs/live-api/capabilities>) [`google_g38doc`, `google_livecap`]:
  - Async `NON_BLOCKING` function calling is the default, with scheduling options SILENT, WHEN_IDLE and INTERRUPTED.
  - Built-in VAD. On interruption, generation "is canceled and discarded".
  - Audio is 16 kHz in and 24 kHz out, as PCM.
- **Model card** (<https://deepmind.google/models/model-cards/gemini-3-8-audio/>) [`deepmind_g38card`]: context up to 128K; Live Avatar supports "a few minutes of continuous interaction"; occasional timeouts.
- **AA (independent):** 3.8 Live 92%, TTFA 1.18 s; Extended Thinking (High) 98%, TTFA 1.35 s.

### 2.5 Live API session limits and pricing
- **Session limits** (<https://ai.google.dev/gemini-api/docs/live-session>) [`google_livesession`]:
  - Audio-only: 15 min; audio+video: 2 min, both without compression.
  - A connection lasts about 10 min.
  - Sliding-window `contextWindowCompression` allows "an unlimited amount of time".
  - Resumption tokens are valid for 2 h. The server sends GoAway with `timeLeft` before closing.
- **Pricing** (<https://ai.google.dev/gemini-api/docs/pricing>) [`google_pricing`]. One shared row covers 3.8 Live, 3.8 Extended Thinking and 3.1 Flash Live:
  - Audio input: $3.00 per 1M tokens (**$0.005/min**).
  - Audio output: $12.00 per 1M tokens (**$0.018/min**).
  - Text: $0.75 in / $4.50 out per 1M.
  - Gemini 2.5 native audio: audio $3 in / $12 out, text $0.50 / $2.00 per 1M.
- **Common to all:** closed weights, Google-hosted only, Google's own voices, STT and TTS. No provider choice.

## 3. OpenAI: GPT-4o voice, gpt-realtime, gpt-realtime-2 and GPT-Live

openai.com blocks direct fetches (HTTP 403). Pages were read through a reader proxy; cite the canonical URLs. Dates come from the API changelog, <https://developers.openai.com/api/docs/changelog> [`openai_changelog`].

### 3.1 GPT-4o voice (13 May 2024) [`openai2024gpt4o`]
- **Self-reported latency:** responds to audio "in as little as 232 milliseconds, with an average of 320 milliseconds".
- **Previous Voice Mode:** a three-model cascade averaging 2.8 s (GPT-3.5) and 5.4 s (GPT-4).
- **Architecture:** "a single new model end-to-end across text, vision, and audio".
- Source: <https://openai.com/index/hello-gpt-4o/>

### 3.2 gpt-realtime (GA 28 Aug 2025) [`openai2025gptrealtime`]
- **Architecture:** a single speech-to-speech model. It "reduces latency", with no number given.
- **Self-reported** (vs the Dec 2024 model):

  | Benchmark | gpt-realtime | Dec 2024 model |
  |---|---|---|
  | Big Bench Audio | 82.8% | 65.6% |
  | MultiChallenge audio | 30.5% | 20.6% |
  | ComplexFuncBench audio | 66.5% | 49.7% |

- **Price:** $32 per 1M audio input tokens ($0.40 cached) and $64 per 1M audio output tokens, 20% below gpt-4o-realtime-preview.
- **Features:** native async function calling, remote MCP, SIP.
- Source: <https://openai.com/index/introducing-gpt-realtime/>
- **AA (independent):** 83%, TTFA 0.98 s. gpt-4o-realtime (Dec 2024) has TTFA 1.49 s.
- **Third-party benchmarks:**
  - FD-bench v1, "GPT-Realtime" row (FDB README): smooth-turn latency 1.470 s, Pause Synthetic TOR 0.010, interruption latency 1.500 s.
  - Audio MultiChallenge paper: GPT Realtime APR 20.35% with audio output.
- **gpt-realtime-mini:** 6 Oct 2025.

### 3.3 gpt-realtime-1.5 (23 Feb 2026)
- **AA:** 81%, TTFA 0.81 s.
- **TML table:** FD-bench v1 latency 0.59 s; v1.5 avg 48.3; v3 RQ/Pass@1 77.9 / 55.0; Audio MultiChallenge 34.7.
- **FDB-v3 paper ("GPT-Realtime" = gpt-realtime-1.5 in the v3 harness):** Pass@1 0.600, latency 6.89 s, interrupt 13.5%.

### 3.4 gpt-realtime-2 (7 May 2026) [`openai2026realtime2`, `openai_rt2_model`]
Sources: <https://openai.com/index/advancing-voice-intelligence-with-new-models-in-the-api/>, <https://developers.openai.com/api/docs/models/gpt-realtime-2>
- **Model:** "GPT-5-class reasoning". Reasoning effort minimal, low, medium, high or xhigh; the default is low.
- **Behaviour:** preambles ("let me check that") and parallel tool calls.
- **Context:** 32K to 128K. Max output 32K.
- **Benchmarks:** OpenAI states only relative gains over 1.5 (self-reported): +15.2% on Big Bench Audio (high) and +13.8% on Audio MultiChallenge (xhigh).
- **TML table:**

  | Effort | FD v1 latency | v1.5 avg | Audio MultiChallenge | Big Bench Audio |
  |---|---|---|---|---|
  | minimal | 1.18 s | 46.8 | 37.6 | 71.8 |
  | xhigh | 1.63 s | 47.8 | 48.5 | 96.6 (AA, thinking high) |

- **AA (independent):** High 97% (TTFA 1.14 s); Minimal 72% (TTFA 1.12 s).
- **Price:**
  - Audio $32 in / $64 out per 1M tokens; AA converts this to about **$1.15/hr input and $4.61/hr output**.
  - Text $4 in / $24 out per 1M tokens.
  - Released alongside gpt-realtime-translate ($0.034/min) and gpt-realtime-whisper ($0.017/min).
- **Session cap:** 60 min maximum. <https://platform.openai.com/docs/guides/realtime-conversations> [`openai_rtconv`]
- **gpt-realtime-2.1 and 2.1-mini** (6 Jul 2026): p95 latency down "by at least 25%" (self-reported). AA: 2.1 High 96% (TTFA 1.21 s); Minimal 87% (TTFA 0.97 s). <https://community.openai.com/t/new-realtime-models-on-the-api-gpt-realtime-2-1-and-gpt-realtime-2-1-mini/1385896> [`openai_rt21`]

### 3.5 GPT-Live / GPT-Live-1 (ChatGPT Voice Jul 2026; API GA 10 Sep 2026). The design closest to WaaV and TML.
- **Architecture** (<https://openai.com/index/introducing-gpt-live/>) [`openai2026gptlive`]:
  - Full-duplex. It makes "interaction decisions many times per second": speak, listen, pause, interrupt, or call a tool.
  - Hands search and reasoning to a frontier model "behind the scenes", GPT-5.5 at launch.
  - "Strongly preferred" over Advanced Voice Mode in 5–10 minute conversations, with no percentages.
  - Limits: accent and fluency gaps in some languages; no video at launch.
- **Systems post** (<https://openai.com/index/continuous-voice-interaction-with-gpt-live/>) [`openai2026gptlivesys`]:
  - Turn detector removed from the audio path. WebRTC frontend written in Go.
  - An async RPC boundary means a slow tool "cannot stall the flow of media".
  - Background context compaction.
  - Connection startup cut "from six network round trips to just one". No absolute ms.
  - **Limits it names:** long-session memory pressure; overlapping speech is hard to attribute; distance to inference capacity adds latency.
- **API** (<https://openai.com/index/introducing-gpt-live-1-in-the-api/>, <https://developers.openai.com/api/docs/models/gpt-live-1>) [`openai2026gptliveapi`, `openai_gptlive1_model`]:
  - "**$0.05 per minute** for the front-end voice layer". The backend model and harness are billed separately.
  - Can delegate to "a backend text model … such as GPT-6 Astra or a third-party model".
  - Self-reported: "improves Full Duplex Bench performance by 30 percentage points over GPT-Realtime-2.1"; the FDB version is not stated.
  - Speak's evaluation found almost 80% fewer interruptions than turn-based systems.
  - No numeric turn-taking latency. Concurrency 50/300/500 by tier. No image or video input.
- **AA:** GPT-Live-1 (Astra, medium) 90%, TTFA 1.34 s.

### 3.6 Implication for positioning (our analysis)
The front/background split is no longer unique. Since TML's May 2026 preview, both OpenAI (GPT-Live, voice layer plus backend model) and Google (3.8 Live, background tools while talking) ship it as a **hosted, closed** product. WaaV must therefore differentiate on what they cannot offer:
1. **Open-source and self-hostable.** Audio and transcripts stay on-premises.
2. **Any vendor in every slot.** 44 STT/TTS vendors, and any LLM, orchestrator or agent (A2A, MCP, webhook) as the background.
3. **An explicit, inspectable per-turn decision.** Request, correction, stop, answer or wait, made by a 2B open model.
4. **Confirmation gates for side-effecting tools,** plus caller controls.
5. **Cost:** the front model runs locally on llama.cpp. Compare GPT-Live's $0.05/min for the voice layer alone.
6. **Measured, reproducible numbers** on FD-bench, rather than unspecified "+30 pp".

## 4. Full-duplex open models (and open cascades)

All numbers are self-reported by the authors unless marked otherwise. The FD-bench v1 rows are in §6.1.

### 4.1 Kyutai Moshi (Défossez et al., arXiv 2410.00037, Sep 2024) [`defossez2024moshi`]
- **Design:** calls itself "the first real-time full-duplex spoken large language model". It models the user's audio and its own audio as parallel streams, with no explicit turns. "Inner Monologue" predicts time-aligned text tokens before audio tokens. <https://arxiv.org/abs/2410.00037>
- **Backbone:** Helium, a 7B text LLM pretrained on 2.1T English tokens. An RQ-Transformer adds a 6-layer depth transformer. <https://arxiv.org/html/2410.00037v2>
- **Codec:** Mimi, 24 kHz audio at 12.5 Hz frames, 8 codebooks, about 1.1 kbps, 80 ms frames. <https://github.com/kyutai-labs/moshi>
- **Latency:** "a theoretical latency of 160ms, 200ms in practice" (abstract). The repo breaks this into 80 ms frame + 80 ms acoustic delay and says "as low as 200 ms" on an L4.
- **Licence:** code MIT/Apache; weights CC-BY 4.0.
- **Third-party scores:**
  - FDB v1: Moshi talks over pauses (Pause TOR 0.985 synthetic) and never backchannels (BC TOR 1.000), but has fast turn latency (0.265 s) and interruption latency (0.257 s).
  - FDB v1.5 `user_intr`: Respond 0.50, stop latency 1.16 s.
  - Big Bench Audio: 4.4% (NVIDIA VoiceChat card).

### 4.2 Kyutai Unmute (open cascade; the closest analogue to WaaV) [`kyutai_unmute`, `kyutai_stt`]
- **Pipeline:** streaming STT, then any text LLM, then streaming TTS, pitched as "Make any LLM listen and speak". Claim: "In total, the response latency is below a second". <https://kyutai.org/unmute/>
- **Repo** (MIT, <https://github.com/kyutai-labs/unmute>):
  - Kyutai TTS is 1.6B; the production LLM is GPT-OSS-120B via OpenRouter; any OpenAI-compatible server works.
  - TTS latency is about 750 ms with everything on one L40S and about 450 ms on separate GPUs.
  - Needs a GPU with 16 GB or more.
- **Semantic VAD** (Kyutai STT, <https://kyutai.org/stt>): the STT model predicts "the probability that the user is done talking". `stt-1b-en_fr` has a 500 ms delay. Throughput is 400 real-time streams per H100.
- **Gap vs WaaV:**
  - no separate front decision model
  - no background tasks or confirmations
  - Kyutai's own STT/TTS only, English/French

### 4.3 NVIDIA PersonaPlex (Roy et al., arXiv 2602.06053, Jan 2026) [`roy2026personaplex`]
- **Design:** fine-tuned from Moshi. A hybrid system prompt combines a voice prompt with a text role prompt.
- **Training data:** synthetic customer-service dialogues (1,840 h) and QA dialogues (410 h); the released checkpoint adds 1,217 h of Fisher. <https://arxiv.org/html/2602.06053v1>
- **FD-bench v1:**
  - paper model: smooth-turn latency 0.070 s, interruption TOR 1.000 with GPT-4o 4.210 and latency 0.400 s, Pause TOR 0.584 (synthetic)
  - released checkpoint (Table 6): turn latency 0.170 s, Pause TOR 0.358 / 0.431, interruption latency 0.240 s
- **Naturalness:** DMOS 3.90±0.15, vs 3.72 for Gemini (Live 2.0) and 3.11 for Moshi.
- **Service-Duplex-Bench** (the authors' own): Gemini 4.73 vs PersonaPlex 4.48.
- **Limits:**
  - tool integration is future work
  - English only
  - NVIDIA Open Model License <https://huggingface.co/nvidia/personaplex-7b-v1>
- **Caveat:** the "Gemini" row in PersonaPlex's comparison is the deprecated Gemini Live 2.0.

### 4.4 NVIDIA NemotronLabs VoiceChat (NGC model card, Jul 2026) [`nvidia2026voicechat`]
Source: <https://catalog.ngc.nvidia.com/orgs/nim/nvidia/models/nemotron-labs-voicechat>
- **Architecture:** an 11B hybrid Mamba/Transformer full-duplex network, built from:
  - Fast Conformer encoder (Nemotron-Speech-Streaming 0.6B)
  - Nemotron Nano v2 9B backbone
  - NVIDIA TTS decoder and codec
- **Language, serving and licence:** English only; served on vLLM; OpenMDW 1.1, "for research purposes only".
- **Tool calling:** NVIDIA calls it "the first open full-duplex model to support tool calling". It has a separate tool channel and an optional "on-hold" message.
- **Self-reported numbers:**

  | Benchmark | Result |
  |---|---|
  | Headline turn-taking latency | ~450 ms |
  | FDB v1 smooth turn TOR / latency | 0.82 / 448 ms |
  | FDB v1 interruption TOR / latency / GPT-4o | 1 / 480 ms / 4.33 |
  | FDB v1 Pause TOR, synthetic / Candor | 0.153 / 0.255 |
  | FDB v3 tool selection / argument accuracy / Pass@1 | 82.5% / 44.2% / 33% |
  | VoiceBench (text-output average) | 55.1 |
  | Big Bench Audio | 37.0% |
  | BFCL-v3 | 56.1% |

- **Stated limits** (each one a WaaV differentiator):
  - trained on audio contexts of 2 minutes or less
  - 5 tools or fewer, no reliable parallel calls
  - **"users cannot interrupt during tool execution"**
  - long tool responses delay speech
  - may not handle backchannels
  - not for noisy or reverberant rooms

### 4.5 Freeze-Omni (Wang et al., arXiv 2411.00774, Nov 2024) [`wang2024freezeomni`]
- **Architecture:** a frozen Qwen2-7B-Instruct, a speech encoder of about 350M, and a speech decoder of about 120M (TiCodec, 24 kHz).
- **Duplex control:** Silero VAD plus chunk-wise state prediction: keep listening, interrupted, or turn ended. This is architecturally close to a cascade with a learned turn head.
- **Latency:** statistical average 745 ms (P50 753, P90 1020). The authors estimate about 1.2 s in real use.
- **Spoken QA:** Web Q 44.73, LlaMA Q 72, Audio TriviaQA 53.88. <https://arxiv.org/html/2411.00774v5>
- **FDB v1.5 `user_intr`:** Respond 0.72, stop latency 1.42 s.

### 4.6 dGSLM (Nguyen et al., TACL 2023; arXiv 2203.16502) [`nguyen2023dgslm`]
- **Design:** a textless dual-tower Transformer with cross-attention, trained on about 2,000 h of two-channel Fisher audio. It generates both channels at once, including laughter and overlaps.
- **Significance:** the first generative full-duplex spoken dialogue LM. <https://arxiv.org/abs/2203.16502>
- **FDB v1:** high TOR everywhere (Pause 0.934); interruption GPT-4o 0.201.

### 4.7 Sesame CSM (27 Feb 2025) [`sesame2025csm`, `sesame_csm1b`]
Blog: "Crossing the uncanny valley of conversational voice", <https://www.sesame.com/research/crossing_the_uncanny_valley_of_voice>
- **Architecture:** two autoregressive transformers (Llama variants). A backbone predicts codebook 0 and a small decoder predicts the rest, over Mimi split-RVQ at 12.5 Hz.
- **Sizes and data:** 1B/3B/8B backbones; about 1M h of audio.
- **Evaluation:**
  - WER and speaker similarity are "saturated".
  - In a CMOS test on Expresso, listeners showed no preference without context, but preferred humans with 90 s of context. Figures only.
- **Self-stated limits:** English-focused; does not model turn-taking or pacing; not full duplex.
- **Release:** CSM-1B under Apache-2.0, 13 Mar 2025, "not a full conversational system". <https://huggingface.co/sesame/csm-1b>

### 4.8 Qwen Omni (brief) [`xu2025qwen3omni`, `qwen2026qwen35omni`]
- **Qwen3-Omni-30B-A3B** (arXiv 2509.17765): Thinker-Talker MoE. Theoretical first-packet latency 234 ms. Speech in 19 languages and out in 10. Apache-2.0. <https://arxiv.org/abs/2509.17765>
- **Qwen3.5-Omni** (arXiv 2604.15804, Apr 2026): "hundreds of billions of parameters", Hybrid-Attention MoE. <https://arxiv.org/abs/2604.15804>
- **TML's table:** "Qwen 3.5 Omni-plus-realtime" FDB v1 latency 2.14 s, v1.5 average 39.0 (TML-reported).

### 4.9 Summary of the gap (our analysis)
- None of the open end-to-end duplex models runs asynchronous background work while talking.
- Only VoiceChat calls tools, and callers cannot interrupt it during tool execution.
- All are single-model, single-voice stacks, mostly English-only, and need a 7–11B model on a large GPU.
- Unmute shows that an open cascade can stay under a second, but it has no task layer.

## 5. Cascade and agent frameworks

All vendor latency and accuracy figures are **self-reported** unless marked otherwise.

### 5.1 Pipecat (Daily) and Smart Turn [`pipecat_repo`, `pipecat_fc`, `smartturn_repo`, `daily2025smartturnv3`, `daily2025smartturnv31`]
- **Pipecat:** an open-source Python framework (BSD-2-Clause) that is self-hostable and works with any STT, LLM or TTS provider. <https://github.com/pipecat-ai/pipecat>
- **Async tools:**
  - `cancel_on_interruption=False` makes a function async: "the conversation is not held while it runs". A late result "is injected back into the context as a developer message, triggering a new LLM inference".
  - Progress updates (`is_final=False`), LLM-cancellable tools, `timeout_secs`, parallel calls.
  - **No caller-confirmation pattern is documented.** <https://docs.pipecat.ai/pipecat/learn/function-calling.md>
- **Turn-taking:** Silero VAD silence triggers **Smart Turn**, an audio end-of-turn classifier.
  - Model: Whisper-Tiny encoder plus a linear head, about 8M parameters; int8 CPU build about 8 MB, fp32 GPU build about 32 MB. 23 languages, BSD-2. Input is 16 kHz, up to 8 s. <https://github.com/pipecat-ai/smart-turn>, <https://huggingface.co/pipecat-ai/smart-turn-v3>
  - Smart Turn v3.2 is a Jan 2026 commit in that repo; no separate announcement post was found.
- **Smart Turn v3** (11 Sep 2025), <https://www.daily.co/blog/announcing-smart-turn-v3-with-cpu-inference-in-just-12ms/>:
  - "Nearly 50x smaller" than v2.
  - 12.6 ms on AWS c7a.2xlarge CPU; 3.3 ms on L40S.
  - Accuracy: English 94.31%; best is Turkish 97.10%, worst Vietnamese 81.27%.
- **Smart Turn v3.1** (3 Dec 2025), <https://www.daily.co/blog/improved-accuracy-in-smart-turn-v3-1/>:
  - English accuracy 88.3% (v3.0) → 94.7% (8 MB) and 95.6% (32 MB) on the new test set.
  - Latency 9 ms (8 MB) on c7a.2xlarge; 2 ms on L40S; preprocessing is extra (1–9 ms).
- **Third-party v3.2 ONNX re-export** (not Pipecat's numbers): 92.90% accuracy, 8.48% FP, CPU mean 36.3 ms. <https://huggingface.co/soniqo/Smart-Turn-v3.2-ONNX>

### 5.2 LiveKit Agents [`livekit_turndetector`, `livekit2025eot`, `livekit2026eot`, `livekit2026adaptive`, `livekit2026async`, `livekit_pricing`]
- **Framework:** Apache-2.0, self-hostable, any provider. Some top models are cloud-only, or metered when self-hosted.
- **Text turn detector `MultilingualModel`:**
  - Fine-tuned from Qwen2.5-0.5B-Instruct; 396 MB; about 50–160 ms per turn on CPU; 14 languages; LiveKit Model License.
  - Default endpointing min 0.5 s, max 3.0 s.
  - Now marked deprecated (removed in SDK 2.0). <https://docs.livekit.io/agents/logic/turns/turn-detector>
- **v0.4.1-intl** (Dec 2025): "39.23% relative reduction in false-positive interruptions". The false-positive rate falls from 18.66% to 11.34% at a fixed 99.3% true-positive rate. Distilled from a Qwen2.5-7B teacher. <https://livekit.com/blog/improved-end-of-turn-model-cuts-voice-ai-interruptions-39>
- **Audio turn detector v1 and v1-mini** (Jun 2026), <https://livekit.com/blog/solving-end-of-turn-detection>:
  - v1 false cutoffs: 9.9% at a 300 ms budget, 4.5% at 600 ms.
  - Vendor comparison: Deepgram Flux 12.9% / 9.9%.
  - v1 runs on LiveKit Inference only; v1-mini is open-weight.
  - The eot-bench benchmark is open.
- **Adaptive interruption handling** (Mar 2026), <https://livekit.com/blog/adaptive-interruption-handling>:
  - Tells a real barge-in from a backchannel: "86% precision and 100% recall (at 500 ms overlap speech)".
  - Rejects 51% of VAD barge-ins; inference ≤30 ms.
  - Resumes playback after a false interruption.
  - **Cloud-only model.**
- **Async tools** (livekit-agents 1.6.0, 11 Jun 2026; blog 7 Jul 2026), <https://livekit.com/blog/async-tools-voice-agents>. **The closest competitor feature to WaaV's background tasks.**
  - The agent "acknowledges immediately, narrates progress while the work runs".
  - `ctx.update()`, `ctx.with_filler()`, `ToolFlag.CANCELLABLE` (`cancel_task`), `ctx.disallow_interruptions()`.
  - `on_duplicate="confirm"` re-asks the LLM for explicit confirmation of a *duplicate* call. That is a duplicate guard, not a general caller confirmation for side effects.
  - Demo with a mock backend: about 14 s of silence when blocking versus "under a second" acknowledgement when async.
- **Pricing** (<https://livekit.com/pricing>):
  - Agent session $0.01/min after the included minutes.
  - Self-hosted metering: audio end-of-turn $0.50 per 1,000 requests; adaptive interruption $1.20 per 1,000 requests.

### 5.3 Vapi (hosted, closed) [`vapi_pricing`, `vapi_home`, `vapi_speech`, `vapi_tools`]
- **Pricing:** hosting $0.05/min plus pass-through providers. Vapi's own estimate is $82–129 per 1,000 min. <https://vapi.ai/pricing>
- **Latency:** "<500ms" (self-reported, <https://vapi.ai/>). A competitor (Retell) claims Vapi runs "1–2 s base".
- **Turn-taking** (<https://docs.vapi.ai/customization/speech-configuration>):
  - Smart-endpointing choices: the LiveKit model, Vapi's own, Deepgram Flux, Assembly, or a custom server.
  - `stopSpeakingPlan` sets numWords, voiceSeconds (0.2 s) and backoff (1 s).
- **Tools:** spoken messages for request-start, delayed, complete and failed, plus an "Async" toggle whose runtime behaviour is not documented. No confirmation pattern. <https://docs.vapi.ai/tools/custom-tools>
- **Provider choice:** BYO custom LLM endpoint. Not self-hostable.

### 5.4 Retell AI (hosted, closed) [`retell_pricing`, `retell2026turn`, `retell_interrupt`]
- **Pricing:** "$0.07-$0.31 / min". GPT Realtime 2 via Retell costs $0.38/min. <https://www.retellai.com/pricing>
- **Turn-taking:** a "proprietary turn-taking system at roughly 600ms latency" (self-reported, 19 Oct 2026). <https://www.retellai.com/blog/turn-taking-voice-ai-hidden-problem>
- **Controls** (<https://www.retellai.com/learn/how-ai-voice-agents-handle-interruptions-and-natural-turn-taking>):
  - interruption sensitivity 0–1
  - backchannel off by default
  - endpointing 100–10,000 ms
- No documented background tools or confirmations. Not self-hostable.

### 5.5 Deepgram Voice Agent API [`deepgram_pricing`, `deepgram_flux`, `deepgram_fc`]
- **Pricing:** Standard $0.075/min; BYO LLM+TTS $0.050/min. <https://deepgram.com/pricing>
- **Flux STT** has turn detection built into the STT: "~260ms end-of-turn detection" (self-reported), with EagerEndOfTurn events. <https://audioxpress.com/news/deepgram-launches-advanced-conversational-speech-recognition-model>
- **Function calls:** client-side or server-side. No documented "keep talking while a function runs" and no confirmation pattern. <https://developers.deepgram.com/docs/voice-agent-function-call-request>
- **Self-hosting:** a self-hosted option is linked from the docs.

### 5.6 ElevenLabs Agents (Conversational AI) and Eleven Realtime [`elevenlabs_pricing`, `elevenlabs_flow`, `elevenlabs_toolint`, `elevenlabs_realtime`]
- **Pricing:** $0.08/min plus LLM. <https://elevenlabs.io/pricing/api>
- **Turn-taking** (<https://elevenlabs.io/docs/eleven-agents/customization/conversation-flow>):
  - turn eagerness: Eager, Normal or Patient
  - soft-timeout filler (e.g. "Hhmmmm...yeah.")
  - interruptions on or off
- **Tool interruptions:** per-tool `interruption_mode` (allow, `disable_during_tool`, ...). No documented async or background tool mode and no confirmation feature.
- **Eleven Realtime** (Oct 2026), an end-to-end-tuned speech-to-speech model, OpenAI-Realtime-compatible. Self-reported, with AA said to have measured them:

  | Metric | v1 Mini | v1 Max |
  |---|---|---|
  | Time to first audio | 620 ms | 570 ms |
  | "Full-Duplex-Bench" (version and metric unspecified) | 95.8% | not given |
  | Big Bench Audio | not given | 98.8% |

  Source: <https://elevenlabs.io/realtime>

### 5.7 What the frameworks lack, relative to WaaV interaction mode (our analysis)
1. **No separate fast front model that classifies each caller turn** (request, correction, stop, answer, wait). Everywhere else the main LLM decides both the reply and the conversational act, and VAD or EOT models decide only *when*.
2. **Background work while talking now exists in LiveKit (Jun 2026) and Pipecat** as async tool calls. It is not documented for Retell, Deepgram or ElevenLabs, and only as a toggle in Vapi. None treats an *agent* (an A2A peer or an orchestrator LLM like gpt-oss-20b) as a first-class background task with progress, cancellation and correction routing.
3. **Confirmations for side-effecting tools** are left to prompting everywhere. LiveKit's `confirm` covers only duplicate calls.
4. **Self-hosting:** only Pipecat and LiveKit are fully self-hostable, and LiveKit's best turn and interruption models are cloud-only or metered. Vapi, Retell and ElevenLabs are hosted only.
5. **Echo gating/AEC and backchannel discrimination** come as vendor features, for example LiveKit's adaptive interruption, which is cloud-only.
6. **Do NOT claim the broadest vendor coverage.** Pipecat's supported-services page lists **38 STT, 59 TTS, 32 LLM and 11 speech-to-speech services**, including OpenAI GPT-Live and Gemini Live (<https://docs.pipecat.ai/server/services/supported-services>, counted 2026-10-27) [`pipecat_services`]. WaaV's 44 speech vendors are a strength, but not a superlative. Lead instead on "a gateway (not a framework you code against) + front/background split + confirmations + self-hosted front model".

## 6. Benchmarks for interactive voice

### 6.1 Full-Duplex-Bench v1 (Lin et al., arXiv 2503.04721, Mar 2025) [`lin2025fdbv1`]
Paper: <https://arxiv.org/abs/2503.04721> (HTML: <https://arxiv.org/html/2503.04721>). Code and leaderboard: <https://github.com/DanielLin94144/Full-Duplex-Bench> (`v1_v1.5/README.md`).

FDB v1 is the first open benchmark for **turn-taking behaviour** of full-duplex spoken dialogue models. The model receives a time-synchronous user stream (`input.wav`) and must produce a time-synchronous `output.wav`. The output is transcribed with time alignment (since June 2025 by `nvidia/parakeet-tdt-0.6b-v2`) and scored automatically.

It covers four dimensions: **Pause Handling**, **Backchanneling**, **Smooth Turn-Taking** and **User Interruption**.

**Metric definitions** (from the paper HTML):
- **Takeover (TO):** 0 if the model stays silent or only backchannels, 1 otherwise. A segment counts as a backchannel if it lasts under 1 s and has fewer than 2 words.
- **TOR (Takeover Rate):** the mean of TO over turns. Lower is better for Pause Handling and Backchannel, where the model should not grab the floor. Higher is better for Smooth Turn-Taking and User Interruption.
- **Backchannel Freq:** backchannel events per second, computed only when TOR = 0.
- **Backchannel JSD:** Jensen-Shannon divergence between the model's and humans' backchannel-timing distributions in 200 ms windows. Range 0 to 1; a silent model gets a uniform distribution.
- **Smooth turn-taking latency:** "the time (in seconds) between the end of the user's speech and the start of the model's response", computed only when TO = 1.
- **Interruption metrics:** a GPT-4o relevance score (0 to 5) and latency after the interruption.

**Data:**

| Source | Task | Samples |
|---|---|---|
| Candor | Pause Handling | 216 |
| Candor | Smooth Turn-Taking | 119 |
| ICC | Backchannel | 55 |
| Synthetic (GPT-4o + ChatTTS) | User Interruption | 200 |
| Synthetic (GPT-4o + ChatTTS) | Pause Handling | 137 |

**Leaderboard (repo README, verbatim):**

| Model | Pause Syn TOR↓ | Pause Candor TOR↓ | BC TOR↓ | BC Freq↑ | BC JSD↓ | Turn Candor TOR↑ | Turn Latency (s)↓ | Intr TOR↑ | Intr GPT-4o↑ | Intr Latency (s)↓ |
|---|---|---|---|---|---|---|---|---|---|---|
| PersonaPlex | 0.584 | 0.662 | 0.327 | 0.025 | 0.649 | 0.992 | 0.070 | 1.000 | 4.210 | 0.400 |
| dGSLM | 0.934 | 0.935 | 0.691 | 0.015 | 0.934 | 0.975 | 0.352 | 0.917 | 0.201 | 2.531 |
| Moshi | 0.985 | 0.980 | 1.000 | 0.001 | 0.957 | 0.941 | 0.265 | 1.000 | 0.765 | 0.257 |
| Freeze-Omni | 0.642 | 0.481 | 0.636 | 0.001 | 0.997 | 0.336 | 0.953 | 0.867 | 3.615 | 1.409 |
| Gemini Live 2.0 (deprecated) | 0.255 | 0.310 | 0.091 | 0.012 | 0.896 | 0.655 | 1.301 | 0.891 | 3.376 | 1.183 |
| GPT-Realtime | 0.010 | 0.120 | 0.000 | 0.007 | 0.980 | 1.000 | 1.470 | 0.970 | 3.850 | 1.500 |
| Gemini 3.1 Flash Live (minimal) | 0.022 | 0.153 | 0.727 | 0.044 | 0.807 | 1.000 | 0.567 | 1.000 | 3.575 | 0.337 |
| Gemini 3.1 Flash Live (high) | 0.007 | 0.102 | 0.491 | 0.030 | 0.853 | 0.992 | 0.940 | 0.995 | 4.121 | 0.651 |

How to read the table:
- Gemini 3.1 Flash Live was run through the official API `gemini-3.1-flash-live-preview` with `--thinking-level {minimal|high}` (README, May 2026).
- The README does not pin which GPT-Realtime snapshot was used.
- Note how the very low turn latencies of end-to-end duplex models (PersonaPlex 0.070 s, Moshi 0.265 s) come with very high Pause-Handling TOR. They talk over the user's pauses. The commercial cascade-like APIs show the opposite trade-off: GPT-Realtime has Pause TOR 0.010 but latency 1.470 s.
- This is the trade-off WaaV's VAD + Smart Turn + text turn detector stack is designed to sit in the middle of.

NVIDIA reports for its own **NemotronLabs VoiceChat** (self-reported, <https://catalog.ngc.nvidia.com/orgs/nim/nvidia/models/nemotron-labs-voicechat>):
- pause TOR 0.153 (synthetic) / 0.255 (Candor)
- smooth-turn TOR 0.82, latency 448 ms
- interruption TOR 1, latency 480 ms, GPT-4o 4.33

The **PersonaPlex** released checkpoint (arXiv 2602.06053, Table 6) reports:
- pause TOR 0.358 / 0.431
- turn TOR 0.908, latency 0.170 s
- interruption TOR 0.950, GPT-4o 4.290, latency 0.240 s

### 6.2 Full-Duplex-Bench v1.5 (Lin et al., arXiv 2507.23159, Jul 2025; v4 Apr 2026) [`lin2025fdbv15`]
Paper: <https://arxiv.org/html/2507.23159>. Data and evaluation code are in the same repository.

v1.5 adds **overlap handling** through four simulated scenarios, each with an expected behaviour:
- **user interruption** (`user_intr`): the model should yield and respond
- **user backchannel** (`user_backch`, e.g. "uh-huh"): the model should keep talking
- **talking to others** (`talking_other`): the model should resume
- **background speech** (`bkg_speech`, far-field third party): the model should resume

**Behaviour categories.** GPT-4o classifies the ASR transcript of each post-overlap segment as one of:
- **Respond:** addresses the overlapping speech
- **Resume:** ignores the overlap and continues
- **Uncertain:** asks for clarification or repetition
- **Unknown:** irrelevant output or silence

**Other metrics:**
- **Stop latency** = t_model_stop − t_user_start
- **Response latency** = t_model_start − t_user_end
- speech activity is detected with Silero VAD
- **prosodic adaptation:** WPM, pitch and intensity, pre vs post overlap, paired t-tests
- **speech quality:** UTMOSv2

**How an "average" is formed:** **the v1.5 paper defines no aggregate or average score.** It reports per-scenario distributions and describes trade-offs. TML's "FD-bench v1.5 Average" (TML 77.8, GPT-realtime-1.5 48.3, Gemini 3.1 FL minimal 54.3) is a TML-defined composite, and TML does not publish the formula. A plausible but **unverified** reading is the mean, over the four scenarios, of the rate of the *desired* behaviour: Respond for `user_intr`, Resume for the other three.

If we report a WaaV v1.5 average, we must state the formula and also give the per-scenario numbers.

**Paper results (Table 2, verbatim excerpt)**, scenario by scenario. Models: Freeze-Omni, Moshi, Gemini = gemini-2.0-flash-live-001, Nova Sonic, GPT-4o = gpt-4o-realtime-preview-2024-12-17.

| Scenario | Metric | Freeze-Omni | Moshi | Gemini 2.0 Live | Nova Sonic | GPT-4o RT |
|---|---|---|---|---|---|---|
| user_intr | Respond↑ | 0.72 | 0.50 | 0.33 | 0.24 | 0.78 |
| user_intr | Stop latency (s)↓ | 1.42 | 1.16 | 2.20 | 2.25 | 0.23 |
| user_intr | Resp latency (s)↓ | 1.35 | 1.47 | 2.62 | 2.75 | 1.50 |
| user_backch | Resume↑ | 0.80 | 0.06 | 0.93 | 0.98 | 0.70 |
| talking_other | Resume↑ | 0.25 | 0.19 | 0.99 | 0.90 | 0.02 |
| bkg_speech | Resume↑ | 0.25 | 0.07 | 0.30 | 0.98 | 0.04 |

Key observation:
- GPT-4o Realtime yields fastest (stop latency 0.23 s), but it answers side conversations and background speech: Respond 0.91 and 0.93 respectively.
- Nova Sonic ignores both correctly (Resume 0.90 and 0.98) but is slow to yield (2.25 s).
- No system does both well. This is an argument for an explicit **addressee/relevance decision** in a front model, which is what WaaV's front model decides per turn (request, correction, stop, answer, wait).

### 6.3 Full-Duplex-Bench v2 (Lin et al., arXiv 2510.07838, Oct 2025; v2 Apr 2026) [`lin2026fdbv2`]
- **What it is:** a live, multi-turn, streaming evaluation. A WebRTC/WebSocket Node.js orchestrator connects the system under test (the **Examinee**) to an automated AI **Examiner**, which drives staged goals at two pacing speeds, `slow` (patient) and `fast` (aggressive).
- **Task families:** daily, **correction**, **entity tracking** and **safety**.
- **Scoring:** NeMo ASR plus a Gemini 2.5 Flash LLM-as-judge rate turn-taking fluency, multi-turn instruction following and task-specific competence.
- **Findings:** current systems struggle under overlap, handle corrections poorly, and lose track of entities. The abstract gives no headline numbers.
- **Sources:** <https://arxiv.org/abs/2510.07838>; repo `v2/README.md`.
- **Relevance to WaaV:** the correction family maps directly onto the front model's `correction` turn class. FDB-v2 is probably the best public harness for a live WaaV demo-to-number pipeline.

### 6.4 Full-Duplex-Bench v3 (Lin, Chen, Chen, Lee; arXiv 2604.04847, Apr 2026) [`lin2026fdbv3`]
- **What it is:** a benchmark of **tool use under real disfluency**.
- **Data:** 100 real human recordings, 12 speakers (native and non-native: Korean and Russian backgrounds), mostly on built-in microphones; 79 unique scenarios.
  - five disfluency types: fillers, pauses, hesitations, false starts, self-corrections (21 of 100 are self-correction scenarios)
  - four domains: travel/identity, finance/billing, housing/location, e-commerce
  - 12 mock APIs
  - difficulty from easy (1 call) to hard (3 calls)
- **Harness:** LiveKit, with an optional API-latency injector.

**Metrics:**
- Tool-selection F1
- argument accuracy (GPT-4o judge)
- **Pass@1:** exactly the expected tools, all arguments correct
- **Response Quality:** GPT-4o judge
- **turn-take rate**
- **latency:** first response, tool call and task completion, measured from end of user speech
- **interrupt rate:** share of turns where the agent speaks before the user finishes
- **filler rate**

**Main results (paper HTML, <https://arxiv.org/html/2604.04847>):**

| System | Tool F1 | Arg Acc | Resp Qual | Pass@1 | Take-turn | Latency | Interrupt | Filler |
|---|---|---|---|---|---|---|---|---|
| GPT-Realtime (gpt-realtime-1.5 per repo) | 0.876 | 0.680 | 0.792 | 0.600 | 96.0% | 6.89 s | 13.5% | 16.9% |
| Gemini Live 2.5 | 0.786 | 0.593 | 0.554 | 0.490 | 92.0% | 7.26 s | 14.1% | 8.9% |
| Gemini Live 3.1 | 0.817 | 0.588 | 0.718 | 0.540 | 78.0% | 4.25 s | 19.2% | 31.7% |
| Grok | 0.797 | 0.542 | 0.617 | 0.430 | 94.0% | 6.65 s | 25.5% | 44.3% |
| Ultravox v0.7 | 0.794 | 0.513 | 0.510 | 0.410 | 96.0% | 8.40 s | 47.9% | 88.0% |
| Cascade (Whisper→GPT-4o→TTS) | 0.803 | 0.562 | 0.600 | 0.450 | 100.0% | 10.12 s | 33.0% | 26.9% |

Takeaways:
- Self-correction handling and hard multi-step reasoning are the most consistent failure modes across systems.
- The naive cascade baseline has perfect turn-taking and the worst latency (10.12 s). That gap is what an immediate front-model acknowledgement plus background task execution is meant to close.
- The cascade baseline here has no front/background split.

### 6.5 FLEXI (Ge et al., arXiv 2509.22243, Sep 2025) [`ge2025flexi`]
- **What it is:** a benchmark for full-duplex human-LLM speech interaction across six scenarios. A secondary summary lists them as standard turn-taking, pause handling, user interrupt, **model interrupt**, user backchannel and model backchannel.
- **Novelty:** it is the first to test **model-initiated interruption in emergency scenarios**.
- **Metrics and data:** latency, quality and conversational effectiveness. Data are LLM-generated, TTS-synthesised and manually validated.
- **Findings:** large gaps between open and commercial systems in emergency awareness, turn termination and latency. The authors argue for "next token-pair prediction".
- **Sources:** <https://arxiv.org/abs/2509.22243>; code at <https://github.com/ChristineCHEN274/FLEXI>.

### 6.6 Talk Arena / "Mind the Gap" (Li, Held, Ryan, et al., ACL 2025; arXiv 2502.15919) [`li2025mindthegap`, `talkarena`]
- **What it is:** a crowd-sourced, blind, **interactive** comparison of large audio models (<https://talkarena.org/>). Users speak freely and vote for the preferred response, which is audio-in, text-out in the original setting.
- **Models in the original setting:** GPT-4o, Gemini, Qwen2-Audio, DiVA-Llama 3 and Typhoon-Audio.
- **Scale:** the paper collected 7,500 interactions from 484 participants.
- **Finding:** no static benchmark correlates strongly with interactive preference (Kendall τ ≤ 0.33 for all). Only 2 of 20 datasets correlated significantly positively.
- **Use for our paper:** this motivates live demos and interactive evaluation over static scores.
- **Sources:** <https://arxiv.org/abs/2502.15919>; ACL Anthology 2025.acl-long.428.

### 6.7 VoiceBench (Chen et al., arXiv 2410.17196, Oct 2024) [`chen2024voicebench`]
- **What it is:** a multi-faceted benchmark for LLM-based voice assistants with real and synthetic spoken instructions. It varies speaker, environment (noise) and content.
- **Subsets** (repo <https://github.com/MatthewCYM/VoiceBench>):

  | Subset | Samples |
  |---|---|
  | alpacaeval | 199 |
  | commoneval | 200 |
  | wildvoice | 1,000 |
  | sd-qa | 553 |
  | mmsu | 3,074 |
  | openbookqa | 455 |
  | bbh | 1,000 |
  | ifeval | 345 |
  | advbench | 520 |

- **Scoring:** GPT-4o-mini judges the open-ended subsets.
- **Leaderboard:** <https://matthewcym.github.io/VoiceBench/>
- **Link to TML:** TML's "IFEval (VoiceBench)" row is this ifeval subset.

### 6.8 Big Bench Audio (Artificial Analysis) [`aa_bigbenchaudio`]
- **What it is:** 1,000 spoken reasoning questions, 250 from each of four BIG-Bench Hard categories: Formal Fallacies, Navigate, Object Counting, Web of Lies.
- **Audio:** TTS-rendered from 23 voice configurations (OpenAI, Azure, AWS Polly), English only.
- **Use:** run speech-to-speech or speech-to-text; the leaderboard is at <https://artificialanalysis.ai/speech-to-speech>.
- **Source:** <https://huggingface.co/datasets/ArtificialAnalysis/big_bench_audio>
- **Scores cited in this document:** TML 75.7 (96.5 with the background agent); Gemini 3.1 FL high 96.6 (TML table); NemotronLabs VoiceChat 37.0%, Moshi 4.4% and PersonaPlex 19.1% (NVIDIA model card).

### 6.9 Audio MultiChallenge (Gosai et al., Scale AI, arXiv 2512.14865, Dec 2025; ACL 2026) [`gosai2025audiomc`]
- **What it is:** multi-turn evaluation of spoken dialogue systems on **natural human speech**.
- **Data:** 452 conversations, 47 speakers, 1,712 instance-specific rubrics, about 15 h of user audio.
- **Axes:** Inference Memory, Instruction Retention, Self Coherence, and a new **Voice Editing** axis covering mid-utterance repairs and backtracking. Audio-cue tasks require recalling ambient sounds and paralinguistics.

**Metrics:**
- **APR (Average Pass Rate):** a task passes only if *every* binary rubric passes.
- **ARS (Average Rubric Score):** partial credit.

**Results in the paper:**
- best: Gemini 3 Pro Preview (Thinking), 54.65% APR
- GPT Realtime: 20.35% APR with audio output
- GPT-4o Audio Preview: 23.23% APR with audio output
- Self Coherence degrades with longer audio context

**Sources:** <https://labs.scale.com/papers/audiomc>, <https://arxiv.org/html/2512.14865>; leaderboard <https://scale.com/leaderboard/audiomc>.

**Relevance to WaaV:** Voice Editing is the "correction" turn class in benchmark form.

### 6.10 Sesame TurnBench (Eskimez et al., 28 Aug 2026) [`sesame2026turnbench`]
- **What it is:** an open benchmark and leaderboard for turn-taking **detectors**, with two tracks:
  - **End-of-Turn (EOT):** false positives are mid-turn pauses.
  - **Interruption (INT):** false positives are backchannels, noise or channel bleed.
- **Labels:** grounded in conversation analysis, at transition-relevance places.
- **Metrics:** recall and FPR, plus signed latency Δt at P10, P50 and P90. Systems are ranked by recall under a maximum FPR of 0.15.
- **Data:** about 30 h of dual-channel dyadic English (154 dialogues, 106 voice actors), triple-annotated with Fleiss' κ = 0.78. A 104 h training set is also released.
- **Systems evaluated (14):** including Smart Turn v3, Kyutai Semantic VAD, OpenAI Realtime, Gemini 3.1 Live and Moshi.
- **Results (test set):**

  | System | Track | Recall | FPR | Note |
  |---|---|---|---|---|
  | Voice Activity Projection (best on both tracks) | EOT | 0.845 | 0.055 | |
  | Voice Activity Projection | INT | 0.945 | 0.107 | INT latency 994 ms |
  | Gemini 3.1 Live | EOT | 0.657 | 0.022 (lowest listed) | |

- **Conclusion:** "No system is fast, selective, and high-recall at the same time."
- **Source:** <https://www.sesame.com/blog/turnbench>
- **Use for WaaV:** this is the natural benchmark for WaaV's VAD + Smart Turn v3.2 + text-detector stack and its echo gating. Run both tracks.

### 6.11 Other leaderboards used by vendors
- **Artificial Analysis Speech-to-Speech:** Big Bench Audio accuracy plus time to first audio (TTFA), and a "Quality Index". <https://artificialanalysis.ai/speech-to-speech>
- **Scale Audio MultiChallenge:** <https://scale.com/leaderboard/audiomc>
- **Speech Agent Arena:** mentioned by Google and ElevenLabs. Its source has not been verified here.
- **τ-Voice:** cited by Google. Its source has not been verified here.

## 7. Launch-video references and the recommended Bud WaaV video

### 7.1 OpenAI "Spring Update" / GPT-4o (13 May 2024) [`openai_springupdate_2024`, `openai_translation_2024`, `openai2024gpt4o`]
- **Videos:**
  - Livestream: <https://www.youtube.com/watch?v=DQacCB9tDaw>, 26:13 (1,573 s).
  - Clip: "Live demo of GPT-4o realtime translation", <https://www.youtube.com/watch?v=c2DFg53Zhvw>, 86 s.
- **Structure** (YouTube chapters):

  | Time | Chapter |
  |---|---|
  | 0:00 | Intro |
  | 1:23 | Desktop app |
  | 2:37 | GPT-4o features and access |
  | 5:31 | Store/API |
  | 8:04 | Safety and live demos |
  | 9:17 | Voice and emotion demos |
  | 13:39 | Vision/math tutoring |
  | 17:15 | Coding/screen share |
  | 21:55 | Audience requests and wrap-up |

- **The problem is stated explicitly on stage.** The old voice mode was "three models… transcription, intelligence and then text to speech… this also brings a lot of latency… breaks that immersion".
- **The payoff is named live.** "You can now interrupt the model… you don't have this awkward 2 to 3 second lag… the model picks up on emotion", following the nervous-breathing demo.
- **What made it compelling:**
  - live interruptions
  - audience requests taken from X (an unscripted feel)
  - a wired phone "so we have consistent internet", which reads as honest engineering
  - emotional range in the voice
- **Metrics stay on the blog, not on screen:** 232 ms minimum / 320 ms average, vs 2.8 s and 5.4 s for the old Voice Mode (SR, <https://openai.com/index/hello-gpt-4o/>).
- **Look:** a warm living-room set. The phone UI (black orb) is mirrored onto a wall monitor, with a small "Spring Update / ● Live" bug and no captions.

### 7.2 Google Project Astra (I/O, 14 May 2024) [`google_astra_video_2024`]
- **Video:** <https://www.youtube.com/watch?v=nXVvvRhiGjI>, 2:17 (137 s).
- **Opening card**, white on black: "This is a two-part demo of Project Astra… Each part was captured in a single take, in real time". One take on a Pixel phone and one on prototype glasses.
- **Structure** (chapters):

  | Time | Beat |
  |---|---|
  | 0:00 | Speaker recognition |
  | 0:31 | Creative alliteration |
  | 0:45 | Code analysis |
  | 1:04 | Location ("King's Cross") |
  | 1:21 | **Object memory** ("where did I leave my glasses?": "near a red apple"), the wow beat |
  | 1:41 | Whiteboard system design ("add a cache") |
  | 1:53 | Creative generation |

  It ends on the Gemini sparkle.
- **Look:** first-person handheld POV, no narrator, small centred subtitle chips with "User:" prefixes.
- **Why "single take" mattered:** Google's Dec 2023 "Hands-on with Gemini" video (<https://www.youtube.com/watch?v=UIZAiXYceBI>) was criticised as staged. Its own description admits "latency has been reduced and Gemini outputs have been shortened for brevity". Google confirmed it was assembled from stills and text prompts (TechCrunch <https://techcrunch.com/2023/12/07/googles-best-gemini-demo-was-faked/>; Engadget <https://www.engadget.com/google-admits-that-a-gemini-ai-demo-video-was-staged-055718855.html>) [`google_gemini_handson_2023`, `techcrunch_gemini_faked_2023`, `engadget_gemini_staged_2023`].
- **Lesson:** never compress latency, and disclose every cut.

### 7.3 Kyutai Moshi unveiling (Paris, 3 Jul 2024) [`kyutai_moshi_keynote_2024`, `decoder_moshi_2024`]
- **Video:** <https://www.youtube.com/watch?v=hm2IJSKcYvo>, 42:24.
- **Structure:**
  - 0:00 lab intro with kinetic type ("6 months", "8 people")
  - 2:27 live demo: Everest roleplay, emotions and styles, voices
  - 14:15 "The case for voice AI"
  - a technical deep-dive (multistream, Mimi, on-device, safety)
  - Q&A at 37:50
- **The most relevant precedent for Bud: live metrics on screen.** The demo UI shows a "Server Audio Stats" panel next to the streaming transcript. Frame at about 5:32: "Audio played: 0:33.27 / Missed audio: 0:00.24 / Latency: 0.262 / Min/Max buffer: 0.000 / 0.157", plus a pulsing orb and a level meter.
- **Look:** dark stage, neon-green accents, picture-in-picture presenter inset.
- **Claims:**
  - Theoretical latency 160 ms and about 200 ms in practice (SR; arXiv 2410.00037).
  - Press coverage: <https://the-decoder.com/french-ai-lab-kyutai-unveils-conversational-ai-assistant-moshi-plans-open-source-release>
- **Reception:** no independent review of the keynote was found. A 42-minute live keynote is not a format to copy for a 2–3 minute launch film; the transferable piece is the live stats panel.

### 7.4 Sesame "Crossing the uncanny valley of voice" (27 Feb 2025) [`sesame2025csm`, `coai_sesame_2025`]
- **No launch video.** The launch was a research blog post plus an interactive browser demo. The demo itself became the viral artefact. <https://www.sesame.com/research/crossing_the_uncanny_valley_of_voice>
- **Framing:** "voice presence: the magical quality that makes spoken interactions feel real, understood, and valued".
- **Evidence:** CMOS without context showed no clear preference vs human recordings; with 90 s of context, raters favoured the humans.
- **Why it spread:** deliberate disfluencies (breaths, chuckles, self-corrections), memory of the conversation, and "is this really AI?" reactions shared as screen recordings (secondary: <https://getcoai.com/news/sesames-ai-voice-presence-creates-emotional-bonds-with-human-like-imperfections>).
- **Lesson:** a try-it-yourself link is the strongest proof. The Bud video should end with a one-command self-host or a hosted demo number to call.

### 7.5 Bonus: OpenAI GPT-Live launch films (Jul 2026) [`openai_gptlive_video_2026`, `openai_gptlive_video2_2026`]
- **Videos:**
  - <https://www.youtube.com/watch?v=EAN5Cj347PY> (3:34)
  - <https://www.youtube.com/watch?v=K-fYBO8t3-A> (2:35)
- **Look:** cinematic and ad-like (shallow depth of field, real people on sofas). Live caption chips in white with a trailing dot, and a yellow "Live-1" title card.
- **Contrast:** this is brand-film polish, the opposite of Moshi's engineering-stats look. For an open-source, self-hosted infrastructure product, the engineering-honest register is the credible one.

### 7.6 Patterns across the references
| Element | GPT-4o 2024 | Astra 2024 | Moshi 2024 | Sesame 2025 | TML 2026 |
|---|---|---|---|---|---|
| Length | 26 min live (86 s clips) | 2:17 | 42 min | (no video) | 2:15 |
| Explicit problem statement | Yes, latency and cascades | No | Yes, "case for voice" | Yes, uncanny valley (blog) | Blog only |
| Real-time proof | Live on stage | "Single take, real time" card | Live plus stats panel | Try it yourself | One continuous shot |
| Interruptions shown | Yes, central | Little | Yes | n/a | Yes (overlap, translation) |
| Numbers on screen | No | No | **Yes, live latency** | No | No |
| Narrator | Presenters | None | Presenters | n/a | None |
| Typography | Minimal bug | White-on-black card, subtitle chips | Kinetic type, mono stats | n/a | Small grey caption chips |

### 7.7 Recommended structure: Bud WaaV interaction-mode launch video (about 2:30)
**Principles:**
- Use **one real, unedited call** as the spine, with Astra's "single take" disclosure card.
- Use **live metrics overlays** (Moshi's stats panel, made product-grade). They are computed from the gateway's own event stream, not added in post.
- No narrator during the demo. Captions name each capability as it happens, in the style of TML's chips.
- Numbers appear **only** where they can be footnoted, with source and "measured on <hardware>, <date>".
- Keep one imperfect moment visible.

| Time | Section | On screen | Audio |
|---|---|---|---|
| 0:00–0:06 | **Disclosure card** | White on near-black: "One take. A real phone call. Nothing sped up. Metrics are live from the gateway." | Silence, then a phone ring |
| 0:06–0:22 | **Hook / problem** | Split screen of a typical voice bot: the caller says "Actually, wait…" and the bot talks over them; or the caller asks for something slow and gets 8 s of dead air. A live counter ticks: "silence 6.8 s…" (an FDB-v3 cascade baseline is 10.12 s, cite on screen). | One voice-over line: "Voice agents either make you wait, or talk over you." |
| 0:22–0:30 | **Thesis card** | "A small, fast front model decides every turn. Slow work runs in the background." With the WaaV logo. | Voice-over, one line |
| 0:30–1:40 | **Live demo, single take** | Layout: caller camera (left two-thirds); right rail **Live panel** (mono font); bottom **timeline strip** (two waveforms, caller and agent, overlap shaded; turn-decision ticks) | Real call audio only |
| 1:40–1:58 | **How it works, 18 s** | Animated cascade: mic → Silero VAD + Smart Turn v3.2 + text turn detector → STT → **front model (Qwen3.5-2B, llama.cpp)** → TTS; side lanes **Tasks**: any LLM / gpt-oss-20b orchestrator / A2A / MCP / webhook; "44 speech vendors, swap any" with a logo wall | Voice-over |
| 1:58–2:15 | **Numbers** | 3–4 cards, each footnoted: (1) median ack latency (ours, measured live); (2) FDB v1 smooth-turn latency next to published references (TML 0.40 s SR; Gemini 3.1 FL minimal 0.567 s and GPT-Realtime 1.470 s from the FDB README); (3) FDB v1.5 per-scenario Respond/Resume rates; (4) FDB v3 Pass@1 with background agents. Only show what we have measured. | Voice-over or none |
| 2:15–2:30 | **Close / CTA** | "Open source. Self-hosted. Your models, your vendors, your data." Repo URL, a one-line `docker run`/install, "Call the demo: +…" | Ring-out |

**Demo beats inside 0:30–1:40.** Each beat triggers a caption chip and a Live-panel update.
1. **Instant ack + background task.**
   - Caller: "Can you find me a flight to Bengaluru on Monday and hold a seat?"
   - Front decision chip: `request`, with the decision latency in ms.
   - Ack audio starts within the measured time-to-first-audio.
   - The Tasks lane spawns "orchestrator (gpt-oss-20b) → flights agent (A2A): searching…" with a progress bar.
2. **Small talk while it works.** The caller asks an unrelated question and the front model answers (`answer`) while the task keeps running. This shows the conversation is not blocked.
3. **Correction mid-task.** "Actually make it Tuesday." Chip: `correction` → task updated. The task lane shows a re-plan rather than a new task.
4. **Barge-in.** The agent starts reading three options and the caller cuts in: "the second one". Chip: "barge-in · TTS stopped in N ms". The timeline shows the overlap.
5. **Backchannel ignored.** The caller says "mm-hm" mid-sentence and the agent keeps going. Chip: "backchannel · floor kept". Echo gate indicator stays green, showing the agent's own audio was not mistaken for the caller.
6. **Confirmation gate.** Agent: "That will charge ₹… to your card ending 4242. Shall I book it?" Chip: "side-effecting tool · awaiting confirmation". The caller says "yes" and the task completes.
7. **Caller control.** "Wait, hold on." Chip: `wait` → agent yields silently. Then "OK, go on."

**Live-panel fields** (all from gateway events; monospace; updated in real time):
- `front decision`: class + ms
- `time-to-first-audio`
- `floor`: caller / agent / overlap
- `turn end`: VAD / Smart Turn p / text-TD p
- `echo gate`: on / off
- `barge-in stop`: ms
- `tasks`: n running, with status
- `vendors`: STT / TTS / LLM names, which visually proves the swap-anything claim

**Typography and graphics:**
- One sans for captions, one monospace for the Live panel.
- Bud brand colours (see `/home/bud/launch/brand`).
- Caption chips lower-left in TML style.
- No stock music during the demo; a subtle bed only under the architecture and numbers sections.

**Honesty rules for the footnotes:**
- Every competitor number on screen gets a small source line (e.g. "FD-bench v1 README, May 2026", "self-reported, TML blog").
- Do not compare against TML's v1.5 "average" unless we use the same formula.
- Show the hardware the demo runs on.

## 8. References for WaaV's own components (for the system section of the paper)
- **Qwen3.5-2B (front model)** [`qwen35_2b`] (<https://huggingface.co/Qwen/Qwen3.5-2B>):
  - 2B parameters; hybrid architecture of Gated DeltaNet linear attention plus gated attention, 24 layers.
  - 262,144-token native context; 201 languages and dialects.
  - Apache-2.0; non-thinking mode by default.
- **llama.cpp** [`llamacpp`]: <https://github.com/ggml-org/llama.cpp>
- **gpt-oss-20b (example background orchestrator)** [`openai2025gptoss`] (arXiv 2508.10925, <https://arxiv.org/abs/2508.10925>):
  - 21B total and 3.6B active parameters (MoE), Apache-2.0, per the Hugging Face card <https://huggingface.co/openai/gpt-oss-20b>.
  - The Hugging Face card says MXFP4 weights let it "run within 16GB of memory".
- **Silero VAD** [`silero_vad`] (<https://github.com/snakers4/silero-vad>):
  - "less than 1ms" per 30+ ms chunk on one CPU thread (SR); 8/16 kHz.
  - The README calls the licence MIT, though a badge on the same page says CC BY-NC 4.0. Check the LICENSE file before citing.
- **Smart Turn v3.2** [`smartturn_repo`] and **LiveKit turn detector** [`livekit_turndetector`]: see §5.1 and §5.2.
- **MCP** [`mcp_spec`]: <https://modelcontextprotocol.io/specification>
- **A2A** [`a2a_spec`, `lf_a2a_press`]: <https://github.com/a2aproject/A2A>. Launched by Google in April 2025 and moved to the Linux Foundation on 23 Jun 2025, <https://linuxfoundation.org/press/linux-foundation-launches-the-agent2agent-protocol-project-to-enable-secure-intelligent-communication-between-ai-agents>.

## 9. Open items before publication
1. **Measure WaaV on FDB v1 and v1.5.** The repo supports server-client inference, so this is doable. Report the v1.5 per-scenario Respond/Resume/Stop/Resp table, and state the formula for any average.
2. **Run FDB-v3** with WaaV + gpt-oss-20b as the background (LiveKit harness, or a WaaV adapter), and report Pass@1, latency, interrupt rate and filler rate. Compare with the paper's cascade baseline: Pass@1 0.450, 10.12 s, 33.0% interrupt.
3. **Run TurnBench** (Sesame) for the VAD + Smart Turn + text-TD stack, on both the EOT and INT tracks.
4. **Re-verify vendor pages** on publication day. Prices and model names in this field change monthly; for example, Gemini 3.1 Flash Live is already "legacy".
5. **Unverified items:** the "Speech Agent Arena" and "τ-Voice" leaderboards, the Smart Turn v3.2 official notes (the only source is a repo commit), and Retell's and Vapi's latency claims (marketing only).
