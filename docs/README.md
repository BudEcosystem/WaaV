# WaaV documentation map

User-facing documentation for the gateway lives in [`gateway/docs/`](../gateway/docs/): start with the
[API reference](../gateway/docs/api-reference.md), the [WebSocket protocol](../gateway/docs/websocket.md) and
the [interaction mode tutorial](../gateway/docs/tutorials/interaction_mode_tutorial.md).

This folder holds the project's engineering record: how to build it, the designs behind its features, the
plans that drove its releases, and the audits and measurements along the way. Plans and reports are dated
snapshots; the code and `gateway/docs/` are the current truth.

| Folder | What is in it |
|---|---|
| [`build/`](build/) | [BUILD.md](build/BUILD.md), the verified build procedure and its gotchas, and [BUILD_NOTES.md](build/BUILD_NOTES.md) |
| [`duplex-agent/`](duplex-agent/) | The design of interaction mode (1.1), chapter by chapter, with the front model's prompt and evaluation |
| [`launch/`](launch/) | The interaction mode launch: the [paper](launch/paper/main.pdf), the [video](launch/video/launch_v4.mp4), the demo and benchmark scripts behind them, and the [defects they found](launch/to_be_fixed.md) |
| [`segmented-stt/`](segmented-stt/) | The design of segmented speech-to-text, its capability map and the [customer contract](segmented-stt/customer-contract-reference.md) |
| [`research/`](research/) | Design studies: [realtime reasoning](research/REALTIME_REASONING.md), the [Pipecat comparison](research/PIPECAT_ANALYSIS.md), the [vLLM-Omni comparison](research/VLLM_OMNI_VS_WAAV.md), the [dashboard UI](research/dashboard_ui_analysis.md) |
| [`plans/`](plans/) | Implementation plans, among them the [duplex agent](plans/DUPLEX_AGENT_PLAN.md), [production hardening](plans/PRODUCTION_PLAN.md), the [realtime roadmap](plans/REALTIME_ROADMAP.md), [SDK standardisation](plans/SDK_STANDARDIZATION_PLAN.md) and the [Opus codec](plans/OPUS_CODEC_PLAN.md) |
| [`reports/`](reports/) | Audits and measurements: the [codebase audit](reports/AUDIT_REPORT.md), the [production-readiness review](reports/BRUTAL_REVIEW.md), the [latency analysis](reports/LATENCY_ANALYSIS.md), the [OpenAI cross-check](reports/OPENAI_CROSSCHECK.md) |
| [`infer/`](infer/) | WaaV Infer, the local inference engine the gateway can use: its [gateway integration](infer/INFER_GATEWAY_INTEGRATION.md), model onboarding and recipes |
| [`archive/`](archive/) | The [1.0 README](archive/README-1.0.md), with the full June 2026 release notes, and an old code-review scratchpad |

Elsewhere in the repository:

- [`gateway/docs/research/cartesia/`](../gateway/docs/research/cartesia/): the Cartesia API research behind that provider.
- [`gateway/CHANGELOG.md`](../gateway/CHANGELOG.md): what changed in each release.
- [`gateway/scripts/`](../gateway/scripts/): provider smoke tests (`test_providers.sh`, `test_all_71_providers.sh`) and vendor probes.
- [`inferv2/`](../inferv2/): WaaV Infer's engine specification and reviews.
