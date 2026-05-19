# Rust Transport Protocol Performance Test Report

Generated: 2026-05-13 CST

## Summary

This report compares **HTTP/3 over QUIC using Cloudflare `quiche`** against **persistent HTTP/2 over TLS/TCP using `tokio-rustls` plus Rust `h2`**. Both transports use the packet-derived SBI JSON corpus from `../../SBI_PAYLOADS.json`.

Implementation issues found and fixed:

- HTTP/2 sockets now set `TCP_NODELAY`; without it, Nagle/delayed-ACK behavior caused false `~40 ms` tail-latency spikes.
- HTTP/2 now uses TLS with ALPN `h2`, so the comparison no longer gives H2 a plain-TCP setup while H3 pays QUIC TLS cost.
- HTTP/2 request sending no longer clones the `SendRequest` handle per message.
- The benchmark now exposes two HTTP/2 server execution strategies: `h2` uses the idiomatic one Tokio task per request handler, while `h2-inline` tracks active streams inside the connection task.
- Payloads are loaded as reusable `Bytes` values so HTTP/2 no longer allocates and copies the request body on every message.
- The harness supports `-repeat`; the tables below report the median of three repeats instead of a single run.
- H3 now uses the same virtual-user workload model as H2: one outstanding request per user, and each user's next request is sent only after that user's previous response.
- Tokio is pinned to two worker threads so H2 runtime scheduling does not silently vary with host CPU count.
- CSV output records `runtime_worker_threads`, `workload_model`, `client_scheduler`, `server_scheduler`, and `run_order` so raw results remain self-describing.
- Protocols are interleaved by repeat for each load scenario to reduce fixed protocol-order bias.
- H2 client/server tasks are cancelled and awaited after each measured scenario to avoid cross-repeat task leakage.
- HTTP/3 no longer uses fixed sleeps or a CPU-burning yield loop. The `quiche` driver now uses `mio::Poll` readiness plus quiche timeouts.
- HTTP/3 temporary certificate files are removed when each scenario exits.
- HTTP/3 handles backpressure for request bodies and blocked responses instead of counting partial writes as failures.
- H3 pending request bodies keep reusable `Bytes` handles instead of allocating a `Vec` copy on backpressure.

Key observations after optimization:

- In the no-failure run, default `h2` has the strongest one-user throughput. `h3-quiche` has higher median throughput from `8` users upward.
- With `1000` messages per user, median `h3-quiche` throughput reaches `23k-89k msg/s`, default `h2` reaches `35k-70k msg/s`, and `h2-inline` reaches `23k-89k msg/s`.
- Under large symmetric netem delay, H2 and H3 now complete the fixed virtual-user workload at the same throughput in this harness.
- No request errors occurred in either baseline or netem runs.

## Method

Baseline command:

```sh
cargo run --release -- -repeat 3 -users 1,8,32,128 -messages 1000 -protocols h2,h2-inline,h3 -failures none -output /tmp/transportbench_rust_tls_repeat3_schedmeta_1000msg.csv
```

Failure-condition command:

```sh
cargo run --release -- -repeat 3 -netem -users 8 -messages 5 -protocols h2,h2-inline,h3 -failures loss1,delay100ms,delay1s -output /tmp/transportbench_rust_tls_repeat3_schedmeta_failure.csv
```

Validation:

```sh
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
python3 -m unittest scripts/test_summarize_csv.py
python3 scripts/summarize_csv.py /tmp/transportbench_rust_tls_repeat3_schedmeta_1000msg.csv /tmp/transportbench_rust_tls_repeat3_schedmeta_failure.csv
cargo run --release -- -users 1 -messages 2 -protocols h3 -failures none -output /tmp/transportbench_rust_h3_cleanup_smoke.csv
```

Environment:

| Item | Value |
| --- | --- |
| OS | Linux agentic-core 5.15.0-161-generic x86_64 |
| Rust | rustc 1.95.0 |
| Cargo | cargo 1.95.0 |
| QUIC / HTTP/3 | `quiche 0.28.0` |
| HTTP/2 | `h2 0.4`, persistent TLS/TCP, ALPN `h2`, `TCP_NODELAY` enabled |
| Event loop | `mio 1.2` for the `quiche` Sans-I/O driver |
| Payload corpus | `../../SBI_PAYLOADS.json` |
| Payload count | 162 |
| Average payload size | 1063 bytes |

The benchmark runs client and server in one process. CPU and RSS therefore represent combined local client+server process cost.

## Baseline Load Results

Failure profile: `none`. Message count: `1000` per user. Values are medians across three repeats.

| Protocol | Users | Total messages | Throughput msg/s | Mean ms | P50 ms | P95 ms | P99 ms | Max ms | Errors |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| h2 | 1 | 1000 | 35340.69 | 0.028 | 0.027 | 0.039 | 0.044 | 0.061 | 0 |
| h2-inline | 1 | 1000 | 22919.30 | 0.043 | 0.042 | 0.060 | 0.073 | 0.115 | 0 |
| h3-quiche | 1 | 1000 | 23376.15 | 0.032 | 0.027 | 0.054 | 0.088 | 0.184 | 0 |
| h2 | 8 | 8000 | 58175.28 | 0.137 | 0.129 | 0.227 | 0.269 | 0.419 | 0 |
| h2-inline | 8 | 8000 | 69146.45 | 0.115 | 0.110 | 0.189 | 0.228 | 0.326 | 0 |
| h3-quiche | 8 | 8000 | 80839.47 | 0.060 | 0.055 | 0.103 | 0.138 | 0.374 | 0 |
| h2 | 32 | 32000 | 64115.51 | 0.497 | 0.490 | 0.721 | 0.824 | 1.368 | 0 |
| h2-inline | 32 | 32000 | 88924.57 | 0.358 | 0.360 | 0.471 | 0.556 | 0.913 | 0 |
| h3-quiche | 32 | 32000 | 87867.75 | 0.207 | 0.194 | 0.374 | 0.520 | 2.971 | 0 |
| h2 | 128 | 128000 | 69942.45 | 1.824 | 1.782 | 2.618 | 3.253 | 6.577 | 0 |
| h2-inline | 128 | 128000 | 71694.83 | 1.779 | 1.725 | 2.475 | 3.131 | 6.905 | 0 |
| h3-quiche | 128 | 128000 | 89023.97 | 0.567 | 0.489 | 1.187 | 1.638 | 13.156 | 0 |

## Failure Condition Results

Load: `8` users, `5` messages per user, `40` total messages per protocol/profile. Values are medians across three repeats.

| Profile | Protocol | Throughput msg/s | Mean ms | P50 ms | P95 ms | P99 ms | Max ms | Errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| loss1 | h2 | 42352.56 | 0.162 | 0.172 | 0.275 | 0.327 | 0.350 | 0 |
| loss1 | h2-inline | 58715.60 | 0.116 | 0.115 | 0.178 | 0.229 | 0.236 | 0 |
| loss1 | h3-quiche | 68986.09 | 0.067 | 0.063 | 0.111 | 0.150 | 0.172 | 0 |
| delay100ms | h2 | 39.93 | 200.344 | 200.347 | 200.491 | 200.539 | 200.544 | 0 |
| delay100ms | h2-inline | 39.93 | 200.340 | 200.336 | 200.437 | 200.503 | 200.508 | 0 |
| delay100ms | h3-quiche | 39.94 | 200.241 | 200.244 | 200.269 | 200.276 | 200.279 | 0 |
| delay1s | h2 | 4.00 | 2000.332 | 2000.319 | 2000.503 | 2000.536 | 2000.538 | 0 |
| delay1s | h2-inline | 4.00 | 2000.337 | 2000.332 | 2000.454 | 2000.476 | 2000.482 | 0 |
| delay1s | h3-quiche | 4.00 | 2000.254 | 2000.263 | 2000.296 | 2000.302 | 2000.303 | 0 |

## CPU and RSS

### Baseline

| Protocol | Users | CPU user ms | CPU system ms | RSS delta bytes | Max RSS KB |
| --- | ---: | ---: | ---: | ---: | ---: |
| h2 | 1 | 12.191 | 16.135 | 0 | 46320 |
| h2-inline | 1 | 30.421 | 17.384 | 0 | 46320 |
| h3-quiche | 1 | 30.061 | 26.702 | 0 | 46320 |
| h2 | 8 | 141.698 | 100.033 | 0 | 46320 |
| h2-inline | 8 | 112.017 | 94.451 | 0 | 46320 |
| h3-quiche | 8 | 102.797 | 80.817 | 270336 | 46320 |
| h2 | 32 | 544.268 | 248.332 | 536576 | 46320 |
| h2-inline | 32 | 389.109 | 281.044 | 0 | 46320 |
| h3-quiche | 32 | 397.217 | 287.973 | 9719808 | 46320 |
| h2 | 128 | 1992.898 | 1184.352 | 2428928 | 46320 |
| h2-inline | 128 | 1760.560 | 1352.847 | 1888256 | 46320 |
| h3-quiche | 128 | 1558.574 | 1116.331 | 38739968 | 91232 |

### Failure Conditions

| Profile | Protocol | CPU user ms | CPU system ms | RSS delta bytes | Max RSS KB |
| --- | --- | ---: | ---: | ---: | ---: |
| loss1 | h2 | 1.456 | 0.380 | 0 | 46392 |
| loss1 | h2-inline | 1.214 | 0.000 | 0 | 46392 |
| loss1 | h3-quiche | 0.882 | 0.194 | 0 | 46392 |
| delay100ms | h2 | 3.296 | 0.000 | 0 | 46392 |
| delay100ms | h2-inline | 2.804 | 0.000 | 0 | 46392 |
| delay100ms | h3-quiche | 1.533 | 0.296 | 0 | 46392 |
| delay1s | h2 | 2.742 | 0.614 | 0 | 46392 |
| delay1s | h2-inline | 2.553 | 0.697 | 0 | 46392 |
| delay1s | h3-quiche | 1.874 | 0.033 | 0 | 46392 |

## Interpretation

The original poor HTTP/2 result was a harness bug, not protocol behavior. After enabling `TCP_NODELAY`, the false `~40 ms` tail-latency spikes disappeared.

The original low-load HTTP/3 penalty was also partly a harness artifact. The first fix removed fixed sleeps and showed H3 low-load latency comparable to HTTP/2, but it did that by yielding aggressively. The final implementation uses `mio` readiness and quiche timeouts, which is more representative and avoids burning CPU under delayed network conditions. This raises H3 single-user latency slightly compared with the busy-yield loop, but it removes an implementation artifact from CPU and delay-profile results.

With the optimized implementation and a fixed two-worker Tokio runtime, default `h2` is strongest at one user (`35,341 msg/s`) while `h3-quiche` leads at `8` and `128` users. At `32` users, `h2-inline` reaches `88,925 msg/s`, H3 reaches `87,868 msg/s`, and default H2 reaches `64,116 msg/s`. At `128` users H3 reaches `89,024 msg/s` versus `69,942 msg/s` for default H2 and `71,695 msg/s` for H2 inline.

The `h2` versus `h2-inline` split shows that HTTP/2 server execution strategy is a meaningful harness variable. Inline stream handling improves high-concurrency H2 throughput, but the idiomatic spawned-handler mode is faster at one user. The current result is useful for comparing these concrete implementations and checking sensitivity, but it should not be treated as a pure theorem about H2 versus H3.

One client-side scheduling difference remains exposed rather than fully eliminated: H2 virtual users are Tokio tasks sharing one H2 connection, while H3 virtual users are driven from the custom readiness loop. Both use the same serial-per-user request semantics, but their local client schedulers are not identical.

The delayed netem profiles changed after aligning H3 to the same virtual-user sequencing as H2. With `delay1s`, all three modes complete `40` messages in about `10 s`, with median request latency near `2000 ms`. The earlier H3 `12 s` result was therefore a workload-scheduler artifact, not protocol behavior.

## Audit Checklist

Objective: optimize the H2 and H3 implementations, identify implementation artifacts that affect results, and make the result represent protocol behavior as much as this harness can support.

| Requirement | Status | Evidence |
| --- | --- | --- |
| Persistent HTTP/2 connection | Done | `h2` uses one TLS/TCP connection per scenario and clones `SendRequest` handles for streams. |
| Avoid H2 delayed-ACK/Nagle artifact | Done | Client and server TCP sockets set `TCP_NODELAY`. |
| Avoid H2 plain-TCP versus H3 QUIC-TLS mismatch | Done | H2 uses `tokio-rustls` with ALPN `h2`; H3 uses QUIC TLS. |
| Avoid H2 per-message body allocation | Done | Payload corpus is loaded as reusable `Bytes`; H2 sends cloned handles. |
| Expose H2 server scheduling sensitivity | Done | Report includes both `h2` and `h2-inline`. |
| Avoid H3 fixed sleep/yield artifact | Done | H3 uses `mio::Poll` and quiche transport timeouts. |
| Avoid H3 backpressure-as-error artifact | Done | H3 retries pending request bodies and blocked responses. |
| Avoid H3 pending-body allocation artifact | Done | H3 pending bodies keep reusable `Bytes` handles instead of copying payloads into `Vec`s. |
| Align H2/H3 user workload shape | Done | Both model users as serial streams with one outstanding request per user. |
| Eliminate H2/H3 client scheduler difference | Not achieved | H2 virtual users are Tokio tasks; H3 virtual users are driven by the custom quiche readiness loop. |
| Reduce single-run noise | Done | Report uses medians across three repeats and records `repeat_index` in CSV. |
| Reduce host-dependent runtime scheduling | Done | Tokio runtime is pinned to two worker threads. |
| Preserve benchmark controls in raw artifacts | Done | CSV records `runtime_worker_threads`, `workload_model`, `client_scheduler`, `server_scheduler`, and `run_order`. |
| Reduce fixed protocol-order bias | Done | Protocols are run interleaved by repeat for each failure/load/message scenario. |
| Reproduce median summaries | Done | `scripts/summarize_csv.py` verifies control metadata and emits median rows; `scripts/test_summarize_csv.py` covers median and metadata-stability behavior. |
| Avoid cross-repeat H2 task leakage | Done | H2 connection and server tasks are aborted and awaited after measurement. |
| Avoid H3 temporary-file accumulation | Done | H3 certificate temp directories are removed via `Drop`; a smoke run kept the `/tmp/transportbench-rust-*` directory count unchanged. |
| Validate implementation | Done | `cargo fmt -- --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo build --release`, Python summarizer tests, repeat-3 baseline, repeat-3 netem matrix, and clean loopback qdisc. |
| Prove pure protocol-only results | Not achieved | This harness still contains unavoidable implementation differences: `h2`/Tokio scheduling, H2 server strategy, `rustls`, `quiche` crypto/TLS internals, and the custom Sans-I/O H3 driver. |

## Limits

- This is a loopback benchmark. Absolute latencies are not representative of a deployed network.
- HTTP/2 and HTTP/3 now both include TLS, but they still use different libraries and different TLS/crypto integrations.
- The benchmark removes known major harness artifacts, but it cannot prove the results are purely protocol-only. Remaining differences include H2 client scheduling through Tokio tasks, the H2 server stream execution strategy, `rustls`, `quiche`'s crypto path, and the minimal custom Sans-I/O H3 driver.
- `tc/netem` applies to loopback and affects both directions.
- RSS is a coarse memory metric; it does not provide allocator-level allocation counts.
- The `quiche` harness is now readiness-driven, but it is still a minimal custom Sans-I/O driver, not a production HTTP/3 stack.
- Results are now repeated three times in the report, but they should still be repeated across host loads before drawing final conclusions.

Raw CSV artifacts:

- `/tmp/transportbench_rust_tls_repeat3_schedmeta_1000msg.csv`
- `/tmp/transportbench_rust_tls_repeat3_schedmeta_failure.csv`
- `/tmp/transportbench_rust_tls_repeat3_schedmeta_summary.csv`
