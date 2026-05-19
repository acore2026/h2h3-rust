#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
container_name="${H2LOAD_DOCKER_NAME:-}"
name_args=()
if [[ -n "$container_name" ]]; then
  name_args=(--name "$container_name")
fi

exec docker run --rm --network host \
  "${name_args[@]}" \
  -v /tmp:/tmp \
  -v "$repo_root:$repo_root" \
  -w "$PWD" \
  quic-h2-bench-client \
  /opt/nghttp2/bin/h2load "$@"
