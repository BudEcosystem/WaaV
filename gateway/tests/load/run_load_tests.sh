#!/bin/bash
#
# WaaV Gateway Load Test Runner
#
# Usage: ./run_load_tests.sh [test_type]
#
# test_type options:
#   rest      - Run REST API throughput test only
#   websocket - Run WebSocket load test only
#   mixed     - Run mixed workload test only
#   all       - Run all load tests (default)
#   quick     - Run quick smoke test (30 seconds each)
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATEWAY_DIR="$(dirname "$(dirname "$SCRIPT_DIR")")"

# Configuration
HTTP_URL="${HTTP_URL:-http://localhost:3001}"
WS_URL="${WS_URL:-ws://localhost:3001}"
PORT="${PORT:-3001}"
TEST_TYPE="${1:-all}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "=========================================="
echo "  WaaV Gateway Load Test Runner"
echo "=========================================="
echo ""
echo "HTTP URL: $HTTP_URL"
echo "WS URL:   $WS_URL"
echo "Test:     $TEST_TYPE"
echo ""

# Check if k6 is installed
if ! command -v k6 &> /dev/null; then
    echo -e "${RED}Error: k6 is not installed${NC}"
    echo ""
    echo "Install k6:"
    echo "  Ubuntu/Debian: sudo apt-get install k6"
    echo "  macOS:         brew install k6"
    echo "  Other:         https://k6.io/docs/get-started/installation/"
    exit 1
fi

# Check if server is running
check_server() {
    echo -n "Checking server at $HTTP_URL... "
    if curl -s "$HTTP_URL/" > /dev/null 2>&1; then
        echo -e "${GREEN}OK${NC}"
        return 0
    else
        echo -e "${RED}NOT RUNNING${NC}"
        echo ""
        echo "Start the server first:"
        echo "  cargo run --release"
        echo ""
        echo "Or start in background:"
        echo "  cargo run --release &"
        return 1
    fi
}

# Run REST throughput test
run_rest_test() {
    local duration="${1:-full}"
    echo ""
    echo "=========================================="
    echo "  REST API Throughput Test"
    echo "=========================================="

    local opts=""
    if [ "$duration" == "quick" ]; then
        opts="--duration 30s --vus 20"
    fi

    k6 run $opts \
        -e BASE_URL="$HTTP_URL" \
        "$SCRIPT_DIR/rest_throughput.js"
}

# Run WebSocket load test
run_ws_test() {
    local duration="${1:-full}"
    echo ""
    echo "=========================================="
    echo "  WebSocket Load Test"
    echo "=========================================="

    local opts=""
    if [ "$duration" == "quick" ]; then
        opts="--duration 30s --vus 10"
    fi

    k6 run $opts \
        -e BASE_URL="$WS_URL" \
        "$SCRIPT_DIR/websocket_load.js"
}

# Run mixed workload test
run_mixed_test() {
    local duration="${1:-full}"
    echo ""
    echo "=========================================="
    echo "  Mixed Workload Test"
    echo "=========================================="

    local opts=""
    if [ "$duration" == "quick" ]; then
        opts="--duration 30s --vus 15"
    fi

    k6 run $opts \
        -e HTTP_URL="$HTTP_URL" \
        -e WS_URL="$WS_URL" \
        "$SCRIPT_DIR/mixed_workload.js"
}

# Main execution
check_server || exit 1

case "$TEST_TYPE" in
    rest)
        run_rest_test
        ;;
    websocket|ws)
        run_ws_test
        ;;
    mixed)
        run_mixed_test
        ;;
    quick)
        echo ""
        echo "Running quick smoke tests (30 seconds each)..."
        run_rest_test quick
        run_ws_test quick
        run_mixed_test quick
        ;;
    all)
        run_rest_test
        run_ws_test
        run_mixed_test
        ;;
    *)
        echo -e "${RED}Unknown test type: $TEST_TYPE${NC}"
        echo ""
        echo "Valid options: rest, websocket, mixed, quick, all"
        exit 1
        ;;
esac

echo ""
echo "=========================================="
echo -e "  ${GREEN}Load tests completed!${NC}"
echo "=========================================="
echo ""
echo "Results saved to:"
echo "  tests/load/*_summary.json"
