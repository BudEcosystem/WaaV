"""Score a launch-day take: higher is a cleaner, complete demo."""
import json, sys
d = sys.argv[1]
ev = [json.loads(l) for l in open(f"{d}/events.jsonl")]
said = [e for e in ev if e["type"] == "assistant_transcript" and e.get("delta")]
txt = " ".join(e["delta"] for e in said).lower()
res = {e.get("agent"): e for e in ev if e["type"] == "task_result" and e.get("agent")}
score, notes = 0, []
for a in ("browser", "terminal", "writer"):
    if a in res and res[a].get("state") == "completed":
        score += 2
    else:
        notes.append(f"{a} not completed")
if "saved" in (res.get("writer", {}).get("summary") or "").lower() and "wasn't saved" not in (res.get("writer", {}).get("summary") or "").lower() and "didn't save" not in (res.get("writer", {}).get("summary") or "").lower():
    score += 3
else:
    notes.append("not saved")
bad = txt.count("didn't catch that") + txt.count("yes or a no") + txt.count("couldn't get that done") + txt.count("could not wait")
score -= 3 * bad
if any(w in txt for w in ("twelve", " 12 ", "zero downloads", "no downloads")):
    score += 3
else:
    notes.append("numbers not spoken")
dup = len(said) - len({e["delta"] for e in said})
score -= dup
print(json.dumps({"take": d, "score": score, "bad_lines": bad, "dup_lines": dup, "notes": notes}))
