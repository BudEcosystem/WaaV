#!/usr/bin/env bash
# Coverage ratchet (plan W-T1, exit E8/E12).
#
# Enforces a NON-DECREASING line-coverage floor for the gateway lib against a committed
# baseline (gateway/coverage-baseline.json). This is a ratchet, not an absolute target:
# the floor only ever goes up (when re-blessed), and CI fails if a change drops measured
# coverage below the committed baseline.
#
# Usage:
#   coverage-ratchet.sh check <llvm-cov-summary.json>   # gate: measured >= baseline
#   coverage-ratchet.sh bless <llvm-cov-summary.json>   # rewrite baseline to measured (commit it)
#
# The summary JSON is produced by:
#   cargo llvm-cov --json --summary-only ... > summary.json
# and the line-coverage percent is read from `.data[0].totals.lines.percent`.
#
# Coverage scope excludes #[ignore]/live_* tests: the `coverage` job runs the lib test set
# (no --ignored, no live cassettes), so the numerator is exactly the non-ignored lib tests.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
baseline_file="${COVERAGE_BASELINE:-$here/../../coverage-baseline.json}"

mode="${1:-}"
summary="${2:-}"

if [[ -z "$mode" || -z "$summary" ]]; then
  echo "usage: $0 {check|bless} <llvm-cov-summary.json>" >&2
  exit 2
fi
if [[ ! -f "$summary" ]]; then
  echo "coverage summary not found: $summary" >&2
  exit 2
fi
if [[ ! -f "$baseline_file" ]]; then
  echo "baseline not found: $baseline_file" >&2
  exit 2
fi

# Measured line-coverage ratio in [0,1]. llvm-cov reports a percent (0..100).
measured_pct="$(jq -r '.data[0].totals.lines.percent' "$summary")"
if [[ -z "$measured_pct" || "$measured_pct" == "null" ]]; then
  echo "could not read .data[0].totals.lines.percent from $summary" >&2
  exit 2
fi
measured_ratio="$(jq -n --argjson p "$measured_pct" '($p / 100.0)')"
baseline_ratio="$(jq -r '.line_ratio' "$baseline_file")"

printf 'coverage: measured=%.4f baseline=%.4f\n' "$measured_ratio" "$baseline_ratio"

case "$mode" in
  bless)
    today="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    tmp="$(mktemp)"
    jq --argjson r "$measured_ratio" --arg t "$today" \
       '.line_ratio = $r | .measured_at = $t' \
       "$baseline_file" > "$tmp"
    mv "$tmp" "$baseline_file"
    printf 'blessed baseline -> %.4f (%s)\n' "$measured_ratio" "$today"
    ;;
  check)
    # Fail only on a real regression. A tiny float epsilon avoids flapping on the last digit.
    if jq -e -n --argjson m "$measured_ratio" --argjson b "$baseline_ratio" \
         '($m + 0.0005) >= $b' >/dev/null; then
      echo "coverage ratchet OK (measured >= baseline)"
      # Nudge contributors to bless upward when they meaningfully improve coverage.
      if jq -e -n --argjson m "$measured_ratio" --argjson b "$baseline_ratio" \
           '$m > ($b + 0.01)' >/dev/null; then
        echo "note: coverage rose >1pt above baseline — consider re-blessing coverage-baseline.json"
      fi
      exit 0
    else
      echo "::error::coverage ratchet FAILED: measured ($measured_ratio) < baseline ($baseline_ratio)" >&2
      echo "A change reduced lib line-coverage below the committed floor." >&2
      echo "Add tests, or (if intentional) lower the baseline deliberately in a reviewed commit." >&2
      exit 1
    fi
    ;;
  *)
    echo "unknown mode: $mode (expected check|bless)" >&2
    exit 2
    ;;
esac
