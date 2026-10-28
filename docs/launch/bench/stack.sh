#!/bin/bash
# Bud WaaV interaction-mode stack for FD-bench: front (Qwen3.5-2B), background (qwen2.5-3b), gateway 1.1.0.
# Usage: stack.sh start|stop|status
set -u
B=/home/bud/bench/waav; L=$B/logs; LL=/home/bud/ditto/llamacpp-build/b11200-cuda/llama-b11200
IX=/tmp/claude-1000/-home-bud-ditto-waav/23581eb7-4add-42ec-ba12-262f2c2fee07/scratchpad/interaction
FRONT_SLOTS=${FRONT_SLOTS:-8}
start() {
  cd $L
  (LD_LIBRARY_PATH=$LL GGML_CUDA_ENABLE_UNIFIED_MEMORY=1 setsid $LL/llama-server -m /home/bud/models/qwen3.5-2b-gguf/Qwen3.5-2B-Q4_K_M.gguf \
     -ngl 99 -fa on -np $FRONT_SLOTS -c $((FRONT_SLOTS*4096)) --port 19741 --host 127.0.0.1 --no-webui > front.log 2>&1 & echo $! > front.pid)
  (LD_LIBRARY_PATH=$LL GGML_CUDA_ENABLE_UNIFIED_MEMORY=1 setsid $LL/llama-server -m /home/bud/ditto/bud-gaia/.bud-test/models/qwen2.5-3b-instruct-q4_k_m.gguf \
     -ngl 99 -fa on -np 8 -c 32768 --jinja --alias qwen3b --port 19742 --host 127.0.0.1 --no-webui > background.log 2>&1 & echo $! > background.pid)
  for p in 19741 19742; do for i in $(seq 1 180); do curl -fsS http://127.0.0.1:$p/health 2>/dev/null | grep -q '"ok"' && break; sleep 1; done; done
  start_gw_only
  status
}
start_gw_only() {
  cd $L
  ( set -a; . $IX/secrets/deepgram.env; . $IX/secrets/anthropic.env; . $IX/secrets/elevenlabs.env; . $IX/secrets/desk.env; set +a
    export WAAV_INTERACTION_TASK_BACKENDS="$(cat $B/task_backends.json)" WAAV_INTERACTION_ALLOW_CLIENT_BACKENDS=1
    export HOST=127.0.0.1 PORT=4410 WAAV_ENV_FILE= ORT_DYLIB_PATH=$IX/ort/onnxruntime-linux-aarch64-1.22.0/lib/libonnxruntime.so \
      CACHE_PATH=$HOME/.cache/waav-gateway WAAV_ALLOW_LOOPBACK_ENDPOINTS=1 OPENAI_BASE_URL=http://127.0.0.1:8124/v1 OPENAI_API_KEY=local-dummy \
      WAAV_INTERACTION_ENABLED=1 WAAV_INTERACTION_ALLOW_CLIENT_TOOLS=1 WAAV_INTERACTION_FRONT_URL=http://127.0.0.1:${FRONT_PORT:-19741} \
      RUST_LOG=info,ort=warn,waav_gateway::core::interaction=debug NO_COLOR=1
    setsid ${GW_BIN:-$IX/bin-1.1.0/waav-gateway} > gateway.log 2>&1 & echo $! > gateway.pid )
  for i in $(seq 1 60); do curl -fsS http://127.0.0.1:4410/livez >/dev/null 2>&1 && break; sleep 0.5; done
}
stop() { for n in gateway front background; do [ -f $L/$n.pid ] && kill $(cat $L/$n.pid) 2>/dev/null; rm -f $L/$n.pid; done; }
status() { for p in 19741 19742; do echo "$p $(curl -s http://127.0.0.1:$p/health)"; done; echo "4410 $(curl -s http://127.0.0.1:4410/livez)"; }
restart_gateway() { [ -f $L/gateway.pid ] && kill $(cat $L/gateway.pid) 2>/dev/null; sleep 1; start_gw_only; }
"$1"
