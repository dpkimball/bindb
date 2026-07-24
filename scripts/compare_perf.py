#!/usr/bin/env python3
"""
Compare two perf-report.json files and emit a Markdown delta table.

Usage:
    python3 scripts/compare_perf.py baseline.json current.json

Exits 0 always (comparison is informational, not a gate).
"""
import json
import sys
from pathlib import Path


def load(path: str) -> dict:
    try:
        with open(path) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def headroom(value: float, bound: float, higher_is_better: bool) -> float:
    if higher_is_better:
        return (value / bound - 1.0) * 100.0 if bound else 0.0
    return (1.0 - value / bound) * 100.0 if value else 100.0


def delta_marker(delta_pct: float, higher_is_better: bool) -> str:
    if abs(delta_pct) < 3.0:
        return "→"
    improved = (delta_pct > 0) == higher_is_better
    return "✅" if improved else "⚠️"


def format_value(value: float, unit: str) -> str:
    if abs(value) >= 1000:
        return f"{value:,.0f} {unit}"
    if abs(value) >= 10:
        return f"{value:.1f} {unit}"
    return f"{value:.3f} {unit}"


def main() -> None:
    if len(sys.argv) < 3:
        print("Usage: compare_perf.py <baseline.json> <current.json>", file=sys.stderr)
        sys.exit(1)

    baseline_data = load(sys.argv[1])
    current_data = load(sys.argv[2])

    baseline_sha = baseline_data.get("git_sha", "unknown")
    current_sha = current_data.get("git_sha", "unknown")
    current_pass = current_data.get("overall_pass", False)
    overall_icon = "✅" if current_pass else "❌"

    baseline_metrics = {m["id"]: m for m in baseline_data.get("metrics", [])}
    current_metrics = {m["id"]: m for m in current_data.get("metrics", [])}

    lines = []
    lines.append(f"## {overall_icon} Perf report — `{current_sha}` vs baseline `{baseline_sha}`\n")

    if not baseline_metrics:
        lines.append("_No baseline available — this is the first run._\n")
    else:
        lines.append("| Metric | Baseline | Current | Δ | SLA | Headroom |")
        lines.append("|--------|----------|---------|---|-----|----------|")

        any_regression = False
        for mid, cur in current_metrics.items():
            cv = cur["value"]
            unit = cur["unit"]
            sla_bound = cur["bound"]
            higher = cur["higher_is_better"]
            passes = cur["pass"]
            sla_icon = "✅" if passes else "❌"
            hr = headroom(cv, sla_bound, higher)

            if mid in baseline_metrics:
                bv = baseline_metrics[mid]["value"]
                delta_pct = (cv - bv) / bv * 100.0 if bv != 0 else 0.0
                marker = delta_marker(delta_pct, higher)
                if marker == "⚠️":
                    any_regression = True
                baseline_str = format_value(bv, unit)
                delta_str = f"{delta_pct:+.1f}%"
            else:
                baseline_str = "_(new)_"
                delta_str = "—"
                marker = "🆕"

            current_str = format_value(cv, unit)
            hr_str = f"{hr:+.0f}%"
            lines.append(
                f"| {cur['name']} | {baseline_str} | {current_str}"
                f" | {marker} {delta_str} | {sla_icon} | {hr_str} |"
            )

        lines.append("")
        if any_regression:
            lines.append(
                "> ⚠️ One or more metrics regressed >3% from baseline. "
                "Review before merging."
            )
        else:
            lines.append("> All metrics within 3% of baseline or improved.")

    lines.append("")
    lines.append(
        f"_SLA bounds from `tests/perf_sla.rs`. "
        f"Headroom = slack remaining before SLA breach. "
        f"Δ < 3% shown as → (within noise floor)._"
    )

    print("\n".join(lines))


if __name__ == "__main__":
    main()
