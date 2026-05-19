# QUIC Overload-Handling Experiment

Claim: QUIC stream-level rejection can reduce Control Plane overload processing.

Result:
- Supported if reset mode shows lower server bytes read, lower server processing time, and lower overload rejection latency.
- Not fully supported if the client requires standard HTTP 429/503 semantics.
- Additional mapping is needed from QUIC stream error code to retry/backoff behavior.

Observed outcome: not consistently supported by the completed matched scenarios.

| Payload bytes | Concurrency | Threshold | app503 bytes read | reset bytes read | app503 server ms | reset server ms | app503 reject mean ms | reset reject mean ms |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 16384 | 200 | 20 | 15564800 | 15564800 | 9705.662 | 10284.825 | 2.764 | 2.828 |
