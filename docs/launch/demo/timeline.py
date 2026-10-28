import json, sys
d = sys.argv[1]
rows = []
for l in json.load(open(f'{d}/caller_lines.json')): rows.append((l['start'], 'CALLER', l['text']))
for line in open(f'{d}/events.jsonl'):
    e = json.loads(line); t = e['t']; ty = e['type']
    if ty == 'assistant_transcript' and e.get('delta'): rows.append((t, 'AGENT[' + str(e.get('kind')) + ']', e['delta']))
    elif ty in ('task_created', 'task_result', 'task_question', 'task_cancelled'):
        rows.append((t, '  ' + ty, f"{e.get('agent') or 'request'} {e.get('task_ref')} {(e.get('summary') or e.get('title') or e.get('prompt') or '')[:120]}"))
    elif ty == 'agent_audio_stopped': rows.append((t, '  STOPPED', e.get('reason')))
for r in sorted(rows): print(f"{r[0]:7.2f} {r[1]:22s} {r[2]}")
