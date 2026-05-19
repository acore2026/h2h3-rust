# Rust Transport Benchmark

Rust benchmark harness for comparing HTTP/2 over TLS/TCP with HTTP/3 over QUIC
under SBI-style request payloads. It is the Rust companion to the parent Go
serialization and transport experiments.

The crate measures request throughput, latency percentiles, error counts, CPU
time, and resident memory. Generated CSVs, charts, run logs, and narrative
reports are kept under `reports/`.

## What It Tests

- `h2`: persistent HTTP/2 over one local TLS/TCP connection using `tokio-rustls`
  and the Rust `h2` crate.
- `h3-quiche`: HTTP/3 over QUIC using Cloudflare `quiche` through a `mio`
  readiness loop.
- `overload`: raw QUIC stream overload handling, comparing application `503`
  responses with early stream resets.

The default payload corpus is `../../SBI_PAYLOADS.json`.

## Layout

```text
src/main.rs          command dispatcher
src/cli.rs           default H2/H3 scenario matrix
src/h2.rs            HTTP/2 client and server path
src/h3.rs            HTTP/3/quiche client and server path
src/rust_pps.rs      fixed-PPS Rust client against Rust server
src/nginx_pps.rs     fixed-PPS Rust client against NGINX
src/h2load_rate.rs   h2load-driven target-PPS runs
src/overload.rs      raw QUIC overload experiment
src/netem_csv.rs     netem control and shared CSV output
src/tests.rs         unit tests
reports/             generated reports, CSVs, charts, and logs
scripts/             helper scripts
```

## Requirements

- Rust toolchain with Cargo.
- Linux for the default resource accounting and optional loopback `tc/netem`
  failure profiles.
- `h2load` for `h2load-rate` runs.
- Docker only for workflows that call `scripts/h2load-docker.sh` or sample an
  NGINX container.

## Quick Start

Run a small local H2/H3 smoke test:

```sh
cargo run -- -users 1 -messages 5 -protocols h2,h3 -output reports/transportbench_rust_smoke.csv
```

Run the default scenario matrix:

```sh
cargo run --release -- -output reports/transportbench_rust.csv
```

Repeat each scenario to reduce run-to-run noise:

```sh
cargo run --release -- -repeat 3 -users 1,8,32,128 -messages 1000 -protocols h2,h3 -output reports/transportbench_rust_repeat3.csv
```

Summarize repeated CSV output:

```sh
python3 scripts/summarize_csv.py reports/transportbench_rust_repeat3.csv
```

## Benchmark Modes

### Default H2/H3 Matrix

```sh
cargo run --release -- \
  -payloads ../../SBI_PAYLOADS.json \
  -protocols h2,h3 \
  -users 1,8,32,128 \
  -messages 100,1000 \
  -failures none \
  -repeat 1 \
  -output reports/transportbench_rust.csv
```

Useful flags:

- `-protocols h2,h3`
- `-users 1,8,32,128`
- `-messages 100,1000`
- `-repeat 3`
- `-output reports/name.csv`

Failure profiles use Linux loopback `tc/netem` and require `-netem`:

```sh
cargo run --release -- \
  -netem \
  -failures none,loss1,delay100ms,delay1s \
  -users 8 \
  -messages 5 \
  -protocols h2,h3 \
  -output reports/transportbench_rust_netem.csv
```

### Rust Client and Rust Server Fixed-PPS

Runs both endpoints in this Rust process. CPU and memory columns describe the
combined client/server process.

```sh
cargo run --release -- rust-pps \
  -protocols h2,h3 \
  -payload-sizes 1024 \
  -pps 1000,3000,6000,10000 \
  -duration 10 \
  -max-inflight 100 \
  -output reports/rust_pps_results/results.csv
```

### Rust Client Against NGINX

Runs the Rust load generator against a local NGINX H2/H3 target and samples the
NGINX container with `docker stats`.

```sh
cargo run --release -- rust-pps-nginx \
  -target https://localhost:8443/api \
  -nginx-container quic-nginx \
  -protocols h2,h3 \
  -payload-sizes 1024 \
  -pps 1000,3000,6000,10000 \
  -duration 10 \
  -max-inflight 100 \
  -output reports/nginx_rust_client_results/results.csv
```

### h2load Target-PPS

Starts the Rust H2/H3 server locally and drives it with `h2load`.

```sh
cargo run --release -- h2load-rate \
  -protocols h2 \
  -payload-sizes 1024 \
  -pps 1000,3000,6000,10000 \
  -duration 10 \
  -output reports/h2load_rate_rust/results.csv \
  -log-dir reports/h2load_rate_rust/logs
```

Some system `h2load` builds are H2-only or do not support `--rps`. For H2, the
runner can fall back to `--timing-script-file`. For H3, use an HTTP/3-capable
`h2load`, for example:

```sh
cargo run --release -- h2load-rate \
  -h2load ./scripts/h2load-docker.sh \
  -protocols h2,h3 \
  -payload-sizes 1024 \
  -pps 1000,3000 \
  -duration 10
```

### Raw QUIC Overload Experiment

Compares early application-layer `503 overloaded` responses (`app503`) with
early stream-level resets (`reset`).

```sh
cargo run --release -- overload -output-dir reports/overload_results
```

Small smoke run:

```sh
cargo run --release -- overload \
  -requests 20 \
  -concurrency 4 \
  -payload-sizes 1024 \
  -thresholds 2 \
  -output-dir reports/quic_overload_smoke
```

Fixed 5% rejection experiment:

```sh
cargo run --release -- overload \
  -requests 1000 \
  -concurrency 200 \
  -payload-sizes 16384 \
  -reject-percent 5 \
  -modes app503,reset \
  -output-dir reports/quic_overload_5pct
```

The overload command writes:

- `results_app503.csv`
- `results_reset.csv`
- `summary_report.md`
- `latency_comparison.png`
- `server_bytes_read_comparison.png`

## Reports

Existing report artifacts live in `reports/`:

- `reports/TRANSPORT_RUST_QUICHE_TEST_REPORT.md`
- `reports/data-volume-report.md`
- `reports/h2load_rate_rust/`
- `reports/rust_pps_results/`
- `reports/rust_pps_full_results/`
- `reports/nginx_rust_client_results/`
- `reports/overload_*`

Keep new generated output under `reports/` so the crate root stays reserved for
source, Cargo metadata, and this README.

## Validation

Run unit tests:

```sh
cargo test
```

Run the CSV summarizer tests:

```sh
python3 -m unittest scripts/test_summarize_csv.py
```

## Measurement Notes

- HTTP/2 uses TLS with ALPN `h2`, so the comparison is not plain TCP versus QUIC
  TLS.
- HTTP/2 sets `TCP_NODELAY` on client and server TCP sockets.
- HTTP/2 keeps one persistent TCP connection and avoids per-message
  `SendRequest` handle cloning.
- H2 active server streams are driven inside the connection task.
- HTTP/3 uses `quiche` through a readiness loop with quiche timeouts.
- H3 request-body and response backpressure are retried instead of counted as
  failed requests.
- Payloads are loaded as reusable `Bytes` values to avoid avoidable per-message
  body copies.
- Both transports model `users` as independent serial request streams: one
  outstanding request per user, then the next request after the response.
- Tokio is pinned to two worker threads so runtime scheduling does not silently
  change with host CPU count.
- CSV output records benchmark-control metadata such as worker thread count,
  workload model, client scheduler, server scheduler, and run order.
- Protocols are interleaved by repeat for each load scenario to reduce fixed
  protocol-order bias.
- Memory metrics use process RSS deltas and `ru_maxrss`; they are not
  allocator-level allocation counters.

