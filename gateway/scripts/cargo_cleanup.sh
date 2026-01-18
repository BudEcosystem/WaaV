#!/bin/bash
# =============================================================================
# Cargo Build Cleanup Script
# Prevents target directory from growing unbounded with incremental builds
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET_DIR="${PROJECT_DIR}/target"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

get_target_size() {
    if [[ -d "$TARGET_DIR" ]]; then
        du -sh "$TARGET_DIR" 2>/dev/null | cut -f1
    else
        echo "0"
    fi
}

show_usage() {
    cat << EOF
Usage: $(basename "$0") [COMMAND] [OPTIONS]

Commands:
  status      Show current target directory size and breakdown
  sweep       Clean build artifacts older than 7 days (default)
  sweep-all   Clean all non-installed build artifacts
  clean       Full cargo clean (removes entire target directory)
  doc         Remove only documentation artifacts
  deps        Remove dependency artifacts only, keep project builds
  check [GB]  Check if size exceeds threshold (default: 5GB), exit 1 if exceeded
  auto [GB]   Automatic cleanup if threshold exceeded (default: 5GB)
  help        Show this help message

Examples:
  $(basename "$0")          # Same as 'sweep'
  $(basename "$0") status   # Check current size
  $(basename "$0") sweep    # Remove artifacts older than 7 days
  $(basename "$0") clean    # Full clean (use when size is critical)
  $(basename "$0") check 3  # Check if >3GB, exit 1 if true (for CI)
  $(basename "$0") auto 5   # Clean automatically if >5GB

Recommended workflow:
  - Run 'sweep' weekly or when target/ exceeds 5GB
  - Run 'clean' before major releases or when issues occur
  - Add 'auto 5' to pre-build hooks for automatic management
EOF
}

cmd_status() {
    log_info "Checking target directory status..."

    if [[ ! -d "$TARGET_DIR" ]]; then
        log_info "Target directory does not exist. Nothing to clean."
        return 0
    fi

    echo ""
    echo "=== Target Directory Size ==="
    echo "Total: $(get_target_size)"
    echo ""
    echo "=== Breakdown by subdirectory ==="
    du -sh "$TARGET_DIR"/*/ 2>/dev/null | sort -hr | head -10 || echo "No subdirectories found"
    echo ""

    # Check incremental compilation cache
    if [[ -d "$TARGET_DIR/debug/incremental" ]]; then
        INCREMENTAL_SIZE=$(du -sh "$TARGET_DIR/debug/incremental" 2>/dev/null | cut -f1)
        echo "Incremental cache (debug): $INCREMENTAL_SIZE"
    fi
    if [[ -d "$TARGET_DIR/release/incremental" ]]; then
        INCREMENTAL_SIZE=$(du -sh "$TARGET_DIR/release/incremental" 2>/dev/null | cut -f1)
        echo "Incremental cache (release): $INCREMENTAL_SIZE"
    fi

    # Count number of files
    FILE_COUNT=$(find "$TARGET_DIR" -type f 2>/dev/null | wc -l)
    echo ""
    echo "Total files: $FILE_COUNT"
}

cmd_sweep() {
    log_info "Running cargo-sweep (removing artifacts older than 7 days)..."

    BEFORE_SIZE=$(get_target_size)
    log_info "Size before: $BEFORE_SIZE"

    cd "$PROJECT_DIR"

    # Check if cargo-sweep is installed
    if ! command -v cargo-sweep &> /dev/null; then
        log_warn "cargo-sweep not found. Installing..."
        cargo install cargo-sweep
    fi

    # Run sweep - remove artifacts older than 7 days
    cargo sweep --time 7 || true

    AFTER_SIZE=$(get_target_size)
    log_info "Size after: $AFTER_SIZE"
    log_info "Sweep complete!"
}

cmd_sweep_all() {
    log_info "Running cargo-sweep --installed (removing all non-installed artifacts)..."

    BEFORE_SIZE=$(get_target_size)
    log_info "Size before: $BEFORE_SIZE"

    cd "$PROJECT_DIR"

    if ! command -v cargo-sweep &> /dev/null; then
        log_warn "cargo-sweep not found. Installing..."
        cargo install cargo-sweep
    fi

    # Remove all artifacts except those from installed toolchains
    cargo sweep --installed || true

    AFTER_SIZE=$(get_target_size)
    log_info "Size after: $AFTER_SIZE"
    log_info "Sweep complete!"
}

cmd_clean() {
    log_info "Running full cargo clean..."

    BEFORE_SIZE=$(get_target_size)
    log_info "Size before: $BEFORE_SIZE"

    cd "$PROJECT_DIR"
    cargo clean

    AFTER_SIZE=$(get_target_size)
    log_info "Size after: $AFTER_SIZE"
    log_info "Full clean complete!"
}

cmd_doc() {
    log_info "Cleaning documentation artifacts..."

    if [[ -d "$TARGET_DIR/doc" ]]; then
        BEFORE_SIZE=$(du -sh "$TARGET_DIR/doc" 2>/dev/null | cut -f1)
        log_info "Doc size before: $BEFORE_SIZE"

        cd "$PROJECT_DIR"
        cargo clean --doc

        log_info "Documentation artifacts removed!"
    else
        log_info "No documentation artifacts found."
    fi
}

cmd_deps() {
    log_info "Cleaning dependency artifacts (keeping project builds)..."

    BEFORE_SIZE=$(get_target_size)
    log_info "Size before: $BEFORE_SIZE"

    cd "$PROJECT_DIR"

    # Remove deps directories which contain compiled dependencies
    if [[ -d "$TARGET_DIR/debug/deps" ]]; then
        rm -rf "$TARGET_DIR/debug/deps"
        log_info "Removed debug/deps"
    fi
    if [[ -d "$TARGET_DIR/release/deps" ]]; then
        rm -rf "$TARGET_DIR/release/deps"
        log_info "Removed release/deps"
    fi

    # Also clean build directory (build scripts output)
    if [[ -d "$TARGET_DIR/debug/build" ]]; then
        rm -rf "$TARGET_DIR/debug/build"
        log_info "Removed debug/build"
    fi
    if [[ -d "$TARGET_DIR/release/build" ]]; then
        rm -rf "$TARGET_DIR/release/build"
        log_info "Removed release/build"
    fi

    AFTER_SIZE=$(get_target_size)
    log_info "Size after: $AFTER_SIZE"
}

cmd_check() {
    # Check if target directory exceeds threshold (default 5GB)
    # Exit with error code 1 if exceeded (useful for CI)
    local THRESHOLD_GB="${2:-5}"

    if [[ ! -d "$TARGET_DIR" ]]; then
        log_info "Target directory does not exist. Check passed."
        exit 0
    fi

    # Get size in bytes
    local SIZE_BYTES
    SIZE_BYTES=$(du -sb "$TARGET_DIR" 2>/dev/null | cut -f1)
    local THRESHOLD_BYTES=$((THRESHOLD_GB * 1024 * 1024 * 1024))

    local SIZE_HUMAN
    SIZE_HUMAN=$(get_target_size)

    if [[ $SIZE_BYTES -gt $THRESHOLD_BYTES ]]; then
        log_error "Target directory ($SIZE_HUMAN) exceeds ${THRESHOLD_GB}GB threshold!"
        log_warn "Run './scripts/cargo_cleanup.sh sweep' to clean up"
        exit 1
    else
        log_info "Target directory size ($SIZE_HUMAN) is within ${THRESHOLD_GB}GB threshold."
        exit 0
    fi
}

cmd_auto() {
    # Automatic cleanup: check threshold and sweep if exceeded
    local THRESHOLD_GB="${2:-5}"

    if [[ ! -d "$TARGET_DIR" ]]; then
        log_info "Target directory does not exist. Nothing to do."
        return 0
    fi

    local SIZE_BYTES
    SIZE_BYTES=$(du -sb "$TARGET_DIR" 2>/dev/null | cut -f1)
    local THRESHOLD_BYTES=$((THRESHOLD_GB * 1024 * 1024 * 1024))

    local SIZE_HUMAN
    SIZE_HUMAN=$(get_target_size)

    if [[ $SIZE_BYTES -gt $THRESHOLD_BYTES ]]; then
        log_warn "Target directory ($SIZE_HUMAN) exceeds ${THRESHOLD_GB}GB. Running automatic cleanup..."
        cmd_sweep
    else
        log_info "Target directory ($SIZE_HUMAN) is within ${THRESHOLD_GB}GB. No cleanup needed."
    fi
}

# =============================================================================
# Main entry point
# =============================================================================
main() {
    cd "$PROJECT_DIR"

    case "${1:-sweep}" in
        status)
            cmd_status
            ;;
        sweep)
            cmd_sweep
            ;;
        sweep-all)
            cmd_sweep_all
            ;;
        clean)
            cmd_clean
            ;;
        doc)
            cmd_doc
            ;;
        deps)
            cmd_deps
            ;;
        check)
            cmd_check "$@"
            ;;
        auto)
            cmd_auto "$@"
            ;;
        help|--help|-h)
            show_usage
            ;;
        *)
            log_error "Unknown command: $1"
            show_usage
            exit 1
            ;;
    esac
}

main "$@"
