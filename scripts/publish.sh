#!/usr/bin/env bash
#
# Publish the workspace to crates.io, in dependency order.
#
# Requires a crates.io token first, once per machine:
#
#     cargo login            # paste a token from https://crates.io/settings/tokens
#
# Publishing is irreversible. A version can be yanked but never replaced, and a
# name is never freed. So this stops at the first failure rather than carrying
# on, and waits for each crate to reach the index before the next one needs it.
#
# Re-running after a partial failure is safe: a crate already at this version
# reports "already uploaded" and is skipped.
set -euo pipefail

# Topological, derived from the manifests rather than guessed. rusty-memory-
# conform and -contrast are publish = false and are absent on purpose.
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

for c in "${CRATES[@]}"; do
  printf '\n=== %s\n' "$c"
  if cargo publish -p "$c"; then
    # The index is eventually consistent, and the next crate cannot resolve
    # this one until it lands. `cargo publish` already waits, but a slow index
    # has been known to outlast it.
    printf 'waiting for the index to carry %s\n' "$c"
    for _ in $(seq 1 30); do
      if cargo search "$c" --limit 1 2>/dev/null | grep -q "^$c "; then
        break
      fi
      sleep 10
    done
  else
    status=$?
    if cargo search "$c" --limit 1 2>/dev/null | grep -q "^$c "; then
      echo "already on the index; continuing"
      continue
    fi
    echo "FAILED on $c (exit $status). Nothing after this was published."
    exit "$status"
  fi
done

printf '\nAll published. The only check that proves it, from outside the workspace:\n'
printf '    cargo new /tmp/adopt && cd /tmp/adopt && cargo add rusty-memory && cargo build\n'
