#!/usr/bin/env python3
"""Summarize repeated transport benchmark CSV files.

The benchmark emits one row per scenario repeat. This helper verifies that
benchmark-control metadata is stable within each scenario and prints median
metric rows, grouped by failure profile, protocol, user count, and message
count.
"""

from __future__ import annotations

import argparse
import csv
import statistics
import sys
from collections import defaultdict
from pathlib import Path


GROUP_COLUMNS = (
    "failure_profile",
    "protocol",
    "users",
    "messages_per_user",
    "total_messages",
)

METADATA_COLUMNS = (
    "runtime_worker_threads",
    "workload_model",
    "client_scheduler",
    "server_scheduler",
    "run_order",
    "payload_count",
    "payload_bytes_avg",
)

MEDIAN_COLUMNS = (
    "duration_ms",
    "throughput_msg_s",
    "errors",
    "latency_mean_ms",
    "latency_p50_ms",
    "latency_p95_ms",
    "latency_p99_ms",
    "latency_min_ms",
    "latency_max_ms",
    "cpu_user_ms",
    "cpu_system_ms",
    "rss_bytes_delta",
    "maxrss_kb_after",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", nargs="+", type=Path, help="Benchmark CSV file(s)")
    return parser.parse_args()


def median_text(rows: list[dict[str, str]], column: str) -> str:
    values = [float(row[column]) for row in rows]
    value = statistics.median(values)
    if column in {"errors", "rss_bytes_delta", "maxrss_kb_after"}:
        return str(int(value))
    if column == "throughput_msg_s":
        return f"{value:.2f}"
    return f"{value:.3f}"


def stable_value(rows: list[dict[str, str]], column: str) -> str:
    values = {row[column] for row in rows}
    if len(values) != 1:
        key = ", ".join(f"{name}={rows[0][name]}" for name in GROUP_COLUMNS)
        raise ValueError(f"{column} is not stable for group {key}: {sorted(values)}")
    return values.pop()


def summarize(path: Path) -> list[dict[str, str]]:
    with path.open(newline="") as handle:
        reader = csv.DictReader(handle)
        required = set(GROUP_COLUMNS + METADATA_COLUMNS + MEDIAN_COLUMNS + ("repeat_index",))
        missing = sorted(required.difference(reader.fieldnames or []))
        if missing:
            raise ValueError(f"{path}: missing columns: {', '.join(missing)}")

        groups: dict[tuple[str, ...], list[dict[str, str]]] = defaultdict(list)
        for row in reader:
            groups[tuple(row[column] for column in GROUP_COLUMNS)].append(row)

    out = []
    for key in sorted(groups):
        rows = groups[key]
        summary = {column: value for column, value in zip(GROUP_COLUMNS, key)}
        summary["repeat_count"] = str(len(rows))
        for column in METADATA_COLUMNS:
            summary[column] = stable_value(rows, column)
        for column in MEDIAN_COLUMNS:
            summary[column] = median_text(rows, column)
        out.append(summary)
    return out


def main() -> int:
    args = parse_args()
    fieldnames = (
        "source",
        *GROUP_COLUMNS,
        "repeat_count",
        *METADATA_COLUMNS,
        *MEDIAN_COLUMNS,
    )
    writer = csv.DictWriter(sys.stdout, fieldnames=fieldnames)
    writer.writeheader()
    try:
        for path in args.csv:
            for row in summarize(path):
                row["source"] = str(path)
                writer.writerow(row)
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
