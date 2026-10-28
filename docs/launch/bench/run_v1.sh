#!/bin/bash
# FD-bench v1.0 on Bud WaaV interaction mode: every subset, two sessions at a time, on gateway 4411.
R=/home/bud/bench/runs/v1_waav
PY=/tmp/claude-1000/-home-bud-ditto-waav/23581eb7-4add-42ec-ba12-262f2c2fee07/scratchpad/interaction/tools/lk-venv/bin/python3
for s in ${SUBSETS:-candor_turn_taking synthetic_user_interruption synthetic_pause_handling candor_pause_handling icc_backchannel}; do
  echo "$(date '+%T') START $s"
  WAAV_WS=ws://127.0.0.1:4411/ws WAAV_GW_LOG=/home/bud/bench/waav/logs_v1/gateway.log $PY fdb_waav.py --root $R/$s --concurrency 2
  echo "$(date '+%T') END $s"
done
