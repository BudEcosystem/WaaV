#!/bin/bash
#
# WaaV Gateway Resource Monitor
# Continuously monitors CPU, memory, file descriptors, and network for the gateway process
#
# Usage: ./scripts/monitor_resources.sh [output_dir]
#
# Outputs timestamped metrics to:
#   - resources.csv: CPU%, Memory MB, FD count
#   - network.csv: Network stats
#   - system.txt: System info snapshot

set -e

OUTPUT_DIR="${1:-/tmp/waav_monitor_$(date +%s)}"
INTERVAL=1  # Sample every 1 second
GATEWAY_NAME="waav-gateway"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}WaaV Gateway Resource Monitor${NC}"
echo "========================================"

# Create output directory
mkdir -p "$OUTPUT_DIR"
echo "Output directory: $OUTPUT_DIR"

# Find gateway PID
find_gateway_pid() {
    pgrep -f "$GATEWAY_NAME" 2>/dev/null | head -1
}

# Get initial PID
GATEWAY_PID=$(find_gateway_pid)
if [ -z "$GATEWAY_PID" ]; then
    echo -e "${YELLOW}Warning: Gateway process not found (looking for '$GATEWAY_NAME')${NC}"
    echo "Waiting for gateway to start..."

    # Wait for gateway to start (up to 60 seconds)
    for i in {1..60}; do
        GATEWAY_PID=$(find_gateway_pid)
        if [ -n "$GATEWAY_PID" ]; then
            break
        fi
        sleep 1
    done

    if [ -z "$GATEWAY_PID" ]; then
        echo -e "${RED}Error: Gateway not found after 60 seconds${NC}"
        exit 1
    fi
fi

echo -e "${GREEN}Monitoring PID: $GATEWAY_PID${NC}"

# Write system info
echo "System Information" > "$OUTPUT_DIR/system.txt"
echo "==================" >> "$OUTPUT_DIR/system.txt"
echo "" >> "$OUTPUT_DIR/system.txt"
echo "Date: $(date)" >> "$OUTPUT_DIR/system.txt"
echo "Hostname: $(hostname)" >> "$OUTPUT_DIR/system.txt"
echo "Kernel: $(uname -r)" >> "$OUTPUT_DIR/system.txt"
echo "CPU: $(grep "model name" /proc/cpuinfo | head -1 | cut -d: -f2 | xargs)" >> "$OUTPUT_DIR/system.txt"
echo "CPU Cores: $(nproc)" >> "$OUTPUT_DIR/system.txt"
echo "Total RAM: $(free -h | grep Mem | awk '{print $2}')" >> "$OUTPUT_DIR/system.txt"
echo "Available RAM: $(free -h | grep Mem | awk '{print $7}')" >> "$OUTPUT_DIR/system.txt"
echo "" >> "$OUTPUT_DIR/system.txt"
echo "File Descriptor Limits:" >> "$OUTPUT_DIR/system.txt"
echo "  Soft: $(ulimit -Sn)" >> "$OUTPUT_DIR/system.txt"
echo "  Hard: $(ulimit -Hn)" >> "$OUTPUT_DIR/system.txt"
echo "" >> "$OUTPUT_DIR/system.txt"
echo "Process command:" >> "$OUTPUT_DIR/system.txt"
cat /proc/$GATEWAY_PID/cmdline 2>/dev/null | tr '\0' ' ' >> "$OUTPUT_DIR/system.txt"
echo "" >> "$OUTPUT_DIR/system.txt"

# Initialize CSV files with headers
echo "timestamp,elapsed_sec,cpu_percent,memory_rss_mb,memory_vms_mb,fd_count,threads" > "$OUTPUT_DIR/resources.csv"
echo "timestamp,elapsed_sec,tcp_established,tcp_time_wait,tcp_close_wait" > "$OUTPUT_DIR/network.csv"

# Record start time
START_TIME=$(date +%s.%N)

# Signal handler for clean shutdown
cleanup() {
    echo ""
    echo -e "${GREEN}Monitoring stopped${NC}"
    echo "Results saved to: $OUTPUT_DIR"
    echo ""
    echo "Files:"
    ls -la "$OUTPUT_DIR"
    exit 0
}

trap cleanup SIGINT SIGTERM

echo ""
echo "Press Ctrl+C to stop monitoring"
echo ""
echo "Timestamp          | CPU%  | Mem MB | FDs   | Threads | TCP Est"
echo "-------------------|-------|--------|-------|---------|--------"

# Main monitoring loop
while true; do
    # Check if process still exists
    if ! kill -0 "$GATEWAY_PID" 2>/dev/null; then
        # Try to find new PID (in case gateway restarted)
        NEW_PID=$(find_gateway_pid)
        if [ -n "$NEW_PID" ] && [ "$NEW_PID" != "$GATEWAY_PID" ]; then
            echo -e "${YELLOW}Gateway restarted, new PID: $NEW_PID${NC}"
            GATEWAY_PID=$NEW_PID
        else
            echo -e "${RED}Gateway process terminated${NC}"
            cleanup
        fi
    fi

    TIMESTAMP=$(date +"%Y-%m-%d %H:%M:%S")
    CURRENT_TIME=$(date +%s.%N)
    ELAPSED=$(echo "$CURRENT_TIME - $START_TIME" | bc)

    # Get CPU and memory from /proc/stat
    if [ -f "/proc/$GATEWAY_PID/stat" ]; then
        STAT=$(cat /proc/$GATEWAY_PID/stat 2>/dev/null)
        STATM=$(cat /proc/$GATEWAY_PID/statm 2>/dev/null)

        # Parse memory (pages to MB, assuming 4KB pages)
        RSS_PAGES=$(echo "$STATM" | awk '{print $2}')
        VMS_PAGES=$(echo "$STATM" | awk '{print $1}')
        RSS_MB=$(echo "scale=2; $RSS_PAGES * 4 / 1024" | bc)
        VMS_MB=$(echo "scale=2; $VMS_PAGES * 4 / 1024" | bc)

        # Get thread count
        THREADS=$(echo "$STAT" | awk '{print $20}')

        # Count file descriptors
        FD_COUNT=$(ls /proc/$GATEWAY_PID/fd 2>/dev/null | wc -l)

        # Get CPU usage using ps (more reliable than parsing /proc/stat)
        CPU_PERCENT=$(ps -p $GATEWAY_PID -o %cpu --no-headers 2>/dev/null | xargs || echo "0")

        # Get TCP connection stats
        TCP_ESTABLISHED=$(ss -tn state established 2>/dev/null | grep -c ":3001" || echo "0")
        TCP_TIME_WAIT=$(ss -tn state time-wait 2>/dev/null | grep -c ":3001" || echo "0")
        TCP_CLOSE_WAIT=$(ss -tn state close-wait 2>/dev/null | grep -c ":3001" || echo "0")

        # Write to CSV files
        echo "$TIMESTAMP,$ELAPSED,$CPU_PERCENT,$RSS_MB,$VMS_MB,$FD_COUNT,$THREADS" >> "$OUTPUT_DIR/resources.csv"
        echo "$TIMESTAMP,$ELAPSED,$TCP_ESTABLISHED,$TCP_TIME_WAIT,$TCP_CLOSE_WAIT" >> "$OUTPUT_DIR/network.csv"

        # Display live output (ensure valid numbers)
        CPU_PERCENT="${CPU_PERCENT:-0}"
        RSS_MB="${RSS_MB:-0}"
        FD_COUNT="${FD_COUNT:-0}"
        THREADS="${THREADS:-0}"
        TCP_ESTABLISHED="${TCP_ESTABLISHED:-0}"
        echo "$TIMESTAMP | $CPU_PERCENT | $RSS_MB | $FD_COUNT | $THREADS | $TCP_ESTABLISHED"
    else
        echo -e "${YELLOW}Cannot read process stats${NC}"
    fi

    sleep $INTERVAL
done
