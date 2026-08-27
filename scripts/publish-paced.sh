#!/usr/bin/env bash
#
# Resume a publish that hit a crates.io rate limit.
#
# For the first release this was the *new-crate* limit: a burst of new names
# and then roughly one per ten minutes, which no workspace of fourteen survives
# in a single run. A release after the first publishes new versions of existing
# crates, whose limit is far more generous, so this is now the fallback rather
# than the expected path -- reach for scripts/publish.sh first.
#
# Idempotent: a crate already on the index at this version is skipped, so this
# can be re-run as often as needed and picks up wherever the last run stopped.
set -uo pipefail

VERSION=$(grep -m1 '^version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
if [ -z "$VERSION" ]; then
  echo "could not read the workspace version from Cargo.toml" >&2
  exit 1
fi
printf 'publishing %s\n\n' "$VERSION"

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
  #
  # Matched on name *and* version. Every name has existed since 0.1.0, so a
  # name-only check would skip all fourteen on any later release: nothing
  # published, and a run that reports SKIP fourteen times and ALL PUBLISHED at
  # the end. That is the worst shape a failure can take, because it looks
  # exactly like the idempotent re-run this script is for.
  cargo search "$1" --limit 1 2>/dev/null | grep -q "^$1 = \"$VERSION\""
}

for c in "${CRATES[@]}"; do
  if live "$c"; then
    printf 'SKIP    %s (already on the index at %s)\n' "$c" "$VERSION"
    continue
  fi

  while true; do
    printf 'PUBLISH %s\n' "$c"
    if out=$(cargo publish -p "$c" 2>&1); then
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

printf '\nALL PUBLISHED at %s\n' "$VERSION"
