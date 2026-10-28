#!/bin/bash
# The agent's desktop: a 1920x1080 virtual screen in three panes. Writer top-left, the terminal
# (tmux session "work") bottom-left, the browser (driven by browserd over HTTP) on the right.
export DISPLAY=:1
Xvfb :1 -screen 0 1920x1080x24 -nolisten tcp &
sleep 1
mkdir -p ~/.fluxbox ~/Documents
printf 'session.screen0.toolbar.visible: false\nsession.screen0.workspaces: 1\nsession.screen0.defaultDeco: NORMAL\n' > ~/.fluxbox/init
printf 'background: none\n' > ~/.fluxbox/overlay
fluxbox -rc ~/.fluxbox/init &
sleep 1
xsetroot -solid '#0b1020'
tmux new-session -d -s work -x 96 -y 24
tmux send-keys -t work 'cd ~ && clear' Enter
xterm -fa 'DejaVu Sans Mono' -fs 14 -bg '#0d1117' -fg '#e6edf3' -geometry 96x24+0+560 -T 'Terminal' -e tmux attach -t work &
soffice --writer --nologo --norestore > /home/ava/soffice.log 2>&1 &
python3 /home/ava/browserd.py > /home/ava/browserd.log 2>&1 &
# keep the panes where they belong as windows appear
( for i in $(seq 1 120); do
    pkill -x xmessage 2>/dev/null
    wmctrl -r Terminal -e 0,0,560,958,520 2>/dev/null
    wmctrl -r 'LibreOffice' -e 0,0,0,958,530 2>/dev/null
    wmctrl -r Chromium -e 0,962,0,958,1050 2>/dev/null
    sleep 1; done ) &
sleep infinity
