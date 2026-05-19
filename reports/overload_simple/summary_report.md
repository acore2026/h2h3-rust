# QUIC Overload-Handling Experiment

Claim: QUIC stream-level rejection can reduce Control Plane overload processing.

Result:
- Supported if reset mode shows lower server bytes read, lower server processing time, and lower overload rejection latency.
- Not fully supported if the client requires standard HTTP 429/503 semantics.
- Additional mapping is needed from QUIC stream error code to retry/backoff behavior.

Observed outcome: supported by the completed matched scenarios.

| Payload bytes | Concurrency | Threshold | app503 bytes read | reset bytes read | app503 server ms | reset server ms | app503 reject mean ms | reset reject mean ms |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16384 | 50 | 20 | 16384000 | 3653632 | 3320.481 | 961.326 | 4.144 | 1.056 |
