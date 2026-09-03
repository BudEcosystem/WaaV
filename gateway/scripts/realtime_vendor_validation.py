#!/usr/bin/env python3
"""
WaaV realtime / S2S REAL-VENDOR live-validation tool.

  python3 scripts/realtime_vendor_validation.py <provider> [--gateway-port N]

This is the TURNKEY bridge across the credential boundary: an operator who has a
vendor key validates ANY of WaaV's 12 realtime providers against the REAL vendor
endpoint in one command. It drives the SAME WaaV `/realtime` WebSocket a real
client uses, so a PASS means the gateway's wire mapping round-trips end-to-end
against the live vendor.

How it works
------------
1. Each provider is described by ONE row in `PROVIDERS` (the per-provider table,
   built field-by-field from `src/core/realtime/<p>/protocol.rs`):
     - the gateway env var(s) the operator must set (the key + any extras like
       AZURE_OPENAI_ENDPOINT / YANDEX_FOLDER_ID / AWS creds),
     - the realtime `{type:config, provider, model, voice, instructions, …}` to send,
     - the audio sample rate (per the protocol's `caps().output_sample_rate`),
     - the interaction pattern (a TEXT turn for openai/azure/grok/inworld/yandex;
       AUDIO-FIRST for the server-VAD providers).
2. If the required env var(s) are not set, the tool REFUSES to run and tells the
   operator EXACTLY which vars to export (it never silently no-ops or sends a
   dummy key to the real vendor).
3. It EITHER starts its own gateway (`./target/debug/waav-gateway -c config.yaml`)
   with those env vars passed through — and crucially with NO `*_REALTIME_URL`
   override, so the provider's `connect_spec` dials the REAL vendor host — OR it
   attaches to an already-running gateway via `--gateway-port`.
4. It runs the full S2S round-trip (config → audio/text → collect transcript +
   agent audio + events) with the proven harness methodology: wait for
   `session_created`, pace audio at 40 ms, keep-feed silence, ~30 s budget.
5. It reports PASS/FAIL with agent-audio bytes, transcripts, events, and on a
   wire mismatch it prints the gateway's `error` frame VERBATIM (so a real-vendor
   wire divergence is actionable).

Accuracy note: an inaccurate runbook is worse than none, so every endpoint /
auth / rate / required-field below is sourced from the provider's protocol.rs and
the gateway request schema (`src/handlers/realtime/messages.rs`). The companion
RUNBOOK `docs/realtime-vendor-validation.md` mirrors this table cell-for-cell.
"""

import argparse
import asyncio
import json
import os
import signal
import socket
import subprocess
import sys
import time

GATEWAY_DIR = "/home/bud/ditto/waav/WaaV/gateway"
GATEWAY_BIN = os.path.join(GATEWAY_DIR, "target/debug/waav-gateway")
GATEWAY_CONFIG = os.path.join(GATEWAY_DIR, "config.yaml")

# Source PCM clips the proven harnesses use. 24 kHz mono s16le (~3.9 s); the 16 k
# variant is the same speech downsampled. Resampled on demand to any rate.
PCM_24K = "/tmp/question_pcm.raw"
PCM_16K = "/tmp/question_16k.raw"


# =============================================================================
# THE PER-PROVIDER TABLE — the data-driven core (built from protocol.rs).
# =============================================================================
#
# Each entry:
#   key_env       : the gateway credential env var that supplies the vendor key.
#                   None ⇒ the provider is keyless (Nova Sonic / AWS SigV4).
#   extra_env     : { ENV_VAR: human description } the operator must ALSO set
#                   (endpoint / folder id / AWS creds). All are REQUIRED to run.
#   sample_rate   : the audio rate to feed (per protocol.rs caps().output_sample_rate
#                   for the audio-first providers; the input rate for the others).
#   pattern       : "text"  ⇒ OpenAI-family — wait for session_created, send a
#                            text turn + create_response, collect audio+transcript.
#                   "audio" ⇒ server-VAD — wait for session_created, pace the
#                            question audio + keep-feed silence, collect the reply.
#   config        : EXTRA fields merged into the `{type:config, provider:…}` message
#                   beyond provider/model/voice/instructions (e.g. transcribe_input).
#   model/voice   : a known-good example (model is REQUIRED for some providers —
#                   azure deployment, elevenlabs agent_id, speechmatics template).
#   model_env     : if set, the operator may supply `model` via this env var
#                   (used for resources that are account-specific, e.g.
#                   ELEVENLABS_AGENT_ID, INWORLD_SESSION_ID, ULTRAVOX_MODEL).
#   needs         : free-text "more than a key" caveat surfaced in the refusal/PASS
#                   banner (agent_id / Bedrock model-access / minted session-id).
#   endpoint      : real vendor endpoint (documentation/diagnostics only — the
#                   gateway builds it from connect_spec; shown so the operator can
#                   confirm where traffic goes).
#   auth          : the auth scheme (documentation/diagnostics only).
#   flags         : [FLAG] unknowns to watch on the FIRST real run (from the
#                   protocol.rs [FLAG] comments).
PROVIDERS = {
    "openai": {
        "key_env": "OPENAI_API_KEY",
        "extra_env": {},
        "sample_rate": 24000,
        "pattern": "text",
        "model": "gpt-realtime",
        "voice": "marin",
        "config": {"transcribe_input": True, "modalities": ["audio", "text"],
                   "turn_detection": {"mode": "server_vad"}},
        "endpoint": "wss://api.openai.com/v1/realtime",
        "auth": "Authorization: Bearer <OPENAI_API_KEY>",
        "needs": None,
        "flags": [],
    },
    "azure": {
        "key_env": "AZURE_OPENAI_API_KEY",
        "extra_env": {
            "AZURE_OPENAI_ENDPOINT": "your Azure resource (e.g. \"my-res\" or "
                                     "\"https://my-res.openai.azure.com\") — the "
                                     "gateway injects it as the connect host",
        },
        "sample_rate": 24000,
        "pattern": "text",
        # model == the Azure DEPLOYMENT name (Azure routes by deployment, not model);
        # override with AZURE_DEPLOYMENT since deployment names are account-specific.
        "model": "gpt-realtime",
        "model_env": "AZURE_DEPLOYMENT",
        "voice": "alloy",
        "config": {"transcribe_input": True, "modalities": ["audio", "text"],
                   "turn_detection": {"mode": "server_vad"}},
        "endpoint": "wss://<resource>.openai.azure.com/openai/v1/realtime?deployment=<deployment>",
        "auth": "api-key: <AZURE_OPENAI_API_KEY>  (header, NOT Bearer)",
        "needs": "an Azure resource with a realtime DEPLOYMENT; `model` = that "
                 "deployment name (set AZURE_DEPLOYMENT)",
        "flags": [
            "GA path /openai/v1/realtime (NO api-version query) — if the resource "
            "only serves the dated preview endpoint, session.update may 400.",
        ],
    },
    "grok": {
        "key_env": "GROK_API_KEY",
        "extra_env": {},
        "sample_rate": 24000,
        "pattern": "text",
        "model": "grok-realtime",
        "voice": "alloy",
        "config": {"transcribe_input": True, "modalities": ["audio", "text"],
                   "turn_detection": {"mode": "server_vad"}},
        "endpoint": "wss://api.x.ai/v1/realtime",
        "auth": "Authorization: Bearer <GROK_API_KEY>",
        "needs": None,
        "flags": [
            "Endpoint NOT yet live-probed (no xAI key was held when written). "
            "xAI bootstraps with `conversation.created` (the gateway maps it to "
            "SessionReady); if the model name or host differs, the connect error "
            "is printed verbatim.",
        ],
    },
    "inworld": {
        "key_env": "INWORLD_API_KEY",
        "extra_env": {},
        "sample_rate": 24000,
        "pattern": "text",
        "model": "inworld-realtime",
        # Inworld needs a backend-MINTED session id, supplied to the gateway via the
        # config's endpoint slot. The handler does NOT inject it from server config,
        # so the only turnkey path to the REAL vendor is the INWORLD_REALTIME_URL
        # override (a pre-authed session ws). See `needs` + the runbook.
        "voice": "alloy",
        "config": {"transcribe_input": True, "modalities": ["audio", "text"],
                   "turn_detection": {"mode": "server_vad"}},
        "endpoint": "wss://api.inworld.ai/api/v1/realtime/session?key=<session-id>&protocol=realtime",
        "auth": "Authorization: Basic base64(<INWORLD_API_KEY>)",
        "needs": "a backend-MINTED session id (Inworld's REST session-create). The "
                 "gateway has NO server-config slot to inject it, so real-vendor "
                 "validation requires INWORLD_REALTIME_URL set to a pre-authed "
                 "session ws url (see runbook).",
        "requires_override": True,
        "flags": [
            "Auto session-minting is a pending REST-handshake follow-up; until "
            "then INWORLD_REALTIME_URL is the only turnkey route.",
        ],
    },
    "deepgram": {
        "key_env": "DEEPGRAM_API_KEY",
        "extra_env": {},
        "sample_rate": 24000,
        "pattern": "audio",
        "model": "gpt-4o-mini",            # the `think` LLM (Deepgram-managed creds)
        "voice": "aura-2-thalia-en",       # the `speak` TTS voice
        "config": {"modalities": ["audio", "text"]},
        "endpoint": "wss://agent.deepgram.com/v1/agent/converse",
        "auth": "Authorization: Token <DEEPGRAM_API_KEY>  (NOT Bearer)",
        "needs": None,
        "flags": [],
    },
    "elevenlabs": {
        "key_env": "ELEVENLABS_API_KEY",
        "extra_env": {},
        "sample_rate": 16000,              # ConvAI is pcm_16000 both ways
        "pattern": "audio",
        # model == a PRE-CREATED ConvAI agent_id (the agent IS the model). It is
        # account-specific, so it MUST come from ELEVENLABS_AGENT_ID.
        "model": "",
        "model_env": "ELEVENLABS_AGENT_ID",
        "voice": None,
        "config": {"modalities": ["audio", "text"]},
        "endpoint": "wss://api.elevenlabs.io/v1/convai/conversation?agent_id=<agent_id>",
        "auth": "xi-api-key: <ELEVENLABS_API_KEY>  (header, NOT Bearer)",
        "needs": "a PRE-CREATED ConvAI agent_id (set ELEVENLABS_AGENT_ID). The "
                 "gateway reads it from the config `model` field.",
        "flags": [],
    },
    "gemini": {
        "key_env": "GEMINI_API_KEY",
        "extra_env": {},
        "sample_rate": 16000,              # input 16 k; output is 24 k
        "pattern": "audio",
        "model": "gemini-2.0-flash-live-001",
        "voice": "Puck",
        "config": {"transcribe_input": True, "modalities": ["audio", "text"]},
        "endpoint": ("wss://generativelanguage.googleapis.com/ws/"
                     "google.ai.generativelanguage.v1beta.GenerativeService."
                     "BidiGenerateContent?key=<GEMINI_API_KEY>"),
        "auth": "?key=<GEMINI_API_KEY>  (query param, NO header)",
        "needs": None,
        "flags": [
            "Default model id `gemini-2.0-flash-live-001` is the broadly-available "
            "stable Live model; a preview native-audio model may be required on "
            "some keys — override via the config `model`.",
        ],
    },
    "ultravox": {
        "key_env": "ULTRAVOX_API_KEY",
        "extra_env": {},
        "sample_rate": 16000,              # input 16 k; output is 24 k
        "pattern": "audio",
        "model": "fixie-ai/ultravox",
        "model_env": "ULTRAVOX_MODEL",
        "voice": "Mark",
        "config": {"modalities": ["audio", "text"]},
        "endpoint": "POST https://api.ultravox.ai/api/calls → wss joinUrl (gateway does the REST create-call)",
        "auth": "X-API-Key: <ULTRAVOX_API_KEY>  (on the REST create-call; the WS joinUrl is pre-authed)",
        "needs": "the gateway performs the REST `create call` for you (RestThenWebSocket); "
                 "no pre-created resource needed — just the key.",
        "flags": [
            "Output sample rate is declared 24 kHz in the create-call; not yet "
            "byte-verified against a live Ultravox stream.",
        ],
    },
    "nova_sonic": {
        # Nova Sonic is KEYLESS: it authenticates with AWS SigV4 via the aws-config
        # default chain (env creds / shared config / IAM role). No <provider>_API_KEY.
        "key_env": None,
        "extra_env": {
            "AWS_ACCESS_KEY_ID": "AWS access key (or use an IAM role / shared config)",
            "AWS_SECRET_ACCESS_KEY": "AWS secret key",
            "AWS_REGION": "the Bedrock region, e.g. us-east-1",
        },
        "sample_rate": 16000,              # input 16 k; output is 24 k
        "pattern": "audio",
        "model": "amazon.nova-sonic-v1:0",
        "voice": "matthew",
        "config": {"modalities": ["audio", "text"]},
        "endpoint": "Amazon Bedrock InvokeModelWithBidirectionalStream (HTTP/2 event stream, NOT a WebSocket)",
        "auth": "AWS SigV4 (aws-config default credential chain) — NO api-key",
        "needs": "AWS credentials with BEDROCK MODEL ACCESS to `amazon.nova-sonic-v1:0` "
                 "in the chosen region (request access in the Bedrock console first). "
                 "AWS_SESSION_TOKEN is also honored for temporary creds.",
        "flags": [
            "Region resolves from AWS_REGION (or the config `endpoint` slot). The "
            "bidi event-stream wire is unit-validated against the AWS docs, not a "
            "live capture.",
        ],
    },
    "speechmatics": {
        "key_env": "SPEECHMATICS_API_KEY",
        "extra_env": {},
        "sample_rate": 16000,
        "pattern": "audio",
        # model == a Flow PORTAL template_id (the agent's persona/LLM/voice binding).
        "model": "flow-service-assistant-amelia",
        "model_env": "SPEECHMATICS_TEMPLATE_ID",
        "voice": "amelia",
        "config": {"modalities": ["audio", "text"]},
        "endpoint": "wss://flow.api.speechmatics.com/v1/flow",
        "auth": "Authorization: Bearer <SPEECHMATICS_API_KEY>  (a JWT / temp token)",
        "needs": "a Flow PORTAL template_id (set SPEECHMATICS_TEMPLATE_ID). The key "
                 "is passed AS the Bearer token — supply a JWT / pre-minted temp "
                 "token; a static api-key may be rejected (no in-gateway exchange).",
        "flags": [
            "WaaV does NOT perform the management-platform token EXCHANGE — a "
            "static api-key passed as Bearer may 401; a JWT/temp token works.",
            "Connect PATH /v1/flow + the OUTPUT sample rate (assumed 16 kHz) + the "
            "transcript field names are documented, not byte-verified.",
        ],
    },
    "hume": {
        "key_env": "HUME_API_KEY",
        "extra_env": {},
        "sample_rate": 44100,              # EVI output is linear16 @ 44.1 kHz
        "pattern": "audio",
        # Hume EVI needs no agent/model id; voice is an EVI voice name. A config_id
        # is OPTIONAL (the default EVI config applies); WaaV maps no config_id field.
        "model": "",
        "voice": "kora",
        "config": {"modalities": ["audio", "text"],
                   "instructions": "You are concise. Answer in one short sentence."},
        "endpoint": "wss://api.hume.ai/v0/evi/chat",
        "auth": "?api_key=<HUME_API_KEY>  (query param, NO header)",
        "needs": None,
        "flags": [],
    },
    "yandex": {
        "key_env": "YANDEX_API_KEY",
        "extra_env": {
            "YANDEX_FOLDER_ID": "your Yandex Cloud folder id — the gateway builds "
                                "the gpt://<folder>/<model> URI from it",
        },
        "sample_rate": 24000,
        "pattern": "text",
        "model": "speech-realtime-250923",
        "voice": "alloy",
        "config": {"transcribe_input": True, "modalities": ["audio", "text"],
                   "turn_detection": {"mode": "server_vad"}},
        "endpoint": "wss://ai.api.cloud.yandex.net/v1/realtime?model=gpt://<folder>/<model>",
        "auth": "Authorization: Bearer <key>  (IAM token, prefix t1.)  OR  Api-Key <key>  (static key)",
        "needs": "the Yandex Cloud FOLDER ID (set YANDEX_FOLDER_ID). The key may be "
                 "an IAM token (t1.…) or a static API key.",
        "flags": [
            "Auth scheme is chosen by key shape: `t1.` prefix ⇒ Bearer (IAM token); "
            "otherwise ⇒ `Api-Key` (static key). If the server rejects a static key "
            "on the OpenAI-SDK Bearer path, mint an IAM token.",
        ],
    },
}

# Pretty-print order for the table (matches the prompt's provider order).
PROVIDER_ORDER = [
    "openai", "azure", "grok", "inworld", "deepgram", "elevenlabs",
    "gemini", "ultravox", "nova_sonic", "speechmatics", "hume", "yandex",
]


# =============================================================================
# Audio helpers
# =============================================================================

def _resample_s16le(pcm: bytes, src_hz: int, dst_hz: int) -> bytes:
    """Linear-interpolate mono s16le PCM from src_hz to dst_hz (stdlib only).

    Good enough to exercise the vendor's STT/VAD — we are validating the WIRE,
    not audio quality. `audioop` would be ideal but is removed in Python 3.13, so
    this is a dependency-free nearest/linear resampler.
    """
    if src_hz == dst_hz or not pcm:
        return pcm
    n_in = len(pcm) // 2
    if n_in == 0:
        return b""
    samples = int.from_bytes  # local alias
    src = [int.from_bytes(pcm[i * 2:i * 2 + 2], "little", signed=True) for i in range(n_in)]
    n_out = max(1, int(n_in * dst_hz / src_hz))
    out = bytearray(n_out * 2)
    ratio = (n_in - 1) / max(1, (n_out - 1))
    for j in range(n_out):
        pos = j * ratio
        i0 = int(pos)
        i1 = min(i0 + 1, n_in - 1)
        frac = pos - i0
        val = int(src[i0] * (1.0 - frac) + src[i1] * frac)
        val = max(-32768, min(32767, val))
        out[j * 2:j * 2 + 2] = val.to_bytes(2, "little", signed=True)
    return bytes(out)


def load_question_pcm(sample_rate: int) -> bytes:
    """Return ~4 s of speech PCM at `sample_rate` (mono s16le).

    Prefers the matching-rate source file, else resamples the 24 k clip. Falls
    back to silence if neither file is present (so the tool never hard-fails on a
    missing asset — the round-trip still exercises connect + config + keep-alive).
    """
    if sample_rate == 16000 and os.path.exists(PCM_16K):
        return open(PCM_16K, "rb").read()
    if os.path.exists(PCM_24K):
        return _resample_s16le(open(PCM_24K, "rb").read(), 24000, sample_rate)
    if sample_rate == 16000 and os.path.exists(PCM_16K):
        return _resample_s16le(open(PCM_16K, "rb").read(), 16000, sample_rate)
    # No source clip: 3 s of silence at the target rate (still a valid turn the
    # server VAD will end on).
    return bytes(int(sample_rate * 3.0) * 2)


# =============================================================================
# Env validation + gateway lifecycle
# =============================================================================

def gateway_side_env_for(provider: str) -> dict:
    """{ENV_VAR: description} the GATEWAY process must have — the vendor creds +
    server-config resources (key, endpoint, folder id, AWS creds, the Inworld
    override). In ATTACH mode (`--gateway-port`) these live in the OPERATOR's
    already-running gateway, NOT in this CLI's env, so they are NOT re-checked.
    """
    p = PROVIDERS[provider]
    req = {}
    if p["key_env"]:
        req[p["key_env"]] = "the vendor API key for " + provider
    req.update(p["extra_env"])
    # Inworld can ONLY reach the real vendor via the override (no session-mint path);
    # the gateway reads INWORLD_REALTIME_URL from server config.
    if provider == "inworld":
        req["INWORLD_REALTIME_URL"] = ("a pre-authed Inworld session ws url "
                                       "(wss://api.inworld.ai/api/v1/realtime/"
                                       "session?key=<minted-session-id>&protocol=realtime)")
    return req


def client_side_env_for(provider: str) -> dict:
    """{ENV_VAR: description} that THIS CLI injects into the `{type:config}` message
    (account-specific resources the gateway reads from the client config, not from
    server config). REQUIRED in BOTH spawn and attach mode. Only those with NO
    usable default count: elevenlabs agent_id (no default). Azure deployment /
    speechmatics template / ultravox model have working defaults ⇒ optional.
    """
    p = PROVIDERS[provider]
    req = {}
    if provider == "elevenlabs":
        req[p["model_env"]] = ("a PRE-CREATED ConvAI agent_id (the agent IS the "
                               "model)")
    return req


def required_env_for(provider: str) -> dict:
    """The full set of {ENV_VAR: description} the operator must set to SPAWN a
    gateway for `provider` (gateway-side creds + client-side resources)."""
    req = gateway_side_env_for(provider)
    req.update(client_side_env_for(provider))
    return req


def missing_env(provider: str, attach: bool = False) -> list:
    """Return the (var, description) list the operator still needs to set.

    `attach=True` (an already-running gateway via --gateway-port) drops the
    GATEWAY-SIDE creds (they live in the operator's running gateway) but STILL
    enforces the CLIENT-SIDE resources this CLI must put in the config message.
    """
    needed = client_side_env_for(provider) if attach else required_env_for(provider)
    out = []
    for var, desc in needed.items():
        v = os.environ.get(var, "")
        if not v or not v.strip():
            out.append((var, desc))
    return out


def wait_port(host, port, timeout=90.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.5):
                return True
        except OSError:
            time.sleep(0.2)
    return False


def wait_livez(port, timeout=120.0):
    import urllib.request
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/livez",
                                        timeout=1.0) as r:
                if r.status == 200:
                    return True
        except Exception:  # noqa: BLE001
            time.sleep(0.3)
    return False


def gateway_env(provider: str, port: int) -> dict:
    """The environment the spawned gateway inherits.

    The operator's REAL vendor creds (already in os.environ — we refused to run
    without them) flow through unchanged. We DELIBERATELY do NOT set any
    `*_REALTIME_URL` override for the target provider (except Inworld, whose only
    real-vendor route IS the override the operator supplied) — so the provider's
    `connect_spec` dials the REAL vendor host.
    """
    env = os.environ.copy()
    env["PORT"] = str(port)
    env["HOST"] = "127.0.0.1"
    env["AUTH_REQUIRED"] = "false"
    # We open one WS upgrade; keep the limiter generous regardless.
    env.setdefault("RATE_LIMIT_REQUESTS_PER_SECOND", "1000")
    env.setdefault("RATE_LIMIT_BURST_SIZE", "1000")
    env.setdefault("MAX_CONNECTIONS_PER_IP", "1000")
    # Live ORT/CUDA recipe (the gateway loads local models at startup on this host).
    env.setdefault("ORT_DYLIB_PATH", "/home/bud/.local/ortlib/libonnxruntime.so")
    env.setdefault("CUDA_HOME", "/tmp/nocuda")

    # SAFETY: strip every OTHER provider's REALTIME_URL override so a stale env
    # from a prior mock run can't silently redirect THIS provider's upstream away
    # from the real vendor. We only keep the target provider's override when it is
    # legitimately required (Inworld).
    for canon, var in [
        ("openai", "OPENAI_REALTIME_URL"), ("azure", "AZURE_REALTIME_URL"),
        ("grok", "GROK_REALTIME_URL"), ("inworld", "INWORLD_REALTIME_URL"),
        ("deepgram", "DEEPGRAM_REALTIME_URL"), ("elevenlabs", "ELEVENLABS_REALTIME_URL"),
        ("gemini", "GEMINI_REALTIME_URL"), ("ultravox", "ULTRAVOX_REALTIME_URL"),
        ("hume", "HUME_REALTIME_URL"), ("speechmatics", "SPEECHMATICS_REALTIME_URL"),
        ("yandex", "YANDEX_REALTIME_URL"),
    ]:
        keep = (canon == provider) and PROVIDERS[provider].get("requires_override")
        if not keep and var in env:
            del env[var]
    return env


def start_gateway(provider: str, port: int):
    log_path = f"/tmp/waav_vendor_validation_gateway_{provider}.log"
    log = open(log_path, "w")
    print(f"[gateway] starting on :{port} (real vendor for {provider}); "
          f"log → {log_path}", flush=True)
    p = subprocess.Popen([GATEWAY_BIN, "-c", GATEWAY_CONFIG], cwd=GATEWAY_DIR,
                         env=gateway_env(provider, port), stdout=log,
                         stderr=subprocess.STDOUT)
    if not wait_port("127.0.0.1", port, timeout=120.0):
        print(f"[FATAL] gateway did not open port {port} — see {log_path}",
              flush=True)
    elif not wait_livez(port, timeout=120.0):
        print("[WARN] gateway /livez did not return 200 in time", flush=True)
    return p, log_path


# =============================================================================
# The live round-trip
# =============================================================================

def build_config_message(provider: str) -> dict:
    """The `{type:config, …}` to send for `provider`, with account-specific
    resources resolved from env (model carrier / agent id / template id)."""
    p = PROVIDERS[provider]
    cfg = {"type": "config", "provider": provider}

    model = p.get("model") or ""
    model_env = p.get("model_env")
    if model_env and os.environ.get(model_env, "").strip():
        model = os.environ[model_env].strip()
    if model:
        cfg["model"] = model
    if p.get("voice"):
        cfg["voice"] = p["voice"]
    # A default instruction unless the provider's config supplies its own.
    cfg.setdefault("instructions", "You are concise. Answer in one short sentence.")
    cfg.update(p.get("config", {}))
    return cfg


async def run_text_turn(ws, provider, budget_s, t0, websockets):
    """OpenAI-family: wait for session_created, then drive ONE text turn."""
    audio_bytes = 0
    first_audio_ms = None
    transcripts = []
    events = []
    errors = []
    session_created = False

    # 1) wait for session_created before sending the turn (the upstream session
    #    must be live so the text turn isn't dropped during connect).
    ready_deadline = time.time() + min(15.0, budget_s)
    while time.time() < ready_deadline and not session_created:
        try:
            msg = await asyncio.wait_for(ws.recv(), timeout=ready_deadline - time.time())
        except (asyncio.TimeoutError, websockets.ConnectionClosed):
            break
        if isinstance(msg, (bytes, bytearray)):
            audio_bytes += len(msg)
            continue
        ev = json.loads(msg)
        t = ev.get("type")
        events.append(t)
        if t == "session_created":
            session_created = True
        elif t == "error":
            errors.append(ev.get("message", "?"))

    if session_created:
        await ws.send(json.dumps({"type": "text",
                                  "text": "Say hello and name one ocean."}))
        await ws.send(json.dumps({"type": "create_response"}))

    # 2) collect until response_done (or budget).
    deadline = t0 + budget_s
    done = False
    while not done and time.time() < deadline:
        try:
            msg = await asyncio.wait_for(ws.recv(), timeout=deadline - time.time())
        except (asyncio.TimeoutError, websockets.ConnectionClosed):
            break
        if isinstance(msg, (bytes, bytearray)):
            if first_audio_ms is None:
                first_audio_ms = (time.time() - t0) * 1000
            audio_bytes += len(msg)
            continue
        ev = json.loads(msg)
        t = ev.get("type")
        events.append(t)
        if t == "transcript":
            transcripts.append((ev.get("role"), ev.get("text", "")))
        elif t == "error":
            errors.append(ev.get("message", "?"))
        elif t == "response_done":
            done = True

    return dict(session_created=session_created, audio_bytes=audio_bytes,
                first_audio_ms=first_audio_ms, transcripts=transcripts,
                events=events, errors=errors)


async def run_audio_first(ws, provider, sample_rate, budget_s, t0, websockets):
    """Server-VAD: wait for session_created, pace the question audio + keep-feed
    silence, collect the agent's reply (the proven Deepgram/ElevenLabs pattern)."""
    pcm = load_question_pcm(sample_rate)
    frame = int(sample_rate * 0.04) * 2          # 40 ms s16le
    silence = b"\x00" * frame

    audio_bytes = 0
    first_audio_ms = None
    transcripts = []
    events = []
    errors = []
    session_created = False

    # 1) wait for session_created (provider connected) BEFORE feeding.
    ready_deadline = time.time() + min(15.0, budget_s)
    while time.time() < ready_deadline and not session_created:
        try:
            msg = await asyncio.wait_for(ws.recv(), timeout=ready_deadline - time.time())
        except (asyncio.TimeoutError, websockets.ConnectionClosed):
            break
        if isinstance(msg, (bytes, bytearray)):
            audio_bytes += len(msg)
            continue
        ev = json.loads(msg)
        t = ev.get("type")
        events.append(t)
        if t == "session_created":
            session_created = True
        elif t == "error":
            errors.append(ev.get("message", "?"))

    # 2) feed the question paced real-time, then keep streaming silence so the
    #    server VAD sees a clean turn end and the agent replies.
    async def feed():
        try:
            await asyncio.sleep(0.3)
            for i in range(0, len(pcm), frame):
                await ws.send(pcm[i:i + frame])
                await asyncio.sleep(0.04)
            # up to ~20 s of paced silence (well within the budget).
            for _ in range(500):
                await ws.send(silence)
                await asyncio.sleep(0.04)
        except websockets.ConnectionClosed:
            return

    feeder = asyncio.create_task(feed()) if session_created else None

    # 3) collect the agent response.
    got_audio = False
    deadline = t0 + budget_s
    while time.time() < deadline:
        try:
            msg = await asyncio.wait_for(ws.recv(), timeout=deadline - time.time())
        except (asyncio.TimeoutError, websockets.ConnectionClosed):
            break
        if isinstance(msg, (bytes, bytearray)):
            if first_audio_ms is None:
                first_audio_ms = (time.time() - t0) * 1000
            audio_bytes += len(msg)
            got_audio = True
            continue
        ev = json.loads(msg)
        t = ev.get("type")
        events.append(t)
        if t == "transcript":
            transcripts.append((ev.get("role"), ev.get("text", "")))
        elif t == "error":
            errors.append(ev.get("message", "?"))
        elif t == "response_done" and got_audio:
            break

    if feeder:
        feeder.cancel()
        try:
            await feeder
        except (asyncio.CancelledError, Exception):  # noqa: BLE001
            pass

    return dict(session_created=session_created, audio_bytes=audio_bytes,
                first_audio_ms=first_audio_ms, transcripts=transcripts,
                events=events, errors=errors)


async def validate(provider: str, port: int, budget_s: float = 30.0):
    try:
        import websockets
    except ImportError:
        print("[FATAL] the 'websockets' package is required: pip install websockets",
              flush=True)
        return 2

    url = f"ws://127.0.0.1:{port}/realtime"
    cfg = build_config_message(provider)
    p = PROVIDERS[provider]

    print(f"\n== REAL-VENDOR validation: {provider} via WaaV {url} ==", flush=True)
    print(f"   endpoint : {p['endpoint']}", flush=True)
    print(f"   auth     : {p['auth']}", flush=True)
    print(f"   rate     : {p['sample_rate']} Hz   pattern: {p['pattern']}", flush=True)
    print(f"   config   : {json.dumps(cfg)}", flush=True)

    t0 = time.time()
    try:
        async with websockets.connect(url, max_size=None, open_timeout=15) as ws:
            await ws.send(json.dumps(cfg))
            if p["pattern"] == "text":
                r = await run_text_turn(ws, provider, budget_s, t0, websockets)
            else:
                r = await run_audio_first(ws, provider, p["sample_rate"],
                                          budget_s, t0, websockets)
    except Exception as e:  # noqa: BLE001
        print(f"  [FAIL] client exception: {type(e).__name__}: {e}", flush=True)
        return 1

    from collections import Counter
    ec = dict(Counter(x for x in r["events"] if x))
    fa = f"{r['first_audio_ms']:.0f}ms" if r["first_audio_ms"] else "—"

    # PASS criterion: agent AUDIO came back (the full S2S round-trip closed). A
    # transcript without audio is a PARTIAL (config accepted, no spoken reply).
    ok = r["audio_bytes"] > 0
    status = "  PASS  " if ok else "  FAIL  "
    print(f"\n  [{status}] agent_audio={r['audio_bytes']}B  first_audio={fa}  "
          f"session_created={r['session_created']}", flush=True)
    print(f"           event_counts={ec}", flush=True)
    print(f"           transcripts={r['transcripts'][:8]}", flush=True)

    if r["errors"]:
        # A REAL-vendor wire mismatch surfaces here VERBATIM (the gateway's error
        # frame) — this is the actionable signal for a divergence.
        print("\n  GATEWAY ERROR FRAME(S) (verbatim — a real-vendor wire mismatch "
              "shows up here):", flush=True)
        for msg in r["errors"][:5]:
            print(f"    error: {msg}", flush=True)

    if not ok and not r["errors"]:
        print("\n  No agent audio AND no error frame within the budget. Likely "
              "causes: the vendor accepted the session but produced no spoken "
              "reply in time, or a silent upstream stall. Re-run; if it persists, "
              "inspect the gateway log.", flush=True)

    if p.get("flags"):
        print("\n  [FLAGs to watch on this first real run]:", flush=True)
        for fl in p["flags"]:
            print(f"    - {fl}", flush=True)

    return 0 if ok else 1


# =============================================================================
# CLI
# =============================================================================

def print_provider_list():
    print("Available providers:", flush=True)
    for name in PROVIDER_ORDER:
        p = PROVIDERS[name]
        req = list(required_env_for(name).keys())
        extra = ""
        if p.get("needs"):
            # Print the full note — splitting on '.' mangled model ids that contain
            # a period (e.g. amazon.nova-sonic-v1:0). The note is short.
            extra = "  (needs: " + p["needs"] + ")"
        print(f"  {name:13s} env: {', '.join(req)}{extra}", flush=True)


def main():
    parser = argparse.ArgumentParser(
        prog="realtime_vendor_validation.py",
        description="Validate a WaaV realtime/S2S provider against the REAL vendor "
                    "in one command. Requires the vendor key(s) in the environment.",
        epilog="Example:  OPENAI_API_KEY=sk-... python3 "
               "scripts/realtime_vendor_validation.py openai",
    )
    parser.add_argument("provider", nargs="?",
                        help="one of: " + ", ".join(PROVIDER_ORDER))
    parser.add_argument("--gateway-port", type=int, default=None,
                        help="attach to an ALREADY-RUNNING gateway on this port "
                             "instead of starting one (the operator is responsible "
                             "for having launched it with the vendor creds + NO "
                             "*_REALTIME_URL override).")
    parser.add_argument("--budget", type=float, default=30.0,
                        help="round-trip collection budget in seconds (default 30).")
    parser.add_argument("--list", action="store_true",
                        help="list providers + their required env vars and exit.")
    args = parser.parse_args()

    if args.list:
        print_provider_list()
        return 0

    if not args.provider:
        parser.print_usage()
        print("\nMissing <provider>.", flush=True)
        print_provider_list()
        return 2

    provider = args.provider.lower()
    if provider not in PROVIDERS:
        print(f"Unknown provider: {args.provider!r}", flush=True)
        print(f"Supported: {', '.join(PROVIDER_ORDER)}", flush=True)
        return 2

    attach = args.gateway_port is not None

    # REFUSE to run without the required env — never silently no-op against the
    # real vendor (and never send a dummy key to it). In ATTACH mode the gateway-
    # side creds live in the operator's running gateway, so only the CLIENT-SIDE
    # resources (which THIS CLI puts in the config message) are re-checked.
    missing = missing_env(provider, attach=attach)
    if missing:
        scope = ("client-side resource(s) this tool must send to the running "
                 "gateway") if attach else "environment variable(s)"
        print(f"Cannot validate {provider!r} against the real vendor: required "
              f"{scope} not set.\n", flush=True)
        print("Set these and re-run:", flush=True)
        for var, desc in missing:
            print(f"  export {var}=...    # {desc}", flush=True)
        p = PROVIDERS[provider]
        if p.get("needs"):
            print(f"\nNote: {p['needs']}", flush=True)
        example_vars = " ".join(f"{v}=..." for v, _ in missing)
        tail = f" --gateway-port {args.gateway_port}" if attach else ""
        print(f"\nOne-command form:\n  {example_vars} python3 "
              f"scripts/realtime_vendor_validation.py {provider}{tail}", flush=True)
        return 2

    # The gateway binary is only needed when we SPAWN one (attach mode reuses the
    # operator's running gateway).
    if not attach and not os.path.exists(GATEWAY_BIN):
        print(f"[FATAL] gateway binary not found at {GATEWAY_BIN}\n"
              f"        build it first:  cargo build --bin waav-gateway", flush=True)
        return 2

    # Attach to a running gateway, or start our own pointed at the real vendor.
    gw = None
    log_path = None
    if args.gateway_port is not None:
        port = args.gateway_port
        if not wait_port("127.0.0.1", port, timeout=5.0):
            print(f"[FATAL] no gateway listening on :{port} "
                  f"(remove --gateway-port to start one).", flush=True)
            return 2
        print(f"[gateway] attaching to already-running gateway on :{port} "
              f"(operator-supplied; assuming real-vendor creds + no override).",
              flush=True)
    else:
        port = 3019
        gw, log_path = start_gateway(provider, port)

    try:
        rc = asyncio.run(validate(provider, port, budget_s=args.budget))
    finally:
        if gw is not None:
            gw.send_signal(signal.SIGINT)
            try:
                gw.wait(timeout=10)
            except subprocess.TimeoutExpired:
                gw.kill()
            if rc != 0 and log_path:
                print(f"\n[hint] gateway log for debugging: {log_path}", flush=True)
    return rc


if __name__ == "__main__":
    sys.exit(main())
