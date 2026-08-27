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

# The version being published, read from the workspace rather than typed, so it
# cannot drift from what cargo will actually upload.
VERSION=$(grep -m1 '^version = ' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
if [ -z "$VERSION" ]; then
  echo "could not read the workspace version from Cargo.toml" >&2
  exit 1
fi
printf 'publishing %s\n\n' "$VERSION"

# Is this crate on the index *at this version*?
#
# Not "does the name exist". Every name has existed since 0.1.0, so a check on
# the name alone answers yes for every crate in every release after the first.
# That turned a real failure into "already on the index; continuing" and would
# have reported a clean run having published nothing. The first release could
# not tell the two apart, because for a new crate the name appearing and the
# version appearing were the same event.
at_version() {
  cargo search "$1" --limit 1 2>/dev/null | grep -q "^$1 = \"$VERSION\""
}

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
  if at_version "$c"; then
    printf 'already on the index at %s; skipping\n' "$VERSION"
    continue
  fi
  if cargo publish -p "$c"; then
    # The index is eventually consistent, and the next crate cannot resolve
    # this one until it lands. `cargo publish` already waits, but a slow index
    # has been known to outlast it.
    printf 'waiting for the index to carry %s %s\n' "$c" "$VERSION"
    landed=""
    for _ in $(seq 1 30); do
      if at_version "$c"; then
        landed="yes"
        break
      fi
      sleep 10
    done
    if [ -z "$landed" ]; then
      # Five minutes without the version appearing. Carrying on would fail the
      # next crate with an unresolvable dependency and blame the wrong one.
      echo "published $c but the index never showed $VERSION. Stopping." >&2
      exit 1
    fi
  else
    status=$?
    if at_version "$c"; then
      echo "already on the index at $VERSION; continuing"
      continue
    fi
    echo "FAILED on $c (exit $status). Nothing after this was published."
    exit "$status"
  fi
done

printf '\nAll published at %s. The only check that proves it, from outside the workspace:\n' "$VERSION"
printf '    cargo new /tmp/adopt && cd /tmp/adopt && cargo add rusty-memory && cargo build\n'
