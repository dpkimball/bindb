#!/usr/bin/env python3
"""
Append the current perf run to the rolling history file (last 10 entries).

Usage:
    python3 scripts/update_perf_history.py <history.json> <perf-report.json>

Reads history.json (array, may not exist), appends a new entry from
perf-report.json, trims to 10 entries, and writes back to history.json.
"""
import json
import sys
import time
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 3:
        print("Usage: update_perf_history.py <history.json> <perf-report.json>", file=sys.stderr)
        sys.exit(1)

    history_path = Path(sys.argv[1])
    report_path = Path(sys.argv[2])

    try:
        history = json.loads(history_path.read_text())
        if not isinstance(history, list):
            history = []
    except (FileNotFoundError, json.JSONDecodeError):
        history = []

    try:
        with open(report_path) as f:
            report = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        sys.exit(0)

    entry = {
        "git_sha": report.get("git_sha", "unknown"),
        "ts": int(report.get("generated_at_unix", time.time())),
        "metrics": {m["id"]: m["value"] for m in report.get("metrics", [])},
    }
    history.append(entry)
    history_path.write_text(json.dumps(history[-10:], indent=2))


if __name__ == "__main__":
    main()
