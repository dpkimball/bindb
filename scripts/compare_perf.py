#!/usr/bin/env python3
"""
Compare two perf-report.json files and emit a Markdown delta table,
optionally including a collapsed history table from the last 10 runs.

Usage:
    python3 scripts/compare_perf.py baseline.json current.json [history.json]

Exits 0 always (comparison is informational, not a gate).
"""
import json
import sys
from datetime import datetime, timezone


def load(path: str) -> dict:
    try:
        with open(path) as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def load_history(path: str) -> list:
    try:
        with open(path) as f:
            data = json.load(f)
        return data if isinstance(data, list) else []
    except (FileNotFoundError, json.JSONDecodeError):
        return []


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
    if abs(value) >= 1_000_000:
        return f"{value/1_000_000:.1f}M {unit}".strip()
    if abs(value) >= 1000:
        return f"{value:,.0f} {unit}".strip()
    if abs(value) >= 10:
        return f"{value:.1f} {unit}".strip()
    return f"{value:.3f} {unit}".strip()


def render_history(history: list, current_metrics: dict) -> list:
    runs = history[-10:][::-1]  # last 10, newest first
    if not runs:
        return []

    lines = []
    lines.append("\n<details>")
    lines.append("<summary>📈 History — last 10 runs</summary>\n")

    sha_headers = " | ".join(r.get("git_sha", "?")[:7] for r in runs)
    ts_row = " | ".join(
        datetime.fromtimestamp(r["ts"], tz=timezone.utc).strftime("%m-%d %H:%M")
        if "ts" in r else "—"
        for r in runs
    )
    sep = "|".join("---" for _ in runs)

    lines.append(f"| Metric | Unit | {sha_headers} |")
    lines.append(f"|--------|------|{sep}|")
    lines.append(f"| _(date UTC)_ | | {ts_row} |")

    for mid, cur in current_metrics.items():
        unit = cur["unit"]
        higher = cur["higher_is_better"]
        cells = []
        for run in runs:
            v = run.get("metrics", {}).get(mid)
            if v is None:
                cells.append("—")
            else:
                cells.append(format_value(v, ""))
        cells_str = " | ".join(cells)
        lines.append(f"| {cur['name']} | {unit} | {cells_str} |")

    lines.append("\n</details>")
    return lines


def main() -> None:
    if len(sys.argv) < 3:
        print("Usage: compare_perf.py <baseline.json> <current.json> [history.json]", file=sys.stderr)
        sys.exit(1)

    baseline_data = load(sys.argv[1])
    current_data = load(sys.argv[2])
    history = load_history(sys.argv[3]) if len(sys.argv) >= 4 else []

    baseline_sha = baseline_data.get("git_sha", "unknown")
    current_sha = current_data.get("git_sha", "unknown")
    current_pass = current_data.get("overall_pass", False)
    overall_icon = "✅" if current_pass else "❌"

    baseline_metrics = {m["id"]: m for m in baseline_data.get("metrics", [])}
    current_metrics = {m["id"]: m for m in current_data.get("metrics", [])}

    lines = []
    lines.append(f"## {overall_icon} Perf report — `{current_sha[:7]}` vs baseline `{baseline_sha[:7]}`\n")

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

    lines.extend(render_history(history, current_metrics))

    lines.append("")
    lines.append(
        f"_SLA bounds from `tests/perf_sla.rs`. "
        f"Headroom = slack remaining before SLA breach. "
        f"Δ < 3% shown as → (within noise floor)._"
    )

    print("\n".join(lines))


if __name__ == "__main__":
    main()
