#!/usr/bin/env python3
"""Build stt_live_capabilities.json from the per-provider source files.

The source of truth is:
  meta.json       schema version, map version and revision, latency classes, SDK placeholder models
  profiles.json   named transport profiles a deployment override may select
  rows/*.json     one file per provider: {"provider_id", "provider", "rows"}

The output is one document valid against stt_capability_map.schema.json, with rows sorted by
provider then model, written compactly. With `--routing PATH` it also writes the routing map the
gateway embeds: the same document without the reviewer prose (provenance, notes, rationale). Run it after editing any source file, then run
`python3 validate_rows.py --all`, `python3 resolve.py --self-test`, and regenerate the two reports:
  python3 resolve.py --expected > EXPECTED_RESOLUTION.md
  python3 resolve.py --matrix   > CAPABILITY_MATRIX.md

Standard library only. jsonschema is used for the final check when it is installed.
"""
import glob
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def row_key(row):
    match = row["match"]
    return (match["provider"] == "*", match["provider"], (match.get("model") or match.get("model_glob") or "").lower())


def main():
    doc = json.load(open(os.path.join(HERE, "meta.json")))
    doc["profiles"] = json.load(open(os.path.join(HERE, "profiles.json")))
    providers, rows = {}, []
    for path in sorted(glob.glob(os.path.join(HERE, "rows", "*.json"))):
        source = json.load(open(path))
        if source["provider_id"] != "*":
            providers[source["provider_id"]] = source["provider"]
        rows.extend(source["rows"])
    rows.sort(key=row_key)
    ids = [row["id"] for row in rows]
    duplicates = sorted({i for i in ids if ids.count(i) > 1})
    if duplicates:
        sys.exit(f"duplicate row identifiers: {duplicates}")
    doc["providers"] = providers
    doc["rows"] = rows
    try:
        import jsonschema
        schema = json.load(open(os.path.join(HERE, "stt_capability_map.schema.json")))
        jsonschema.Draft202012Validator(schema).validate(doc)
    except ImportError:
        print("jsonschema is not installed: skipped the schema check", file=sys.stderr)
    out = os.path.join(HERE, "stt_live_capabilities.json")
    with open(out, "w") as handle:
        json.dump(doc, handle, ensure_ascii=False, separators=(",", ":"))
        handle.write("\n")
    print(f"wrote {out}: {len(providers)} providers, {len(rows)} rows, {len(doc['profiles'])} profiles")
    if len(sys.argv) > 2 and sys.argv[1] == "--routing":
        routing = strip_prose(doc)
        with open(sys.argv[2], "w") as handle:
            json.dump(routing, handle, ensure_ascii=False, separators=(",", ":"))
            handle.write("\n")
        print(f"wrote {sys.argv[2]}: routing fields only, {os.path.getsize(sys.argv[2])} bytes")


# Fields that explain a fact to a reviewer and are never read by the resolver. The gateway embeds the
# document without them (chapter 3: the routing map); the sources stay in rows/*.json.
PROSE_KEYS = {"provenance", "notes", "description", "rationale", "conditions", "disabled_reason", "defect",
              "code_ref", "code_refs"}


# The resolver reads these two provenance facts when evidence comes from the map (a recorded live
# probe enables a transport that requires one), so the routing map keeps them.
PROVENANCE_KEPT = ("verified_by", "probe_ref")


def strip_prose(node):
    if isinstance(node, dict):
        out = {k: strip_prose(v) for k, v in node.items() if k not in PROSE_KEYS}
        prov = node.get("provenance")
        if isinstance(prov, dict):
            kept = {k: prov[k] for k in PROVENANCE_KEPT if k in prov}
            if kept:
                out["provenance"] = kept
        return out
    if isinstance(node, list):
        return [strip_prose(v) for v in node]
    return node


if __name__ == "__main__":
    main()
