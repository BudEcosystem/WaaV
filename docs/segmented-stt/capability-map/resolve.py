#!/usr/bin/env python3
"""Reference resolver for the speech-to-text live capability map (schema version 2).

This is the executable specification of the gateway's Rust resolver: given a provider, a model, a
release, a kind of session and a transcription preference, it answers which transport and adapter a
live call gets, or which refusal, and the warnings that apply. It reads only
stt_live_capabilities.json, uses only the Python standard library, and never touches the network.

Rules come from design/W3-capability-map-and-resolution.md (sections 2.4 to 2.7, 2.10, 2.16, 3.5)
as bound by design/INTEGRATION_DECISIONS.md (sections 5, 6, 10, 12, 13 and addendum A1, A6, A7,
A10), and from the field descriptions of stt_capability_map.schema.json. Code names come from the
customer-contract design (W5 sections 3.8 and 3.9), which is the single source of codes.

Usage:
  python3 resolve.py PROVIDER MODEL [--release N] [--session gateway|push_to_talk|plain] [--latency-tier low_latency]
  python3 resolve.py --check-release-0
                     [--mode auto|streaming|segmented] [--language TAG] [--region NAME]
                     [--not-covered] [--bud-leg] [--underlying-model ID] [--profile NAME]
                     [--encoding NAME] [--evidence assume|map] [--json]
  python3 resolve.py --self-test
  python3 resolve.py --expected > EXPECTED_RESOLUTION.md
  python3 resolve.py --matrix > CAPABILITY_MATRIX.md

MODEL may be '' for "no model named".
"""
import argparse
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
MAP_PATH = os.path.join(HERE, "stt_live_capabilities.json")
import datetime
TODAY = datetime.date.today().isoformat()   # lifecycle dates are compared with the run date (--today overrides)
DEFAULT_DEADLINE_MS = 6000          # integration decisions section 2
SLOW_TARGET_MS = 2500               # integration decisions section 15 and A9
RELEASE_NAMES = {
    0: "Release 0, groundwork and honest refusal",
    1: "Release 1, first working calls",
    2: "Release 2, dark launch complete",
    3: "Release 3, default on",
    4: "Release 4, live-only models and low latency",
    5: "Release 5, wider vendor coverage and hardening",
    6: "Release 6, interruption recovery and interim text",
}
# Adapter kinds, from the adapter table of CONVERSION_RULES.md section 3.
FILE_ADAPTERS = {"openai_transcriptions", "groq_transcriptions", "azure_openai_transcriptions", "elevenlabs_batch",
                 "assemblyai_sync", "deepgram_prerecorded", "azure_fast_transcription", "google_recognize",
                 "speechmatics_batch", "regional_rest", "planned_file"}
COMMIT_ADAPTERS = {"openai_realtime_transcription", "cartesia_manual_finalize", "planned_commit"}
# Audio encodings the segmenting engine cannot frame for its detector (W3 section 2.10, "Audio format").
UNDECODABLE = {"flac", "opus", "ogg_opus", "webm_opus", "amr", "amr_wb", "mp3"}
# Release from which a language or region constraint on today's client is enforced (addendum A7).
NATIVE_CONSTRAINTS_FROM = 3
# Session kinds. "gateway": a voice agent whose turn detection is automatic (the gateway ends the
# caller's turn). "push_to_talk": a voice agent in manual mode, a conversation loop or a DAG session.
# "plain": a /ws session with no agent and no conversation loop; the client consumes transcripts itself.
# Addendum A1 refuses only the first kind on a buffering client; addendum B4 keeps "plain" sessions on
# today's buffering client even when covered, unless they ask for segmented.
SESSIONS = ("gateway", "push_to_talk", "plain")
SESSION_LABELS = {"gateway": "voice agent, automatic turns", "push_to_talk": "manual agent, conversation loop or DAG",
                  "plain": "plain /ws session"}
LATENCY_TIERS = ("standard", "low_latency")
MODES = ("auto", "streaming", "segmented")


# ---------------------------------------------------------------------------------------------
# Loading
# ---------------------------------------------------------------------------------------------
class CapabilityMap:
    def __init__(self, path=MAP_PATH):
        self.doc = json.load(open(path, encoding="utf-8"))
        self.providers = self.doc["providers"]
        self.profiles = self.doc.get("profiles", {})
        self.placeholders = {m.lower() for m in self.doc.get("sdk_placeholder_models", [])}
        self.rows_by_provider = {}
        for row in self.doc["rows"]:
            self.rows_by_provider.setdefault(row["match"]["provider"], []).append(row)
        self.global_row = self.rows_by_provider["*"][0]
        # Provider spellings: canonical ids and aliases, lowercased; and the same with '-' as '_'.
        self.names, self.flat_names = {}, {}
        for pid, entry in self.providers.items():
            for s in [pid] + list(entry.get("aliases", [])):
                self.names[s.strip().lower()] = pid
                self.flat_names[s.strip().lower().replace("-", "_")] = pid

    def provider_id(self, raw):
        s = (raw or "").strip().lower()
        return self.names.get(s) or self.flat_names.get(s.replace("-", "_"))

    def transports(self, row):
        """The row's transports with profile references replaced by the profile's transport."""
        out = []
        for t in row["transports"]:
            if "profile" in t:
                p = self.profiles[t["profile"]]
                out.append(dict(p["transport"], _profile=t["profile"]))
            else:
                out.append(t)
        return out


def glob_match(pattern, text):
    return re.fullmatch(".*".join(map(re.escape, pattern.lower().split("*"))), text.lower()) is not None


def literal_length(pattern):
    return len(pattern.replace("*", ""))


def row_key(row):
    m = row["match"]
    return m.get("model") or m.get("model_glob")


def adapter_kind(adapter):
    if adapter == "native":
        return "native"
    if adapter in FILE_ADAPTERS:
        return "segmented"
    if adapter in COMMIT_ADAPTERS:
        return "commit"
    return "stream_not_built"   # planned_stream


# ---------------------------------------------------------------------------------------------
# Row lookup (W3 section 2.4 and 2.5)
# ---------------------------------------------------------------------------------------------
def find_row(cmap, pid, lookup):
    """Exact model or alias, else the most specific anchored glob, else the provider default."""
    rows = cmap.rows_by_provider.get(pid, [])
    low = lookup.strip().lower()
    for row in rows:
        m = row["match"]
        if "model" in m and (m["model"].lower() == low or low in [a.lower() for a in m.get("model_aliases", [])]):
            return row, "exact"
    best = None
    for row in rows:
        g = row["match"].get("model_glob")
        if g and g != "*" and glob_match(g, low):
            if best is None or (literal_length(g), g) > (literal_length(best["match"]["model_glob"]), best["match"]["model_glob"]):
                best = row
    if best is not None:
        return best, "pattern"
    for row in rows:
        if row["match"].get("model_glob") == "*":
            return row, "provider_default"
    return cmap.global_row, "global_default"


def has_specific_row(cmap, pid, model):
    row, layer = find_row(cmap, pid, model)
    return layer in ("exact", "pattern")


# ---------------------------------------------------------------------------------------------
# Constraints and evidence
# ---------------------------------------------------------------------------------------------
def language_matches(code, lang):
    c, l = code.lower(), lang.lower()
    if c == l:
        return True
    if "-" not in c and l.split("-")[0] == c:      # a bare language covers every region of it
        return True
    if "-" not in l and c.split("-")[0] == l:      # a bare session language meets a regional code
        return True
    return False


def constraint_blocks(t, language, region):
    """Why the transport cannot serve this language or region, or None. An unknown language or
    region is not checked (the expected tables say "where the constraint admits the session")."""
    c = t.get("constraints") or {}
    lc = c.get("languages")
    if lc and language:
        hit = any(language_matches(code, language) for code in lc["codes"])
        if (lc["mode"] == "only" and not hit) or (lc["mode"] == "except" and hit):
            return "language_not_live"
    rc = c.get("regions")
    if rc and region:
        hit = region.lower() in [v.lower() for v in rc["values"]]
        if (rc["mode"] == "only" and not hit) or (rc["mode"] == "except" and hit):
            return "region_not_served"
    return None


def evidence_present(row, t, evidence, deadline_ms):
    need = t.get("enable_requires") or []
    if not need or evidence == "assume":
        return True
    prov = row.get("provenance", {})
    for req in need:
        if req == "live_probe" and not (prov.get("verified_by") == "live_probe" and prov.get("probe_ref")):
            return False
        if req == "latency_within_deadline":
            ok = any(m.get("quantity") in ("end_of_speech_to_final", "request_round_trip")
                     and m.get("percentile") in ("p95", "p99") and m.get("value_ms", 10 ** 9) <= deadline_ms
                     for m in (t.get("latency") or {}).get("measurements", []))
            if not ok:
                return False
    return True


def seed_p99(t):
    for m in (t.get("latency") or {}).get("measurements", []):
        if m.get("percentile") == "p99" and m.get("quantity") == "end_of_speech_to_final":
            return m.get("value_ms")
    return None


# ---------------------------------------------------------------------------------------------
# The resolver
# ---------------------------------------------------------------------------------------------
def warn(code, delivery, **detail):
    return {"code": code, "delivery": delivery, "detail": detail}


def native_delivery(release, covered):
    """Where a warning about today's client goes (W3 2.16, addendum A6): nowhere but the log and the
    counter in Release 0 and on a session the switch does not cover in Releases 1 and 2; a notice in
    ready.stt otherwise."""
    if release == 0 or (release < 3 and not covered):
        return "log"
    return "notice"


def resolve(cmap, provider, model, release=0, session="gateway", mode="auto", covered=None, language=None,
            region=None, bud_leg=False, underlying_model=None, profile=None, encoding="pcm_s16le",
            evidence="assume", deadline_ms=DEFAULT_DEADLINE_MS, today=None, latency_tier="standard"):
    """Resolve one live session. Returns a dict: outcome (native, segmented, commit or refused), the
    chosen transport, the refusal, the warnings with their delivery, and notes."""
    today = today or TODAY
    out = _resolve(cmap, provider, model, release, session, mode, covered, language, region, bud_leg,
                   underlying_model, profile, encoding, evidence, deadline_ms, today, latency_tier)
    # Delivery on today's client (W3 2.16, W5 3.8): a frame only for the buffering warning and for an
    # answer to the session's own preference; every other fact is a notice or only logged.
    if out.get("outcome") == "native":
        d = native_delivery(out["release"], out["covered"])
        for w in out["warnings"]:
            if w["code"] not in ("stt_buffered_until_commit", "stt_mode_unavailable"):
                w["delivery"] = d
        # On a session the engine does not cover, a substitution is used for the row lookup only and
        # today's client receives the string byte for byte (W3 2.4 and 3.5 step 8).
        raw = (model or "").strip()
        if not out["covered"] and out.get("layer") != "declared_default" and out.get("model_sent") != raw:
            out["model_sent"] = raw
            out["notes"].append("Not covered: the substitution served the row lookup only; today's client "
                                "receives the model string as given"
                                + (", and may reject it at setup as it does today." if any(
                                    w["code"] == "stt_placeholder_model_ignored" for w in out["warnings"]) else "."))
    return out


def _resolve(cmap, provider, model, release, session, mode, covered, language, region, bud_leg,
              underlying_model, profile, encoding, evidence, deadline_ms, today, latency_tier="standard"):
    assert 0 <= release <= 6 and session in SESSIONS and mode in MODES and latency_tier in LATENCY_TIERS
    if covered is None:
        covered = release >= 1
    covered = covered and release >= 1      # in Release 0 no session is covered
    notes, warnings = [], []
    if release == 0 and mode != "auto":
        notes.append("Release 0 has no transcription_mode; the preference is ignored.")
        mode = "auto"
    out = {"provider_input": provider, "model_input": model, "release": release, "session": session, "mode": mode,
           "covered": covered, "notes": notes, "warnings": warnings, "refusal": None, "transport": None}

    # 1. Provider.
    pid = cmap.provider_id(provider)
    if pid is None:
        row = cmap.global_row
        out.update(provider=(provider or "").strip().lower(), row_id=row["id"], layer="global_default",
                   model_sent=model, kind="native", outcome="native",
                   label="today's client through the plugin registry (unclassified provider)")
        notes.append("Unknown to the map: today's factory builds it unchanged and nothing new is reported. If the "
                     "registry does not know it either, a Bud leg is refused before admission and a standalone "
                     "session fails with today's unknown-provider error.")
        return out
    entry = cmap.providers[pid]
    out["provider"] = pid
    kn = entry.get("key_normalisation") or {}
    model_string = entry["model_string"]

    # 2. Model normalisation (W3 2.4). The lookup and the string sent change together.
    raw = (model or "").strip()
    sent = raw
    if model_string == "deployment_name":
        lookup = (underlying_model or raw).strip()
    else:
        lookup = raw
    sensitive = model_string == "sensitive"
    if model_string in ("absent", "sensitive"):
        lookup = ""
        if sensitive:
            out["model_input"] = "<not shown: this provider's model field is not a model>" if raw else ""
    if kn.get("region_suffix_separator") and lookup:
        sep = kn["region_suffix_separator"]
        if sep in lookup:
            lookup, suffix = lookup.split(sep, 1)
            if suffix and not region:
                region = suffix
            notes.append(f"Region suffix '{suffix}' taken from the model string; the string sent is unchanged.")
    if kn.get("strip_provider_prefix") and "/" in lookup:
        head, tail = lookup.split("/", 1)
        if cmap.provider_id(head) == pid and tail:
            lookup = sent = tail
            notes.append(f"Provider prefix '{head}/' removed from the model.")
    placeholder = False
    if (kn.get("placeholder_as_unset") and not bud_leg and lookup.lower() in cmap.placeholders
            and not has_specific_row(cmap, pid, lookup)):
        placeholder = True
        received = lookup
        lookup = sent = ""

    # 3. Row and layer.
    if not lookup:
        # No model named, or a provider whose model string is not looked up: the declared answer.
        dm = entry.get("default_model")
        if dm:
            row, _ = find_row(cmap, pid, dm)
        else:
            row = next(r for r in cmap.rows_by_provider[pid] if r["match"].get("model_glob") == "*")
        layer = "declared_default" if (model_string in ("absent", "sensitive") and raw) else "model_unset"
        if model_string == "absent":
            sent = ""          # the vendor has no model parameter; the string is ignored
    else:
        row, layer = find_row(cmap, pid, lookup)
    out.update(row_id=row["id"], layer=layer, model_sent=sent if not sensitive else out["model_input"])
    if placeholder:
        warnings.append(warn("stt_placeholder_model_ignored", "frame", received=received, model=entry.get("default_model")))
    named = bool(raw) and not placeholder and model_string not in ("absent", "sensitive")
    life = row["lifecycle"]
    wu = row.get("when_unusable") or {}

    # 4. Retired models are refused in every release, except where today's client silently serves
    #    another model: integration decision 12 warns such a session until its refuse_from_release.
    substituted_until = (wu.get("today") == "streams_substituted_model" and wu.get("refuse_from_release") is not None
                         and release < wu["refuse_from_release"])
    if life["status"] == "retired" and not substituted_until:
        ref = wu.get("refusal") or {}
        return refuse(out, "stt_model_retired", None, ref.get("text", "This model is no longer served by the vendor."),
                      replacement=life.get("replacement"))

    # 5. Transports, after a deployment override's profile (W3 3.5 step 6).
    transports = cmap.transports(row)
    if profile:
        p = cmap.profiles.get(profile)
        if p is None or pid not in p.get("allowed_providers", []):
            notes.append(f"deployment_setting_not_applied: profile '{profile}' is unknown or does not allow {pid}.")
        else:
            transports = [dict(p["transport"], _profile=profile)]
            fb = p.get("fallback_profile")
            if fb:
                transports.append(dict(cmap.profiles[fb]["transport"], _profile=fb))
            out["layer"] = layer = "deployment_override"

    # 6. Which transports are usable for this session.
    usable, skipped = [], []
    for i, t in enumerate(transports):
        efr = t.get("enabled_from_release")
        kind = adapter_kind(t["adapter"])
        why = None
        if efr is None:
            why = "never_enabled"
        elif efr > release:
            why = "not_released"
        elif not evidence_present(row, t, evidence, deadline_ms):
            why = "evidence_missing"
        elif kind != "native" and not covered:
            why = "not_covered"
        else:
            block = constraint_blocks(t, language, region)
            if block and (kind != "native" or release >= NATIVE_CONSTRAINTS_FROM):
                why = block
            elif block:
                notes.append(f"The session's {('language' if block == 'language_not_live' else 'region')} is outside "
                             f"the native client's constraint; today's path is kept until Release 3 (addendum A7).")
        (skipped if why else usable).append((i, t, why))
    if session == "plain" and wu.get("today") == "buffers_until_hangup" and mode != "segmented":
        # Addendum B4: a plain /ws client that ends its own turns keeps today's buffering client.
        kept = [u for u in usable if adapter_kind(u[1]["adapter"]) == "native"]
        skipped += [(i, t, "plain_keeps_today") for i, t, _ in usable if adapter_kind(t["adapter"]) != "native"]
        usable = kept
    if profile and skipped and skipped[0][0] == 0 and usable:
        warnings.append(warn("stt_transport_fallback", "frame", declared=profile, effective=usable[0][1].get("_profile")))

    # 7. Pick by preference.
    chosen = usable[0] if usable else None
    if latency_tier == "low_latency" and mode == "auto" and release >= 4 and usable:
        # Addendum B9: the low-latency tier prefers a usable commit transport over a file transport.
        commit = [u for u in usable if adapter_kind(u[1]["adapter"]) == "commit"]
        if commit:
            chosen = commit[0]
    if mode == "streaming" and usable:
        live = [u for u in usable if u[1]["input_mode"] == "live_stream"]
        if not live:
            return refuse(out, "stt_not_streaming", None, "The session asked for streaming and this model has no "
                          "live-stream transport in this release.")
        chosen = live[0]
    if mode == "segmented" and usable:
        seg = [u for u in usable if adapter_kind(u[1]["adapter"]) == "segmented"]
        if seg:
            chosen = seg[0]
        else:
            has_file = any(adapter_kind(t["adapter"]) == "segmented" for t in transports)
            warnings.append(warn("stt_mode_unavailable", "frame", requested="segmented", source="request",
                                 reason="not_enabled" if has_file else "no_transport"))

    # 8. No usable transport: today's behaviour, the release's refusal, or today's codes.
    if chosen is None:
        return no_usable_transport(cmap, out, pid, entry, row, transports, skipped, wu, release, session, mode,
                                   covered, named, today)

    i, t, _ = chosen
    kind = adapter_kind(t["adapter"])
    out["transport"] = {"index": i, "adapter": t["adapter"], "input_mode": t["input_mode"],
                        "enabled_from_release": t.get("enabled_from_release"), "profile": t.get("_profile"),
                        "constraints": t.get("constraints"), "enable_requires": t.get("enable_requires")}
    if t.get("enable_requires") and evidence == "assume":
        notes.append("Enabled only once " + " and ".join(
            {"live_probe": "a live probe with a real key has succeeded",
             "latency_within_deadline": "a measurement shows it answers within the deadline"}[r]
            for r in t["enable_requires"]) + ".")
    if t.get("constraints"):
        notes.append("Constraints: " + "; ".join(
            f"{k} {v['mode']} {', '.join(v.get('codes') or v.get('values'))}" for k, v in t["constraints"].items()))

    if kind != "native":
        if encoding and encoding.lower() in UNDECODABLE:
            return refuse(out, "stt_segmentation_unavailable", "audio_format",
                          f"The session's audio encoding '{encoding}' cannot be cut into utterances by the gateway.")
        out["kind"] = kind
        out["outcome"] = kind
        out["label"] = {"segmented": "per-utterance upload", "commit": "gateway-driven commit"}[kind] + f" ({t['adapter']})"
        if kind == "segmented" and mode != "segmented":
            warnings.append(warn("stt_segmented_mode", "frame"))
        bill = t.get("billing") or row["billing"]
        if kind == "segmented" and (bill.get("min_billed_ms") or 0) > 0:
            warnings.append(warn("stt_min_billed_duration", "frame", min_billed_ms=bill["min_billed_ms"]))
        p99 = seed_p99(t)
        if release >= 2 and p99 is not None and p99 > SLOW_TARGET_MS:
            warnings.append(warn("stt_latency_slow", "frame", final_latency_slow_ms=p99, target_ms=SLOW_TARGET_MS))
        if (layer in ("provider_default", "global_default") and not row.get("applies_to_all_models")):
            warnings.append(warn("stt_capability_assumed", "frame", capability_source=layer))
        lifecycle_warning(warnings, life, "frame", today)
        if layer == "model_unset" and entry.get("default_model"):
            out["model_sent"] = entry["default_model"]
        return out

    # Today's client, chosen as a listed native transport.
    out["kind"] = "native"
    out["outcome"] = "native"
    out["label"] = "native stream"
    d = native_delivery(release, covered)
    status = (t.get("gateway_client") or {}).get("status")
    if status in ("known_broken", "unverified"):
        warnings.append(warn("stt_client_unverified", d, status=status))
        out["label"] += f" (client {status.replace('_', ' ')})"
    handling = entry.get("native_model_handling", "verbatim")
    if named and handling in ("substituted", "ignored") and layer in ("pattern", "provider_default"):
        warnings.append(warn("stt_model_substituted", d, requested=raw, handling=handling,
                             model_that_runs=entry.get("default_model")))
    if layer in ("provider_default", "global_default") and not row.get("applies_to_all_models"):
        warnings.append(warn("stt_capability_assumed", d, capability_source=layer))
    lifecycle_warning(warnings, life, d, today)
    return out


def lifecycle_warning(warnings, life, delivery, today):
    st = life["status"]
    if st == "deprecated" or (st == "retiring" and life.get("shutdown_on")):
        sd = life.get("shutdown_on")
        warnings.append(warn("stt_model_deprecated", delivery, shutdown_on=sd,
                             past_shutdown=bool(sd and sd <= today), replacement=life.get("replacement")))


def without_operator_clause(text):
    """Addendum B10: a refusal whose cause is not the rollout switch must not tell the customer to ask
    the operator to enable segmented speech-to-text; it says it is not available in this release."""
    clause = (r"ask the operator (?:to enable segmented speech-to-text|whether segmented speech-to-text can be "
              r"enabled) for this deployment")
    new = re.sub(r", ([^,.]+), or " + clause + r"\.", r" or \1.", text)      # "A, B, or ask ..." -> "A or B."
    new = re.sub(r",? or " + clause + r"\.", ".", new)                         # "A, or ask ..." -> "A."
    new = re.sub(clause + r", ", "", new)                                       # "A, ask ..., or C" -> "A, or C"
    if new != text:
        new += " Segmented speech-to-text for this provider is not available in this release."
    return new


def refuse(out, code, reason, text, **details):
    out.update(kind="refused", outcome="refused", refusal={"code": code, "reason": reason, "text": text, **details},
               label=f"refused {code}" + (f" ({reason})" if reason else ""))
    out["transport"] = None
    return out


def no_usable_transport(cmap, out, pid, entry, row, transports, skipped, wu, release, session, mode, covered,
                        named, today):
    notes, warnings = out["notes"], out["warnings"]
    ref = wu.get("refusal") or {}
    today_kind = wu.get("today")
    # A transport the session could use if the switch covered it.
    would_if_covered = any(why == "not_covered" for _, _, why in skipped)
    # Constraints on the only usable transports: the session's language or region is not served.
    blocked = [why for _, _, why in skipped if why in ("language_not_live", "region_not_served")]

    # Self-hosted and Azure OpenAI keep today's two codes while not covered (addendum A1).
    if not entry.get("registered_in_gateway") and today_kind == "refused_at_setup" and (release == 0 or not covered):
        code = "stt_not_streaming" if session == "gateway" else "unsupported_deployment"
        return refuse(out, code, None, "Today's refusal of a deployment that transcribes uploaded files, with today's "
                      "text (the code is stt_not_streaming on a voice-agent leg and unsupported_deployment on a "
                      "named-deployment leg).")

    rfr = wu.get("refuse_from_release")
    if wu and rfr is not None and release >= rfr:
        return refuse(out, ref.get("code", "stt_live_unsupported"), ref.get("reason"), ref.get("text", ""))

    if not wu:
        # A row whose every listed transport is skipped and which records no today behaviour: the
        # only way is a constraint or a not-covered session on a row whose native client was the
        # first entry. Name the reason (W3 3.5 step 7).
        reason = "language_not_live" if "language_not_live" in blocked else "disabled" if blocked else "client_not_implemented"
        if would_if_covered and not blocked:
            reason = "not_covered_yet"
        return refuse(out, "stt_live_unsupported", reason,
                      "No transport of this model can serve the session's language or region on a live call."
                      if blocked else "No transport of this model is usable for this session.")

    d = native_delivery(release, covered)
    if today_kind == "buffers_until_hangup":
        if session == "gateway":
            # Addendum B10: not_covered_yet only when a covered session would get a transport now.
            if would_if_covered:
                return refuse(out, "stt_live_unsupported", "not_covered_yet", ref.get("text", ""))
            return refuse(out, "stt_live_unsupported", ref.get("reason") or "client_not_implemented",
                          without_operator_clause(ref.get("text", "")))
        if mode == "streaming":
            return refuse(out, "stt_not_streaming", None, "Today's client for this model returns text only when the "
                          "client sends audio_end or hangs up; it does not stream.")
        out.update(kind="native", outcome="native", label="today's buffering client")
        warnings.insert(0, warn("stt_buffered_until_commit", "frame", provider=pid, model=out.get("model_input")))
        lifecycle_warning(warnings, row["lifecycle"], d, today)
        return out
    if today_kind in ("refused_at_setup", "fails"):
        reason, text = ref.get("reason"), ref.get("text", "")
        if would_if_covered:
            reason = "not_covered_yet"
        elif blocked and not any(why in ("not_released", "evidence_missing") for _, _, why in skipped):
            # Wire reasons of W5 section 3.9: a language served by batch only, or a region not served.
            reason = "language_not_live" if "language_not_live" in blocked else "disabled"
            text = ("This model is not served on a live call in the session's "
                    + ("language" if reason == "language_not_live" else "region") + ".")
        if reason != "not_covered_yet":
            text = without_operator_clause(text)
        return refuse(out, ref.get("code", "stt_live_unsupported"), reason, text)
    if today_kind == "blind_timed_uploads":
        if mode == "streaming":
            return refuse(out, "stt_not_streaming", None, "Today's client for this model uploads timed pieces; it does not stream.")
        out.update(kind="native", outcome="native", label="today's timed-upload client (known broken)")
        warnings.append(warn("stt_client_unverified", d, status="known_broken",
                             why="uploads blind timed pieces and marks each as an end of turn"))
        lifecycle_warning(warnings, row["lifecycle"], d, today)
        return out
    if today_kind == "streams_substituted_model":
        out.update(kind="native", outcome="native",
                   label=f"today's client, streaming {wu.get('substituted_model')} instead")
        warnings.append(warn("stt_model_substituted", d, requested=out.get("model_input") if named else None,
                             model_that_runs=wu.get("substituted_model")))
        lifecycle_warning(warnings, row["lifecycle"], d, today)
        return out
    if today_kind in ("streams", "unknown"):
        out.update(kind="native", outcome="native",
                   label="today's client" + (" (behaviour for this model not established)" if today_kind == "unknown" else ""))
        if today_kind == "unknown":
            warnings.append(warn("stt_client_unverified", d, status="unverified", why="today's behaviour for this model is unknown"))
        if out["layer"] in ("provider_default", "global_default") and not row.get("applies_to_all_models"):
            warnings.append(warn("stt_capability_assumed", d, capability_source=out["layer"]))
        lifecycle_warning(warnings, row["lifecycle"], d, today)
        return out
    return refuse(out, "stt_live_unsupported", ref.get("reason", "client_not_implemented"), ref.get("text", ""))


# ---------------------------------------------------------------------------------------------
# Self-test
# ---------------------------------------------------------------------------------------------
def self_test(cmap):
    R = lambda *a, **k: resolve(cmap, *a, **k)  # noqa: E731
    cases = []

    def case(name, res, **expect):
        problems = []
        for k, v in expect.items():
            if k == "code":
                got = (res.get("refusal") or {}).get("code")
            elif k == "reason":
                got = (res.get("refusal") or {}).get("reason")
            elif k == "adapter":
                got = (res.get("transport") or {}).get("adapter")
            elif k == "warns":
                have = {w["code"] for w in res["warnings"]}
                missing = set(v) - have
                if missing:
                    problems.append(f"warnings lack {sorted(missing)} (have {sorted(have)})")
                continue
            elif k == "no_warns":
                have = {w["code"] for w in res["warnings"]}
                extra = set(v) & have
                if extra:
                    problems.append(f"unexpected warnings {sorted(extra)}")
                continue
            else:
                got = res.get(k)
            if got != v:
                problems.append(f"{k}: expected {v!r}, got {got!r}")
        cases.append((name, problems))

    # Release 1 vendors.
    case("OpenAI file model is uploaded per utterance from Release 1",
         R("openai", "gpt-transcribe", release=1), outcome="segmented", adapter="openai_transcriptions")
    case("Groq Whisper is uploaded per utterance from Release 1 and carries the 10 s minimum",
         R("groq", "whisper-large-v3-turbo", release=1), outcome="segmented", adapter="groq_transcriptions",
         warns={"stt_min_billed_duration", "stt_segmented_mode"})
    case("ElevenLabs scribe_v2 is uploaded per utterance from Release 1",
         R("elevenlabs", "scribe_v2", release=1), outcome="segmented", adapter="elevenlabs_batch")
    case("A self-hosted deployment uses the default profile from Release 1, with no assumed warning",
         R("self_hosted", "openai/whisper-large-v3", release=1, bud_leg=True), outcome="segmented",
         adapter="openai_transcriptions", no_warns={"stt_capability_assumed"})
    case("WaaV Infer uses its own profile from Release 1",
         R("waav-infer", "parakeet", release=1, bud_leg=True), outcome="segmented", adapter="openai_transcriptions")
    case("Azure OpenAI deployment uses the Azure profile from Release 1",
         R("azure-openai", "my-transcriber", release=1, bud_leg=True), outcome="segmented",
         adapter="azure_openai_transcriptions")
    # Streaming models stay native in every release.
    for rel in range(7):
        case(f"Deepgram nova-3 stays native in release {rel}", R("deepgram", "nova-3", release=rel),
             outcome="native", adapter="native", label="native stream", no_warns={"stt_segmented_mode"})
    # File-only model before and after its enabling release.
    case("Deepgram whisper-large is refused before Release 3",
         R("deepgram", "whisper-large", release=2), code="stt_live_unsupported", reason="client_not_implemented")
    case("Deepgram whisper-large is uploaded per utterance from Release 3",
         R("deepgram", "whisper-large", release=3), outcome="segmented", adapter="deepgram_prerecorded")
    case("Deepgram whisper on an uncovered Release 3 session: not covered yet",
         R("deepgram", "whisper-large", release=3, covered=False), code="stt_live_unsupported", reason="not_covered_yet")
    case("ElevenLabs scribe_v2 is refused in Release 0 on every session",
         R("elevenlabs", "scribe_v2", release=0, session="push_to_talk"), code="stt_live_unsupported",
         reason="client_not_implemented")
    # Live-only model before and after Release 4.
    case("gpt-live-transcribe is refused on a push-to-talk session before Release 4",
         R("openai", "gpt-live-transcribe", release=3, session="push_to_talk"), code="stt_live_unsupported",
         reason="client_not_implemented")
    case("gpt-live-transcribe goes through the gateway-driven commit from Release 4",
         R("openai", "gpt-live-transcribe", release=4), outcome="commit", adapter="openai_realtime_transcription")
    case("Cartesia moves to manual finalize in Release 4", R("cartesia", "ink-whisper", release=4),
         outcome="commit", adapter="cartesia_manual_finalize")
    case("Cartesia streams natively before Release 4", R("cartesia", "ink-whisper", release=3),
         outcome="native", adapter="native")
    # Asynchronous-only model.
    case("AssemblyAI universal-2 streams a substituted model in Release 2",
         R("assemblyai", "universal-2", release=2), outcome="native", warns={"stt_model_substituted"})
    case("AssemblyAI universal-2 is refused as asynchronous-only from Release 3",
         R("assemblyai", "universal-2", release=3), code="stt_live_unsupported", reason="async_only")
    case("A retired model that today's client silently replaces is warned before Release 3",
         R("baidu", "19362", release=1), outcome="native", warns={"stt_model_substituted"})
    case("A retired model that today's client silently replaces is refused from Release 3",
         R("baidu", "19362", release=3), code="stt_model_retired")
    case("A Yandex asynchronous-only model keeps today's timed uploader before Release 3",
         R("yandex", "deferred-general", release=0), outcome="native")
    case("Gladia solaria-3 is refused as asynchronous-only from Release 3",
         R("gladia", "solaria-3", release=3), code="stt_live_unsupported", reason="async_only")
    # Unknown model of a known provider, and an unknown provider.
    case("An unknown OpenAI id fails slow to upload, marked assumed",
         R("openai", "gpt-9-transcribe", release=1), outcome="segmented", row_id="openai:any",
         warns={"stt_capability_assumed"})
    case("An unknown Deepgram id stays native, assumed, as a notice",
         R("deepgram", "nova-9-experimental", release=3), outcome="native", row_id="deepgram:nova-any")
    case("An unknown provider passes through to today's factory",
         R("acme-speech", "x", release=5), outcome="native", row_id="global:any", layer="global_default")
    # Prefixes and keys.
    case("Bhashini keeps its provider-named prefix",
         R("bhashini", "bhashini/iitm/asr-misc--gpu--t4", release=0, session="push_to_talk"),
         row_id="bhashini:bhashini-iitm-asr-misc--gpu--t4")
    case("OpenAI strips a leading openai/ prefix", R("openai", "openai/whisper-1", release=1), row_id="openai:whisper-1",
         model_sent="whisper-1")
    case("Huawei takes the region from the model suffix",
         R("huawei-cloud", "chinese_16k_common@cn-east-3", release=5), row_id="huawei-cloud:chinese_16k_common")
    case("The Python SDK placeholder on ElevenLabs is treated as no model",
         R("elevenlabs", "nova-3", release=0), row_id="elevenlabs:scribe_v2_realtime",
         warns={"stt_placeholder_model_ignored"})
    case("On an uncovered session today's client receives the model string unchanged",
         R("openai", "openai/whisper-1", release=0, session="push_to_talk"), row_id="openai:whisper-1",
         model_sent="openai/whisper-1", warns={"stt_buffered_until_commit"})
    case("An empty model resolves to the provider's declared default, not a guess",
         R("openai", "", release=1), row_id="openai:gpt-transcribe", layer="model_unset",
         model_sent="gpt-transcribe", no_warns={"stt_capability_assumed"})
    case("An alias selects its row", R("groq", "turbo", release=1), row_id="groq:whisper-large-v3-turbo")
    case("A provider alias with a hyphen resolves", R("Azure", "default", release=0), provider="microsoft-azure")
    # Lifecycle.
    case("whisper-1 carries a deprecation warning", R("openai", "whisper-1", release=1), outcome="segmented",
         warns={"stt_model_deprecated"})
    case("A retired model is refused in every release", R("groq", "distil-whisper-large-v3-en", release=6),
         code="stt_model_retired")
    # Push-to-talk versus gateway-decided turns on a buffering client.
    case("OpenAI file model, voice agent with automatic turns, Release 0: refused, no segmented path yet",
         R("openai", "gpt-transcribe", release=0, session="gateway"), code="stt_live_unsupported",
         reason="client_not_implemented")
    case("OpenAI file model, voice agent, uncovered Release 1 session: not covered yet",
         R("openai", "gpt-transcribe", release=1, session="gateway", covered=False), code="stt_live_unsupported",
         reason="not_covered_yet")
    case("A covered session waiting for its vendor's release is not told to ask the operator",
         R("bhashini", "ai4bharat/conformer-hi-gpu--t4", release=2, session="gateway"),
         code="stt_live_unsupported", reason="client_not_implemented")
    case("A plain /ws session on a buffering model keeps today's client even when covered (B4)",
         R("openai", "whisper-1", release=3, session="plain"), outcome="native", warns={"stt_buffered_until_commit"})
    case("A plain /ws session that asks for segmented gets the engine (B4)",
         R("openai", "whisper-1", release=2, session="plain", mode="segmented"), outcome="segmented")
    case("A manual-mode agent or conversation loop on a buffering model gets the engine when covered (B4)",
         R("groq", "whisper-large-v3", release=1, session="push_to_talk"), outcome="segmented")
    case("The low-latency tier reaches gpt-transcribe on the socket from Release 4 (B9)",
         R("openai", "gpt-transcribe", release=4, latency_tier="low_latency"), outcome="commit",
         adapter="openai_realtime_transcription")
    case("The standard tier keeps gpt-transcribe on file upload in Release 4",
         R("openai", "gpt-transcribe", release=4), outcome="segmented")
    case("OpenAI file model, push-to-talk, Release 0: today's client with a warning",
         R("openai", "gpt-transcribe", release=0, session="push_to_talk"), outcome="native",
         warns={"stt_buffered_until_commit"})
    case("Groq, uncovered Release 2 session, push-to-talk: today's client with a warning",
         R("groq", "whisper-large-v3", release=2, session="push_to_talk", covered=False), outcome="native",
         warns={"stt_buffered_until_commit"})
    case("Self-hosted before Release 1 keeps today's codes on a voice-agent leg",
         R("self_hosted", "whisper-large-v3", release=0, session="gateway", bud_leg=True), code="stt_not_streaming")
    case("Self-hosted before Release 1 keeps today's codes on a named-deployment leg",
         R("self_hosted", "whisper-large-v3", release=0, session="push_to_talk", bud_leg=True),
         code="unsupported_deployment")
    case("Yandex keeps the blind uploader until Release 5, reported as known broken",
         R("yandex", "general", release=2, session="gateway"), outcome="native", warns={"stt_client_unverified"})
    # Preferences.
    case("Streaming-only preference on a file-only model is refused",
         R("openai", "whisper-1", release=2, mode="streaming"), code="stt_not_streaming")
    case("Segmented preference on a streaming model takes the file transport when it exists",
         R("deepgram", "nova-3", release=3, mode="segmented"), outcome="segmented", adapter="deepgram_prerecorded")
    case("Segmented preference on a model with no file transport streams with a warning",
         R("elevenlabs", "scribe_v2_realtime", release=2, mode="segmented"), outcome="native",
         warns={"stt_mode_unavailable"})
    # Constraints.
    case("Amazon batch-only language keeps today's path before Release 3",
         R("aws-transcribe", "standard", release=2, language="cy-GB"), outcome="native")
    case("Amazon batch-only language is refused from Release 3",
         R("aws-transcribe", "standard", release=3, language="cy-GB"), code="stt_live_unsupported",
         reason="language_not_live")
    case("Deepgram hosted Whisper is not offered in the EU region",
         R("deepgram", "whisper-large", release=3, region="eu"), code="stt_live_unsupported")
    case("Tencent flash is uploaded in Release 5 on the China site",
         R("tencent", "16k_zh", release=5, region="china"), outcome="segmented", adapter="regional_rest")
    case("Tencent outside China keeps the known-broken streaming client in Release 5",
         R("tencent", "16k_zh", release=5, region="international"), outcome="native",
         warns={"stt_client_unverified"})
    # Evidence gates.
    case("Speechmatics melia-1 needs evidence the map does not carry yet",
         R("speechmatics", "melia-1", release=5, evidence="map"), code="stt_live_unsupported")
    case("A regional vendor without a recorded live probe stays on today's path in Release 5 (map evidence)",
         R("fpt-ai", "general", release=5, session="push_to_talk", evidence="map"), outcome="native",
         warns={"stt_buffered_until_commit"})
    # Model string that is not a model.
    case("Reverie's model field is never shown", R("reverie", "my-app-id-123", release=0),
         model_input="<not shown: this provider's model field is not a model>", row_id="reverie:any")
    case("A compressed audio format cannot be segmented",
         R("openai", "gpt-transcribe", release=1, encoding="opus"), code="stt_segmentation_unavailable",
         reason="audio_format")
    case("A declared socket profile that no release builds falls back to its file profile",
         R("self_hosted", "mistralai/Voxtral-Mini-4B-Realtime-2602", release=2, bud_leg=True, profile="vllm-realtime"),
         outcome="segmented", warns={"stt_transport_fallback"})

    failed = 0
    for name, problems in cases:
        status = "ok  " if not problems else "FAIL"
        if problems:
            failed += 1
        print(f"{status} {name}" + ("" if not problems else "\n       " + "\n       ".join(problems)))
    print(f"{len(cases)} cases, {failed} failed")
    return 1 if failed else 0


# ---------------------------------------------------------------------------------------------
# Expected resolution tables
# ---------------------------------------------------------------------------------------------
def sample_model(cmap, row):
    """A model string that selects this row, for running the resolver over the whole map."""
    m = row["match"]
    if "model" in m:
        return m["model"]
    g = m["model_glob"]
    pid = m["provider"]
    fillers = ("zz-unknown-model", "q9-unknown", "zzzz") if g == "*" else ("x", "q9z", "zz-yy")
    for filler in fillers:
        cand = g.replace("*", filler)
        if pid == "*":
            return cand
        r, _ = find_row(cmap, pid, cand)
        if r["id"] == row["id"]:
            return cand
    raise ValueError(f"no sample model selects row {row['id']}")


def short_outcome(res):
    if res["outcome"] == "refused":
        r = res["refusal"]
        return f"refused `{r['code']}`" + (f" (`{r['reason']}`)" if r.get("reason") else "")
    if res["outcome"] == "segmented":
        s = f"per-utterance upload (`{res['transport']['adapter']}`)"
    elif res["outcome"] == "commit":
        s = f"gateway-driven commit (`{res['transport']['adapter']}`)"
    else:
        s = res["label"]
    extra = []
    t = res.get("transport") or {}
    if t.get("enable_requires"):
        extra.append("after " + " and ".join({"live_probe": "a live probe", "latency_within_deadline": "a deadline measurement"}[x]
                                             for x in t["enable_requires"]))
    if t.get("constraints") and res["outcome"] != "native":
        extra.append("; ".join(f"{k} {v['mode']} {len(v.get('codes') or v.get('values'))} listed" if len(v.get('codes') or v.get('values')) > 3
                               else f"{k} {v['mode']} {', '.join(v.get('codes') or v.get('values'))}" for k, v in t["constraints"].items()))
    return s + (f" [{'; '.join(extra)}]" if extra else "")


def warn_list(res):
    seen = []
    for w in res["warnings"]:
        tag = f"`{w['code']}`" + ("" if w["delivery"] == "frame" else f" ({w['delivery']})")
        if tag not in seen:
            seen.append(tag)
    return ", ".join(seen) if seen else ""


def category(res):
    return res["outcome"]


def model_label(cmap, row):
    m = row["match"]
    entry = cmap.providers.get(m["provider"], {})
    if "model" in m:
        s = f"`{m['model']}`"
        if entry.get("default_model") and entry["default_model"].lower() == m["model"].lower():
            s += " (default)"
        return s
    if m["model_glob"] == "*":
        return "any other id (`*`)"
    return f"`{m['model_glob']}`"


def expected_markdown(cmap, evidence="assume"):
    lines = []
    A = lines.append
    rows = [r for r in cmap.doc["rows"]]
    A("# Expected resolution of the capability map, Release 0 to Release 6"
      + (" (evidence recorded in the map only)" if evidence == "map" else ""))
    A("")
    A("**How this file is made.** It is generated, not written by hand. In the WaaV repository, from the "
      "directory `docs/segmented-stt/capability-map/` (the authoritative copy), run:")
    A("")
    A("```")
    A("python3 resolve.py --expected > EXPECTED_RESOLUTION.md")
    A("```")
    A("")
    A(f"The generator reads `stt_live_capabilities.json` (map version {cmap.doc['map_version']}, revision "
      f"{cmap.doc['map_revision']}, {len(rows)} rows) and calls `resolve()` once per row, per release and per kind of "
      "session, with a model string that selects that row (the model itself for an exact row; a filler such as "
      "`x` in place of each `*` for a pattern row; `zz-unknown-model` for a provider default row). Nothing in the "
      "tables is typed by hand. The capability-map design's planned gateway test "
      "`the_shipped_map_resolves_as_the_release_table_says` is to hold the Rust resolver to the same answers; "
      "`python3 resolve.py --self-test` runs this resolver's own cases.")
    A("")
    A("**Assumptions behind every line.**")
    A("")
    A("- The session is *covered*: from Release 1 its deployment is on the allow-list (Releases 1 and 2) or the "
      "switch default is on (Release 3 onward), and no control record disables it. In Release 0 no session is "
      "covered. A session that is not covered gets what Release 0 shows, except that refusals scheduled for a later "
      "release (`refuse_from_release`) and the language refusals of Release 3 still apply, and a model whose "
      "transport is enabled is refused with `not_covered_yet` rather than `client_not_implemented`.")
    A("- The preference is `auto` (no `transcription_mode`). The session names the model (a session that names "
      "none gets the row marked \"(default)\").")
    A("- The language and region are not given, so language and region constraints are not checked; where the "
      "chosen transport carries one, the line says so in brackets. Today's clients keep their path for a language "
      "outside their constraint until Release 3 (addendum A7).")
    A("- Transports that need evidence first (`enable_requires`: a live probe with a real key, or a measurement "
      "within the deadline) are shown as enabled in their release with the condition in brackets. "
      "`python3 resolve.py --expected --evidence map` gives the stricter tables in which only the evidence the "
      "map carries today counts.")
    A("- Three kinds of session. **Voice agent, automatic turns**: a voice agent whose turn detection is "
      "`semantic` or `server_vad`, so the gateway must end each caller turn. **Manual agent, conversation loop "
      "or DAG**: a voice agent in manual mode, a conversation loop or a DAG session (addendum A1). **Plain /ws "
      "session**: no agent and no conversation loop; the client consumes transcripts and ends its own turns "
      "(addendum B4: kept on today's buffering client unless it asks for segmented). "
      "\"same\" means the outcome is the same as in the first outcome column. For a self-hosted or Azure OpenAI "
      "deployment that is not covered, the voice-agent column shows the agent-leg code (`stt_not_streaming`) and "
      "the other column the named-deployment code (`unsupported_deployment`).")
    A("- Warnings are listed for the voice-agent column. `(log)` means the fact is only logged and counted "
      "(Release 0, and sessions the switch does not cover in Releases 1 and 2); `(notice)` means an entry in "
      "`ready.stt`; no mark means a `config_warning` frame. Codes are the customer contract's (W5 section 3.8).")
    A("")
    A("Outcome words: **native stream**: today's streaming client, unchanged. **today's ... client**: a native "
      "client that does not stream, kept with a warning. **per-utterance upload**: the segmented engine with "
      "the named transcriber. **gateway-driven commit**: a vendor socket on which the gateway's detector sends "
      "the commit. **refused**: refused at setup with the code shown.")
    A("")
    counts = {}
    previous = {}
    for rel in range(7):
        outcome_now = {}
        body = []
        B = body.append
        B("| Provider | Models | Voice agent, automatic turns | Manual agent, conversation loop or DAG | Plain /ws session | Warnings |")
        B("| --- | --- | --- | --- | --- | --- |")
        c = {s: {"native": 0, "segmented": 0, "commit": 0, "refused": 0} for s in SESSIONS}
        refused_codes = {s: {} for s in SESSIONS}
        by_provider = {}
        for row in rows:
            model = sample_model(cmap, row)
            prov = row["match"]["provider"]
            prov_in = prov if prov != "*" else "unlisted-provider"
            res = {}
            for s in SESSIONS:
                res[s] = resolve(cmap, prov_in, model, release=rel, session=s, evidence=evidence,
                                 bud_leg=(prov in ("self_hosted", "azure_openai", "waav-infer")))
                assert res[s]["row_id"] == row["id"], (row["id"], res[s]["row_id"], model)
                c[s][category(res[s])] += 1
                if res[s]["outcome"] == "refused":
                    code = res[s]["refusal"]["code"]
                    refused_codes[s][code] = refused_codes[s].get(code, 0) + 1
            g = short_outcome(res["gateway"])
            p = short_outcome(res["push_to_talk"])
            q = short_outcome(res["plain"])
            wg, wp, wq = warn_list(res["gateway"]), warn_list(res["push_to_talk"]), warn_list(res["plain"])
            w = wg if wg == wp == wq else (f"voice agent: {wg or 'none'}; manual agent or loop: {wp or 'none'}; "
                                           f"plain: {wq or 'none'}")
            key = (g, "same" if p == g else p, "same" if q == g else q, w)
            by_provider.setdefault(prov, {}).setdefault(key, []).append(model_label(cmap, row))
            outcome_now[row["id"]] = (g, p, q)
        for prov in sorted(by_provider, key=lambda x: (x != "*", x)):
            name = "unknown provider (global default)" if prov == "*" else f"`{prov}`"
            first = True
            for (g, p, q, w), models in by_provider[prov].items():
                ms = ", ".join(models) if len(models) <= 12 else ", ".join(models[:10]) + f" and {len(models) - 10} more"
                B(f"| {name if first else ''} | {ms} | {g} | {p} | {q} | {w} |")
                first = False
        A(f"## {RELEASE_NAMES[rel]}")
        A("")
        if previous:
            changed = sorted({rid.split(":", 1)[0] for rid, o in outcome_now.items() if previous.get(rid) != o})
            n = sum(1 for rid, o in outcome_now.items() if previous.get(rid) != o)
            A(f"Outcome changed from the previous release for {n} row(s)"
              + (f", of these providers: {', '.join('`' + x + '`' for x in changed)}." if changed else "."))
            A("")
        lines.extend(body)
        A("")
        previous = outcome_now
        counts[rel] = (c, refused_codes)
    A("## Counts per release")
    A("")
    A("Each row of the map counted once (a model or a pattern), for each kind of session, preference `auto`, "
      "covered session. \"Native\" counts every outcome that keeps today's client: a streaming client, a "
      "substituting or known-broken client, and a buffering client kept with a warning.")
    A("")
    A("| Release | Session | Native | Per-utterance upload | Gateway-driven commit | Refused | Refusal codes |")
    A("| --- | --- | --- | --- | --- | --- | --- |")
    for rel in range(7):
        c, rc = counts[rel]
        for s in SESSIONS:
            label = SESSION_LABELS[s]
            codes = ", ".join(f"`{k}` {v}" for k, v in sorted(rc[s].items()))
            A(f"| {rel} | {label} | {c[s]['native']} | {c[s]['segmented']} | {c[s]['commit']} | {c[s]['refused']} | {codes} |")
    A("")
    return "\n".join(lines) + "\n", counts


# ---------------------------------------------------------------------------------------------
# Capability matrix (CAPABILITY_MATRIX.md), computed from the map
# ---------------------------------------------------------------------------------------------
BUD_LEG_PROVIDERS = ("self_hosted", "azure_openai", "waav-infer")


def vendor_interface(cmap, row):
    if row["lifecycle"]["status"] == "retired":
        return "retired"
    parts = []
    for t in cmap.transports(row):
        if t["input_mode"] == "file_upload":
            parts.append("file, asynchronous job" if t.get("file_request_mode") == "async_poll" else "file, one request")
        elif t["adapter"] in COMMIT_ADAPTERS:
            parts.append("socket with client commit")
        else:
            parts.append("stream")
    if not parts and ((row.get("when_unusable") or {}).get("refusal") or {}).get("reason") == "async_only":
        parts.append("file, asynchronous job only")
    return "; ".join(dict.fromkeys(parts)) or "no transport recorded in the row"


def both_sessions(cmap, pid, model, rel):
    leg = pid in BUD_LEG_PROVIDERS
    out = {s: short_outcome(resolve(cmap, pid, model, release=rel, session=s, bud_leg=leg)) for s in SESSIONS}
    if len(set(out.values())) == 1:
        return out["gateway"]
    return "; ".join(f"{SESSION_LABELS[s]}: {o}" for s, o in out.items())


def fmt_ms(v):
    for unit, n in (("h", 3600000), ("min", 60000), ("s", 1000)):
        if v >= n and v % n == 0:
            return f"{v // n} {unit}"
    return f"{v} ms"


def fmt_bytes(v):
    return f"{v // 1048576} MiB" if v % 1048576 == 0 else f"{v // 1000000} MB" if v % 1000000 == 0 else f"{v} bytes"


def trunc(s, n):
    return s if len(s) <= n else s[: n - 1].rsplit(" ", 1)[0] + " …"


def representative_rows(cmap, pid, rows):
    """One row per adapter that some release enables. Preference: a row on which that adapter is the
    first enabled transport, then the default model's row, then an exact row, then a pattern."""
    dm = (cmap.providers[pid].get("default_model") or "").lower()
    cands = {}
    for row in rows:
        if row["lifecycle"]["status"] == "retired":
            continue
        enabled = [t for t in cmap.transports(row) if t.get("enabled_from_release") is not None]
        for k, t in enumerate(enabled):
            cands.setdefault(t["adapter"], []).append(
                ((k != 0, row["match"].get("model", "").lower() != dm or not dm, "model" not in row["match"],
                  row_key(row).lower()), row, t))
    order = sorted(cands, key=lambda a: (a != "native", a))
    return {a: min(cands[a], key=lambda c: c[0])[1:] for a in order}


def transport_facts(row, t):
    """Limits and billing of one transport, in words."""
    lim = t.get("limits") or {}
    bits = []
    if t["input_mode"] == "file_upload":
        if lim.get("max_upload_bytes"):
            bits.append(f"at most {fmt_bytes(lim['max_upload_bytes'])} per request")
        if lim.get("max_audio_ms"):
            bits.append(f"at most {fmt_ms(lim['max_audio_ms'])} of audio per request")
        if lim.get("single_process"):
            bits.append("one request at a time")
        if (t.get("segment_profile") or {}).get("upload_policy") == "per_turn":
            bits.append("one upload per caller turn")
    else:
        if lim.get("max_session_ms"):
            bits.append(f"a connection lasts at most {fmt_ms(lim['max_session_ms'])}")
        if lim.get("idle_timeout_ms"):
            bits.append(f"closes after {fmt_ms(lim['idle_timeout_ms'])} idle")
    ap = lim.get("assumed_plan")
    rates = []
    for r in lim.get("rates") or []:
        if r.get("plan") not in (None, ap):
            continue
        metric = {"requests": "requests", "concurrent_requests": "uploads in flight", "concurrent_sessions":
                  "open sessions", "audio_seconds": "audio seconds", "tokens": "tokens"}[r["metric"]]
        per = "" if r["per"] == "none" else f" per {r['per']}"
        rates.append(f"{r['value']:,} {metric}{per}")
    if rates:
        scopes = sorted({r["scope"] for r in lim.get("rates") or [] if r.get("plan") in (None, ap)})
        bits.append("limits " + ", ".join(rates[:4]) + (" and more" if len(rates) > 4 else "")
                    + f" (scope {', '.join(scopes)}" + (f"; plan {ap} assumed" if ap else "") + ")")
    if lim.get("ramp"):
        bits.append("traffic must ramp up")
    b = t.get("billing") or row["billing"]
    bill = [f"billed per {b['unit'].replace('audio_', 'audio ').replace('_', ' ')}" if b["unit"] not in ("none", "unknown")
            else ("no vendor bill" if b["unit"] == "none" else "billing unit unknown")]
    if b.get("min_billed_ms"):
        bill.append(f"minimum {fmt_ms(b['min_billed_ms'])} per request")
    if b.get("increment_ms"):
        bill.append(f"rounded up to {fmt_ms(b['increment_ms'])}")
    for cm in b.get("conditional_minimums") or []:
        bill.append(f"minimum {fmt_ms(cm['min_billed_ms'])} with {cm['feature']}" + (f" above {cm['above']}" if "above" in cm else ""))
    for sc in b.get("surcharges") or []:
        bill.append(f"+{sc['percent']:g}% with {sc['feature']}")
    if b.get("bills_silence") is True:
        bill.append("all call audio billed")
    elif b.get("bills_silence") is False and t["input_mode"] == "file_upload":
        bill.append("only uploaded speech billed")
    return ", ".join(bits + bill)


def matrix_markdown(cmap):
    L = []
    A = L.append
    by_pid = {}
    for row in cmap.doc["rows"]:
        by_pid.setdefault(row["match"]["provider"], []).append(row)
    A("# Speech-to-text capability matrix")
    A("")
    A("**How this file is made.** Generated from `stt_live_capabilities.json` (map version "
      f"{cmap.doc['map_version']}, {len(cmap.doc['rows'])} rows, {len(cmap.providers)} providers) by running, in "
      "`docs/segmented-stt/capability-map/` in the WaaV repository, `python3 resolve.py --matrix > CAPABILITY_MATRIX.md`. Every line is computed "
      "from the map and the resolver: the outcomes by calling `resolve()` for each row in each release, the limit, "
      "billing, defect and unverified lines from the rows' fields. Outcomes assume what `EXPECTED_RESOLUTION.md` "
      "assumes: a covered session, preference `auto`, no language or region given, and transports that need a "
      "live probe or a measurement shown as enabled in their release.")
    A("")
    A("Words used. **Vendor interface**: what the vendor offers for the model, as the row's transports record "
      "it: *stream* (a socket that transcribes audio as it arrives), *socket with client commit* (the vendor "
      "transcribes when the caller's side marks the end of an utterance), *file, one request*, *file, "
      "asynchronous job* (submit, then poll; too slow for a call). **Today**: what Release 0, groundwork and "
      "honest refusal, gives: today's code plus the honest refusals. **Plan**: each release in which the outcome "
      "changes. Outcomes: *native stream* (today's streaming client, unchanged), *today's ... client* (a native "
      "client that does not stream, kept with a warning), *per-utterance upload*, *gateway-driven commit*, "
      "*refused* with its code. Where the voice-agent and the other sessions differ, both are given.")
    A("")
    A("## Overview")
    A("")
    A("| Provider | Rows | Rows that stream today | New paths, by release | Refused in Release 6 |")
    A("| --- | --- | --- | --- | --- |")
    for pid in sorted(p for p in by_pid if p != "*"):
        rows = by_pid[pid]
        streams, firsts, refused6 = 0, {}, {}
        for row in rows:
            model = sample_model(cmap, row)
            leg = pid in BUD_LEG_PROVIDERS
            r0 = resolve(cmap, pid, model, release=0, bud_leg=leg)
            if r0["outcome"] == "native" and r0["label"].startswith("native stream"):
                streams += 1
            for rel in range(1, 7):
                o = resolve(cmap, pid, model, release=rel, bud_leg=leg)
                if o["outcome"] in ("segmented", "commit"):
                    k = (rel, o["transport"]["adapter"])
                    firsts[k] = firsts.get(k, 0) + 1
                    break
            o6 = resolve(cmap, pid, model, release=6, bud_leg=leg)
            if o6["outcome"] == "refused":
                rk = o6["refusal"].get("reason") or o6["refusal"]["code"]
                refused6[rk] = refused6.get(rk, 0) + 1
        newp = "; ".join(f"Release {rel}: `{ad}` ({n})" for (rel, ad), n in sorted(firsts.items())) or "none"
        ref = ", ".join(f"{k} {v}" for k, v in sorted(refused6.items())) or "none"
        A(f"| `{pid}` | {len(rows)} | {streams} | {newp} | {ref} |")
    A("")
    A("Counted for a voice agent with automatic turns. \"Rows that stream today\" counts rows whose Release 0 outcome "
      "is a native streaming client. \"New paths\" counts rows by the release in which they first get a "
      "per-utterance upload or a gateway-driven commit. \"Refused in Release 6\" counts rows still refused at the "
      "end of the plan, by reason (or by code when the code has no reason).")
    A("")
    for pid in sorted(p for p in by_pid if p != "*"):
        rows = by_pid[pid]
        e = cmap.providers[pid]
        A(f"## {pid}")
        A("")
        meta = []
        if e.get("aliases"):
            meta.append("Aliases " + ", ".join(f"`{a}`" for a in e["aliases"]))
        meta.append(f"model string: {e['model_string'].replace('_', ' ')}")
        meta.append("no model named: " + (f"`{e['default_model']}`" if e.get("default_model") else "the provider default row"))
        meta.append("a client is registered today" if e.get("registered_in_gateway") else "no live client is registered today")
        if e.get("native_model_handling") in ("substituted", "ignored"):
            meta.append("today's client " + {"substituted": "replaces a model id it does not know",
                                              "ignored": "ignores the model id"}[e["native_model_handling"]])
        A("; ".join(meta) + ".")
        av = e.get("availability")
        if av and av.get("status") != "open":
            A("")
            A(f"Availability: {av['status'].replace('_', ' ')}" + (f" since {av['since']}" if av.get("since") else "")
              + (f". {av['note']}" if av.get("note") else "."))
        if e.get("prerequisites"):
            A("")
            A("Prerequisites: " + " ".join(x["detail"] if x["detail"].endswith(".") else x["detail"] + "."
                                          for x in e["prerequisites"]))
        A("")
        groups = {}
        for row in rows:
            model = sample_model(cmap, row)
            seq = [both_sessions(cmap, pid, model, rel) for rel in range(7)]
            steps = [f"Release {rel}: {seq[rel]}" for rel in range(1, 7) if seq[rel] != seq[rel - 1]]
            key = (vendor_interface(cmap, row), seq[0], "; ".join(steps) or "unchanged")
            groups.setdefault(key, []).append(model_label(cmap, row))
        A("| Models | Vendor interface | Today | Plan |")
        A("| --- | --- | --- | --- |")
        for (vi, today, steps), models in groups.items():
            ms = ", ".join(models) if len(models) <= 8 else ", ".join(models[:6]) + f" and {len(models) - 6} more"
            A(f"| {ms} | {vi} | {today} | {steps} |")
        A("")
        reps = representative_rows(cmap, pid, rows)
        for adapter, (row, t) in reps.items():
            when = "today" if t["enabled_from_release"] == 0 else f"from Release {t['enabled_from_release']}"
            A(f"- `{adapter}` ({when}; figures of `{row_key(row)}`): {transport_facts(row, t)}.")
        if len({transport_facts(r, t) for r in rows for t in cmap.transports(r)
                if t.get("enabled_from_release") is not None and t["adapter"] in reps}) > len(reps):
            A("- Other rows of this provider carry different limits or billing; the map has each one.")
        defects = []
        for row in rows:
            for t in cmap.transports(row):
                gc = t.get("gateway_client") or {}
                if t["adapter"] == "native" and gc.get("status") == "known_broken":
                    d = gc.get("defect", "").split(". ")[0].rstrip(".") + "."
                    if d not in defects:
                        defects.append(d)
        if defects:
            A("- Today's client, defect found by reading: " + " ".join(trunc(d, 260) for d in defects[:2])
              + (f" ({len(defects) - 2} more in the map.)" if len(defects) > 2 else ""))
        unv = []
        for row, _ in list(reps.values()) + [(r, None) for r in rows]:
            for u in (row.get("provenance") or {}).get("unverified") or []:
                item = f"`{row_key(row)}`: {trunc(u.strip(), 140)}"
                if item not in unv:
                    unv.append(item)
        total = len({u for r in rows for u in (r.get("provenance") or {}).get("unverified") or []})
        if unv:
            A(f"- Unverified ({total} distinct items across the rows), for example: " + "; ".join(unv[:3]) + ".")
        A("")
    return "\n".join(L) + "\n"


# ---------------------------------------------------------------------------------------------
def check_release_0(cmap, today):
    """Addendum B2: Release 0 may refuse only what the map records as unable to work today, plus /ws
    voice agents with automatic turns on a buffering client (a deliberate choice). Lists every Release 0
    refusal with its recorded today behaviour; returns 1 if any refusal is not justified that way.
    The 'today' values are taken from reading code, not from running it."""
    justified, unexpected = {}, []
    for row in cmap.doc["rows"]:
        pid = row["match"]["provider"]
        if pid == "*":
            continue
        wu = row.get("when_unusable") or {}
        today_kind = wu.get("today")
        life = row["lifecycle"]
        for s in SESSIONS:
            res = resolve(cmap, pid, sample_model(cmap, row), release=0, session=s, covered=False,
                          bud_leg=pid in BUD_LEG_PROVIDERS, today=today)
            if res["outcome"] != "refused":
                continue
            if today_kind in ("fails", "refused_at_setup"):
                why = f"today: {today_kind}"
            elif today_kind == "buffers_until_hangup" and s == "gateway":
                why = "voice agent, automatic turns, buffering client (by choice)"
            elif life["status"] == "retired" and (life.get("shutdown_on") or "0000") <= today:
                why = "retired before today"
            else:
                unexpected.append((row["id"], s, today_kind, res["refusal"]["code"], res["refusal"].get("reason")))
                continue
            justified[why] = justified.get(why, 0) + 1
    print("Release 0 refusals by justification (row x session kind):")
    for why, n in sorted(justified.items()):
        print(f"  {n:4d}  {why}")
    if unexpected:
        print(f"Refusals the map does not justify ({len(unexpected)}):")
        for u in unexpected:
            print("  ", u)
        return 1
    print("No Release 0 refusal of a session the map records as working today.")
    return 0


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("provider", nargs="?")
    ap.add_argument("model", nargs="?", default="")
    ap.add_argument("--release", type=int, default=0)
    ap.add_argument("--session", choices=SESSIONS, default="gateway")
    ap.add_argument("--mode", choices=MODES, default="auto")
    ap.add_argument("--language")
    ap.add_argument("--region")
    ap.add_argument("--not-covered", action="store_true", help="the rollout switch does not cover the session")
    ap.add_argument("--bud-leg", action="store_true", help="a Bud deployment leg (no placeholder rule)")
    ap.add_argument("--underlying-model")
    ap.add_argument("--profile", help="deployment override profile")
    ap.add_argument("--encoding", default="pcm_s16le")
    ap.add_argument("--evidence", choices=("assume", "map"), default="assume",
                    help="assume: transports that need a probe or a measurement are enabled in their release; "
                         "map: only the evidence recorded in the map counts")
    ap.add_argument("--latency-tier", choices=LATENCY_TIERS, default="standard",
                    help="deployment latency tier (addendum B9); low_latency prefers a commit transport from Release 4")
    ap.add_argument("--today", help="date for lifecycle checks, YYYY-MM-DD (default: the run date)")
    ap.add_argument("--check-release-0", action="store_true",
                    help="list every Release 0 refusal and fail on any the map does not justify (addendum B2)")
    ap.add_argument("--expected-json", metavar="DIR",
                    help="write DIR/release-<n>.json: the outcome for every row and session kind, for the gateway's "
                         "test the_shipped_map_resolves_as_the_release_table_says")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--expected", action="store_true", help="print EXPECTED_RESOLUTION.md")
    ap.add_argument("--matrix", action="store_true", help="print CAPABILITY_MATRIX.md")
    ap.add_argument("--map", default=MAP_PATH)
    a = ap.parse_args(argv[1:])
    cmap = CapabilityMap(a.map)
    if a.self_test:
        return self_test(cmap)
    if a.expected_json:
        os.makedirs(a.expected_json, exist_ok=True)
        for rel in range(7):
            entries = []
            for row in cmap.doc["rows"]:
                pid = row["match"]["provider"]
                model = sample_model(cmap, row)
                for sess in SESSIONS:
                    res = resolve(cmap, pid if pid != "*" else "unlisted-provider", model, release=rel, session=sess,
                                  bud_leg=pid in BUD_LEG_PROVIDERS, today=a.today)
                    entries.append({"row_id": row["id"], "provider": pid, "model": model, "session": sess,
                                    "outcome": res["outcome"],
                                    "adapter": (res.get("transport") or {}).get("adapter"),
                                    "refusal_code": (res.get("refusal") or {}).get("code"),
                                    "refusal_reason": (res.get("refusal") or {}).get("reason"),
                                    "warnings": sorted({w["code"] for w in res["warnings"]})})
            with open(os.path.join(a.expected_json, f"release-{rel}.json"), "w") as handle:
                json.dump({"release": rel, "map_version": cmap.doc["map_version"], "entries": entries}, handle,
                          ensure_ascii=False, indent=1)
                handle.write("\n")
        print(f"wrote {a.expected_json}/release-0.json to release-6.json")
        return 0
    if a.check_release_0:
        return check_release_0(cmap, a.today or TODAY)
    if a.expected:
        text, _ = expected_markdown(cmap, evidence=a.evidence)
        sys.stdout.write(text)
        return 0
    if a.matrix:
        sys.stdout.write(matrix_markdown(cmap))
        return 0
    if not a.provider:
        ap.print_help()
        return 2
    res = resolve(cmap, a.provider, a.model, release=a.release, session=a.session, mode=a.mode,
                  covered=not a.not_covered, language=a.language, region=a.region, bud_leg=a.bud_leg,
                  underlying_model=a.underlying_model, profile=a.profile, encoding=a.encoding, evidence=a.evidence,
                  today=a.today, latency_tier=a.latency_tier)
    if a.json:
        print(json.dumps(res, indent=2, ensure_ascii=False))
        return 0
    print(f"{RELEASE_NAMES[a.release]}; session {a.session}; preference {res['mode']}; covered {res['covered']}")
    print(f"provider {res.get('provider')}  row {res.get('row_id')}  layer {res.get('layer')}  model sent {res.get('model_sent')!r}")
    print("outcome  " + short_outcome(res).replace("`", ""))
    if res.get("refusal"):
        print("refusal  " + (res["refusal"].get("text") or ""))
    for w in res["warnings"]:
        print(f"warning  {w['code']} [{w['delivery']}] {json.dumps(w['detail'], ensure_ascii=False)}")
    for n in res["notes"]:
        print("note     " + n)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
