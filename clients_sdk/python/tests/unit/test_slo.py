"""
Tests for SLO (Service Level Objective) tracker module.
"""

import pytest

from bud_waav.metrics.slo import (
    SLOComparison,
    SLODefinition,
    SLOTracker,
    SLOViolation,
)
from bud_waav.ws.session import PercentileStats


class TestSLODefinition:
    """Tests for SLO definition."""

    def test_defaults(self):
        """Should have LT comparison by default."""
        slo = SLODefinition(name="test", metric="stt.ttft", threshold=500.0)
        assert slo.comparison == SLOComparison.LT
        assert slo.percentile is None

    def test_with_percentile(self):
        """Should accept percentile parameter."""
        slo = SLODefinition(
            name="STT TTFT p99",
            metric="stt.ttft",
            threshold=500.0,
            percentile="p99",
        )
        assert slo.percentile == "p99"


class TestSLOTracker:
    """Tests for the SLO tracker."""

    def test_add_and_list_slos(self):
        """Should add and list SLO definitions."""
        tracker = SLOTracker()
        slo1 = SLODefinition(name="slo-1", metric="m1", threshold=100.0)
        slo2 = SLODefinition(name="slo-2", metric="m2", threshold=200.0)

        tracker.add_slo(slo1)
        tracker.add_slo(slo2)

        slos = tracker.get_slos()
        assert len(slos) == 2

    def test_remove_slo(self):
        """Should remove an SLO by name."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(name="slo-1", metric="m1", threshold=100.0))
        tracker.add_slo(SLODefinition(name="slo-2", metric="m2", threshold=200.0))

        assert tracker.remove_slo("slo-1") is True
        assert len(tracker.get_slos()) == 1
        assert tracker.remove_slo("nonexistent") is False

    def test_check_no_violation_raw_value(self):
        """Should pass when raw value is within threshold."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="latency", metric="stt.ttft", threshold=500.0,
            comparison=SLOComparison.LT,
        ))

        violations = tracker.check({"stt.ttft": 200.0})
        assert len(violations) == 0

    def test_check_violation_raw_value(self):
        """Should detect violation when raw value exceeds threshold."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="latency", metric="stt.ttft", threshold=500.0,
            comparison=SLOComparison.LT,
        ))

        violations = tracker.check({"stt.ttft": 600.0})
        assert len(violations) == 1
        assert violations[0].slo_name == "latency"
        assert violations[0].actual_value == 600.0
        assert violations[0].threshold == 500.0

    def test_check_violation_at_boundary(self):
        """Value equal to threshold should violate LT comparison."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="latency", metric="m", threshold=500.0,
            comparison=SLOComparison.LT,
        ))

        violations = tracker.check({"m": 500.0})
        assert len(violations) == 1  # 500 >= 500, so LT is violated

    def test_check_lte_comparison(self):
        """LTE: value must be <= threshold."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="lte", metric="m", threshold=500.0,
            comparison=SLOComparison.LTE,
        ))

        assert len(tracker.check({"m": 500.0})) == 0  # 500 <= 500, passes
        assert len(tracker.check({"m": 501.0})) == 1  # 501 > 500, violated

    def test_check_gt_comparison(self):
        """GT: value must be > threshold."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="throughput", metric="tps", threshold=100.0,
            comparison=SLOComparison.GT,
        ))

        assert len(tracker.check({"tps": 150.0})) == 0  # 150 > 100, passes
        assert len(tracker.check({"tps": 50.0})) == 1   # 50 <= 100, violated

    def test_check_gte_comparison(self):
        """GTE: value must be >= threshold."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="gte", metric="m", threshold=100.0,
            comparison=SLOComparison.GTE,
        ))

        assert len(tracker.check({"m": 100.0})) == 0  # 100 >= 100, passes
        assert len(tracker.check({"m": 99.0})) == 1   # 99 < 100, violated

    def test_check_eq_comparison(self):
        """EQ: value must equal threshold."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="exact", metric="m", threshold=1.0,
            comparison=SLOComparison.EQ,
        ))

        assert len(tracker.check({"m": 1.0})) == 0  # Equal, passes
        assert len(tracker.check({"m": 2.0})) == 1  # Not equal, violated

    def test_check_with_percentile_stats(self):
        """Should extract percentile value from PercentileStats object."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="p99", metric="stt.ttft", threshold=500.0,
            comparison=SLOComparison.LT, percentile="p99",
        ))

        stats = PercentileStats(p50=100.0, p95=300.0, p99=450.0, min=50.0, max=480.0, mean=200.0)
        violations = tracker.check({"stt.ttft": stats})
        assert len(violations) == 0  # p99=450 < 500

        stats_bad = PercentileStats(p50=100.0, p95=300.0, p99=550.0, min=50.0, max=600.0, mean=200.0)
        violations = tracker.check({"stt.ttft": stats_bad})
        assert len(violations) == 1  # p99=550 >= 500

    def test_check_with_percentile_dict(self):
        """Should extract percentile from dict metrics."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="p95", metric="tts.ttfb", threshold=1000.0,
            comparison=SLOComparison.LT, percentile="p95",
        ))

        violations = tracker.check({"tts.ttfb": {"p50": 500, "p95": 800, "p99": 1200}})
        assert len(violations) == 0  # p95=800 < 1000

    def test_check_skips_missing_metrics(self):
        """Should silently skip metrics not in the provided dict."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(name="slo", metric="missing", threshold=100.0))

        violations = tracker.check({"other": 200.0})
        assert len(violations) == 0

    def test_get_health_all_passing(self):
        """Health should be 1.0 when all checks pass."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(name="slo", metric="m", threshold=500.0))

        tracker.check({"m": 100.0})
        tracker.check({"m": 200.0})
        tracker.check({"m": 300.0})

        assert tracker.get_health() == 1.0

    def test_get_health_some_failing(self):
        """Health should reflect pass/fail ratio."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="slo", metric="m", threshold=500.0, comparison=SLOComparison.LT,
        ))

        tracker.check({"m": 100.0})  # Pass
        tracker.check({"m": 600.0})  # Fail
        tracker.check({"m": 200.0})  # Pass
        tracker.check({"m": 700.0})  # Fail

        assert tracker.get_health() == 0.5

    def test_get_health_no_checks(self):
        """Health should be 1.0 when no checks performed."""
        tracker = SLOTracker()
        assert tracker.get_health() == 1.0

    def test_get_violations_all(self):
        """Should return all violations sorted by time."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(name="slo-1", metric="m1", threshold=100.0))
        tracker.add_slo(SLODefinition(name="slo-2", metric="m2", threshold=200.0))

        tracker.check({"m1": 150.0, "m2": 250.0})

        violations = tracker.get_violations()
        assert len(violations) == 2

    def test_get_violations_by_name(self):
        """Should filter violations by SLO name."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(name="slo-1", metric="m1", threshold=100.0))
        tracker.add_slo(SLODefinition(name="slo-2", metric="m2", threshold=200.0))

        tracker.check({"m1": 150.0, "m2": 250.0})

        violations = tracker.get_violations("slo-1")
        assert len(violations) == 1
        assert violations[0].slo_name == "slo-1"

    def test_violation_history_limited(self):
        """Violation history should be limited to max entries."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(name="slo", metric="m", threshold=100.0))

        # Generate more violations than MAX_VIOLATIONS
        for _ in range(150):
            tracker.check({"m": 200.0})

        violations = tracker.get_violations("slo")
        assert len(violations) == 100  # Limited to _MAX_VIOLATIONS

    def test_reset(self):
        """Reset should clear violation history and counters."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(name="slo", metric="m", threshold=100.0))

        tracker.check({"m": 200.0})
        assert tracker.get_health() < 1.0

        tracker.reset()
        assert tracker.get_health() == 1.0
        assert len(tracker.get_violations()) == 0

    def test_multiple_slos_same_metric(self):
        """Should support multiple SLOs on the same metric."""
        tracker = SLOTracker()
        tracker.add_slo(SLODefinition(
            name="p50", metric="latency", threshold=100.0,
            percentile="p50",
        ))
        tracker.add_slo(SLODefinition(
            name="p99", metric="latency", threshold=500.0,
            percentile="p99",
        ))

        stats = PercentileStats(p50=80.0, p95=300.0, p99=600.0)
        violations = tracker.check({"latency": stats})

        assert len(violations) == 1  # Only p99 violated
        assert violations[0].slo_name == "p99"
