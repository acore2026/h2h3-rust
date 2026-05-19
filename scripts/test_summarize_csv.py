#!/usr/bin/env python3
"""Smoke tests for summarize_csv.py."""

from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import summarize_csv


FIELDNAMES = (
    "protocol",
    "users",
    "messages_per_user",
    "total_messages",
    "repeat_index",
    "runtime_worker_threads",
    "workload_model",
    "client_scheduler",
    "server_scheduler",
    "run_order",
    "payload_count",
    "payload_bytes_avg",
    "failure_profile",
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


def write_csv(path: Path, rows: list[dict[str, str]]) -> None:
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=FIELDNAMES)
        writer.writeheader()
        writer.writerows(rows)


def row(repeat_index: int, throughput: str, duration: str = "10.000") -> dict[str, str]:
    return {
        "protocol": "h2",
        "users": "1",
        "messages_per_user": "1000",
        "total_messages": "1000",
        "repeat_index": str(repeat_index),
        "runtime_worker_threads": "2",
        "workload_model": "serial_per_user",
        "client_scheduler": "tokio_user_tasks",
        "server_scheduler": "in_connection_futures",
        "run_order": "interleaved_by_repeat",
        "payload_count": "162",
        "payload_bytes_avg": "1063",
        "failure_profile": "none",
        "duration_ms": duration,
        "throughput_msg_s": throughput,
        "errors": "0",
        "latency_mean_ms": "0.100",
        "latency_p50_ms": "0.100",
        "latency_p95_ms": "0.200",
        "latency_p99_ms": "0.300",
        "latency_min_ms": "0.010",
        "latency_max_ms": "0.400",
        "cpu_user_ms": "1.000",
        "cpu_system_ms": "2.000",
        "rss_bytes_delta": "0",
        "maxrss_kb_after": "100",
    }


class SummarizeCsvTest(unittest.TestCase):
    def test_summarize_computes_medians_and_preserves_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "bench.csv"
            write_csv(path, [row(1, "100.00"), row(2, "300.00"), row(3, "200.00")])

            [summary] = summarize_csv.summarize(path)

        self.assertEqual(summary["repeat_count"], "3")
        self.assertEqual(summary["runtime_worker_threads"], "2")
        self.assertEqual(summary["workload_model"], "serial_per_user")
        self.assertEqual(summary["client_scheduler"], "tokio_user_tasks")
        self.assertEqual(summary["server_scheduler"], "in_connection_futures")
        self.assertEqual(summary["run_order"], "interleaved_by_repeat")
        self.assertEqual(summary["throughput_msg_s"], "200.00")
        self.assertEqual(summary["duration_ms"], "10.000")

    def test_summarize_rejects_unstable_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            path = Path(temp_dir) / "bench.csv"
            rows = [row(1, "100.00"), row(2, "200.00")]
            rows[1]["workload_model"] = "global_window"
            write_csv(path, rows)

            with self.assertRaisesRegex(ValueError, "workload_model is not stable"):
                summarize_csv.summarize(path)


if __name__ == "__main__":
    unittest.main()
