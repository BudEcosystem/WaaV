#!/bin/bash
# WaaV - One-Command Startup with Full Regression Testing
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATEWAY_DIR="$SCRIPT_DIR/gateway"
DASHBOARD_DIR="$SCRIPT_DIR/clients_sdk/dashboard"
CERT_PATH="$DASHBOARD_DIR/cert.pem"
KEY_PATH="$DASHBOARD_DIR/key.pem"

# Get local IP for network access
LOCAL_IP=$(ip -4 addr show | grep -oP 'inet \K[\d.]+' | grep -v '127.0.0.1' | head -1)

echo "============================================"
echo "  WaaV - Full Regression Test & Run"
echo "============================================"
echo ""

cd "$GATEWAY_DIR"

# Step 1: Run all tests
echo "[1/5] Running full test suite..."
echo "      (1031 unit tests + 108 doc tests)"
echo ""
cargo test --jobs 2 -- --test-threads=4

echo ""
echo "[2/5] Building release binary..."
cargo build --release

echo ""
echo "[3/5] Starting WaaV Gateway with TLS..."
export TLS_ENABLED=true
export TLS_CERT_PATH="$CERT_PATH"
export TLS_KEY_PATH="$KEY_PATH"
export RUST_LOG=info

./target/release/waav-gateway &
GATEWAY_PID=$!

# Give gateway time to start
sleep 4

echo ""
echo "[4/5] Starting Dashboard..."
cd "$DASHBOARD_DIR"
python3 serve_https.py &
DASHBOARD_PID=$!
sleep 2

echo ""
echo "[5/5] Health checks..."
GATEWAY_OK=false
DASHBOARD_OK=false

if curl -sk https://localhost:3001/ 2>/dev/null | grep -q "OK"; then
    GATEWAY_OK=true
fi

if curl -sk https://localhost:8443/ 2>/dev/null | grep -q "DOCTYPE"; then
    DASHBOARD_OK=true
fi

if $GATEWAY_OK && $DASHBOARD_OK; then
    echo ""
    echo "============================================"
    echo "  WaaV is running!"
    echo "============================================"
    echo ""
    echo "  Gateway:   https://localhost:3001"
    echo "             https://$LOCAL_IP:3001"
    echo ""
    echo "  Dashboard: https://localhost:8443"
    echo "             https://$LOCAL_IP:8443"
    echo ""
    echo "  WebSocket: wss://$LOCAL_IP:3001/ws"
    echo ""
    echo "  Press Ctrl+C to stop"
    echo ""

    # Trap Ctrl+C to kill both processes
    trap "kill $GATEWAY_PID $DASHBOARD_PID 2>/dev/null; exit 0" INT TERM
    wait $GATEWAY_PID
else
    echo "Health check failed!"
    [ "$GATEWAY_OK" = false ] && echo "  - Gateway not responding"
    [ "$DASHBOARD_OK" = false ] && echo "  - Dashboard not responding"
    kill $GATEWAY_PID $DASHBOARD_PID 2>/dev/null
    exit 1
fi
