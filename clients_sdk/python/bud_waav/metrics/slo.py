"""
Service Level Objective (SLO) tracker for Bud WaaV SDK.

Tracks performance metrics against configurable thresholds and reports
violations. Useful for production monitoring and alerting.
"""

import time
from collections import deque
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Optional


class SLOComparison(Enum):
    """Comparison operators for SLO thresholds."""

    LT = "lt"
    LTE = "lte"
    GT = "gt"
    GTE = "gte"
    EQ = "eq"


@dataclass
class SLODefinition:
    """Definition of a Service Level Objective.

    Attributes:
        name: Human-readable name for this SLO (e.g., "STT TTFT p99").
        metric: Metric key to track (e.g., "stt.ttft", "tts.ttfb").
        threshold: Threshold value.
        comparison: Comparison operator. Default: LT (value must be less than threshold).
        percentile: Which percentile to check (p50, p95, p99). If None, checks raw values.
    """

    name: str
    metric: str
    threshold: float
    comparison: SLOComparison = SLOComparison.LT
    percentile: Optional[str] = None


@dataclass
class SLOViolation:
    """Record of an SLO violation.

    Attributes:
        slo_name: Name of the violated SLO.
        metric: Metric key.
        threshold: The SLO threshold.
        actual_value: The actual measured value that violated the SLO.
        comparison: The comparison that failed.
        timestamp_ms: Monotonic timestamp of the violation.
    """

    slo_name: str
    metric: str
    threshold: float
    actual_value: float
    comparison: SLOComparison
    timestamp_ms: float


class SLOTracker:
    """Tracks metrics against configurable SLO thresholds.

    Records violations and computes health percentages.

    Example::

        tracker = SLOTracker()

        # Define SLOs
        tracker.add_slo(SLODefinition(
            name="STT TTFT p99 < 500ms",
            metric="stt.ttft",
            threshold=500.0,
            comparison=SLOComparison.LT,
            percentile="p99",
        ))

        tracker.add_slo(SLODefinition(
            name="TTS TTFB p95 < 1000ms",
            metric="tts.ttfb",
            threshold=1000.0,
            comparison=SLOComparison.LT,
            percentile="p95",
        ))

        # Check SLOs against current metrics
        from bud_waav.ws.session import PercentileStats
        metrics = {"stt.ttft": PercentileStats(p50=100, p95=300, p99=450, ...)}
        violations = tracker.check(metrics)

        # Get overall health
        health = tracker.get_health()  # 0.0 to 1.0
    """

    _MAX_VIOLATIONS = 100

    def __init__(self) -> None:
        self._slos: dict[str, SLODefinition] = {}
        self._violations: dict[str, deque[SLOViolation]] = {}
        self._check_count: int = 0
        self._pass_count: int = 0

    def add_slo(self, slo: SLODefinition) -> None:
        """Add an SLO definition.

        Args:
            slo: The SLO definition to add.
        """
        self._slos[slo.name] = slo
        if slo.name not in self._violations:
            self._violations[slo.name] = deque(maxlen=self._MAX_VIOLATIONS)

    def remove_slo(self, name: str) -> bool:
        """Remove an SLO definition.

        Args:
            name: Name of the SLO to remove.

        Returns:
            True if the SLO was found and removed.
        """
        if name in self._slos:
            del self._slos[name]
            self._violations.pop(name, None)
            return True
        return False

    def check(self, metrics: dict[str, Any]) -> list[SLOViolation]:
        """Check all SLOs against provided metrics.

        Args:
            metrics: Dictionary mapping metric keys to values.
                Values can be raw floats or PercentileStats objects.

        Returns:
            List of violations found in this check.
        """
        violations: list[SLOViolation] = []
        now_ms = time.monotonic() * 1000.0

        for slo in self._slos.values():
            value = metrics.get(slo.metric)
            if value is None:
                continue

            # Extract percentile value if specified
            actual: Optional[float] = None
            if slo.percentile and hasattr(value, slo.percentile):
                actual = getattr(value, slo.percentile)
            elif slo.percentile and isinstance(value, dict):
                actual = value.get(slo.percentile)
            elif isinstance(value, (int, float)):
                actual = float(value)
            else:
                continue

            if actual is None:
                continue

            self._check_count += 1
            violated = _check_violation(actual, slo.threshold, slo.comparison)

            if violated:
                violation = SLOViolation(
                    slo_name=slo.name,
                    metric=slo.metric,
                    threshold=slo.threshold,
                    actual_value=actual,
                    comparison=slo.comparison,
                    timestamp_ms=now_ms,
                )
                violations.append(violation)
                self._violations[slo.name].append(violation)
            else:
                self._pass_count += 1

        return violations

    def get_health(self) -> float:
        """Get overall health percentage.

        Returns:
            Health as a float from 0.0 (all checks failing) to 1.0 (all passing).
            Returns 1.0 if no checks have been performed.
        """
        if self._check_count == 0:
            return 1.0
        return self._pass_count / self._check_count

    def get_violations(self, slo_name: Optional[str] = None) -> list[SLOViolation]:
        """Get violation history.

        Args:
            slo_name: If provided, only return violations for this SLO.
                If None, return all violations.

        Returns:
            List of violations, most recent last.
        """
        if slo_name:
            return list(self._violations.get(slo_name, []))

        all_violations: list[SLOViolation] = []
        for violations in self._violations.values():
            all_violations.extend(violations)
        all_violations.sort(key=lambda v: v.timestamp_ms)
        return all_violations

    def get_slos(self) -> list[SLODefinition]:
        """Get all registered SLO definitions.

        Returns:
            List of SLO definitions.
        """
        return list(self._slos.values())

    def reset(self) -> None:
        """Reset all violation history and counters."""
        for violations in self._violations.values():
            violations.clear()
        self._check_count = 0
        self._pass_count = 0


def _check_violation(actual: float, threshold: float, comparison: SLOComparison) -> bool:
    """Check if a value violates an SLO threshold.

    Returns True if the SLO is VIOLATED (bad).
    """
    if comparison == SLOComparison.LT:
        return actual >= threshold  # Violation: value should be < threshold but isn't
    elif comparison == SLOComparison.LTE:
        return actual > threshold
    elif comparison == SLOComparison.GT:
        return actual <= threshold
    elif comparison == SLOComparison.GTE:
        return actual < threshold
    elif comparison == SLOComparison.EQ:
        return actual != threshold
    return False
