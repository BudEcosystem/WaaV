#!/bin/bash
# WaaV Gateway Live Testing Runner
# Orchestrates all test phases for comprehensive testing

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AUDIO_DIR="$SCRIPT_DIR/audio"
RESULTS_DIR="$SCRIPT_DIR/results"
SCRIPTS_DIR="$SCRIPT_DIR/scripts"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Gateway configuration
GATEWAY_URL="${GATEWAY_URL:-ws://localhost:3001/ws}"
GATEWAY_HTTP_URL="${GATEWAY_HTTP_URL:-http://localhost:3001}"

echo -e "${BLUE}======================================${NC}"
echo -e "${BLUE}WaaV Gateway Live Testing Suite${NC}"
echo -e "${BLUE}======================================${NC}"
echo ""
echo "Gateway URL: $GATEWAY_URL"
echo "Audio Dir: $AUDIO_DIR"
echo "Results Dir: $RESULTS_DIR"
echo ""

# Create directories
mkdir -p "$AUDIO_DIR" "$RESULTS_DIR"

# Check dependencies
echo -e "${YELLOW}Checking dependencies...${NC}"

check_command() {
    if command -v "$1" &> /dev/null; then
        echo -e "  ${GREEN}✓${NC} $1 found"
        return 0
    else
        echo -e "  ${RED}✗${NC} $1 not found"
        return 1
    fi
}

DEPS_OK=true
check_command python3 || DEPS_OK=false
check_command ffmpeg || DEPS_OK=false
check_command curl || DEPS_OK=false

# Check Python packages
echo ""
echo -e "${YELLOW}Checking Python packages...${NC}"

check_python_package() {
    if python3 -c "import $1" 2>/dev/null; then
        echo -e "  ${GREEN}✓${NC} $1"
        return 0
    else
        echo -e "  ${RED}✗${NC} $1 (install with: pip install $1)"
        return 1
    fi
}

check_python_package websockets || DEPS_OK=false
check_python_package numpy || DEPS_OK=false

if [ "$DEPS_OK" = false ]; then
    echo ""
    echo -e "${RED}Missing dependencies. Please install them and try again.${NC}"
    exit 1
fi

# Check gateway connectivity
echo ""
echo -e "${YELLOW}Checking gateway connectivity...${NC}"

if curl -s --max-time 5 "$GATEWAY_HTTP_URL/" | grep -q "OK"; then
    echo -e "  ${GREEN}✓${NC} Gateway is reachable at $GATEWAY_HTTP_URL"
else
    echo -e "  ${RED}✗${NC} Gateway not reachable at $GATEWAY_HTTP_URL"
    echo ""
    echo "Please ensure the WaaV Gateway is running:"
    echo "  cd /home/bud/Desktop/bud_waav/WaaV/gateway"
    echo "  cargo run --release --features 'turn-ensemble,plugins-dynamic,noise-filter,openapi,dag-routing,simd-native'"
    echo ""
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Phase 1: Generate/Download Test Audio
echo ""
echo -e "${BLUE}======================================${NC}"
echo -e "${BLUE}Phase 1: Test Audio Setup${NC}"
echo -e "${BLUE}======================================${NC}"

# Check if audio files exist
AUDIO_COUNT=$(find "$AUDIO_DIR" -name "*.wav" 2>/dev/null | wc -l)

if [ "$AUDIO_COUNT" -lt 5 ]; then
    echo "Generating synthetic audio samples..."
    python3 "$SCRIPTS_DIR/audio_generator.py"

    echo ""
    echo "Downloading real audio samples from the web..."
    python3 "$SCRIPTS_DIR/download_real_audio.py"
else
    echo "Found $AUDIO_COUNT audio files in $AUDIO_DIR"
    echo "Skipping audio generation (use --force-audio to regenerate)"
fi

# List available audio files
echo ""
echo "Available test audio files:"
ls -lh "$AUDIO_DIR"/*.wav 2>/dev/null | awk '{print "  " $9 " (" $5 ")"}'

# Phase 2: Run Tests
echo ""
echo -e "${BLUE}======================================${NC}"
echo -e "${BLUE}Phase 2: Running Live Tests${NC}"
echo -e "${BLUE}======================================${NC}"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_FILE="$RESULTS_DIR/test_results_$TIMESTAMP.json"

python3 "$SCRIPTS_DIR/waav_test_client.py" \
    --audio-dir "$AUDIO_DIR" \
    --results-dir "$RESULTS_DIR" \
    --gateway "$GATEWAY_URL"

# Phase 3: Generate Report
echo ""
echo -e "${BLUE}======================================${NC}"
echo -e "${BLUE}Phase 3: Test Report${NC}"
echo -e "${BLUE}======================================${NC}"

# Find latest results file
LATEST_RESULTS=$(ls -t "$RESULTS_DIR"/test_results_*.json 2>/dev/null | head -1)

if [ -n "$LATEST_RESULTS" ]; then
    echo "Results saved to: $LATEST_RESULTS"
    echo ""

    # Parse and display summary
    python3 -c "
import json
import sys

with open('$LATEST_RESULTS') as f:
    data = json.load(f)

summary = data.get('summary', {})
print(f\"Total Tests: {summary.get('total', 0)}\")
print(f\"Passed: {summary.get('passed', 0)}\")
print(f\"Failed: {summary.get('failed', 0)}\")
print()

# Show failed tests
failed = [r for r in data.get('results', []) if not r.get('success')]
if failed:
    print('Failed Tests:')
    for r in failed:
        print(f\"  - {r.get('test_name')}: {r.get('message', 'Unknown error')}\")
"
else
    echo "No results file found."
fi

echo ""
echo -e "${GREEN}Testing complete!${NC}"
echo ""
echo "Results location: $RESULTS_DIR"
echo "Audio files: $AUDIO_DIR"
