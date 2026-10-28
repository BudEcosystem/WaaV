#!/bin/bash
# Record one take into $1, retrying when the call could not be set up (a vendor connection timeout).
OUT=$1
for try in 1 2 3; do
  ./reset_take.sh > /dev/null 2>&1
  rm -rf "$OUT"
  timeout 1500 /home/bud/launch/venv/bin/python launch_day.py --out "$OUT" > "$OUT.log" 2>&1
  [ -f "$OUT/events.jsonl" ] && { echo "take ok on try $try"; break; }
  echo "try $try failed: $(grep -o 'voice_setup_failed\|[A-Za-z]*Error[^"]*' "$OUT.log" | head -1)"; sleep 20
done
tail -12 "$OUT.log"
