# Releasing

Fourteen crates go to crates.io in dependency order. `rm-conform` and
`rm-contrast` are `publish = false` and stay in the workspace.

Publishing is irreversible: a version can be yanked but never replaced, and a
name is never freed. Everything below is arranged so that the irreversible step
happens last and only once.

## Before

1. **Bump.** Every version lives in the root `Cargo.toml` — one `version` and
   one per workspace dependency, seventeen strings that must move together.
   `cargo metadata --no-deps` should then report a single distinct version:

   ```sh
   cargo metadata --no-deps --format-version 1 \
     | python -c "import json,sys; print({p['version'] for p in json.load(sys.stdin)['packages']})"
   ```

2. **CHANGELOG.** Rename `Unreleased` to the version. If a prepared version was
   never published — 0.2.0 was — say so in its own section rather than folding
   it in, because its breaking changes are still real for anyone upgrading past
   it.

3. **README.** The `rusty-memory = "x.y"` line ships *inside* the published
   crate, so it must name the version being published, not the last one. It
   said `= "0.2"` while crates.io had 0.1.0, which does not resolve at all:

   ```
   error: failed to select a version for the requirement `rusty-memory = "^0.2"`
   candidate versions found which didn't match: 0.1.0
   ```

4. **The full gate.** `cargo test --workspace`, `cargo clippy --workspace
   --all-targets -- -D warnings`, `cargo fmt --all --check`.

5. **Dry run.** `cargo publish --dry-run --workspace` packages and verifies each
   crate without uploading. On this laptop it OOMs at default parallelism —
   `rustc-LLVM ERROR: out of memory` — so pass `-j 1`, or run it in CI. That is
   a limit of the machine, not of the package.

## Publishing

```sh
cargo login          # once per machine, token from crates.io/settings/tokens
bash scripts/publish.sh
```

The script reads the version from `Cargo.toml`, publishes in dependency order,
waits for each crate to appear on the index at *that version* before moving on,
and stops at the first real failure.

Use `scripts/publish-paced.sh` only if a rate limit stops the first script. Its
ten-minute pacing is for the **new-crate** limit, which applied to 0.1.0 when
every name was new; new versions of existing crates are limited far more
generously.

### The check that has bitten twice

Both scripts once asked whether a crate's *name* was on the index and treated
that as "already published". For the first release that is sound — a new
crate's name appearing and its version appearing are the same event. For every
release after it, every name already exists, so a name-only check answers yes
for everything: `publish.sh` swallowed real failures as "already on the index",
and `publish-paced.sh` skipped all fourteen and printed `ALL PUBLISHED` having
uploaded nothing.

Both now match `name = "version"`. If you touch that logic, check it can still
tell one release from another:

```sh
cargo search rusty-memory-core --limit 1
#   at the published version -> matches
#   at the version being released -> must not match, until it does
```

## After

The only check that proves it, from outside the workspace:

```sh
cargo new /tmp/adopt && cd /tmp/adopt
cargo add rusty-memory && cargo build
```

Then update the documentation that describes the *published* state, which the
release has just falsified:

- `docs/positioning.md`, "What is blocking adoption right now" — the version
  drift item, and the count of published crates.
- Anything else asserting which version is live. `rg -n '0\.[0-9]+\.0' docs/
  README.md` finds the candidates.

A stale blocker list is worse than none: it hides the real blocker underneath
it, and that is how "nothing is published" survived in `positioning.md` for a
month after fourteen crates went up.
