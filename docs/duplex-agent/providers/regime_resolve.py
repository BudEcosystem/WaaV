#!/usr/bin/env python3
"""Executable specification of WaaV's regime selection and owner resolution.

Planning artefact for the full-duplex interaction plan (not gateway code). Standard library only;
uses the `jsonschema` package for schema validation when it is importable and a built-in minimal
validator otherwise (the self-test runs both and requires them to agree).

What it specifies, over provider_capabilities.json:
  * select_regime: W6-providers-endpoints.md section 3.3 (Decision 2), as aligned with
    INTEGRATION_DECISIONS.md sections 1 to 11 and Addenda A to C: lifecycle refusal, surface reach,
    half-cascade, the regime arms, "a forced regime may only narrow", and the speech.truth_ack rule
    (decisions section 5, sign-off 10).
  * owner resolution (filler_owner, backchannel_owner, idle_owner): W6 sections 3.3 and 3.9,
    including Azure's optional model_family (Addendum A2 item 10: until it is set, fillers resolve
    to gateway with config_warning model_family_unknown).
  * the session-level gating around it: server switch, build features, refused surfaces,
    release gating (decisions section 1), locale and the mid-call language step-down (Addendum
    C7.1), background requirement, Bud-mode surfaces, lenient parsing of server-only keys
    (including the server task catalog and duplex.compute, Addendum A2 items 4 and 5), front
    profile resolution (decisions section 2.1 as changed by Addendum B1 and B7: `auto` follows W2's
    qualified-profile list, so it yields `scripted` in Release 1 on every deployment), lifecycle
    warnings and refusals (W6 section 3.14).
  * the wire rules of W6 section 3.12.4 (the task-event mapping), as changed by Addendum C4:
    a CancelUnconfirmed is task_progress with cancelling true and the state unchanged, never
    task_cancelled or unknown; floor_state reasons include stream_gap; the /ws error carries an
    optional code (Addendum A2 item 9).

Readings this file implements. Items 1, 2, 4, 5 and 6 were questions this artefact raised; Addendum
A2 items 1, 7 and 8 adopted them, and W6 section 3.3 now states them:
  1. "May only narrow" = the forced regime must be one the provider can run (possible_regimes);
     the arms that force the delegating regime (native client delegation, permanent native
     filler without exact speech, always-async tools without exact speech) are exclusive.
  2. The half-cascade test is `half_cascade_possible` (text output AND a documented switch for
     the provider's own reply), not `text_output_possible`; they differ only for Ultravox.
  3. The native-duplex arm is tested before the provider-speech arm, so an Infer model that gains
     an injection channel is not classed as provider speech (W6 section 8, Release 5).
  4. In a forced delegating regime, `auto` owners resolve as `provider`.
  5. Half-cascade owners: filler gateway, backchannel none, idle gateway; WaaV's text-to-speech is
     the voice (voice_owner gateway).
  6. On a cascade surface a forced provider_speech or delegating regime yields regime_downgraded.

Usage:
  python3 regime_resolve.py --self-test        run the executable checks (exit 1 on failure)
  python3 regime_resolve.py --matrix           print the generated coverage matrix (Markdown)
  python3 regime_resolve.py --write-coverage   refresh the generated region of PROVIDER_COVERAGE.md
  python3 regime_resolve.py --validate         validate JSON files, examples and wire_events.md
  python3 regime_resolve.py --resolve '{"surface":"realtime","provider":"gemini",...}'
"""

import argparse
import copy
import datetime as _dt
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CAPS_PATH = os.path.join(HERE, "provider_capabilities.json")
CAPS_SCHEMA_PATH = os.path.join(HERE, "capabilities.schema.json")
CONFIG_SCHEMA_PATH = os.path.join(HERE, "interaction_config.schema.json")
EXAMPLES_DIR = os.path.join(HERE, "examples")
WIRE_EVENTS_PATH = os.path.join(HERE, "wire_events.md")
COVERAGE_PATH = os.path.join(HERE, "PROVIDER_COVERAGE.md")
BEGIN_MARK = "<!-- BEGIN GENERATED: python3 regime_resolve.py --matrix -->"
END_MARK = "<!-- END GENERATED -->"

REFERENCE_DATE = "2026-10-05"   # the date the 'expected' blocks are evaluated at
LICENCES = ("lfm_open_eligible", "liquid_commercial")
# W2's profiles (Addendum B7): the attested Liquid profiles in preference order, then the Apache one
LFM_PROFILES = ("lfm-standard", "lfm-large")
APACHE_PROFILES = ("apache-standard",)
STEP_DOWN = "steps down to the plain conversation path"
REPEAT_REQUEST = "repeat-request template, continues in English"
AS_CONTROLLING = "as the /ws session that drives it"
MID_CALL_LANGUAGE = {"step_down": STEP_DOWN, "repeat_request": REPEAT_REQUEST,
                     "as_controlling_ws_session": AS_CONTROLLING}
FORCED = {"cascade": "Cascade", "provider_speech": "ProviderSpeech", "delegating": "Delegating"}
OWNER_KINDS = ("filler", "backchannel", "idle")
OWNER_VALUES = ("auto", "gateway", "provider", "none")
NARROWED_ACTS = ["wait", "progress", "refuse"]
HALF_CASCADE_OWNERS = {   # reading 5, adopted by Addendum A2 item 8 (W6 sections 3.3 and 3.9)
    "filler": {"auto": "gateway", "gateway": "gateway", "provider": "gateway", "none": "none"},
    "backchannel": {"auto": "none", "gateway": "none", "provider": "none", "none": "none"},
    "idle": {"auto": "gateway", "gateway": "gateway", "provider": "gateway", "none": "none"},
}
# Vocabularies (INTEGRATION_DECISIONS.md section 4; cross-check-consistency.md table 4.1)
TASK_STATES = ["submitted", "working", "input_required", "auth_required", "completed", "failed",
               "cancelled", "rejected", "unknown"]
EFFECTS_WORDS = ["none_committed", "unknown"]
ANSWER_BY = ["voice", "dtmf", "client", "timeout", "session_end"]
OUTCOMES = ["accept", "decline", "cancel", "timeout", "superseded", "unanswerable"]
HITL_KINDS = ["confirm", "choose", "value", "form", "auth_url", "offer"]
ACTS = ["wait", "backchannel", "delegate", "update", "abort", "social", "echo", "progress",
        "question", "refuse"]
UTTERANCE_KINDS = ["ack", "answer", "progress", "hold", "choice_offer", "bridge", "relay",
                   "question", "confirm", "notice", "offer", "checkin", "followup", "greeting"]
TRANSCRIPT_SOURCES = ["front", "background", "gateway", "provider", "gateway_clip"]
INTERRUPTION_CLASSES = ["interrupt", "backchannel", "talking_to_others", "background", "noise", "echo"]
FLOOR_REASONS = ["user_speech", "front_ack", "relay", "hold", "idle", "reconnect", "stream_gap"]   # Addendum C4
ANSWER_STOP_REASONS = ["revised", "superseded", "cancelling", "speech_policy", "marker_then_tool_call",
                       "regenerating", "speakable_cap", "run_failed"]                         # W3 AnswerStopReason
LIVE_STATES = ["submitted", "working", "input_required", "auth_required"]
# The one wire-names table (W6 section 3.12.4): /ws name -> (/v1/realtime name, direction)
WIRE_EVENTS = {
    "task_created": ("bud.task.created", "out"),
    "task_progress": ("bud.task.progress", "out"),
    "task_question": ("bud.task.question", "out"),
    "task_answer_received": ("bud.task.answer_received", "out"),
    "task_result": ("bud.task.result", "out"),
    "task_cancelled": ("bud.task.cancelled", "out"),
    "floor_state": ("bud.floor.state", "out"),
    "unverified_claim": ("bud.claim.unverified", "out"),
    "agent_audio_stopped": ("bud.audio.stopped", "out"),
    "audio_segment": (None, "out"),
    "playback_control": ("bud.playback.control", "out"),
    "overlap": ("bud.overlap", "out"),
    "ready": ("bud.interaction.state", "out"),
    "interaction_updated": ("bud.interaction.updated", "out"),
    "config_warning": ("bud.warning", "out"),
    "error": (None, "out"),          # existing /ws error; optional code from Release 1 (Addendum A2 item 9)
    "assistant_transcript": (None, "out"),
    "function_call_cancelled": (None, "out"),
    "task_answer": ("bud.task.answer", "in"),
    "task_cancel": ("bud.task.cancel", "in"),
    "interaction_update": ("bud.interaction.update", "in"),
    "playback_position": (None, "in"),
}


class Refusal(Exception):
    """A RegimeRefusal or a session-level refusal. fatal=False means 'interaction off, session
    runs exactly as today'; fatal=True means the session itself is refused."""

    def __init__(self, code, fatal, detail=None, warnings=None):
        super().__init__(code)
        self.code = code
        self.fatal = fatal
        self.detail = detail or {}
        self.warnings = list(warnings or [])


def _date(s):
    return _dt.date.fromisoformat(s) if isinstance(s, str) else s


def load_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def _get(d, dotted, default=None):
    cur = d
    for part in dotted.split("."):
        if not isinstance(cur, dict) or part not in cur:
            return default
        cur = cur[part]
    return cur


class Resolver:
    def __init__(self, data):
        self.data = data
        self.surfaces = {s["id"]: s for s in data["surfaces"]}
        self.cascade = next(p for p in data["providers"] if p["id"] == "cascade")

    # ------------------------------------------------------------------ lookup and lifecycle
    def lookup(self, provider_id, model=None):
        entries = [p for p in self.data["providers"] if p["provider_id"] == provider_id]
        if not entries:
            raise KeyError("unknown provider %r" % provider_id)
        if not model:
            model = next((e["default_model"] for e in entries if e["default_model"]), "") or ""
        for e in entries:
            if any(re.search(p, model) for p in e["model_patterns"]):
                return e, model
        raise KeyError("no entry for %s model %r" % (provider_id, model))

    @staticmethod
    def lifecycle_at(entry, model, today):
        """W6 3.14: evaluated against an injected clock. Returns status active|retiring|retired."""
        today = _date(today)
        for rule in entry["lifecycle"]["rules"]:
            if not re.search(rule["models"], model or ""):
                continue
            if today >= _date(rule["shutdown"]):
                return {"status": "retired", "code": rule["refusal_code"], "shutdown": rule["shutdown"],
                        "replacement": rule["replacement"]}
            if rule["warning_from"] is None or today >= _date(rule["warning_from"]):
                return {"status": "retiring", "code": rule["warning_code"], "shutdown": rule["shutdown"],
                        "replacement": rule["replacement"]}
        return {"status": "active", "code": None, "shutdown": None, "replacement": None}

    # ------------------------------------------------------------------ surface reach
    def reach(self, surf, caps):
        """Return (reached, refusal_code_or_None, fatal)."""
        pid = caps["provider_id"]
        pr = surf.get("provider_refusals", {}).get(pid)
        if pr:
            return False, pr["code"], pr["fatal"]
        if surf["kind"] == "refused":
            return True, None, False          # refused later with the surface's own code
        if surf["kind"] == "cascade":
            return pid == "cascade", None, False
        if pid == "cascade":
            return False, "unsupported_vendor", True
        own = caps.get("surface_refusals", {}).get(surf["id"])
        if own:
            return False, own, True
        reaches = surf.get("reaches")
        if reaches == "all" or (isinstance(reaches, list) and pid in reaches):
            return True, None, False
        un = surf.get("unreached_refusal", {"code": "unsupported_vendor", "fatal": True})
        return False, un["code"], un["fatal"]

    # ------------------------------------------------------------------ the regime arms
    @staticmethod
    def _absent(verb):
        return verb["support"] in ("absent", "retired")

    def _exclusive_delegating(self, caps):
        v, bg, own = caps["verbs"], caps["background"], caps["ownership"]
        if bg["delegation"] == "native_client":
            return True
        if (caps["native_mechanisms"]["filler"]["kind"] == "always_on" and own["turn_owner"] == "provider"
                and self._absent(v["say_exact"])):
            return True                                                  # Gemini 3.8 Live
        if self._absent(v["say_exact"]) and bg["async_tools"] == "always_async":
            return True                                                  # Nova 2 Sonic
        return False

    def _native_duplex_ready(self, caps, release, flags):
        return (bool(flags.get("infer_injection_channel")) and bool(flags.get("native_duplex_criteria"))
                and release >= 5)

    def auto_regime(self, caps, release, flags):
        v, bg, own = caps["verbs"], caps["background"], caps["ownership"]
        if caps["kind"] == "native_duplex":                              # reading 3
            if self._native_duplex_ready(caps, release, flags):
                return "NativeDuplex"
            raise Refusal("interaction_unsupported", False,
                          {"reason": "native duplex model has no injection channel"})
        if self._exclusive_delegating(caps):
            return "Delegating"
        if (v["say_exact"]["support"] == "native" or v["say_guided"]["support"] == "native"
                or v["add_context"]["support"] == "native"):
            return "ProviderSpeech"
        if own["turn_owner"] == "provider" and bg["tools_declarable"]:
            return "Delegating"                                          # Gemini 3.1 Flash Live
        raise Refusal("interaction_unsupported", False, {"reason": "no regime fits the descriptor"})

    def possible_regimes(self, caps, release, flags):
        """Reading 1: the regimes a forced `interaction.regime` may narrow to."""
        out = set()
        if caps["ownership"]["half_cascade_possible"]:
            out.add("Cascade")
        if caps["kind"] == "native_duplex":
            if self._native_duplex_ready(caps, release, flags):
                out.add("NativeDuplex")
            return out
        if self._exclusive_delegating(caps):
            out.add("Delegating")
            return out
        v = caps["verbs"]
        if (v["say_exact"]["support"] == "native" or v["say_guided"]["support"] == "native"
                or v["add_context"]["support"] == "native"):
            out.add("ProviderSpeech")
        if caps["ownership"]["turn_owner"] == "provider" and caps["background"]["tools_declarable"]:
            out.add("Delegating")
        return out

    # ------------------------------------------------------------------ select_regime (W6 3.3)
    def select_regime(self, surf, caps, model, cfg, server, today, release=5, flags=None):
        flags = flags or {}
        warnings = []
        lc = self.lifecycle_at(caps, model, today)
        if lc["status"] == "retired":
            raise Refusal(lc["code"], True, {"shutdown": lc["shutdown"], "replacement": lc["replacement"]})
        if surf["kind"] == "cascade":
            if cfg.get("regime", "auto") not in ("auto", "cascade"):
                warnings.append("regime_downgraded")                     # reading 6
            return self.decide(surf, self.cascade, model, "Cascade", "full", cfg, warnings, flags)
        ok, code, fatal = self.reach(surf, caps)
        if not ok:
            raise Refusal(code, fatal, {"surface": surf["id"], "provider": caps["provider_id"]})
        speech = cfg.get("speech", {})
        if speech.get("voice_owner") == "gateway" or speech.get("truth") == "enforced":
            if caps["ownership"]["half_cascade_possible"]:                # reading 2
                return self.decide(surf, caps, model, "Cascade", "half_cascade", cfg, warnings, flags)
            if speech.get("truth") == "enforced":
                raise Refusal("truth_enforcement_unavailable", True,
                              {"provider": caps["provider_id"], "model": model})
            warnings.append("voice_owner_unavailable")
        try:
            regime = self.auto_regime(caps, release, flags)
        except Refusal as r:
            r.warnings = warnings + r.warnings
            raise
        variant = None
        forced = cfg.get("regime", "auto")
        if forced != "auto":
            want = FORCED[forced]
            if want in self.possible_regimes(caps, release, flags):
                regime = want
                variant = "half_cascade" if want == "Cascade" else None
            else:
                warnings.append("regime_downgraded")
        d = self.decide(surf, caps, model, regime, variant, cfg, warnings, flags)
        return self.require_truth_ack(d, caps, model, server.get("truth_ack", []))

    def require_truth_ack(self, d, caps, model, ack):
        """INTEGRATION_DECISIONS.md section 5: monitored regimes run only for listed providers;
        entries are "provider" or "provider:model"; server-side only."""
        if d["truth"] == "enforced":
            return d
        pid = caps["provider_id"]
        if any(e == pid or e == "%s:%s" % (pid, model) for e in ack):
            return d
        alt = "speech.truth: enforced" if caps["ownership"]["half_cascade_possible"] else None
        raise Refusal("truth_ack_required", False,
                      {"provider": pid, "model": model, "path": "interaction.speech.truth_ack",
                       "alternative": alt}, warnings=d["warnings"])

    # ------------------------------------------------------------------ owners (W6 3.3, 3.9)
    @staticmethod
    def _auto_value(caps, kind, model, flags=None):
        auto = caps["owners"][kind]["auto"]
        if isinstance(auto, dict) and "if_model_family_matches" in auto:
            family = (flags or {}).get("model_family")          # Addendum A2 item 10 (Azure)
            if not family:
                return auto["unknown"]
            return auto["then"] if re.search(auto["if_model_family_matches"], family) else auto["else"]
        if isinstance(auto, dict):
            return auto["then"] if re.search(auto["if_model_matches"], model or "") else auto["else"]
        return auto

    @staticmethod
    def family_unknown(caps, kind, flags):
        auto = caps["owners"][kind]["auto"]
        return isinstance(auto, dict) and "if_model_family_matches" in auto and not (flags or {}).get("model_family")

    def resolve_owner(self, caps, kind, requested, regime, variant, model, flags):
        """Return (result, warning_or_None, how)."""
        requested = requested or "auto"
        if regime == "Cascade" and variant == "half_cascade":
            res = HALF_CASCADE_OWNERS[kind][requested]
            how = "half-cascade: WaaV text-to-speech is the voice (Addendum A2 item 8)"
        elif regime == "NativeDuplex":
            res, how = "provider", "intrinsic to the native duplex model"
        else:
            table = self.cascade if regime == "Cascade" else caps
            if requested == "auto":
                default_regime = None
                if table is caps:
                    try:
                        default_regime = self.auto_regime(caps, 5, flags)
                    except Refusal:
                        default_regime = None
                want = "provider" if (regime == "Delegating" and default_regime != "Delegating") \
                    else self._auto_value(table, kind, model, flags)       # reading 4
            else:
                want = requested
            out = table["owners"][kind][want]
            res, how = out["result"], out["how"]
            flag = out.get("unless_operator_flag")
            if flag and flag in flags.get("operator_flags", []):
                res = out["then"]
        warn = None
        if requested != "auto" and res != requested:
            warn = "%s_owner_overridden" % kind
        return res, warn, how

    # ------------------------------------------------------------------ the decision
    def confirmation_path(self, caps, regime, variant, irreversible, client_ui, voice_mismatch):
        """W6 section 3.5.1: the speech path for a confirmation line."""
        if regime == "Cascade":
            return "exact_waav_tts"
        if caps["verbs"]["say_exact"]["support"] == "native":
            return "native_exact"
        if caps["ownership"]["voice_reproducible"]["value"] in ("same", "same_when_voice_known"):
            return "gateway_clip_same_voice"
        if irreversible:                       # never guided
            if client_ui:
                return "client_ui"
            if voice_mismatch in ("confirmations_only", "always"):
                return "gateway_clip_different_voice"
            return "unanswerable_no_verbatim_channel"
        if caps["verbs"]["say_guided"]["support"] in ("native", "emulated"):
            return "say_guided_exact_words_checked"   # an answer to a question that fails the check is void
        return "client_ui" if client_ui else "unanswerable_no_verbatim_channel"

    def decide(self, surf, caps, model, regime, variant, cfg, warnings, flags):
        speech = cfg.get("speech", {})
        owners, how = {}, {}
        for kind in OWNER_KINDS:
            res, warn, h = self.resolve_owner(caps, kind, cfg.get(kind + "_owner", "auto"), regime,
                                              variant, model, flags)
            owners[kind + "_owner"] = res
            how[kind + "_owner"] = h
            if warn:
                warnings.append(warn)
            if (regime == "ProviderSpeech" and cfg.get(kind + "_owner", "auto") == "auto"
                    and self.family_unknown(caps, kind, flags) and "model_family_unknown" not in warnings):
                warnings.append("model_family_unknown")                 # Addendum A2 item 10
        if regime == "Cascade":
            front = "local"
        elif regime == "ProviderSpeech":
            front = "narrator_front_model" if speech.get("narrator") == "front_model" else "narrator_templates"
        else:
            front = "provider"
        masked = []
        if front == "narrator_front_model":
            masked = [a for a in ACTS if a not in NARROWED_ACTS]
        elif front == "local" and owners["backchannel_owner"] != "gateway":
            masked = ["backchannel"]
        truth = "enforced" if regime == "Cascade" else "monitored"
        voice_mismatch = speech.get("voice_mismatch", "confirmations_only")
        client_ui = bool(flags.get("client_ui", surf.get("client_ui", True)))
        relay = ("verbatim_streamed" if regime == "Cascade" else caps["relay"]["mode"])
        if regime == "Cascade":
            ack = "E + ack_due_ms (300 ms; 700 ms in fast mode), skipped when the first answer sentence is ready; hard limit E + 2,000 ms"
        elif regime == "ProviderSpeech" and owners["filler_owner"] == "gateway":
            ack = "later of E + 300 ms and the provider's delegate call; skipped when an answer sentence is ready or the provider spoke after E; hard limit E + 2,000 ms with a call open"
        elif regime == "ProviderSpeech" and owners["filler_owner"] == "provider":
            ack = "none from the gateway: the provider's own preambles or soft timeout; gateway acknowledgement acts masked"
        elif regime == "ProviderSpeech":
            ack = "none (filler_owner none)"
        else:
            ack = "none from the gateway: the provider is the front (W7 reports provider_first_audio_after_e_ms)"
        return {
            "regime": regime,
            "variant": variant if regime != "Cascade" else (variant or "full"),
            "front": front,
            "masked_acts": masked,
            "filler_owner": owners["filler_owner"],
            "backchannel_owner": owners["backchannel_owner"],
            "idle_owner": owners["idle_owner"],
            "owner_how": how,
            "voice_owner": "gateway" if regime == "Cascade" else "provider",
            "truth": truth,
            "relay": relay,
            "confirm_irreversible": self.confirmation_path(caps, regime, variant, True, client_ui, voice_mismatch),
            "confirm_reversible": self.confirmation_path(caps, regime, variant, False, client_ui, voice_mismatch),
            "ack": ack,
            "egress": surf.get("egress"),
            "provisional_runs": (caps["background"]["provisional_runs"] is not None) if regime != "Cascade" else True,
            "warnings": warnings,
        }

    # ------------------------------------------------------------------ front profile (decisions 2.1)
    @staticmethod
    def resolve_front_profile(server, cfg):
        """Decisions section 2.1 as changed by Addendum B1 and B7, following W2's resolution order.
        `server["qualified_profiles"]` stands for W2's pinned qualified-profile list (a file shipped
        with the gateway, not configuration); it is empty in Release 1, so `auto` yields `scripted`
        on every deployment, attested or not. An LFM profile also needs the licence attestation.
        The operator may force a profile for evaluation with front_model.profile and
        allow_unqualified (echoed qualified: false)."""
        warnings = []
        fm = server.get("front_model", {}) or {}
        lic = fm.get("licence")
        qualified = list(server.get("qualified_profiles", []))

        def usable(p, need_qualified=True):
            if p in LFM_PROFILES and lic not in LICENCES:
                return "not_licensed"
            if need_qualified and p not in qualified:
                return "not_qualified"
            return None

        def auto():
            if lic in LICENCES:
                for p in LFM_PROFILES:                      # prefer the faster 1.2B when both qualify
                    if p in qualified:
                        return p, True
            for p in APACHE_PROFILES:
                if p in qualified:
                    return p, True
            return "scripted", False

        resolved, is_qualified = auto()
        operator = fm.get("profile", "auto") or "auto"
        if operator != "auto":
            why = usable(operator, need_qualified=not fm.get("allow_unqualified", False))
            if why is None:
                resolved, is_qualified = operator, operator in qualified
        requested = _get(cfg, "front.profile", "auto") or "auto"
        if requested != "auto":
            enabled = fm.get("profiles_enabled", ["auto"])
            reason = "not_enabled" if requested not in enabled else (
                None if requested == "scripted" else usable(requested, not fm.get("allow_unqualified", False)))
            if reason:
                warnings.append(("front_profile_substituted",
                                 {"requested": requested, "effective": resolved, "reason": reason}))
            else:
                resolved, is_qualified = requested, requested in qualified
        if resolved == "scripted":
            warnings.append(("front_model_scripted", {}))
        return resolved, warnings, is_qualified

    # ------------------------------------------------------------------ lenient parse
    @staticmethod
    def parse_block(block, from_client=True):
        """A small part of InteractionConfig::from_json_lenient: server-only keys and bad enums."""
        warnings = []
        if block is None or block is False:
            return None, warnings
        if block is True:
            return {}, warnings
        cfg = copy.deepcopy(block)

        def strip_licence(node, path):
            if isinstance(node, dict):
                for k in list(node):
                    p = path + "." + k
                    if k == "licence":
                        warnings.append(("interaction_override_refused", {"path": p}))
                        del node[k]
                    else:
                        strip_licence(node[k], p)
        strip_licence(cfg, "interaction")
        # Server-only paths of the one interaction key (W6 3.10.2). Caller memory and tenant personas
        # are not here: their settings live under W2's front_model key (Addendum A2 item 3, C5),
        # which no session home can reach. The server task catalog sits under interaction.tasks.*
        # (A2 item 4) and the compute pool under interaction.duplex.compute.* (A2 item 5).
        server_only = ["defaults", "allow_client_override", "realtime_legacy_wire", "speech.truth_ack",
                       "duplex.compute", "tasks.task_backends", "tasks.allow_client_backends",
                       "tasks.allow_client_tools", "tasks.allow_client_push", "tasks.phone_continuation",
                       "tasks.speech_policy"]
        if from_client:
            server_only.append("speech.fallback")
        for path in server_only:
            parent, _, leaf = path.rpartition(".")
            node = _get(cfg, parent) if parent else cfg
            if isinstance(node, dict) and leaf in node:
                del node[leaf]
                warnings.append(("interaction_override_refused", {"path": "interaction." + path}))
        enums = {"regime": ["auto", "cascade", "provider_speech", "delegating"],
                 "filler_owner": list(OWNER_VALUES), "backchannel_owner": list(OWNER_VALUES),
                 "idle_owner": list(OWNER_VALUES),
                 "speech.voice_owner": ["auto", "provider", "gateway"],
                 "speech.truth": ["monitored", "enforced"],
                 "speech.narrator": ["templates", "front_model"],
                 "speech.voice_mismatch": ["never", "confirmations_only", "always"]}
        for path, allowed in enums.items():
            parent, _, leaf = path.rpartition(".")
            node = _get(cfg, parent) if parent else cfg
            if isinstance(node, dict) and leaf in node and node[leaf] not in allowed:
                warnings.append(("interaction_value_ignored", {"path": "interaction." + path, "value": node[leaf]}))
                del node[leaf]
        return cfg, warnings

    # ------------------------------------------------------------------ full session resolution
    def resolve_session(self, s):
        """Resolve one session description. Keys: surface, provider, model, model_family (Azure,
        Addendum A2 item 10), release, today, bud_mode, interaction (client block), server {enabled,
        build_features, truth_ack, front_model, qualified_profiles}, has_background, language,
        echo_gates_pass, client_ui, operator_flags, infer_injection_channel, native_duplex_criteria."""
        surf = self.surfaces[s["surface"]]
        release = s.get("release", 5)
        today = s.get("today", REFERENCE_DATE)
        server = s.get("server", {})
        flags = {k: s[k] for k in ("operator_flags", "infer_injection_channel", "native_duplex_criteria",
                                   "client_ui", "model_family") if k in s}
        provider = "cascade" if surf["kind"] == "cascade" else s.get("provider", "cascade")
        result = {"surface": surf["id"], "release": release, "interaction": "off", "fatal": False,
                  "refusal": None, "detail": {}, "warnings": []}

        def done(code=None, fatal=False, detail=None):
            result["refusal"] = code
            result["fatal"] = fatal
            result["detail"] = detail or {}
            if code is None and not fatal:
                result["interaction"] = "on"
            return result

        try:
            caps, model = self.lookup(provider, s.get("model"))
        except KeyError as e:
            return done("unknown_provider", True, {"error": str(e)})
        result["provider_entry"] = caps["id"]
        result["model"] = model
        # Lifecycle track: warnings and refusals apply with interaction on or off (W6 3.14)
        lc = self.lifecycle_at(caps, model, today)
        if lc["status"] == "retired":
            return done(lc["code"], True, {"shutdown": lc["shutdown"], "replacement": lc["replacement"]})
        if lc["status"] == "retiring":
            result["warnings"].append(lc["code"])
        # Provider adapters that do not exist yet in this release (GPT-Live from Release 3, Infer 5)
        avail = caps.get("available_from_release") or 0
        if release < avail:
            return done("unknown_provider", True,
                        {"reason": "adapter not available before Release %d" % avail})
        # Bud-mode constraints of today's surfaces (existing behaviour)
        bud = bool(s.get("bud_mode"))
        if surf["bud_mode"] == "refused" and bud:
            return done(surf["bud_refusal"]["code"], True)
        if surf["bud_mode"] == "only" and not bud:
            ref = surf.get("non_bud_refusal", {"code": "bud_mode_required"})
            return done(ref["code"], True)
        # Fatal reach refusals hold whether or not interaction is on
        ok, code, fatal = self.reach(surf, caps)
        if not ok and fatal:
            return done(code, True, {"surface": surf["id"], "provider": caps["provider_id"]})
        cfg, pw = self.parse_block(s.get("interaction"), from_client=s.get("from_client", True))
        if cfg is None:
            return done("interaction_absent")  # off, today's behaviour, no warning
        for w, d in pw:
            result["warnings"].append(w)
        if not server.get("enabled", True):
            return done("interaction_disabled_by_server")
        if not server.get("build_features", True):
            return done("interaction_missing_build_features")
        if surf["kind"] == "refused":
            return done(surf["refusal"]["code"], surf["refusal"]["fatal"])
        first = surf["first_release"]
        if surf.get("first_release_if_echo_gates_fail") and not s.get("echo_gates_pass", False):
            first = surf["first_release_if_echo_gates_fail"]
        if first is None or release < first:
            return done("interaction_surface_unavailable", False, {"surface": surf["id"], "release": first})
        lang = s.get("language") or _get(cfg, "front.caller.language") or "en"
        if not (lang.startswith("en") or (lang.startswith("hi") and release >= 4)):
            return done("interaction_locale_unsupported", False, {"language": lang})
        if surf.get("needs_background") and not s.get("has_background", False):
            return done("interaction_needs_background")
        try:
            d = self.select_regime(surf, caps, model, cfg, server, today, release, flags)
        except Refusal as r:
            result["warnings"].extend(r.warnings)
            return done(r.code, r.fatal, r.detail)
        result["warnings"].extend(d.pop("warnings"))
        result.update(d)
        if d["front"] == "local":
            prof, fw, qual = self.resolve_front_profile(server, cfg)
            result["front_profile"] = prof
            result["front_qualified"] = qual
            for w, det in fw:
                result["warnings"].append(w)
        if release < 4 and "dtmf" in (_get(cfg, "hitl.channels") or []):
            result["warnings"].append("interaction_value_ignored")
        result["mid_call_unsupported_language"] = self.mid_call_language(surf)
        return done()

    @staticmethod
    def mid_call_language(surf):
        """Addendum C7.1: the plain conversation path starts with an empty history, so only /ws
        conversation and agent sessions can step down (interaction_language_step_down); raw /ws,
        the GA cascade and regimes B and C play a repeat-request template and continue in English."""
        return MID_CALL_LANGUAGE[surf.get("mid_call_language", "repeat_request")]

    # ------------------------------------------------------------------ expectations per entry
    def compute_expected(self, caps):
        """Recompute an entry's 'expected' block (evaluated at REFERENCE_DATE, release 5)."""
        model = caps["default_model"] if caps["default_model"] and caps["kind"] != "cascade" else ""
        if caps["id"] == "openai.gpt-realtime-2":
            model = "gpt-realtime-2.1"
        if caps["id"] == "gemini.3.8-live-extended-thinking":
            model = "gemini-3.8-live-extended-thinking"
        if caps["id"] == "gemini.legacy":
            model = "gemini-3.1-flash-live-preview"
        surf = self.surfaces["ws_conversation" if caps["kind"] == "cascade" else "ws_realtime_voice"]
        ack_all = {"truth_ack": [caps["provider_id"]]}

        def run(cfg, server):
            try:
                d = self.select_regime(surf, caps, model, cfg, server, REFERENCE_DATE, 5, {})
                return d, None
            except Refusal as r:
                return None, r

        def summarise(d, r, full):
            if r:
                out = {"refusal": r.code}
                if r.code == "truth_ack_required":
                    out["alternative"] = r.detail.get("alternative")
                if r.warnings:
                    out["warning"] = r.warnings[0]
                return out
            if full:
                return {"regime": d["regime"], "variant": d["variant"] if d["regime"] == "Cascade" else None,
                        "front": d["front"], "filler_owner": d["filler_owner"],
                        "backchannel_owner": d["backchannel_owner"], "idle_owner": d["idle_owner"],
                        "truth": d["truth"]}
            out = {"regime": d["regime"]}
            if d["regime"] == "Cascade":
                out["variant"] = d["variant"]
            if d["warnings"]:
                out["warning"] = d["warnings"][0]
            return out

        exp = {}
        d, r = run({}, ack_all)
        exp["auto_with_truth_ack"] = summarise(d, r, True)
        d, r = run({}, {"truth_ack": []})
        exp["auto_without_truth_ack"] = summarise(d, r, True)
        d, r = run({"speech": {"voice_owner": "gateway"}}, ack_all)
        exp["voice_owner_gateway"] = summarise(d, r, False)
        d, r = run({"speech": {"truth": "enforced"}}, ack_all)
        exp["truth_enforced"] = summarise(d, r, False)
        forced = {}
        for name in ("cascade", "provider_speech", "delegating"):
            d, r = run({"regime": name}, ack_all)
            if r:
                forced[name] = {"refusal": r.code}
            else:
                forced[name] = {"regime": d["regime"], "downgraded": "regime_downgraded" in d["warnings"]}
        exp["forced"] = forced
        exp["interaction_release"] = self.interaction_release(caps)
        return exp

    def interaction_release(self, caps):
        """First release in which interaction mode reaches the provider: for non-Bud gateways,
        for Bud deployments, and on phone and room calls."""
        flags = {"infer_injection_channel": True, "native_duplex_criteria": True}
        if caps["kind"] != "cascade":
            try:
                self.auto_regime(caps, 5, flags)
            except Refusal:
                return {"non_bud": None, "bud": None, "phone_and_rooms": None}
        avail = caps.get("available_from_release") or 0

        def first(pred):
            best = None
            for surf in self.data["surfaces"]:
                if surf["kind"] == "refused" or surf["first_release"] is None or not pred(surf):
                    continue
                ok, _, _ = self.reach(surf, caps)
                if not ok:
                    continue
                r = max(surf["first_release"], avail)
                best = r if best is None else min(best, r)
            return best

        out = {"non_bud": first(lambda s: s["bud_mode"] != "only"),
               "bud": first(lambda s: s["bud_mode"] != "refused"),
               "phone_and_rooms": first(lambda s: s["id"] in ("livekit", "sip", "ws_realtime_voice"))}
        if caps["kind"] == "cascade":
            out["phone_and_rooms_if_echo_gates_fail"] = self.surfaces["livekit"]["first_release_if_echo_gates_fail"]
        return out


# ====================================================================== merging the homes (W6 3.10.2)
def merge_interaction(layers):
    """Precedence lowest to highest; objects merge key by key, arrays and scalars replace.
    `layers` is a list of (source_name, value); returns (merged, sources_used)."""
    merged, used = None, []
    for name, value in layers:
        if value is None or value is False:
            continue
        if value is True:
            value = {}
        used.append(name)
        merged = _merge(merged or {}, value)
    return merged, used


def _merge(a, b):
    out = copy.deepcopy(a)
    for k, v in b.items():
        if isinstance(v, dict) and isinstance(out.get(k), dict):
            out[k] = _merge(out[k], v)
        else:
            out[k] = copy.deepcopy(v)
    return out


def leaf_paths(node, prefix=""):
    if isinstance(node, dict) and node:
        for k, v in node.items():
            yield from leaf_paths(v, prefix + "." + k if prefix else k)
    else:
        yield prefix


# ====================================================================== a minimal JSON Schema validator
class MiniValidator:
    """The subset of draft 2020-12 the plan's schemas use: type (incl. lists), enum, const,
    properties, required, additionalProperties (bool or schema), items, minItems, maxItems,
    uniqueItems, minimum, maximum, maxLength, pattern, $ref (local #/$defs/...), allOf, anyOf,
    oneOf, not, boolean schemas. Annotations (description, default, x-*) are ignored."""

    TYPES = {"object": dict, "array": list, "string": str, "boolean": bool, "null": type(None)}

    def __init__(self, root):
        self.root = root

    def _resolve(self, ref):
        if not ref.startswith("#/"):
            raise ValueError("only local refs are supported: %s" % ref)
        node = self.root
        for part in ref[2:].split("/"):
            node = node[part]
        return node

    def _is_type(self, inst, t):
        if t == "boolean":
            return isinstance(inst, bool)
        if t == "integer":
            return isinstance(inst, int) and not isinstance(inst, bool)
        if t == "number":
            return isinstance(inst, (int, float)) and not isinstance(inst, bool)
        return isinstance(inst, self.TYPES[t])

    def errors(self, inst, schema=None, path="$"):
        schema = self.root if schema is None else schema
        if schema is True:
            return []
        if schema is False:
            return ["%s: not allowed" % path]
        errs = []
        if "$ref" in schema:
            errs += self.errors(inst, self._resolve(schema["$ref"]), path)
        if "type" in schema:
            ts = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
            if not any(self._is_type(inst, t) for t in ts):
                return errs + ["%s: expected type %s, got %r" % (path, ts, type(inst).__name__)]
        if "enum" in schema and inst not in schema["enum"]:
            errs.append("%s: %r not in %r" % (path, inst, schema["enum"]))
        if "const" in schema and inst != schema["const"]:
            errs.append("%s: %r is not %r" % (path, inst, schema["const"]))
        if isinstance(inst, dict):
            props = schema.get("properties", {})
            for k in schema.get("required", []):
                if k not in inst:
                    errs.append("%s: missing required %r" % (path, k))
            for k, v in inst.items():
                if k in props:
                    errs += self.errors(v, props[k], "%s.%s" % (path, k))
                elif "additionalProperties" in schema:
                    errs += self.errors(v, schema["additionalProperties"], "%s.%s" % (path, k))
        if isinstance(inst, list):
            if "items" in schema:
                for i, v in enumerate(inst):
                    errs += self.errors(v, schema["items"], "%s[%d]" % (path, i))
            if "minItems" in schema and len(inst) < schema["minItems"]:
                errs.append("%s: fewer than %d items" % (path, schema["minItems"]))
            if "maxItems" in schema and len(inst) > schema["maxItems"]:
                errs.append("%s: more than %d items" % (path, schema["maxItems"]))
            if schema.get("uniqueItems"):
                seen = [json.dumps(x, sort_keys=True) for x in inst]
                if len(seen) != len(set(seen)):
                    errs.append("%s: items not unique" % path)
        if isinstance(inst, (int, float)) and not isinstance(inst, bool):
            if "minimum" in schema and inst < schema["minimum"]:
                errs.append("%s: %r < %r" % (path, inst, schema["minimum"]))
            if "maximum" in schema and inst > schema["maximum"]:
                errs.append("%s: %r > %r" % (path, inst, schema["maximum"]))
        if isinstance(inst, str):
            if "pattern" in schema and not re.search(schema["pattern"], inst):
                errs.append("%s: %r does not match %r" % (path, inst, schema["pattern"]))
            if "maxLength" in schema and len(inst) > schema["maxLength"]:
                errs.append("%s: longer than %d" % (path, schema["maxLength"]))
        for sub in schema.get("allOf", []):
            errs += self.errors(inst, sub, path)
        if "anyOf" in schema and not any(not self.errors(inst, s, path) for s in schema["anyOf"]):
            errs.append("%s: matches none of anyOf" % path)
        if "oneOf" in schema and sum(1 for s in schema["oneOf"] if not self.errors(inst, s, path)) != 1:
            errs.append("%s: does not match exactly one of oneOf" % path)
        if "not" in schema and not self.errors(inst, schema["not"], path):
            errs.append("%s: matches a 'not' schema" % path)
        return errs


def schema_errors(root, instance, def_name=None):
    """Validate with jsonschema when importable (draft 2020-12) and always with MiniValidator;
    return (errors, engines_agree)."""
    sub = {"$ref": "#/$defs/%s" % def_name} if def_name else None
    mini = MiniValidator(root).errors(instance, sub)
    try:
        import jsonschema  # noqa: F401
        from jsonschema import Draft202012Validator
        wrapper = dict(root)
        if def_name:
            wrapper = {"$schema": root.get("$schema"), "$defs": root["$defs"], "$ref": "#/$defs/%s" % def_name}
        js = [e.message for e in Draft202012Validator(wrapper).iter_errors(instance)]
        return (js or mini), (bool(js) == bool(mini))
    except ImportError:
        return mini, True


# ====================================================================== the matrix (PROVIDER_COVERAGE.md)
def _rel(r):
    return "never" if r is None else ("R%d" % r)


CONFIRM_LABELS = {
    "exact_waav_tts": "exact words, WaaV voice",
    "native_exact": "native exact speech, provider voice",
    "gateway_clip_same_voice": "gateway clip, same voice",
    "client_ui": "client UI",
    "gateway_clip_different_voice": "gateway clip, different voice (introduced)",
    "unanswerable_no_verbatim_channel": "unanswerable by voice",
    "say_guided_exact_words_checked": "guided speech, exact-words check",
}
RELAY_LABELS = {
    "verbatim": "relays verbatim (WaaV voice), streamed per sentence",
    "exact_words_checked": "relays guided, exact-words check, first sentence then one batch",
    "verbatim_provider_voice": "relays exact words in the provider voice, first sentence then batches",
    "gateway_clip": "relays as gateway clips in the agent voice",
    "verified_paraphrase": "results whole, reworded by the provider (verified paraphrase)",
    "none": "none",
}


def _cell(text):
    """Escape a value for a Markdown table cell."""
    return str(text).replace("|", "\\|")


def matrix(res):
    data = res.data
    lines = []
    w = lines.append
    w("Generated from provider_capabilities.json by `python3 regime_resolve.py --matrix` "
      "(regime rule evaluated at %s; B and C rows assume the provider is listed in "
      "`interaction.speech.truth_ack`)." % REFERENCE_DATE)
    w("")
    w("### 1. Default regime and experience per provider and model")
    w("")
    w("Fillers, listening sounds and idle prompts name who makes them (`gateway`, `provider` or `none`). "
      "Confirmations are for an action that cannot be undone, on a session with a screen and on a phone call.")
    w("")
    w("| Provider and model | Regime (truth_ack listed) | With the default empty truth_ack | Front | Fillers / listening sounds / idle | Voice | Truth and relay | Irreversible confirmation: screen / phone | First release: non-Bud / Bud / phone and rooms |")
    w("|---|---|---|---|---|---|---|---|---|")
    for caps in data["providers"]:
        e = res.compute_expected(caps)
        a, n = e["auto_with_truth_ack"], e["auto_without_truth_ack"]
        rel = e["interaction_release"]
        if "refusal" in a:
            row = [caps["display_name"], "refused: `%s`" % a["refusal"], "refused", "-", "-", "-", "-", "-"]
        else:
            model = caps["default_model"] if caps["kind"] != "cascade" else ""
            if caps["id"] == "openai.gpt-realtime-2":
                model = "gpt-realtime-2.1"
            surf = res.surfaces["ws_conversation" if caps["kind"] == "cascade" else "ws_realtime_voice"]
            ack1 = {"truth_ack": [caps["provider_id"]]}
            d = res.select_regime(surf, caps, model or "", {}, ack1, REFERENCE_DATE, 5, {})
            dp = res.select_regime(surf, caps, model or "", {}, ack1, REFERENCE_DATE, 5, {"client_ui": False})
            label = a["regime"] + (" (%s)" % a["variant"] if a.get("variant") and a["variant"] != "full" else "")
            no_ack = "same" if "refusal" not in n else "interaction off: `%s`%s" % (
                n["refusal"], " (alternative: `%s`)" % n["alternative"] if n.get("alternative") else "")
            truth = "%s; %s" % (d["truth"], RELAY_LABELS[caps["truth"]["relay"] if d["truth"] == "monitored" else "verbatim"])
            row = [caps["display_name"], label, no_ack, a["front"].replace("_", " "),
                   "%s / %s / %s" % (a["filler_owner"], a["backchannel_owner"], a["idle_owner"]),
                   "WaaV TTS" if d["voice_owner"] == "gateway" else "provider",
                   truth, "%s / %s" % (CONFIRM_LABELS[d["confirm_irreversible"]],
                                        CONFIRM_LABELS[dp["confirm_irreversible"]])]
        phone = _rel(rel["phone_and_rooms"])
        if rel.get("phone_and_rooms_if_echo_gates_fail"):
            phone += " (R%d if the echo gates fail)" % rel["phone_and_rooms_if_echo_gates_fail"]
        row.append("%s / %s / %s" % (_rel(rel["non_bud"]), _rel(rel["bud"]), phone))
        w("| " + " | ".join(row) + " |")
    w("")
    w("### 2. Operator and client overrides")
    w("")
    w("| Provider and model | `speech.voice_owner: gateway` | `speech.truth: enforced` | `regime: cascade` | `regime: provider_speech` | `regime: delegating` |")
    w("|---|---|---|---|---|---|")
    for caps in data["providers"]:
        if caps["kind"] == "cascade":
            continue
        e = res.compute_expected(caps)

        def fmt(o):
            if "refusal" in o:
                return "refused: `%s`" % o["refusal"] + (" (+`%s`)" % o["warning"] if o.get("warning") else "")
            variant = o.get("variant") or ("half_cascade" if o["regime"] == "Cascade" else None)
            s = o["regime"] + (" (%s)" % variant if variant else "")
            if o.get("warning"):
                s += " + `%s`" % o["warning"]
            if o.get("downgraded"):
                s += " + `regime_downgraded`"
            return s
        f = e["forced"]
        w("| %s | %s | %s | %s | %s | %s |" % (caps["display_name"], fmt(e["voice_owner_gateway"]),
                                               fmt(e["truth_enforced"]), fmt(f["cascade"]),
                                               fmt(f["provider_speech"]), fmt(f["delegating"])))
    w("")
    w("### 3. Owner values: what a requested owner becomes (default regime)")
    w("")
    w("`x` means the request is overridden to `x` with `<kind>_owner_overridden`.")
    w("")
    w("| Provider and model | filler: gateway / provider / none | listening sounds: gateway / provider / none | idle: gateway / provider / none |")
    w("|---|---|---|---|")
    for caps in data["providers"]:
        e = res.compute_expected(caps)
        if "refusal" in e["auto_with_truth_ack"]:
            continue
        regime = e["auto_with_truth_ack"]["regime"]
        cells = []
        for kind in OWNER_KINDS:
            parts = []
            for req in ("gateway", "provider", "none"):
                r_, warn, _ = res.resolve_owner(caps if regime != "Cascade" else res.cascade, kind, req,
                                                regime, "full" if regime == "Cascade" else None,
                                                caps["default_model"] or "", {})
                parts.append(req if not warn else "`%s`" % r_)
            cells.append(" / ".join(parts))
        w("| %s | %s |" % (caps["display_name"], " | ".join(cells)))
    w("")
    w("### 4. Surfaces")
    w("")
    w("| Surface | Kind | First release with interaction | Bud mode | Reaches | Egress | Caller switches to an unsupported language mid-call |")
    w("|---|---|---|---|---|---|---|")
    for s in data["surfaces"]:
        fr = _rel(s["first_release"]) if s["kind"] != "refused" else "refused: `%s`" % s["refusal"]["code"]
        if s.get("first_release_if_echo_gates_fail"):
            fr += " if the echo gates pass, else R%d" % s["first_release_if_echo_gates_fail"]
        reaches = "cascade" if s["kind"] == "cascade" else (s.get("reaches") if isinstance(s.get("reaches"), str)
                                                              else ", ".join(s.get("reaches", [])) or "-")
        lang = "-" if s["kind"] == "refused" else res.mid_call_language(s)
        w("| %s | %s | %s | %s | %s | %s | %s |" % (s["name"], s["kind"], fr, s["bud_mode"], reaches or "-",
                                                    s["egress"], lang))
    w("")
    w("### 5. Lifecycle dates (Lifecycle track; evaluated against an injected clock)")
    w("")
    w("| Provider or model | Warning from | Shutdown (refused from) | Replacement | Codes |")
    w("|---|---|---|---|---|")
    ref = _date(REFERENCE_DATE)

    def warn_from(item):
        if item["warning_from"]:
            return item["warning_from"]
        return "already past" if item["shutdown"] and _date(item["shutdown"]) <= ref else "first lifecycle patch"
    for caps in data["providers"]:
        for rule in caps["lifecycle"]["rules"]:
            w("| %s, models matching `%s` | %s | %s | %s | `%s`, `%s` |" % (
                caps["display_name"], _cell(rule["models"]), warn_from(rule),
                rule["shutdown"], _cell(rule["replacement"] or "none"), rule["warning_code"], rule["refusal_code"]))
        for rel in caps["lifecycle"].get("related", []):
            if rel["shutdown"]:
                w("| %s | %s | %s | %s | `%s`, `%s` |" % (
                    _cell(rel["item"]), warn_from(rel), rel["shutdown"], _cell(rel["replacement"] or "none"),
                    rel.get("warning_code", "model_retiring"), rel.get("refusal_code", "model_retired")))
    w("")
    w("### 6. Exceptions that need sign-off")
    w("")
    w("| Exception | Release or refusal | Sign-off (INTEGRATION_DECISIONS.md section 11) |")
    w("|---|---|---|")
    for x in data["coverage_exceptions"]:
        w("| %s | %s | %s |" % (x["exception"], x["release_or_refusal"], x["sign_off"]))
    return "\n".join(lines) + "\n"


def write_coverage(res, path=COVERAGE_PATH):
    with open(path, encoding="utf-8") as f:
        text = f.read()
    start, end = text.index(BEGIN_MARK), text.index(END_MARK)
    new = text[:start + len(BEGIN_MARK)] + "\n" + matrix(res) + text[end:]
    with open(path, "w", encoding="utf-8") as f:
        f.write(new)


def coverage_is_current(res, path=COVERAGE_PATH):
    with open(path, encoding="utf-8") as f:
        text = f.read()
    start, end = text.index(BEGIN_MARK), text.index(END_MARK)
    return text[start + len(BEGIN_MARK):end].strip() == matrix(res).strip()


# ====================================================================== validation of the files
def wire_examples(path=WIRE_EVENTS_PATH):
    with open(path, encoding="utf-8") as f:
        text = f.read()
    blocks = re.findall(r"```json\n(.*?)\n```", text, re.S)
    return [json.loads(b) for b in blocks], text


def check_wire_example(obj):
    """Return a list of problems for one wire example (values from the shared vocabularies)."""
    probs = []
    t = obj.get("type", "")
    names = {k: v for k, v in WIRE_EVENTS.items()}
    bud_to_ws = {v[0]: k for k, v in WIRE_EVENTS.items() if v[0]}
    base = t if t in names else bud_to_ws.get(t)
    if base is None and t not in ("mcp_approval_request", "conversation.item.create", "error", "transcript", "warning", "session_created"):
        probs.append("unknown event type %r" % t)
    if "state" in obj and obj["state"] not in TASK_STATES:
        probs.append("state %r" % obj["state"])
    if "effects" in obj and obj["effects"] is not None:
        e = obj["effects"]
        if not (e in EFFECTS_WORDS or (isinstance(e, list) and all(isinstance(x, str) for x in e))):
            probs.append("effects %r" % (e,))
    if base == "task_answer_received" and obj.get("by") not in ANSWER_BY:
        probs.append("by %r" % obj.get("by"))
    if base in ("task_answer_received", "task_answer") and obj.get("outcome") not in OUTCOMES:
        probs.append("outcome %r" % obj.get("outcome"))
    if base == "task_question" and obj.get("kind") not in HITL_KINDS:
        probs.append("question kind %r" % obj.get("kind"))
    if base == "assistant_transcript" or t == "transcript":
        if obj.get("act") is not None and obj["act"] not in ACTS:
            probs.append("act %r" % obj["act"])
        if obj.get("kind") is not None and obj["kind"] not in UTTERANCE_KINDS:
            probs.append("utterance kind %r" % obj["kind"])
        if obj.get("source") is not None and obj["source"] not in TRANSCRIPT_SOURCES:
            probs.append("source %r" % obj["source"])
        if obj.get("act") is not None and obj.get("source") != "front":
            probs.append("act set on a line the front did not write")
    if base == "overlap" and obj.get("class") is not None and obj["class"] not in INTERRUPTION_CLASSES:
        probs.append("class %r" % obj["class"])
    if base == "floor_state" and obj.get("reason") not in FLOOR_REASONS:
        probs.append("floor reason %r" % obj.get("reason"))
    # Addendum C4: task_cancelled is sent only for W3's confirmed Cancelled; a CancelRequested or
    # CancelUnconfirmed is task_progress with cancelling true and the state unchanged.
    if base == "task_cancelled" and obj.get("state") != "cancelled":
        probs.append("task_cancelled state must be cancelled (an unconfirmed cancel is task_progress)")
    if base == "task_cancelled" and obj.get("cancelling"):
        probs.append("task_cancelled is sent only once the cancel is confirmed (cancelling false)")
    if base and base.startswith("task_") and obj.get("cancelling") and obj.get("state") not in LIVE_STATES:
        probs.append("cancelling true with a terminal state %r" % obj.get("state"))
    if base == "assistant_transcript" or t == "transcript":
        if "chunk" in obj:
            probs.append("assistant_transcript uses sentence {attempt, seq}, not chunk (Addendum C1)")
        sen = obj.get("sentence")
        if sen is not None and not (isinstance(sen, dict) and set(sen) == {"attempt", "seq"}):
            probs.append("sentence must be {attempt, seq}")
    if base == "task_progress" and obj.get("answer_stopped") is not None:
        a = obj["answer_stopped"]
        if a.get("reason") not in ANSWER_STOP_REASONS or "released_sentences" not in a:
            probs.append("answer_stopped must be {reason in W3's AnswerStopReason, released_sentences}")
    if base == "error" and "code" in obj and not isinstance(obj["code"], str):
        probs.append("error code must be a string")
    if base and base.startswith("task_") and WIRE_EVENTS[base][1] == "out" and "state" not in obj:
        probs.append("task event without state")
    if base and base.startswith("task_") and WIRE_EVENTS[base][1] == "out" and "cancelling" not in obj:
        probs.append("task event without cancelling")
    return probs


def validate_all(res, verbose=True):
    """Validate every artefact; return a list of failure strings (empty when all pass)."""
    fails = []
    caps_schema = load_json(CAPS_SCHEMA_PATH)
    errs, agree = schema_errors(caps_schema, res.data)
    if errs:
        fails.append("provider_capabilities.json: %s" % errs[:5])
    if not agree:
        fails.append("validators disagree on provider_capabilities.json")
    cfg_schema = load_json(CONFIG_SCHEMA_PATH)
    manifest = load_json(os.path.join(EXAMPLES_DIR, "manifest.json"))
    for ex in manifest["examples"]:
        doc = load_json(os.path.join(EXAMPLES_DIR, ex["file"]))
        errs, agree = schema_errors(cfg_schema, doc, ex["def"])
        if ex["valid"] and errs:
            fails.append("example %s should be valid: %s" % (ex["file"], errs[:5]))
        if not ex["valid"] and not errs:
            fails.append("example %s should be invalid" % ex["file"])
        if not agree:
            fails.append("validators disagree on example %s" % ex["file"])
        if verbose:
            print("  example %-48s %s" % (ex["file"], "valid" if not errs else "invalid (as expected)"
                                          if not ex["valid"] else "INVALID"))
    wex, text = wire_examples()
    for obj in wex:
        p = check_wire_example(obj)
        if p:
            fails.append("wire example %s: %s" % (obj.get("type"), p))
    present = {o.get("type") for o in wex}
    for name, (bud, _) in WIRE_EVENTS.items():
        if name not in present:
            fails.append("wire_events.md has no example for %s" % name)
        if bud and bud not in present:
            fails.append("wire_events.md has no example for %s" % bud)
        if "`%s`" % name not in text:
            fails.append("wire_events.md does not name %s" % name)
    if verbose:
        print("  wire_events.md: %d JSON examples checked" % len(wex))
    return fails


# ====================================================================== self-test
def self_test(res):
    cases = []

    def case(name, fn):
        cases.append((name, fn))

    def S(**kw):
        base = {"release": 5, "today": REFERENCE_DATE, "interaction": {}, "has_background": True,
                "server": {"enabled": True, "build_features": True, "truth_ack": [],
                           "front_model": {"licence": "lfm_open_eligible"}}}
        for k, v in kw.items():
            if k == "server":
                base["server"].update(v)
            else:
                base[k] = v
        return res.resolve_session(base)

    def ack(*names):
        return {"truth_ack": list(names)}

    def eq(a, b, what=""):
        if a != b:
            raise AssertionError("%s: got %r, expected %r" % (what, a, b))

    # --- cascade surfaces and release gating
    def t_cascade_surfaces():
        for sid, rel, bud in (("ws_conversation", 1, False), ("ws_agent", 1, True), ("ws_raw", 1, False),
                              ("v1_realtime_cascade", 2, True), ("ws_dag_text", 3, False)):
            r = S(surface=sid, release=rel, bud_mode=bud)
            eq((r["interaction"], r["regime"], r["front"], r["truth"]), ("on", "Cascade", "local", "enforced"), sid)
            eq((r["filler_owner"], r["backchannel_owner"], r["idle_owner"]), ("gateway",) * 3, sid)
    case("regime_cascade_for_ws_conversation_agent_and_dag", t_cascade_surfaces)

    def t_release_gating():
        eq(S(surface="ws_dag_text", release=2)["refusal"], "interaction_surface_unavailable", "dag text R2")
        eq(S(surface="v1_realtime_cascade", release=1, bud_mode=True)["refusal"], "interaction_surface_unavailable", "GA R1")
        eq(S(surface="realtime", provider="grok", release=2)["refusal"], "interaction_surface_unavailable", "realtime R2")
        eq(S(surface="ws_realtime_voice", provider="grok", release=3)["refusal"], "interaction_surface_unavailable", "ws voice R3")
        eq(S(surface="ws_conversation", release=0)["refusal"], "interaction_surface_unavailable", "R0 no interaction")
    case("release_gating_refuses_surfaces_before_their_release", t_release_gating)

    def t_livekit_echo_gates():
        eq(S(surface="livekit", release=1)["refusal"], "interaction_surface_unavailable", "gates fail R1")
        eq(S(surface="livekit", release=2)["regime"], "Cascade", "gates fail R2")
        eq(S(surface="sip", release=1, echo_gates_pass=True)["regime"], "Cascade", "gates pass R1")
    case("livekit_and_sip_in_release_1_only_if_echo_gates_pass", t_livekit_echo_gates)

    def t_dag_audio():
        r = S(surface="ws_dag_audio", release=3)
        eq((r["interaction"], r["refusal"], r["fatal"]), ("off", "interaction_dag_audio_output", False), "dag audio")
        eq(S(surface="dag_realtime_node", provider="gemini")["refusal"], "interaction_dag_audio_output", "dag node")
    case("audio_output_dags_refused_with_interaction_dag_audio_output", t_dag_audio)

    # --- the regime arms
    def t_delegating():
        for p, m in (("gpt_live", None), ("gemini", "gemini-3.8-live"), ("gemini", "gemini-3.8-live-extended-thinking"),
                     ("gemini", "gemini-3.1-flash-live-preview"), ("nova_sonic", None)):
            r = S(surface="realtime", provider=p, model=m, server=ack(p))
            eq((r["regime"], r["front"], r["truth"]), ("Delegating", "provider", "monitored"), "%s %s" % (p, m))
    case("regime_delegating_for_gpt_live_gemini_38_gemini_31_and_nova2", t_delegating)

    def t_provider_speech():
        for p in ("grok", "inworld", "deepgram", "ultravox", "elevenlabs", "openai", "azure", "yandex"):
            r = S(surface="realtime", provider=p, server=ack(p))
            eq((r["regime"], r["front"]), ("ProviderSpeech", "narrator_templates"), p)
    case("regime_provider_speech_for_grok_inworld_deepgram_ultravox_elevenlabs_openai", t_provider_speech)

    def t_half_cascade():
        r = S(surface="realtime", provider="openai", model="gpt-realtime-2.1",
              interaction={"speech": {"voice_owner": "gateway"}})
        eq((r["regime"], r["variant"], r["truth"], r["voice_owner"]), ("Cascade", "half_cascade", "enforced", "gateway"), "openai")
        r = S(surface="realtime", provider="grok", interaction={"speech": {"voice_owner": "gateway"}}, server=ack("grok"))
        eq((r["regime"], "voice_owner_unavailable" in r["warnings"]), ("ProviderSpeech", True), "grok")
        r = S(surface="realtime", provider="ultravox", interaction={"speech": {"voice_owner": "gateway"}}, server=ack("ultravox"))
        eq(r["regime"], "ProviderSpeech", "ultravox has text output but no reply switch")
    case("regime_openai_family_half_cascade_when_voice_owner_gateway", t_half_cascade)

    def t_truth_enforced():
        r = S(surface="v1_realtime_relay", provider="azure", bud_mode=True, interaction={"speech": {"truth": "enforced"}})
        eq((r["regime"], r["variant"]), ("Cascade", "half_cascade"), "azure")
        r = S(surface="realtime", provider="gemini", interaction={"speech": {"truth": "enforced"}}, server=ack("gemini"))
        eq((r["refusal"], r["fatal"]), ("truth_enforcement_unavailable", True), "gemini")
    case("truth_enforced_selects_half_cascade_or_refuses", t_truth_enforced)

    def t_hume():
        r = S(surface="realtime", provider="hume", today="2026-11-12", server=ack("hume"))
        eq((r["refusal"], "provider_sunset" in r["warnings"]), ("interaction_unsupported", True), "before")
        r = S(surface="realtime", provider="hume", today="2026-11-13", interaction=None)
        eq((r["refusal"], r["fatal"]), ("provider_retired", True), "after, even with interaction off")
    case("regime_refuses_hume_after_2026_11_13_and_warns_before", t_hume)

    def t_infer():
        r = S(surface="realtime", provider="infer", release=5, server=ack("infer"))
        eq((r["refusal"], r["fatal"]), ("interaction_unsupported", False), "no channel")
        r = S(surface="realtime", provider="infer", release=5, server=ack("infer"),
              infer_injection_channel=True, native_duplex_criteria=True)
        eq((r["regime"], r["front"], r["truth"]), ("NativeDuplex", "provider", "monitored"), "with channel")
        r = S(surface="realtime", provider="infer", release=5, server=ack(),
              infer_injection_channel=True, native_duplex_criteria=True)
        eq(r["refusal"], "truth_ack_required", "native duplex still needs the acknowledgement")
        eq(S(surface="realtime", provider="infer", release=4)["refusal"], "unknown_provider", "not before R5")
    case("regime_refuses_infer_native_duplex_without_injection_channel", t_infer)

    def t_forced():
        r = S(surface="realtime", provider="grok", interaction={"regime": "cascade"}, server=ack("grok"))
        eq((r["regime"], "regime_downgraded" in r["warnings"]), ("ProviderSpeech", True), "grok cascade")
        r = S(surface="realtime", provider="openai", interaction={"regime": "delegating"}, server=ack("openai"))
        eq((r["regime"], r["filler_owner"], r["idle_owner"]), ("Delegating", "provider", "provider"), "openai delegating")
        r = S(surface="realtime", provider="elevenlabs", interaction={"regime": "delegating"}, server=ack("elevenlabs"))
        eq((r["regime"], "regime_downgraded" in r["warnings"]), ("ProviderSpeech", True), "elevenlabs")
        r = S(surface="realtime", provider="gemini", interaction={"regime": "provider_speech"}, server=ack("gemini"))
        eq((r["regime"], "regime_downgraded" in r["warnings"]), ("Delegating", True), "gemini")
        r = S(surface="realtime", provider="openai", interaction={"regime": "cascade"})
        eq((r["regime"], r["variant"]), ("Cascade", "half_cascade"), "openai forced cascade needs no ack")
    case("forced_regime_can_only_narrow_and_warns_regime_downgraded", t_forced)

    def t_gpt_live_surfaces():
        r = S(surface="v1_realtime_relay", provider="gpt_live", bud_mode=True)
        eq((r["refusal"], r["fatal"]), ("unsupported_vendor", True), "relay")
        r = S(surface="v1_realtime_facade", provider="gpt_live", bud_mode=True, interaction=None)
        eq((r["refusal"], r["fatal"]), ("unsupported_vendor", True), "facade, interaction off")
        r = S(surface="dag_realtime_node", provider="gpt_live")
        eq((r["refusal"], r["fatal"]), ("provider_not_turn_based", True), "dag node")
        r = S(surface="ws_realtime_voice", provider="gpt_live", bud_mode=True, release=4, server=ack("gpt_live"))
        eq(r["regime"], "Delegating", "Bud reaches GPT-Live through /ws in Release 4")
        eq(S(surface="realtime", provider="gpt_live", release=2)["refusal"], "unknown_provider", "adapter in R3")
    case("gpt_live_refused_on_v1_realtime_and_on_the_dag_realtime_node", t_gpt_live_surfaces)

    # --- truth acknowledgement
    def t_truth_ack():
        r = S(surface="realtime", provider="openai", model="gpt-realtime-2.1")
        eq((r["refusal"], r["fatal"], r["detail"]["alternative"]),
           ("truth_ack_required", False, "speech.truth: enforced"), "openai")
        r = S(surface="realtime", provider="grok")
        eq((r["refusal"], r["detail"]["alternative"], r["detail"]["path"]),
           ("truth_ack_required", None, "interaction.speech.truth_ack"), "grok")
        r = S(surface="realtime", provider="openai", model="gpt-realtime-2.1", server=ack("openai:gpt-realtime-2.1"))
        eq(r["regime"], "ProviderSpeech", "provider:model form")
        r = S(surface="realtime", provider="openai", model="gpt-realtime-2", server=ack("openai:gpt-realtime-2.1"))
        eq(r["refusal"], "truth_ack_required", "provider:model form covers that model only")
        eq(S(surface="ws_conversation", release=1)["regime"], "Cascade", "cascade needs no acknowledgement")
    case("truth_ack_required_unless_the_operator_lists_the_provider", t_truth_ack)

    # --- owners
    def t_owner_auto_by_model():
        eq(S(surface="realtime", provider="openai", model="gpt-realtime-2.1", server=ack("openai"))["filler_owner"],
           "provider", "2.x preambles")
        eq(S(surface="realtime", provider="openai", model="gpt-realtime-1.5", server=ack("openai"))["filler_owner"],
           "gateway", "older model")
    case("filler_owner_auto_is_provider_on_gpt_realtime_2x_preambles", t_owner_auto_by_model)

    def t_azure_model_family():
        r = S(surface="realtime", provider="azure", model="contoso-rt-prod", server=ack("azure"))
        eq((r["filler_owner"], "model_family_unknown" in r["warnings"]), ("gateway", True), "no model_family")
        r = S(surface="realtime", provider="azure", model="gpt-realtime-2.1", server=ack("azure"))
        eq((r["filler_owner"], "model_family_unknown" in r["warnings"]), ("gateway", True),
           "a deployment name that looks like a model is still not read")
        r = S(surface="realtime", provider="azure", model="contoso-rt-prod", model_family="gpt-realtime-2.1",
              server=ack("azure"))
        eq((r["filler_owner"], "model_family_unknown" in r["warnings"]), ("provider", False), "family 2.x")
        r = S(surface="realtime", provider="azure", model="contoso-rt-prod", model_family="gpt-realtime",
              server=ack("azure"))
        eq((r["filler_owner"], r["warnings"]), ("gateway", []), "older family")
        r = S(surface="realtime", provider="azure", model="contoso-rt-prod", server=ack("azure"),
              interaction={"filler_owner": "gateway"})
        eq("model_family_unknown" in r["warnings"], False, "an explicit owner needs no family")
        r = S(surface="realtime", provider="azure", model="contoso-rt-prod", interaction={"speech": {"voice_owner": "gateway"}})
        eq((r["regime"], "model_family_unknown" in r["warnings"]), ("Cascade", False), "half-cascade needs no family")
    case("azure_without_model_family_resolves_fillers_to_gateway_with_model_family_unknown", t_azure_model_family)

    def t_owner_overrides():
        r = S(surface="realtime", provider="gemini", interaction={"filler_owner": "gateway"}, server=ack("gemini"))
        eq((r["filler_owner"], "filler_owner_overridden" in r["warnings"]), ("provider", True), "gemini")
        r = S(surface="realtime", provider="openai", interaction={"backchannel_owner": "gateway"}, server=ack("openai"))
        eq((r["backchannel_owner"], "backchannel_owner_overridden" in r["warnings"]), ("none", True), "openai bc")
        r = S(surface="realtime", provider="inworld", interaction={"backchannel_owner": "provider"}, server=ack("inworld"))
        eq((r["backchannel_owner"], r["warnings"]), ("provider", []), "inworld bc")
        r = S(surface="realtime", provider="deepgram", interaction={"filler_owner": "provider"}, server=ack("deepgram"))
        eq((r["filler_owner"], "filler_owner_overridden" in r["warnings"]), ("none", True), "deepgram")
        r = S(surface="ws_conversation", release=1, interaction={"filler_owner": "provider"})
        eq((r["filler_owner"], "filler_owner_overridden" in r["warnings"]), ("gateway", True), "cascade")
    case("owner_values_the_provider_cannot_honour_are_overridden_with_a_warning", t_owner_overrides)

    def t_elevenlabs_operator_flag():
        r = S(surface="realtime", provider="elevenlabs", interaction={"filler_owner": "gateway"}, server=ack("elevenlabs"))
        eq((r["filler_owner"], "filler_owner_overridden" in r["warnings"]), ("provider", True), "no flag")
        r = S(surface="realtime", provider="elevenlabs", interaction={"filler_owner": "gateway"}, server=ack("elevenlabs"),
              operator_flags=["elevenlabs_fillers_off"])
        eq((r["filler_owner"], r["warnings"]), ("gateway", []), "operator turned soft timeout off")
    case("elevenlabs_gateway_fillers_only_when_the_operator_switched_the_agent_fillers_off", t_elevenlabs_operator_flag)

    def t_narrator():
        r = S(surface="realtime", provider="grok", interaction={"speech": {"narrator": "front_model"}}, server=ack("grok"))
        eq(r["front"], "narrator_front_model", "front")
        eq(sorted(set(ACTS) - set(r["masked_acts"])), sorted(NARROWED_ACTS), "masked to wait, progress, refuse")
    case("front_model_narrator_is_masked_to_wait_progress_refuse", t_narrator)

    # --- session gating
    def t_server_switch_and_build():
        eq(S(surface="ws_conversation", release=1, server={"enabled": False})["refusal"],
           "interaction_disabled_by_server", "switch")
        eq(S(surface="ws_conversation", release=1, server={"build_features": False})["refusal"],
           "interaction_missing_build_features", "build")
        r = S(surface="ws_conversation", release=1, interaction=None)
        eq((r["interaction"], r["refusal"], r["warnings"]), ("off", "interaction_absent", []), "absent block")
    case("server_switch_build_features_and_absent_block_turn_interaction_off", t_server_switch_and_build)

    def t_background():
        eq(S(surface="ws_raw", release=1, has_background=False)["refusal"], "interaction_needs_background", "raw")
        eq(S(surface="ws_raw", release=1)["regime"], "Cascade", "raw with background")
        eq(S(surface="realtime", provider="grok", has_background=False, server=ack("grok"))["refusal"],
           "interaction_needs_background", "provider session without a backend")
    case("raw_ws_and_provider_sessions_need_a_background", t_background)

    def t_bud_mode():
        eq(S(surface="realtime", provider="grok", bud_mode=True)["refusal"], "deployment_required", "realtime in Bud")
        eq(S(surface="v1_realtime_relay", provider="openai", bud_mode=False)["refusal"], "bud_mode_required", "v1 non-Bud")
        r = S(surface="v1_realtime_relay", provider="inworld", bud_mode=True)
        eq((r["refusal"], r["fatal"]), ("unsupported_vendor", True), "inworld not on the relay")
        r = S(surface="ws_realtime_voice", provider="inworld", bud_mode=True, release=4, server=ack("inworld"))
        eq(r["regime"], "ProviderSpeech", "inworld reachable through /ws in Release 4")
    case("bud_mode_surfaces_and_vendors_reached", t_bud_mode)

    def t_locale():
        eq(S(surface="ws_conversation", release=3, language="hi-IN")["refusal"], "interaction_locale_unsupported", "R3")
        eq(S(surface="ws_conversation", release=4, language="hi-IN")["regime"], "Cascade", "R4")
        eq(S(surface="ws_conversation", release=4, language="fr")["refusal"], "interaction_locale_unsupported", "fr")
    case("non_english_steps_down_until_the_hindi_packs_in_release_4", t_locale)

    def t_lenient():
        r = S(surface="ws_conversation", release=1, interaction={"front": {"licence": "liquid_commercial"}},
              server={"front_model": {}})
        eq(("interaction_override_refused" in r["warnings"], r["front_profile"]), (True, "scripted"), "licence refused")
        r = S(surface="ws_conversation", release=1, interaction={"regime": "native", "speech": {"truth_ack": ["x"]}})
        eq(("interaction_value_ignored" in r["warnings"], "interaction_override_refused" in r["warnings"], r["regime"]),
           (True, True, "Cascade"), "bad enum and server-only key")
    case("lenient_parse_refuses_server_only_keys_and_ignores_bad_values", t_lenient)

    def t_front_profile():
        r = S(surface="ws_conversation", release=1)
        eq((r["front_profile"], "front_model_scripted" in r["warnings"]), ("scripted", True),
           "Release 1: attested but nothing qualified (Addendum B1)")
        r = S(surface="ws_conversation", release=1, server={"front_model": {}})
        eq(r["front_profile"], "scripted", "no attestation, nothing qualified")
        r = S(surface="ws_conversation", release=2, server={"qualified_profiles": ["lfm-standard"]})
        eq((r["front_profile"], r["front_qualified"]), ("lfm-standard", True), "fine-tuned 1.2B qualified, attested")
        r = S(surface="ws_conversation", release=2, server={"qualified_profiles": ["lfm-large", "lfm-standard"]})
        eq(r["front_profile"], "lfm-standard", "the 1.2B is preferred when both qualify (Addendum B7)")
        r = S(surface="ws_conversation", release=2, server={"qualified_profiles": ["lfm-large"]})
        eq(r["front_profile"], "lfm-large", "only the 2.6B qualified")
        r = S(surface="ws_conversation", release=2,
              server={"front_model": {}, "qualified_profiles": ["lfm-standard", "apache-standard"]})
        eq(r["front_profile"], "apache-standard", "no attestation never runs LFM weights")
        r = S(surface="ws_conversation", release=1,
              server={"front_model": {"licence": "lfm_open_eligible", "profile": "lfm-standard", "allow_unqualified": True}})
        eq((r["front_profile"], r["front_qualified"]), ("lfm-standard", False), "operator-forced for evaluation")
        r = S(surface="ws_conversation", release=1, interaction={"front": {"profile": "lfm-standard"}},
              server={"front_model": {"profiles_enabled": ["auto", "lfm-standard"]}})
        eq((r["front_profile"], "front_profile_substituted" in r["warnings"]), ("scripted", True), "unlicensed request")
        r = S(surface="ws_conversation", release=1, interaction={"front": {"profile": "lfm-standard"}},
              server={"front_model": {"licence": "liquid_commercial", "profiles_enabled": ["auto", "lfm-standard"]}})
        eq((r["front_profile"], "front_profile_substituted" in r["warnings"]), ("scripted", True), "licensed, not qualified")
    case("front_profile_auto_follows_the_qualified_list_and_the_licence_attestation", t_front_profile)

    def t_half_cascade_owners():
        r = S(surface="realtime", provider="openai", model="gpt-realtime-2.1",
              interaction={"speech": {"voice_owner": "gateway"}, "filler_owner": "provider", "backchannel_owner": "gateway"})
        eq((r["regime"], r["variant"], r["voice_owner"], r["front"]), ("Cascade", "half_cascade", "gateway", "local"), "shape")
        eq((r["filler_owner"], r["backchannel_owner"], r["idle_owner"]), ("gateway", "none", "gateway"), "owners")
        eq(("filler_owner_overridden" in r["warnings"], "backchannel_owner_overridden" in r["warnings"]), (True, True), "warned")
        for p in ("grok", "inworld", "deepgram", "elevenlabs", "ultravox", "gemini", "nova_sonic"):
            caps, _ = res.lookup(p)
            eq(caps["ownership"]["half_cascade_possible"], False, "%s has no documented reply switch" % p)
    case("half_cascade_only_on_the_openai_family_with_waav_voice_and_gateway_none_gateway_owners", t_half_cascade_owners)

    def t_language():
        eq(S(surface="ws_conversation", release=1)["mid_call_unsupported_language"], STEP_DOWN, "conversation")
        eq(S(surface="ws_agent", release=1, bud_mode=True)["mid_call_unsupported_language"], STEP_DOWN, "agent")
        for sid, kw in (("ws_raw", {}), ("v1_realtime_cascade", {"bud_mode": True, "release": 2}),
                        ("realtime", {"provider": "gemini", "server": ack("gemini")}),
                        ("ws_realtime_voice", {"provider": "grok", "release": 4, "server": ack("grok")})):
            eq(S(surface=sid, **kw)["mid_call_unsupported_language"], REPEAT_REQUEST, sid)
        eq(S(surface="sip", release=2)["mid_call_unsupported_language"], AS_CONTROLLING, "sip")
    case("language_step_down_only_on_ws_conversation_and_agent_sessions", t_language)

    def t_server_only():
        for path, block in (("duplex.compute", {"duplex": {"compute": {"pool_threads": 4}}}),
                            ("tasks.task_backends", {"tasks": {"task_backends": []}}),
                            ("tasks.allow_client_push", {"tasks": {"allow_client_push": True}})):
            r = S(surface="ws_conversation", release=1, interaction=block)
            eq(("interaction_override_refused" in r["warnings"], r["regime"]), (True, "Cascade"), path)
        cfg, w = res.parse_block({"memory": {"enabled": True}})
        eq(("memory" in cfg, w), (True, []),
           "memory is not a server-only interaction key: its settings are front_model.memory (Addendum A2 item 3); "
           "the gateway reports interaction.memory as unknown_config_keys, which this file does not model")
    case("server_task_catalog_and_duplex_compute_are_refused_from_sessions", t_server_only)

    def t_wire_rules():
        base = {"task_id": "task_1", "task_ref": "t1", "revision": 1, "t_ms": 1}
        ok = dict(base, type="task_progress", state="working", cancelling=True)
        eq(check_wire_example(ok), [], "CancelUnconfirmed as task_progress")
        bad = dict(base, type="task_cancelled", state="unknown", cancelling=True, by="client", reason="x", effects="unknown")
        eq(len(check_wire_example(bad)) >= 2, True, "never task_cancelled with unknown or cancelling")
        eq(check_wire_example({"type": "floor_state", "holder": "none", "lane": None, "reason": "stream_gap", "t_ms": 1}), [],
           "stream_gap reason")
        eq(bool(check_wire_example({"type": "assistant_transcript", "turn_index": 1, "delta": "x", "chunk": {"index": 0}})),
           True, "chunk renamed to sentence")
        eq(check_wire_example({"type": "error", "message": "closed", "code": "question_closed"}), [], "error code")
    case("wire_rules_follow_the_task_event_mapping_of_w6_3_12_4", t_wire_rules)

    def t_bud_reach():
        for p in ("inworld", "ultravox", "yandex"):
            for sid in ("v1_realtime_relay", "v1_realtime_facade"):
                eq(S(surface=sid, provider=p, bud_mode=True)["refusal"], "unsupported_vendor", "%s %s" % (p, sid))
            eq(res.interaction_release(res.lookup(p)[0])["bud"], 4, "%s Bud from Release 4" % p)
        x = [e for e in res.data["coverage_exceptions"] if "Inworld, Ultravox or Yandex" in e["exception"]]
        eq((len(x), x[0]["sign_off"]), (1, "27"), "sign-off 27 (Addendum A3)")
    case("bud_reaches_inworld_ultravox_yandex_only_from_release_4_sign_off_27", t_bud_reach)

    def t_lifecycle():
        caps, model = res.lookup("openai", "")
        eq(model, "gpt-realtime", "empty model takes the default")
        eq(res.lifecycle_at(caps, model, "2026-10-21")["status"], "active", "before warning")
        eq(res.lifecycle_at(caps, model, "2026-10-22")["code"], "model_retiring", "warning")
        eq(res.lifecycle_at(caps, model, "2027-01-20")["code"], "model_retired", "shutdown")
        r = S(surface="realtime", provider="openai", today="2027-01-20", interaction=None)
        eq((r["refusal"], r["fatal"]), ("model_retired", True), "refused at construction")
        r = S(surface="realtime", provider="openai", model="gpt-realtime-2.1", today="2027-01-20", server=ack("openai"))
        eq(r["regime"], "ProviderSpeech", "replacement model unaffected")
        r = S(surface="realtime", provider="nova_sonic", model="amazon.nova-sonic-v1:0", interaction=None)
        eq(r["refusal"], "model_retired", "Nova Sonic v1 ended 2026-09-14")
    case("lifecycle_table_evaluated_against_an_injected_clock", t_lifecycle)

    def t_confirmations():
        r = S(surface="realtime", provider="grok", server=ack("grok"))
        eq(r["confirm_irreversible"], "native_exact", "grok")
        r = S(surface="realtime", provider="gemini", server=ack("gemini"))
        eq(r["confirm_irreversible"], "client_ui", "gemini with a screen")
        r = S(surface="ws_realtime_voice", provider="gemini", release=4, server=ack("gemini"), client_ui=False)
        eq(r["confirm_irreversible"], "gateway_clip_different_voice", "gemini on SIP")
        r = S(surface="ws_realtime_voice", provider="gemini", release=4, server=ack("gemini"), client_ui=False,
              interaction={"speech": {"voice_mismatch": "never"}})
        eq(r["confirm_irreversible"], "unanswerable_no_verbatim_channel", "no channel")
        r = S(surface="realtime", provider="elevenlabs", server=ack("elevenlabs"))
        eq(r["confirm_irreversible"], "gateway_clip_same_voice", "elevenlabs")
        r = S(surface="ws_realtime_voice", provider="openai", release=4, server=ack("openai"), client_ui=False)
        eq((r["confirm_irreversible"], r["confirm_reversible"]),
           ("gateway_clip_different_voice", "say_guided_exact_words_checked"), "openai on a phone")
    case("confirmation_path_follows_w6_3_5_1", t_confirmations)

    def t_ack_rule():
        eq(S(surface="realtime", provider="grok", server=ack("grok"))["ack"].startswith("later of E + 300"), True, "B gateway")
        eq(S(surface="realtime", provider="openai", model="gpt-realtime-2.1", server=ack("openai"))["ack"].startswith("none from the gateway: the provider's own"), True, "B provider")
        eq(S(surface="realtime", provider="gemini", server=ack("gemini"))["ack"].startswith("none from the gateway: the provider is the front"), True, "C")
        eq(S(surface="ws_conversation", release=1)["ack"].startswith("E + ack_due_ms"), True, "A")
    case("acknowledgement_timing_per_regime_follows_w6_3_18", t_ack_rule)

    def t_precedence():
        merged, used = merge_interaction([
            ("built_in", True),
            ("server_default", {"front": {"persona": "acme-support@3"}, "events": {"floor": True}}),
            ("alias:support-interactive", {"events": {"floor": False}, "hitl": {"channels": ["voice", "client"]}}),
            ("agent_entry", None),
            ("client", {"front": {"caller": {"verbosity": "brief"}}}),
        ])
        eq(used, ["built_in", "server_default", "alias:support-interactive", "client"], "sources")
        eq((merged["front"]["persona"], merged["front"]["caller"]["verbosity"], merged["events"]["floor"]),
           ("acme-support@3", "brief", False), "merge")
    case("bud_agent_interaction_from_server_default_alias_and_entry_precedence", t_precedence)

    # --- the data file against its own expectations
    for caps in res.data["providers"]:
        def t_expected(caps=caps):
            got = res.compute_expected(caps)
            for k, v in caps["expected"].items():
                eq(got[k], v, "%s.expected.%s" % (caps["id"], k))
        case("expected_block_matches_rule_for_%s" % caps["id"], t_expected)

    # --- validators and files
    def t_minivalidator():
        sch = {"$defs": {"x": {"type": "object", "additionalProperties": False, "required": ["a"],
                               "properties": {"a": {"type": "integer", "minimum": 1}, "b": {"anyOf": [{"const": "auto"}, {"type": "boolean"}]}}}},
               "$ref": "#/$defs/x"}
        mv = MiniValidator(sch)
        eq(mv.errors({"a": 2, "b": "auto"}), [], "valid")
        eq(bool(mv.errors({"a": 0})), True, "minimum")
        eq(bool(mv.errors({"a": 1, "c": 1})), True, "additionalProperties")
        eq(bool(mv.errors({"a": True})), True, "bool is not integer")
        eq(bool(mv.errors({"a": 1, "b": "x"})), True, "anyOf")
    case("fallback_validator_implements_the_keywords_used", t_minivalidator)

    def t_files():
        fails = validate_all(res, verbose=False)
        eq(fails, [], "validation")
    case("schemas_examples_and_wire_events_validate", t_files)

    def t_examples_resolve():
        manifest = load_json(os.path.join(EXAMPLES_DIR, "manifest.json"))
        server_doc = load_json(os.path.join(EXAMPLES_DIR, manifest["server_example"]))
        server = {"enabled": server_doc["interaction"]["enabled"], "build_features": True,
                  "truth_ack": _get(server_doc, "interaction.speech.truth_ack", []),
                  "front_model": server_doc.get("front_model", {}),
                  "qualified_profiles": manifest.get("qualified_profiles", [])}
        for ex in manifest["examples"]:
            if not ex.get("resolve"):
                continue
            doc = load_json(os.path.join(EXAMPLES_DIR, ex["file"]))
            rs = dict(ex["resolve"])
            layers = []
            for layer in rs.pop("layers", []):
                if layer == "server_default":
                    layers.append(("server_default", _get(server_doc, "interaction.defaults.agent")))
                elif layer.startswith("alias:"):
                    layers.append((layer, _get(server_doc, "aliases")[layer[6:]].get("interaction")))
                elif layer.startswith("document"):
                    name = layer.split(":", 1)[1] if ":" in layer else "client"
                    layers.append((name, _get(doc, ex["interaction_pointer"])))
            if layers:
                block, used = merge_interaction(layers)
                eq(used, ex["expect"].get("sources", used), "%s sources" % ex["file"])
            else:
                block = _get(doc, ex["interaction_pointer"]) if ex.get("interaction_pointer") else doc
            for p in leaf_paths(_get(doc, ex["interaction_pointer"]) or {}):
                if ex.get("session_overrides") is not None and "interaction." + p not in ex["session_overrides"]:
                    raise AssertionError("%s: client path interaction.%s not in session_overrides" % (ex["file"], p))
            rs["interaction"] = block
            rs["server"] = server
            r = res.resolve_session(rs)
            for k, v in ex["expect"].items():
                if k == "sources":
                    continue
                eq(r.get(k), v, "%s %s" % (ex["file"], k))
        upd = next(e for e in manifest["examples"] if e["def"] == "BudInteractionUpdate")
        patch = load_json(os.path.join(EXAMPLES_DIR, upd["file"]))["interaction"]
        allowed_mid = ("front.caller.verbosity", "front.caller.pace", "hitl.channels")
        for p in leaf_paths(patch):
            if not (p.startswith(allowed_mid) or p.startswith("policy.") or p.startswith("events.")):
                raise AssertionError("mid-session path %s not allowed (W6 3.10.5)" % p)
            if "interaction." + p not in upd["session_overrides"]:
                raise AssertionError("mid-session path %s not in the agent's session_overrides" % p)
    case("examples_resolve_to_the_regimes_their_manifest_states", t_examples_resolve)

    def t_coverage_current():
        eq(coverage_is_current(res), True, "PROVIDER_COVERAGE.md generated region is stale; run --write-coverage")
    case("provider_coverage_md_is_regenerated_from_the_current_data", t_coverage_current)

    failed = 0
    for name, fn in cases:
        try:
            fn()
            print("ok    %s" % name)
        except Exception as e:  # noqa: BLE001 - report every failure
            failed += 1
            print("FAIL  %s: %s" % (name, e))
    print("\n%d cases, %d passed, %d failed" % (len(cases), len(cases) - failed, failed))
    return failed == 0


# ====================================================================== command line
def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--matrix", action="store_true")
    ap.add_argument("--write-coverage", action="store_true")
    ap.add_argument("--validate", action="store_true")
    ap.add_argument("--resolve", metavar="JSON")
    ap.add_argument("--caps", default=CAPS_PATH)
    args = ap.parse_args(argv)
    res = Resolver(load_json(args.caps))
    if args.write_coverage:
        write_coverage(res)
        print("wrote the generated region of %s" % COVERAGE_PATH)
    if args.matrix:
        sys.stdout.write(matrix(res))
    if args.validate:
        fails = validate_all(res)
        print("\n".join(fails) if fails else "all files valid")
        if fails:
            return 1
    if args.resolve:
        print(json.dumps(res.resolve_session(json.loads(args.resolve)), indent=2))
    if args.self_test:
        return 0 if self_test(res) else 1
    if not any((args.self_test, args.matrix, args.write_coverage, args.validate, args.resolve)):
        ap.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())
