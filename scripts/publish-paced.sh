#!/usr/bin/env bash
#
# Resume a publish that hit crates.io's new-crate rate limit.
#
# crates.io allows a burst of new crates and then roughly one per ten minutes.
# A workspace of fifteen therefore cannot go up in one run, and the first
# attempt stopped at a 429 with five published.
#
# Idempotent: a crate already on the index is skipped, so this can be re-run
# as often as needed and picks up wherever the last run stopped.
set -uo pipefail

CRATES=(
  rusty-memory-core
  rusty-memory-extract
  rusty-memory-survivor
  rusty-memory-store
  rusty-memory-graph
  rusty-memory-index
  rusty-memory-resolve
  rusty-memory-engine
  rusty-memory-embed
  rusty-memory-providers
  rusty-memory-host
  rusty-memory-cli
  rusty-memory-mcp
  rusty-memory
)

# Naming crates as arguments skips the index checks for everything already up,
# which matters when the runner caps the job: eleven network round trips plus a
# verify build can fill the window before anything is published.
if [ $# -gt 0 ]; then
  CRATES=("$@")
fi

live() {
  # The index lags the API, and `cargo publish` resolves against the index --
  # so ask the index, not the web API, or a crate reads as live before its
  # dependents can actually build against it.
  cargo search "$1" --limit 1 2>/dev/null | grep -q "^$1 "
}

for c in "${CRATES[@]}"; do
  if live "$c"; then
    printf 'SKIP    %s (already on the index)\n' "$c"
    continue
  fi

  while true; do
    printf 'PUBLISH %s\n' "$c"
    out=$(cargo publish -p "$c" 2>&1)
    if [ $? -eq 0 ]; then
      printf 'OK      %s\n' "$c"
      break
    fi
    if printf '%s' "$out" | grep -q 'already exists\|already uploaded'; then
      printf 'OK      %s (was already up)\n' "$c"
      break
    fi
    if printf '%s' "$out" | grep -q '429 Too Many Requests'; then
      # Poll in short increments rather than sleeping the window out: a runner that
      # caps a job below the wait is killed mid-sleep and publishes nothing.
      printf 'LIMIT   %s -- %s\n' "$c" "$(printf '%s' "$out" | grep -o 'try again after [^\"]*' | head -1)"
      sleep 60
      continue
    fi
    printf 'FAILED  %s\n%s\n' "$c" "$out"
    exit 1
  done
done

printf '\nALL PUBLISHED\n'
