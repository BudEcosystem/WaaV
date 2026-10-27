#!/usr/bin/env python3
"""Validate capability-map source files against schema version 2.

A provider file is JSON: {"provider_id": "...", "provider": {...}, "rows": [ ...row... ]}
($defs/provider_file in stt_capability_map.schema.json). profiles.json is $defs/profiles.

Usage:
  python3 validate_rows.py rows/openai.json [more files]   per-file checks
  python3 validate_rows.py --all                            every file in rows/, profiles.json,
                                                            and the checks that span files
Options:
  --verbose          list every schema error, also for rows that are still version 1
  --max-errors N     print at most N errors per file (default 40; 0 = no limit)

Exit code 0 when there are no errors. Warnings never change the exit code.

Per-file checks: the schema; row ids carry the provider; no duplicate model, alias or pattern;
exactly one provider default row; the default model has an exact row; no two patterns of equal
specificity can match the same id; every adapter id is in the adapter table of
CONVERSION_RULES.md and fits the transport; a transport that is not implemented today is
enabled from release 1 or later, or is disabled with a reason; a row with no transport usable
today says what happens (when_unusable); the latency class agrees with the measurements.

Checks with --all: row ids unique across files; provider ids and aliases unique; one global
default row; profiles.json valid, every profile reference resolves and allows the provider; the
schema's adapter list equals the adapter table.
"""
import glob
import json
import os
import re
import sys

import jsonschema

HERE = os.path.dirname(os.path.abspath(__file__))
SCHEMA_PATH = os.path.join(HERE, "stt_capability_map.schema.json")
RULES_PATH = os.path.join(HERE, "CONVERSION_RULES.md")
PROFILES_PATH = os.path.join(HERE, "profiles.json")
META_PATH = os.path.join(HERE, "meta.json")

SCHEMA = json.load(open(SCHEMA_PATH))
DEFS = SCHEMA["$defs"]


def sub_validator(name):
    return jsonschema.Draft202012Validator({"$schema": SCHEMA["$schema"], "$defs": DEFS, "$ref": f"#/$defs/{name}"})


ROW = sub_validator("row")
PROVIDER = sub_validator("provider")
PROFILES = sub_validator("profiles")

# Class thresholds. meta.json (the assembled map's top-level values) overrides them when present.
LATENCY_CLASSES = {"realtime_max_ms": 600, "fast_max_ms": 1200}
if os.path.exists(META_PATH):
    LATENCY_CLASSES.update(json.load(open(META_PATH)).get("latency_classes", {}))

KIND_MODES = {
    "existing streaming client": {"live_stream", "vendor_segmented"},
    "file-upload transcriber family": {"file_upload"},
    "gateway-driven commit socket": {"live_stream", "vendor_segmented"},
    "streaming client not yet written": {"live_stream", "vendor_segmented"},
}
V1_ADAPTER = re.compile(r"^(stream|commit|segmented)_")


# ---------------------------------------------------------------------------------------------
# Tables read from CONVERSION_RULES.md
# ---------------------------------------------------------------------------------------------
def marked_table(text, name):
    """Rows of the markdown table between <!-- name:begin --> and <!-- name:end -->, header dropped."""
    m = re.search(r"<!-- %s:begin -->(.*?)<!-- %s:end -->" % (name, name), text, re.S)
    if not m:
        return None
    rows = []
    for line in m.group(1).splitlines():
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if all(set(c) <= set("-: ") for c in cells):
            continue
        rows.append(cells)
    return rows[1:]


def ticked(cell):
    return re.findall(r"`([^`]+)`", cell)


def load_rules():
    """Returns (adapters, renames, regional_wires, problems)."""
    problems = []
    if not os.path.exists(RULES_PATH):
        return {}, {}, {}, [f"{RULES_PATH}: missing; the adapter table cannot be checked"]
    text = open(RULES_PATH).read()
    adapters, renames, regional = {}, {}, {}
    table = marked_table(text, "adapter-table")
    if table is None:
        problems.append("CONVERSION_RULES.md: no adapter table (markers adapter-table:begin / :end)")
    for cells in table or []:
        ids = ticked(cells[0])
        if len(ids) != 1 or len(cells) < 4:
            problems.append(f"CONVERSION_RULES.md: adapter table row not understood: {cells}")
            continue
        kind = cells[1].strip()
        if kind not in KIND_MODES:
            problems.append(f"CONVERSION_RULES.md: adapter {ids[0]}: unknown kind {kind!r}")
        rel = cells[2].strip().lower()
        release = None if rel == "none" else int(rel) if rel.isdigit() else "bad"
        if release == "bad":
            problems.append(f"CONVERSION_RULES.md: adapter {ids[0]}: release {cells[2]!r} is not 0 to 6 or 'none'")
            release = None
        serves = set(ticked(cells[3]))  # empty set = any provider
        adapters[ids[0]] = {"kind": kind, "release": release, "serves": serves}
    for cells in marked_table(text, "adapter-rename") or []:
        old = ticked(cells[0])
        new = (ticked(cells[2]) or [cells[2]]) if len(cells) > 2 else []
        if len(old) == 1 and new:
            renames.setdefault(old[0], []).append((re.sub(r"`", "", cells[1]), new[0]))
    for cells in marked_table(text, "regional-rest-wires") or []:
        prov = ticked(cells[0])
        if len(prov) == 1 and len(cells) > 1:
            regional[prov[0]] = set(ticked(cells[1]))
    return adapters, renames, regional, problems


ADAPTERS, RENAMES, REGIONAL_WIRES, RULES_PROBLEMS = load_rules()


# ---------------------------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------------------------
def literal_length(pattern):
    return len(pattern.replace("*", ""))


def globs_intersect(p, q):
    """True when some string matches both anchored patterns ('*' is the only metacharacter)."""
    p, q = p.lower(), q.lower()
    memo = {}

    def go(i, j):
        key = (i, j)
        if key in memo:
            return memo[key]
        if i == len(p) and j == len(q):
            r = True
        elif i < len(p) and p[i] == "*":
            r = go(i + 1, j) or (j < len(q) and go(i, j + 1))
        elif j < len(q) and q[j] == "*":
            r = go(i, j + 1) or (i < len(p) and go(i + 1, j))
        elif i < len(p) and j < len(q) and p[i] == q[j]:
            r = go(i + 1, j + 1)
        else:
            r = False
        memo[key] = r
        return r

    return go(0, 0)


def glob_matches(pattern, text):
    return re.fullmatch(".*".join(map(re.escape, pattern.lower().split("*"))), text.lower()) is not None


def expected_class(p99_ms):
    if p99_ms <= LATENCY_CLASSES["realtime_max_ms"]:
        return "realtime"
    return "fast" if p99_ms <= LATENCY_CLASSES["fast_max_ms"] else "slow"


def row_label(row, i):
    m = row.get("match", {}) if isinstance(row, dict) else {}
    key = m.get("model") or m.get("model_glob") or "?"
    return f"rows[{i}] ({key})"


def is_v1_row(row, provider_id=None):
    marks = []
    if "id" not in row:
        marks.append("no 'id'")
    if "no_live_path_reason" in row:
        marks.append("has 'no_live_path_reason'")
    old = []
    for t in row.get("transports", []) or []:
        if not isinstance(t, dict):
            continue
        if "enabled" in t and "enabled_from_release" not in t:
            if "'enabled' in place of 'enabled_from_release'" not in marks:
                marks.append("'enabled' in place of 'enabled_from_release'")
        a = t.get("adapter")
        if isinstance(a, str) and V1_ADAPTER.match(a) and a not in old:
            old.append(a)
    if old:
        hints = []
        for a in old:
            targets = RENAMES.get(a)
            if targets and isinstance(provider_id, str):
                # A case that names providers applies only to them.
                mine = [(w, n) for w, n in targets if "provider " not in w or re.search(r"(?<![a-z0-9_-])%s(?![a-z0-9_-])" % re.escape(provider_id), w)]
                targets = mine or targets
            if targets:
                hints.append(f"{a} -> " + " or ".join(f"{new} ({when})" if when else new for when, new in targets))
            else:
                hints.append(f"{a} -> see the rename table")
        marks.append("version 1 adapter ids: " + "; ".join(hints))
    return marks


# ---------------------------------------------------------------------------------------------
# Checks on one transport, beyond the schema
# ---------------------------------------------------------------------------------------------
def check_transport(where, t, provider_id, errs, warns):
    adapter = t.get("adapter")
    info = ADAPTERS.get(adapter)
    status = (t.get("gateway_client") or {}).get("status")
    efr = t.get("enabled_from_release", "absent")
    if ADAPTERS and isinstance(adapter, str) and info is None:
        errs.append(f"{where}: adapter {adapter!r} is not in the adapter table of CONVERSION_RULES.md")
    if info:
        modes = KIND_MODES.get(info["kind"], set())
        if t.get("input_mode") not in modes:
            errs.append(f"{where}: adapter {adapter} is a {info['kind']}; input_mode {t.get('input_mode')!r} does not fit")
        if info["kind"] == "gateway-driven commit socket" and t.get("endpointing_owner") == "vendor":
            errs.append(f"{where}: adapter {adapter} commits from the gateway's detector; endpointing_owner cannot be 'vendor'")
        if info["serves"] and provider_id not in info["serves"]:
            errs.append(f"{where}: adapter {adapter} does not serve provider {provider_id!r} (adapter table: {', '.join(sorted(info['serves']))})")
        if isinstance(efr, int) and not isinstance(efr, bool):
            if info["release"] is None:
                errs.append(f"{where}: adapter {adapter} is not built in any release; enabled_from_release must be null")
            elif efr < info["release"]:
                errs.append(f"{where}: enabled_from_release {efr} is earlier than the release adapter {adapter} ships in ({info['release']})")
        scheduled = info["release"] is not None and adapter != "native"
        dialect = t.get("dialect") or {}
        if scheduled and adapter != "regional_rest" and dialect.get("language_param") and "language_format" not in dialect:
            errs.append(f"{where}: dialect.language_format is required: adapter {adapter} renders the session language itself")
        if scheduled and info["kind"] == "gateway-driven commit socket" and isinstance(efr, int):
            for need in ("commit", "stream_audio"):
                if need not in t:
                    errs.append(f"{where}: '{need}' is required on a commit transport that a release enables (adapter {adapter})")
        if adapter == "regional_rest" and REGIONAL_WIRES:
            wire = dialect.get("wire")
            if wire not in REGIONAL_WIRES.get(provider_id, set()):
                errs.append(f"{where}: regional_rest does not serve interface {wire!r} of {provider_id} (table of interfaces in CONVERSION_RULES.md)")
    # The rule the assignment names: not implemented today -> release 1 or later, or disabled with a reason.
    if status == "not_implemented":
        if efr == 0:
            errs.append(f"{where}: gateway_client.status is not_implemented but enabled_from_release is 0; use 1 or later, or null with disabled_reason")
        elif efr is None and not t.get("disabled_reason"):
            errs.append(f"{where}: not implemented and never enabled, but no disabled_reason")
        elif efr == "absent":
            errs.append(f"{where}: no enabled_from_release (not implemented today: give release 1 or later, or null with disabled_reason)")
    if status == "known_broken" and efr == 0 and adapter != "native":
        errs.append(f"{where}: a known_broken client may be enabled only on the native adapter")
    # Latency: label against measurements.
    lat = t.get("latency") or {}
    ms = lat.get("measurements")
    if isinstance(ms, list):
        seed = [m for m in ms if isinstance(m, dict) and m.get("percentile") == "p99" and m.get("quantity") == "end_of_speech_to_final"]
        cls = lat.get("class")
        if seed and isinstance(seed[0].get("value_ms"), int):
            want = expected_class(seed[0]["value_ms"])
            if cls != want:
                errs.append(f"{where}: latency.class is {cls!r} but the seed p99 of {seed[0]['value_ms']} ms gives {want!r}")
        elif ms:
            over = any(isinstance(m, dict) and isinstance(m.get("value_ms"), int) and m["value_ms"] > LATENCY_CLASSES["fast_max_ms"] for m in ms)
            if cls not in ("unknown", "slow") or (cls == "slow" and not over):
                errs.append(f"{where}: latency.class {cls!r} needs a p99 end_of_speech_to_final measurement (without one only 'unknown', or 'slow' when a measurement exceeds fast_max_ms)")
    # Rate limits: the assumed plan must exist.
    lim = t.get("limits") or {}
    rates = lim.get("rates")
    if isinstance(rates, list):
        plans = {r.get("plan") for r in rates if isinstance(r, dict)}
        ap = lim.get("assumed_plan")
        if ap is not None and ap not in plans:
            errs.append(f"{where}: limits.assumed_plan {ap!r} is not the plan of any entry in limits.rates")
        named = plans - {None}
        if len(named) > 1 and ap is None:
            warns.append(f"{where}: limits.rates lists several plans ({', '.join(sorted(named))}) and no assumed_plan; the limiter would be seeded from plan-less entries only")


# ---------------------------------------------------------------------------------------------
# One provider file
# ---------------------------------------------------------------------------------------------
def check_file(path, profiles, verbose):
    errs, warns = [], []
    name = os.path.relpath(path, HERE)
    try:
        doc = json.load(open(path))
    except Exception as e:  # noqa: BLE001
        return [f"{name}: not valid JSON: {e}"], [], None
    if not isinstance(doc, dict):
        return [f"{name}: the file must be an object with provider_id, provider and rows"], [], None
    for k in doc:
        if k not in ("provider_id", "provider", "rows"):
            errs.append(f"{name}: unexpected top-level key {k!r}")
    pid = doc.get("provider_id")
    if not isinstance(pid, str) or not pid:
        errs.append(f"{name}: missing provider_id")
    prov = doc.get("provider") if isinstance(doc.get("provider"), dict) else {}
    for e in PROVIDER.iter_errors(doc.get("provider", {})):
        errs.append(f"{name}: provider {'/'.join(map(str, e.absolute_path))}: {e.message[:300]}")
    rows = doc.get("rows")
    if not isinstance(rows, list) or not rows:
        errs.append(f"{name}: rows must be a non-empty list")
        return errs, warns, doc

    seen = {}  # lowercased model, alias or pattern -> row label
    exact = {}  # lowercased model or alias -> row
    globs = []
    defaults = 0
    v1_rows = 0
    for i, row in enumerate(rows):
        label = f"{name}: {row_label(row, i)}"
        if not isinstance(row, dict):
            errs.append(f"{label}: a row must be an object")
            continue
        schema_errors = sorted(ROW.iter_errors(row), key=lambda e: [str(p) for p in e.absolute_path])
        marks = is_v1_row(row, pid)
        if marks:
            v1_rows += 1
            errs.append(f"{label}: still version 1 ({'; '.join(marks)}); convert it with CONVERSION_RULES.md"
                        + ("" if verbose else f" [{len(schema_errors)} schema errors not listed; --verbose lists them]"))
            hidden = [f"{label} {'/'.join(map(str, e.absolute_path))}: {e.message[:300]}" for e in schema_errors]
            errs.extend(hidden if verbose else [None] * len(hidden))  # None = counted, not printed
        else:
            for e in schema_errors:
                errs.append(f"{label} {'/'.join(map(str, e.absolute_path))}: {e.message[:300]}")

        m = row.get("match") if isinstance(row.get("match"), dict) else {}
        if m.get("provider") != pid:
            errs.append(f"{label}: match.provider {m.get('provider')!r} != provider_id {pid!r}")
        rid = row.get("id")
        if isinstance(rid, str) and isinstance(pid, str):
            want = "global" if pid == "*" else pid
            if rid.split(":", 1)[0] != want:
                errs.append(f"{label}: id {rid!r} must begin with '{want}:'")
        keys = []
        if "model" in m:
            keys.append(("model", str(m["model"])))
            keys += [("alias", str(a)) for a in m.get("model_aliases", []) or []]
        elif "model_glob" in m:
            keys.append(("pattern", str(m["model_glob"])))
        for kind, k in keys:
            low = k.lower()
            if low in seen:
                errs.append(f"{label}: {kind} {k!r} is already used by {seen[low]}")
            seen[low] = row_label(row, i)
            if kind == "pattern":
                globs.append((k, row_label(row, i)))
            else:
                exact[low] = row
        if m.get("model_glob") == "*":
            defaults += 1
        elif row.get("applies_to_all_models"):
            errs.append(f"{label}: applies_to_all_models is only for the provider default row")

        # Transports.
        usable_today = False
        ts = row.get("transports") if isinstance(row.get("transports"), list) else []
        for j, t in enumerate(ts):
            where = f"{label}.transports[{j}]"
            if not isinstance(t, dict):
                continue
            if "profile" in t:
                if profiles is None:
                    continue  # resolved under --all
                p = profiles.get(t["profile"])
                if p is None:
                    errs.append(f"{where}: profile {t['profile']!r} is not in profiles.json")
                    continue
                if pid not in (p.get("allowed_providers") or []):
                    errs.append(f"{where}: profile {t['profile']!r} does not allow provider {pid!r}")
                t = p.get("transport") if isinstance(p.get("transport"), dict) else {}
            elif not marks:
                check_transport(where, t, pid, errs, warns)
            if t.get("enabled_from_release") == 0:
                usable_today = True
        has_refs = any(isinstance(t, dict) and "profile" in t for t in ts)
        if not marks and not row.get("passthrough") and not usable_today and "when_unusable" not in row and not (has_refs and profiles is None):
            errs.append(f"{label}: no transport is usable with today's code (none has enabled_from_release 0), so when_unusable must say what a session gets")

        # Lifecycle replacement must resolve (checked after the loop; collect).
        row["_label"] = label

    # Checks across the rows of the file.
    if pid is not None:
        if defaults == 0:
            errs.append(f"{name}: no provider default row (model_glob '*')")
        elif defaults > 1:
            errs.append(f"{name}: {defaults} provider default rows; exactly one is allowed")
    dm = prov.get("default_model")
    if isinstance(dm, str) and dm.lower() not in exact:
        errs.append(f"{name}: provider.default_model {dm!r} has no exact row (neither a model nor a model_alias)")
    if prov.get("model_string") in ("absent", "sensitive") and dm is not None:
        errs.append(f"{name}: provider.model_string is {prov.get('model_string')!r}, so default_model must be null")
    for a in range(len(globs)):
        for b in range(a + 1, len(globs)):
            (pa, la), (pb, lb) = globs[a], globs[b]
            if literal_length(pa) == literal_length(pb) and globs_intersect(pa, pb):
                errs.append(f"{name}: patterns {pa!r} ({la}) and {pb!r} ({lb}) have equal specificity "
                            f"({literal_length(pa)} literal characters) and can match the same id")
    for row in rows:
        if not isinstance(row, dict):
            continue
        label = row.pop("_label", name)
        life = row.get("lifecycle") if isinstance(row.get("lifecycle"), dict) else {}
        for rep in life.get("replacement", []) or []:
            if not isinstance(rep, str):
                continue
            if "*" in rep:
                ok = any(glob_matches(rep, k) and (r.get("lifecycle") or {}).get("status") != "retired" for k, r in exact.items())
            else:
                target = exact.get(rep.lower())
                ok = target is not None and (target.get("lifecycle") or {}).get("status") != "retired"
            if not ok:
                warns.append(f"{label}: lifecycle.replacement {rep!r} does not resolve to an exact row of this provider that is not retired")
        prv = row.get("provenance") if isinstance(row.get("provenance"), dict) else {}
        if prv.get("verified_by") == "live_probe" and "probe_ref" not in prv and "id" in row:
            warns.append(f"{label}: provenance.verified_by is live_probe but there is no probe_ref")
    # Billing floor (capability-map design, pull-request check 14).
    def floor(r):
        b = r.get("billing") if isinstance(r.get("billing"), dict) else {}
        return b.get("min_billed_ms") or 0
    default_rows = [r for r in rows if isinstance(r, dict) and (r.get("match") or {}).get("model_glob") == "*"]
    file_rows = [r for r in rows if isinstance(r, dict) and any(isinstance(t, dict) and t.get("input_mode") == "file_upload" for t in r.get("transports", []) or [])]
    if default_rows and file_rows and "id" in default_rows[0]:
        top = max(floor(r) for r in file_rows)
        if floor(default_rows[0]) < top:
            warns.append(f"{name}: the provider default row's billing.min_billed_ms ({floor(default_rows[0])}) is below the largest among this provider's "
                         f"file-upload rows ({top}); an unknown id would be metered and merged with too small a minimum")
    doc["_v1_rows"] = v1_rows
    return errs, warns, doc


# ---------------------------------------------------------------------------------------------
# Checks that span files
# ---------------------------------------------------------------------------------------------
def check_profiles(errs, warns):
    if not os.path.exists(PROFILES_PATH):
        errs.append("profiles.json: missing")
        return {}
    try:
        profiles = json.load(open(PROFILES_PATH))
    except Exception as e:  # noqa: BLE001
        errs.append(f"profiles.json: not valid JSON: {e}")
        return {}
    v1 = [k for k, v in profiles.items() if isinstance(v, dict) and "transport" not in v and "input_mode" in v]
    if v1:
        errs.append(f"profiles.json: still version 1: {len(v1)} profile(s) are bare transports ({', '.join(sorted(v1))}); "
                    "version 2 wraps each as {transport, allowed_providers, provenance} and renames them (profile name table in CONVERSION_RULES.md)")
        return profiles
    for e in sorted(PROFILES.iter_errors(profiles), key=lambda e: [str(p) for p in e.absolute_path]):
        errs.append(f"profiles.json {'/'.join(map(str, e.absolute_path))}: {e.message[:300]}")
    for pname, p in profiles.items():
        if not isinstance(p, dict):
            continue
        t = p.get("transport") if isinstance(p.get("transport"), dict) else {}
        for prov in p.get("allowed_providers", []) or []:
            check_transport(f"profiles.json: {pname} (as {prov})", t, prov, errs, warns)
        fb = p.get("fallback_profile")
        if fb is not None:
            target = profiles.get(fb)
            if not isinstance(target, dict):
                errs.append(f"profiles.json: {pname}: fallback_profile {fb!r} does not exist")
            elif (target.get("transport") or {}).get("input_mode") != "file_upload":
                errs.append(f"profiles.json: {pname}: fallback_profile {fb!r} must be a file_upload profile")
    return profiles


def check_all(docs, profiles, errs, warns):
    for p in RULES_PROBLEMS:
        errs.append(p)
    enum = set(DEFS["adapter_id"]["enum"])
    if ADAPTERS:
        for a in sorted(enum - set(ADAPTERS)):
            errs.append(f"schema adapter id {a!r} is missing from the adapter table of CONVERSION_RULES.md")
        for a in sorted(set(ADAPTERS) - enum):
            errs.append(f"adapter table lists {a!r}, which the schema's adapter_id list does not contain")
    ids, providers, names = {}, {}, {}
    global_defaults = 0
    for path, doc in docs:
        name = os.path.relpath(path, HERE)
        pid = doc.get("provider_id")
        if pid == "*":
            global_defaults += 1
        elif isinstance(pid, str):
            if pid in providers:
                errs.append(f"{name}: provider_id {pid!r} is also defined in {providers[pid]}")
            providers[pid] = name
        for row in doc.get("rows", []) or []:
            rid = row.get("id") if isinstance(row, dict) else None
            if isinstance(rid, str):
                if rid in ids:
                    errs.append(f"{name}: row id {rid!r} is also used in {ids[rid]}")
                ids[rid] = name
    for path, doc in docs:
        name = os.path.relpath(path, HERE)
        pid = doc.get("provider_id")
        spellings = [pid] if isinstance(pid, str) and pid != "*" else []
        spellings += [a for a in ((doc.get("provider") or {}).get("aliases") or []) if isinstance(a, str)]
        for s in spellings:
            low = s.strip().lower()
            if low in names and names[low] != name:
                errs.append(f"{name}: provider name or alias {s!r} is also claimed by {names[low]}")
            names[low] = name
    if global_defaults != 1:
        errs.append(f"expected exactly one global default file (provider_id '*'), found {global_defaults}")
    for pname, p in (profiles or {}).items():
        for prov in (p.get("allowed_providers") or []) if isinstance(p, dict) else []:
            if prov not in providers:
                errs.append(f"profiles.json: {pname}: allowed_providers names {prov!r}, which no provider file defines")


# ---------------------------------------------------------------------------------------------
def main(argv):
    args = [a for a in argv[1:]]
    verbose = "--verbose" in args
    limit = 40
    if "--max-errors" in args:
        k = args.index("--max-errors")
        limit = int(args[k + 1])
        del args[k:k + 2]
    args = [a for a in args if a != "--verbose"]
    if not args:
        print(__doc__)
        return 2
    run_all = args[0] == "--all"
    files = sorted(glob.glob(os.path.join(HERE, "rows", "*.json"))) if run_all else args

    total_errs, total_warns, n_rows, n_v1, docs = 0, 0, 0, 0, []
    profiles = None
    if run_all:
        perrs, pwarns = [], []
        profiles = check_profiles(perrs, pwarns)
        for e in perrs:
            print("ERROR", e)
        for w in pwarns:
            print("WARN ", w)
        total_errs += len(perrs)
        total_warns += len(pwarns)
        if perrs:
            profiles = None if any("still version 1" in e for e in perrs) else profiles
    for f in files:
        errs, warns, doc = check_file(f, profiles, verbose)
        printable = [e for e in errs if e is not None]
        shown = printable if limit == 0 else printable[:limit]
        for e in shown:
            print("ERROR", e)
        if len(printable) > len(shown):
            print(f"ERROR {os.path.relpath(f, HERE)}: ... {len(printable) - len(shown)} more error(s) not shown (--max-errors 0 shows all)")
        for w in warns:
            print("WARN ", w)
        total_errs += len(errs)
        total_warns += len(warns)
        if doc and isinstance(doc.get("rows"), list):
            n_rows += len(doc["rows"])
            n_v1 += doc.pop("_v1_rows", 0)
            docs.append((f, doc))
    if run_all:
        xerrs, xwarns = [], []
        check_all(docs, profiles, xerrs, xwarns)
        for e in xerrs:
            print("ERROR", e)
        for w in xwarns:
            print("WARN ", w)
        total_errs += len(xerrs)
        total_warns += len(xwarns)
    elif RULES_PROBLEMS:
        for p in RULES_PROBLEMS:
            print("ERROR", p)
        total_errs += len(RULES_PROBLEMS)
    tail = f" ({n_v1} row(s) still version 1)" if n_v1 else ""
    print(f"{len(files)} file(s), {n_rows} row(s){tail}, {total_errs} error(s), {total_warns} warning(s)")
    return 1 if total_errs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
