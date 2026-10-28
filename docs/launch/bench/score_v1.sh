#!/bin/bash
# Score each FD-bench v1 subset as soon as the run has finished it: CPU ASR, then the repo's evaluate.py.
# The run driver's PID is passed as $1, so the wait does not match its own command line.
R=/home/bud/bench/runs/v1_waav
L=$R/eval_logs
RUN_PID=$1
mkdir -p $L
cd /home/bud/bench
done_subset() {   # every sample has output.wav, or the run driver has exited
  local d=$R/$1
  [ "$(ls $d/*/output.wav 2>/dev/null | wc -l)" -ge "$(ls -d $d/*/ | wc -l)" ] || ! kill -0 $RUN_PID 2>/dev/null
}
score() {   # subset asr_task eval_task
  until done_subset $1; do sleep 30; done
  echo "$(date +%H:%M) scoring $1"
  venv/bin/python tools/fdb_asr.py --task $2 --device cuda --skip_existing --root_dir $R/$1 > $L/$1.asr.txt 2>&1;
    venv/bin/python tools/fdb_asr.py --task $2 --device cpu --skip_existing --root_dir $R/$1 >> $L/$1.asr.txt 2>&1
  venv/bin/python tools/fdb_eval.py --task $3 --root_dir $R/$1 --log_dir $L > $L/$1.txt 2>&1
  echo "$(date +%H:%M) $1:"; sed -n '/\[Result\]/,$p' $L/$1.txt
}
# interruption replies are rated 0-5 by the local judge (gpt-oss-20b), started only for this subset
until done_subset synthetic_user_interruption; do sleep 30; done
/home/bud/bench/tools/judge_server.sh start > $L/judge_server.txt 2>&1
score synthetic_user_interruption user_interruption user_interruption
/home/bud/bench/tools/judge_server.sh stop >> $L/judge_server.txt 2>&1
score synthetic_pause_handling default pause_handling
score candor_pause_handling default pause_handling
score icc_backchannel default backchannel
echo "$(date +%H:%M) all scored"
