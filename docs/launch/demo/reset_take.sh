#!/bin/bash
# Clean desktop and fresh desk agents before a take.
D=bud-desk-demo; IX=/tmp/claude-1000/-home-bud-ditto-waav/23581eb7-4add-42ec-ba12-262f2c2fee07/scratchpad/interaction
docker exec $D pkill -x soffice.bin; docker exec $D pkill -x waav-gateway; docker exec $D pkill -x curl; sleep 2
docker exec $D bash -c 'rm -rf ~/waav ~/t1 ~/Documents/* /tmp/*.tgz /tmp/screen.mp4'
docker exec $D tmux kill-session -t work; docker exec $D tmux new-session -d -s work -x 96 -y 24
docker exec $D tmux set -g status off; docker exec $D tmux send-keys -t work "cd ~ && clear" Enter
docker exec $D pkill -x xterm; sleep 0.5
docker exec -d -e DISPLAY=:1 $D xterm -fa "DejaVu Sans Mono" -fs 14 -bg "#0d1117" -fg "#e6edf3" -geometry 96x24+0+560 -T Terminal -e tmux attach -t work
curl -s -X POST http://127.0.0.1:19333/open -H 'content-type: application/json' -d '{"url":"about:blank"}' >/dev/null
docker exec -d -e DISPLAY=:1 $D soffice --writer --nologo --norestore
for i in $(seq 1 40); do docker exec $D bash -c 'DISPLAY=:1 wmctrl -l' | grep -q "LibreOffice Writer" && break; sleep 2; done; sleep 2
docker exec $D bash -c 'export DISPLAY=:1; wmctrl -r LibreOffice -e 0,0,0,958,530; wmctrl -r Terminal -e 0,0,560,958,520; wmctrl -r Chromium -e 0,962,0,958,1050'
cd /home/bud/launch/demo && { [ -f desk_agents.pid ] && kill $(cat desk_agents.pid) 2>/dev/null; sleep 1; : > desk_agents.jsonl
  ( set -a; . $IX/secrets/anthropic.env; . $IX/secrets/desk.env; set +a; nohup /home/bud/launch/venv/bin/python desk_agents.py --log /home/bud/launch/demo/desk_agents.jsonl > desk_agents.out 2>&1 & echo $! > desk_agents.pid ); }
sleep 2; docker exec $D bash -c 'DISPLAY=:1 wmctrl -l' | cut -c30-; tail -1 desk_agents.out
