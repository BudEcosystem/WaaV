#!/bin/bash
# =============================================================================
# Cargo Build Cleanup Daemon
# Monitors target directory every 30 minutes and cleans up automatically
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET_DIR="${PROJECT_DIR}/target"
LOG_FILE="${PROJECT_DIR}/logs/cargo_cleanup_daemon.log"
PID_FILE="${PROJECT_DIR}/.cargo_cleanup_daemon.pid"

# Configuration
CHECK_INTERVAL_SECONDS=1800  # 30 minutes
THRESHOLD_GB=3               # Sweep cleanup when exceeds this size
HARD_LIMIT_GB=5              # Full clean when exceeds this size (hard limit)
MAX_LOG_SIZE_MB=10           # Rotate log when exceeds this size

# Ensure logs directory exists
mkdir -p "$(dirname "$LOG_FILE")"

# =============================================================================
# Logging
# =============================================================================
log() {
    local level="$1"
    shift
    local timestamp
    timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[$timestamp] [$level] $*" >> "$LOG_FILE"

    # Also print to stdout if running in foreground
    if [[ "${FOREGROUND:-false}" == "true" ]]; then
        echo "[$timestamp] [$level] $*"
    fi
}

log_info() { log "INFO" "$@"; }
log_warn() { log "WARN" "$@"; }
log_error() { log "ERROR" "$@"; }

# =============================================================================
# Log rotation
# =============================================================================
rotate_log_if_needed() {
    if [[ -f "$LOG_FILE" ]]; then
        local size_bytes
        size_bytes=$(stat -f%z "$LOG_FILE" 2>/dev/null || stat -c%s "$LOG_FILE" 2>/dev/null || echo "0")
        local max_bytes=$((MAX_LOG_SIZE_MB * 1024 * 1024))

        if [[ $size_bytes -gt $max_bytes ]]; then
            mv "$LOG_FILE" "${LOG_FILE}.old"
            log_info "Log rotated (previous log saved as ${LOG_FILE}.old)"
        fi
    fi
}

# =============================================================================
# Size checking and cleanup
# =============================================================================
get_target_size_bytes() {
    if [[ -d "$TARGET_DIR" ]]; then
        du -sb "$TARGET_DIR" 2>/dev/null | cut -f1
    else
        echo "0"
    fi
}

get_target_size_human() {
    if [[ -d "$TARGET_DIR" ]]; then
        du -sh "$TARGET_DIR" 2>/dev/null | cut -f1
    else
        echo "0"
    fi
}

perform_sweep() {
    log_info "Starting sweep cleanup (artifacts older than 7 days)..."

    local before_size
    before_size=$(get_target_size_human)

    cd "$PROJECT_DIR"

    # Use cargo-sweep if available
    if command -v cargo-sweep &> /dev/null; then
        log_info "Running cargo-sweep --time 7"
        cargo sweep --time 7 >> "$LOG_FILE" 2>&1 || true
    else
        log_warn "cargo-sweep not found, skipping sweep"
        return 1
    fi

    local after_size
    after_size=$(get_target_size_human)

    log_info "Sweep complete: $before_size -> $after_size"
}

perform_full_clean() {
    log_info "Starting FULL CLEAN (hard limit exceeded)..."

    local before_size
    before_size=$(get_target_size_human)

    cd "$PROJECT_DIR"

    log_info "Running cargo clean"
    cargo clean >> "$LOG_FILE" 2>&1 || true

    local after_size
    after_size=$(get_target_size_human)

    log_info "Full clean complete: $before_size -> $after_size"
}

check_and_cleanup() {
    rotate_log_if_needed

    if [[ ! -d "$TARGET_DIR" ]]; then
        log_info "Target directory does not exist. Skipping check."
        return 0
    fi

    local size_bytes
    size_bytes=$(get_target_size_bytes)
    local threshold_bytes=$((THRESHOLD_GB * 1024 * 1024 * 1024))
    local hard_limit_bytes=$((HARD_LIMIT_GB * 1024 * 1024 * 1024))
    local size_human
    size_human=$(get_target_size_human)

    if [[ $size_bytes -gt $hard_limit_bytes ]]; then
        # Critical: exceeds hard limit - do full clean
        log_error "Target directory ($size_human) exceeds ${HARD_LIMIT_GB}GB HARD LIMIT!"
        perform_full_clean

        # Verify cleanup was successful
        local new_size_bytes
        new_size_bytes=$(get_target_size_bytes)
        if [[ $new_size_bytes -gt $hard_limit_bytes ]]; then
            log_error "WARNING: Still exceeds hard limit after full clean!"
        fi
    elif [[ $size_bytes -gt $threshold_bytes ]]; then
        # Soft threshold: do sweep cleanup
        log_warn "Target directory ($size_human) exceeds ${THRESHOLD_GB}GB threshold"
        perform_sweep

        # Check if sweep was enough, if not do full clean
        local new_size_bytes
        new_size_bytes=$(get_target_size_bytes)
        if [[ $new_size_bytes -gt $hard_limit_bytes ]]; then
            log_warn "Sweep wasn't enough, performing full clean"
            perform_full_clean
        fi
    else
        log_info "Target directory ($size_human) is within ${THRESHOLD_GB}GB threshold"
    fi
}

# =============================================================================
# Daemon management
# =============================================================================
is_running() {
    if [[ -f "$PID_FILE" ]]; then
        local pid
        pid=$(cat "$PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
    fi
    return 1
}

start_daemon() {
    if is_running; then
        echo "Daemon is already running (PID: $(cat "$PID_FILE"))"
        exit 1
    fi

    echo "Starting cargo cleanup daemon..."
    echo "  Check interval: ${CHECK_INTERVAL_SECONDS}s ($(( CHECK_INTERVAL_SECONDS / 60 )) minutes)"
    echo "  Threshold: ${THRESHOLD_GB}GB"
    echo "  Log file: $LOG_FILE"

    # Start in background
    nohup "$0" run >> "$LOG_FILE" 2>&1 &
    local pid=$!
    echo "$pid" > "$PID_FILE"

    echo "Daemon started with PID: $pid"
    echo "Check status: $0 status"
    echo "View logs: tail -f $LOG_FILE"
}

stop_daemon() {
    if ! is_running; then
        echo "Daemon is not running"
        rm -f "$PID_FILE"
        exit 0
    fi

    local pid
    pid=$(cat "$PID_FILE")
    echo "Stopping daemon (PID: $pid)..."

    kill "$pid" 2>/dev/null || true

    # Wait for process to stop
    local count=0
    while kill -0 "$pid" 2>/dev/null && [[ $count -lt 10 ]]; do
        sleep 1
        ((count++))
    done

    if kill -0 "$pid" 2>/dev/null; then
        echo "Force killing..."
        kill -9 "$pid" 2>/dev/null || true
    fi

    rm -f "$PID_FILE"
    echo "Daemon stopped"
}

daemon_status() {
    if is_running; then
        local pid
        pid=$(cat "$PID_FILE")
        echo "Daemon is running (PID: $pid)"
        echo ""
        echo "Configuration:"
        echo "  Check interval: $(( CHECK_INTERVAL_SECONDS / 60 )) minutes"
        echo "  Sweep threshold: ${THRESHOLD_GB}GB (triggers cargo-sweep)"
        echo "  Hard limit: ${HARD_LIMIT_GB}GB (triggers cargo clean)"
        echo "  Log file: $LOG_FILE"
        echo ""
        echo "Current target size: $(get_target_size_human)"
        echo ""
        echo "Recent log entries:"
        tail -5 "$LOG_FILE" 2>/dev/null || echo "  (no logs yet)"
    else
        echo "Daemon is not running"
        rm -f "$PID_FILE" 2>/dev/null
    fi
}

run_daemon() {
    log_info "========================================="
    log_info "Cargo cleanup daemon started"
    log_info "Check interval: ${CHECK_INTERVAL_SECONDS}s ($(( CHECK_INTERVAL_SECONDS / 60 )) min)"
    log_info "Sweep threshold: ${THRESHOLD_GB}GB"
    log_info "Hard limit: ${HARD_LIMIT_GB}GB (max allowed)"
    log_info "Project: $PROJECT_DIR"
    log_info "========================================="

    # Save PID
    echo $$ > "$PID_FILE"

    # Handle signals for graceful shutdown
    trap 'log_info "Received shutdown signal"; rm -f "$PID_FILE"; exit 0' SIGTERM SIGINT SIGHUP

    # Initial check
    check_and_cleanup

    # Main loop
    while true; do
        sleep "$CHECK_INTERVAL_SECONDS"
        check_and_cleanup
    done
}

show_usage() {
    cat << EOF
Usage: $(basename "$0") [COMMAND]

Commands:
  start       Start the cleanup daemon in background
  stop        Stop the running daemon
  restart     Restart the daemon
  status      Show daemon status and recent logs
  run         Run in foreground (for debugging)
  once        Run a single check and exit

Configuration (edit script to change):
  CHECK_INTERVAL_SECONDS=$CHECK_INTERVAL_SECONDS ($(( CHECK_INTERVAL_SECONDS / 60 )) minutes)
  THRESHOLD_GB=$THRESHOLD_GB

Files:
  Log: $LOG_FILE
  PID: $PID_FILE

Examples:
  $(basename "$0") start    # Start daemon
  $(basename "$0") status   # Check if running
  $(basename "$0") stop     # Stop daemon
EOF
}

# =============================================================================
# Main
# =============================================================================
main() {
    case "${1:-}" in
        start)
            start_daemon
            ;;
        stop)
            stop_daemon
            ;;
        restart)
            stop_daemon
            sleep 1
            start_daemon
            ;;
        status)
            daemon_status
            ;;
        run)
            FOREGROUND=true run_daemon
            ;;
        once)
            FOREGROUND=true check_and_cleanup
            ;;
        help|--help|-h)
            show_usage
            ;;
        "")
            show_usage
            ;;
        *)
            echo "Unknown command: $1"
            show_usage
            exit 1
            ;;
    esac
}

main "$@"
