# Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `cargo add rusty-memory` work, so the library can be adopted at all.

**Architecture:** Rename every package to `rusty-memory-*` for crates.io identity while keeping the short `rm_*` paths in source via Cargo's `package =` dependency rename, so no `.rs` file changes. Add a `rusty-memory` facade crate that re-exports the adopter-facing surface and is the only crate carrying a semver promise. Publish in dependency order.

**Tech Stack:** Rust, Cargo, crates.io. No new dependencies.

**Positioning:** `docs/positioning.md` — "the library cannot be adopted at all" is listed there as the binding constraint, ahead of positioning.

## Global Constraints

- **`rm-engine` is already taken on crates.io** by an unrelated 0.1.0 risk-management crate. A rename is forced, not chosen.
- **`rusty-memory` is free** as of 2026-08-26 and should be claimed in this work.
- `rm-core`, `rm-store`, `rm-index` and the rest are generic names this project should not squat on crates.io even though they are free. Namespacing all of them is the consistent answer to a rename that was forced on one.
- **No `.rs` file changes.** Cargo's `package =` rename keeps every `use rm_engine::…` working. If any task finds itself editing source, stop — the approach has gone wrong.
- The binary keeps the name `rmem`. `rmem` is taken as a *crate*, so `cargo install rmem` fetches an unrelated memory-usage tool; the install command becomes `cargo install rusty-memory-cli`. Binary names need not be unique on crates.io, so this is a documentation problem, not a blocker.
- Publishing is irreversible: a version can be yanked but never replaced, and a name cannot be freed. Every task ends with `--dry-run` before anything is pushed.

---

### Task 1: Namespace every package, changing no source

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]` and `[workspace.package]`)
- Modify: `crates/*/Cargo.toml` (15 files, `[package] name` only)

**Interfaces:**
- Produces: package names `rusty-memory-core`, `-survivor`, `-store`, `-resolve`, `-index`, `-engine`, `-graph`, `-extract`, `-providers`, `-embed`, `-host`, `-cli`, `-mcp`, `-conform`, `-contrast`.
- Consumes: nothing. Local dependency aliases stay `rm-core` etc., so source is untouched.

- [ ] **Step 1: Rename the packages**

In each `crates/<c>/Cargo.toml`, change only the name:

```toml
[package]
name = "rusty-memory-engine"   # was rm-engine
```

- [ ] **Step 2: Keep the short aliases in the workspace dependency table**

In the root `Cargo.toml`, `[workspace.dependencies]` gains a `package` key per
entry. The key on the left is what source code sees; `package` is what
crates.io sees:

```toml
# The key is the local alias, so `use rm_engine::…` keeps working in every
# crate and no .rs file changes. `package` is the crates.io identity, which
# had to be namespaced: `rm-engine` is taken there by an unrelated crate, and
# `rm-core` and friends are too generic to squat on even though they are free.
rm-core = { path = "crates/rm-core", version = "0.1.0", package = "rusty-memory-core" }
rm-engine = { path = "crates/rm-engine", version = "0.1.0", package = "rusty-memory-engine" }
# ... and so on for all fifteen
```

The `version` key is required as well as `path`: a path-only dependency cannot
be published, because the published crate has no path to follow.

- [ ] **Step 3: One workspace version**

Add to `[workspace.package]`:

```toml
version = "0.1.0"
```

and set `version.workspace = true` in each crate.

Rationale to record in the commit: `rm-core` and `rm-resolve` were already
0.1.0 while the rest were 0.0.0. Fifteen independently versioned crates is a
release burden that buys nothing until somebody depends on a sub-crate, and the
facade in Task 3 exists precisely so nobody needs to. If a sub-crate later
earns its own cadence it can be split out then.

- [ ] **Step 4: Verify nothing in source moved**

```bash
git diff --stat -- '*.rs'
```

Expected: **empty**. That is the check that this task did what it claimed.

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

Expected: PASS, unchanged test count.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/*/Cargo.toml
git commit -m "Namespace the packages for crates.io, leaving every use path alone"
```

---

### Task 2: Decide what is not published

**Files:**
- Modify: `crates/rm-conform/Cargo.toml`, `crates/rm-contrast/Cargo.toml`

- [ ] **Step 1: Mark the harnesses unpublishable**

`rm-conform` is the differential reference model and `rm-contrast` measures
read cost. Both are internal tooling, neither is depended on by anything a user
would install, and publishing them would put two names on crates.io that
promise something to nobody.

```toml
[package]
publish = false
```

Confirm nothing published depends on them:

```bash
rg -l 'rm-conform|rm-contrast' crates/*/Cargo.toml
```

Expected: only their own manifests. If a published crate depends on either, it
must be published too — say so and stop, because `publish = false` will then
fail the dry run in Task 4 rather than silently working.

- [ ] **Step 2: Commit**

```bash
git commit -am "The conformance and cost harnesses are ours, not the public's"
```

---

### Task 3: The facade

**Files:**
- Create: `crates/rusty-memory/Cargo.toml`
- Create: `crates/rusty-memory/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces: the crate an adopter depends on. Everything else becomes an implementation detail with no semver promise.

- [ ] **Step 1: Write the failing test**

```rust
/// The surface an adopter gets from the one crate they are told to depend on.
///
/// This is the semver promise. It exists as a test because a re-export that
/// silently stops compiling is the one breakage a facade is supposed to make
/// impossible, and because the README's example must be reachable from here
/// without naming an internal crate.
#[test]
fn the_facade_carries_everything_the_readme_example_needs() {
    use rusty_memory::{Believed, Engine, Metric, Policy, Strategy, VectorIndex};
    let _ = Policy::new(Strategy::ValidInterval);
    let _ = Policy::new(Strategy::MostRecent);
    let _: fn() -> Believed = || Believed::Unknown;
    let _ = VectorIndex::new(3, Metric::Cosine);
    let _: Option<Engine> = None;
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rusty-memory`
Expected: FAIL — the crate does not exist.

- [ ] **Step 3: Write the facade**

```rust
//! A memory that knows what it doesn't know.
//!
//! Depend on this crate, not on the `rusty-memory-*` crates beneath it. Those
//! are how this one is built and they change without notice; this is the
//! surface with a semver promise attached.
//!
//! The distinction this store exists for is in [`Believed`]: `Value` and
//! `Absent` and `Unknown` are three different answers, and "they have no
//! employer" is not "nobody has ever said".

pub use rm_engine::{Believed, Engine, Metric, Policy, Strategy, VectorIndex};
pub use rm_resolve::{Comparator, Record, Ruleset};
// Re-export the error types too: a caller who cannot name the error a
// function returns cannot handle it, and sending them to an internal crate
// for it defeats the facade.
```

Fill the re-export list from what the README's example and the MCP host
actually use. Anything not needed to use the store stays unexported — a facade
that re-exports everything is not a facade.

- [ ] **Step 4: Run the test**

Run: `cargo test -p rusty-memory`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rusty-memory Cargo.toml
git commit -m "One crate to depend on, and a semver promise attached to it"
```

---

### Task 4: Dry run, in dependency order

**Files:** none — this task produces confidence, not changes.

- [ ] **Step 1: Dry-run every crate in order**

The order is topological and was derived from the manifests, not guessed:

```
 1. rusty-memory-core
 2. rusty-memory-extract      6. rusty-memory-index      11. rusty-memory-host
 3. rusty-memory-survivor     7. rusty-memory-resolve    12. rusty-memory-cli
 4. rusty-memory-store        8. rusty-memory-engine     13. rusty-memory-mcp
 5. rusty-memory-graph        9. rusty-memory-embed      14. rusty-memory
                             10. rusty-memory-providers
```

```bash
for c in rusty-memory-core rusty-memory-extract ... rusty-memory; do
  cargo publish --dry-run -p "$c" || { echo "FAILED: $c"; break; }
done
```

A dry run cannot see crates that are not yet on the index, so later crates in
the list will fail on unresolvable dependencies until the earlier ones are
really published. Expect that, and read each failure: **"could not find
`rusty-memory-core`"** is the expected shape, while a missing `description`,
a missing license file, or an excluded file that is actually needed are real
problems to fix now.

- [ ] **Step 2: Check what would ship**

```bash
cargo package -p rusty-memory-core --list
```

Confirm `LICENSE` is included and that no scratch store (`memory.json`,
`memory.vec`, `*.lock` beside a store) is being packaged. The workspace has
picked up stray store files at its root before.

- [ ] **Step 3: Commit any manifest fixes**

---

### Task 5: Publish, and say what is stable

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Publish in order**

```bash
cargo publish -p rusty-memory-core
# wait for the index, then the next, in the order above
```

Publish is **irreversible**: a version can be yanked but never replaced, and a
name never freed. Do not batch this behind a script that continues past an
error.

- [ ] **Step 2: Document the stability story**

The positioning document notes this is a good story that is not told anywhere
an adopter would look. Add to `README.md`:

```markdown
## Using it

```toml
[dependencies]
rusty-memory = "0.1"
```

Depend on `rusty-memory`. The `rusty-memory-*` crates underneath it are how it
is built, they are published only because Cargo requires it, and they change
without notice.

The CLI and the MCP server install from their own crates, and the binary is
called `rmem` — `cargo install rmem` fetches an unrelated tool of the same
name:

```sh
cargo install rusty-memory-cli    # the `rmem` binary
cargo install rusty-memory-mcp    # the `rmem-mcp` server
```
```

- [ ] **Step 3: Verify from outside the workspace**

The only test that matters. In a scratch directory, with no path override:

```sh
cargo new /tmp/adopt && cd /tmp/adopt
cargo add rusty-memory
# paste the README's example, then:
cargo build
```

A green build here is the deliverable. Everything before it is preparation, and
a workspace that builds itself proves nothing about what an adopter receives.

- [ ] **Step 4: Record the decision**

From a script file, never inline:

```bash
rmem decide "Publish namespaced, behind one facade crate" \
  "rusty-memory is the only crate with a semver promise; the rest are published because Cargo requires it" \
  --context "rm-engine was already taken on crates.io by an unrelated risk-management crate, and rm-core and friends were free but too generic to squat on. Renamed all fifteen with Cargo's package= alias so no source file changed" \
  --because "a facade is not a convenience, it is what lets fifteen internal crates keep moving without breaking anyone. Without one an adopter has to guess which of them is the public API, and every guess becomes a compatibility obligation nobody agreed to" \
  --scope "*"
```

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "Say which crate to depend on, and which are ours to move"
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request states the version published, that `rusty-memory` is the only
crate carrying a semver promise, and that the `rmem` binary name is shared with
an unrelated crate on crates.io so the install command is
`cargo install rusty-memory-cli`.

It also reports the outside-the-workspace build from Task 5 Step 3, because
that is the only evidence that any of this worked.
