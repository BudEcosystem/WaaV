#!/usr/bin/env python3
"""Write tests/expected/grid.jsonl: reference resolutions the Rust resolver must reproduce exactly.

The reference resolver is docs/segmented-stt/capability-map/resolve.py. It runs here over the
routing map the gateway embeds (data/stt_live_routing.json), so both resolvers read the same data.
The sample is deterministic (fixed seed, fixed date) and spans every release, kind of session,
preference, coverage, latency tier and evidence mode, with languages, regions, encodings,
deployment profiles, underlying models, every row's sample model, provider and model spellings,
placeholders and unknown ids.

The first line is a header naming the fields; every other line is one resolution written as
[inputs, outputs], each a list in the header's field order. Positional lists keep the file small;
refusal texts, labels and notes are kept whole, since the Rust resolver reproduces them exactly.

Run from anywhere:  python3 segmented-stt/tests/gen_parity.py
"""

import json
import os
import random
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
REPO = os.path.dirname(CRATE)
sys.path.insert(0, os.path.join(REPO, "docs", "segmented-stt", "capability-map"))

import resolve as R  # noqa: E402

MAP_PATH = os.path.join(CRATE, "data", "stt_live_routing.json")
OUT_PATH = os.path.join(HERE, "expected", "grid.jsonl")
SEED = 20261005
TODAY = "2026-10-05"
LANGUAGES = (None, "en", "cy-GB", "hi", "zh", "es-ES")
REGIONS = (None, "eu", "china", "international")
ENCODINGS = ("pcm_s16le", "mulaw", "opus", None)
DEADLINES = (6000, 6000, 6000, 1500, 3000, 10000)
UNKNOWN_MODELS = (
    "zz-unknown-model",
    "gpt-9-transcribe",
    "whisper-x",
    "nova-9-experimental",
    "Q9-UNKNOWN",
)
IN_FIELDS = (
    "provider",
    "model",
    "release",
    "session",
    "mode",
    "covered",
    "language",
    "region",
    "bud_leg",
    "underlying_model",
    "profile",
    "encoding",
    "evidence",
    "deadline_ms",
    "latency_tier",
)
OUT_FIELDS = (
    "outcome",
    "provider",
    "row_id",
    "layer",
    "model_sent",
    "model_input",
    "label",
    "covered",
    "mode",
    "adapter",
    "transport_index",
    "transport_profile",
    "refusal",
    "warnings",
    "notes",
)


def spellings(cmap, pid):
    """Ways a client may write the provider: the id, an alias, other case, '-' for '_', padding."""
    if pid == "*":
        return ["unlisted-provider", "acme-speech", "  Acme_Speech ", ""]
    entry = cmap.providers[pid]
    out = [
        pid,
        pid.upper(),
        f" {pid} ",
        pid.replace("_", "-") if "_" in pid else pid.replace("-", "_"),
    ]
    out += entry.get("aliases", [])[:3]
    return out


def model_variants(cmap, pid, model):
    out = [model, model.upper(), f"  {model} "]
    entry = cmap.providers.get(pid) or {}
    kn = entry.get("key_normalisation") or {}
    if kn.get("strip_provider_prefix"):
        out.append(f"{pid}/{model}")
        out.append(f"{pid.upper()}/{model}")
    if kn.get("region_suffix_separator"):
        sep = kn["region_suffix_separator"]
        out += [f"{model}{sep}cn-east-3", f"{model}{sep}", f"{model}{sep}eu"]
    return out


def case_inputs(rng, cmap, pid, model, **fixed):
    leg_default = pid in R.BUD_LEG_PROVIDERS
    entry = cmap.providers.get(pid) or {}
    inp = {
        "provider": pid if pid != "*" else "unlisted-provider",
        "model": model,
        "release": rng.randrange(7),
        "session": rng.choice(R.SESSIONS),
        "mode": rng.choice(R.MODES),
        "covered": rng.random() < 0.7,
        "language": rng.choice(LANGUAGES),
        "region": rng.choice(REGIONS),
        "bud_leg": leg_default if rng.random() < 0.85 else not leg_default,
        "underlying_model": None,
        "profile": None,
        "encoding": rng.choice(ENCODINGS),
        "evidence": "map" if rng.random() < 0.3 else "assume",
        "deadline_ms": rng.choice(DEADLINES),
        "latency_tier": "low_latency" if rng.random() < 0.3 else "standard",
    }
    if entry.get("model_string") == "deployment_name" and rng.random() < 0.7:
        own = [R.sample_model(cmap, r) for r in cmap.rows_by_provider.get(pid, [])]
        inp["underlying_model"] = rng.choice(
            own + ["", "unknown-underlying", "  GPT-4O-TRANSCRIBE "]
        )
    elif rng.random() < 0.03:
        inp["underlying_model"] = "whisper-1"
    if leg_default and rng.random() < 0.35:
        inp["profile"] = rng.choice(list(cmap.profiles) + ["no-such-profile"])
    elif rng.random() < 0.02:
        inp["profile"] = rng.choice(list(cmap.profiles))
    inp.update(fixed)
    return inp


def run(cmap, inp):
    res = R.resolve(
        cmap,
        inp["provider"],
        inp["model"],
        release=inp["release"],
        session=inp["session"],
        mode=inp["mode"],
        covered=inp["covered"],
        language=inp["language"],
        region=inp["region"],
        bud_leg=inp["bud_leg"],
        underlying_model=inp["underlying_model"],
        profile=inp["profile"],
        encoding=inp["encoding"],
        evidence=inp["evidence"],
        deadline_ms=inp["deadline_ms"],
        today=TODAY,
        latency_tier=inp["latency_tier"],
    )
    t = res.get("transport") or {}
    ref = res.get("refusal")
    refusal = None
    if ref is not None:
        details = {k: v for k, v in ref.items() if k not in ("code", "reason", "text")}
        refusal = [ref["code"], ref["reason"], ref["text"], details]
    return {
        "outcome": res["outcome"],
        "provider": res["provider"],
        "row_id": res["row_id"],
        "layer": res["layer"],
        "model_sent": res["model_sent"],
        "model_input": res["model_input"],
        "label": res["label"],
        "covered": res["covered"],
        "mode": res["mode"],
        "adapter": t.get("adapter"),
        "transport_index": t.get("index"),
        "transport_profile": t.get("profile"),
        "refusal": refusal,
        "warnings": [[w["code"], w["delivery"], w["detail"]] for w in res["warnings"]],
        "notes": res["notes"],
    }


def cases_for(cmap, rng):
    cases = []
    rows = cmap.doc["rows"]
    # Every row: its sample model under random conditions, then spellings of provider and model.
    for row in rows:
        pid = row["match"]["provider"]
        model = R.sample_model(cmap, row)
        for _ in range(16):
            cases.append(case_inputs(rng, cmap, pid, model))
        for _ in range(3):
            variant = rng.choice(model_variants(cmap, pid, model))
            spelling = rng.choice(spellings(cmap, pid))
            cases.append(case_inputs(rng, cmap, pid, variant, provider=spelling))
    # Every kind of session for every row, at the defaults the release tables assume.
    for row in rows:
        pid = row["match"]["provider"]
        model = R.sample_model(cmap, row)
        rel = rng.randrange(7)
        for session in R.SESSIONS:
            cases.append(
                case_inputs(
                    rng,
                    cmap,
                    pid,
                    model,
                    release=rel,
                    session=session,
                    mode="auto",
                    covered=True,
                    language=None,
                    region=None,
                    encoding="pcm_s16le",
                    evidence="assume",
                    deadline_ms=6000,
                    latency_tier="standard",
                    profile=None,
                    bud_leg=pid in R.BUD_LEG_PROVIDERS,
                )
            )
    providers = list(cmap.providers) + ["*"]
    # No model, placeholders and unknown ids on every provider.
    for pid in providers:
        for rel in range(7):
            cases.append(
                case_inputs(rng, cmap, pid, rng.choice(["", "   "]), release=rel)
            )
        for model in ("nova-3", " NOVA-3 ", "nova-3"):
            for leg in (False, True):
                cases.append(case_inputs(rng, cmap, pid, model, bud_leg=leg))
        for model in UNKNOWN_MODELS:
            for _ in range(3):
                spelling = rng.choice(spellings(cmap, pid))
                cases.append(case_inputs(rng, cmap, pid, model, provider=spelling))
    # Deployment profiles on the providers that allow them, and on some that do not.
    for pid in list(R.BUD_LEG_PROVIDERS) + ["openai", "deepgram"]:
        own = [R.sample_model(cmap, r) for r in cmap.rows_by_provider[pid]]
        for profile in list(cmap.profiles) + ["no-such-profile", ""]:
            for rel in range(7):
                cases.append(
                    case_inputs(
                        rng,
                        cmap,
                        pid,
                        rng.choice(own),
                        profile=profile,
                        release=rel,
                        bud_leg=True,
                    )
                )
    # Transports that need evidence: the map's evidence against the deadline.
    for row in rows:
        if not any(t.get("enable_requires") for t in cmap.transports(row)):
            continue
        pid = row["match"]["provider"]
        model = R.sample_model(cmap, row)
        for rel in (4, 5, 6):
            for deadline in (1500, 6000, 10000):
                cases.append(
                    case_inputs(
                        rng,
                        cmap,
                        pid,
                        model,
                        release=rel,
                        evidence="map",
                        deadline_ms=deadline,
                        covered=True,
                    )
                )
    # Language and region constraints, on rows that carry one.
    for row in rows:
        if not any(t.get("constraints") for t in cmap.transports(row)):
            continue
        pid = row["match"]["provider"]
        model = R.sample_model(cmap, row)
        for lang in LANGUAGES[1:]:
            cases.append(case_inputs(rng, cmap, pid, model, language=lang))
        for region in REGIONS[1:]:
            cases.append(case_inputs(rng, cmap, pid, model, region=region))
    # A placeholder ahead of other warnings, in every release and kind of session.
    for pid, entry in cmap.providers.items():
        if not (entry.get("key_normalisation") or {}).get("placeholder_as_unset"):
            continue
        for rel in range(7):
            for session in R.SESSIONS:
                for covered in (True, False):
                    cases.append(
                        case_inputs(
                            rng,
                            cmap,
                            pid,
                            "nova-3",
                            release=rel,
                            session=session,
                            covered=covered,
                            bud_leg=False,
                            profile=None,
                        )
                    )
    # A region carried in the model string, against an explicit region.
    for pid, entry in cmap.providers.items():
        sep = (entry.get("key_normalisation") or {}).get("region_suffix_separator")
        if not sep:
            continue
        for row in cmap.rows_by_provider[pid]:
            model = R.sample_model(cmap, row)
            for suffix in ("cn-east-3", "cn-north-4", "eu", ""):
                for region in REGIONS + ("cn-north-4",):
                    cases.append(
                        case_inputs(
                            rng,
                            cmap,
                            pid,
                            f"{model}{sep}{suffix}",
                            region=region,
                            release=rng.choice((5, 6)),
                            covered=True,
                        )
                    )
    return cases


def main():
    cmap = R.CapabilityMap(MAP_PATH)
    cases = cases_for(cmap, random.Random(SEED))
    header = {
        "map_version": cmap.doc["map_version"],
        "map_revision": cmap.doc["map_revision"],
        "today": TODAY,
        "seed": SEED,
        "in": IN_FIELDS,
        "out": OUT_FIELDS,
        "refusal": ["code", "reason", "text", "details"],
        "warning": ["code", "delivery", "detail"],
    }
    with open(OUT_PATH, "w", encoding="utf-8") as handle:
        handle.write(
            json.dumps(header, ensure_ascii=False, separators=(",", ":")) + "\n"
        )
        for inp in cases:
            out = run(cmap, inp)
            line = [[inp[k] for k in IN_FIELDS], [out[k] for k in OUT_FIELDS]]
            handle.write(
                json.dumps(line, ensure_ascii=False, separators=(",", ":")) + "\n"
            )
    print(
        f"wrote {OUT_PATH}: {len(cases)} resolutions, {os.path.getsize(OUT_PATH)} bytes"
    )


if __name__ == "__main__":
    main()
