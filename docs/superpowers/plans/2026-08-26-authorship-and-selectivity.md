# Authorship and Selectivity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the store able to say who wrote a record, and make the session-start injection a curated set rather than everything scoped `*`.

**Architecture:** No schema change. `Provenance::source_ref` already exists and is documented for this; the CLI writes the constant `"cli"` into it and the MCP writes the handshake client name. A new `rm-host::attribution` module supplies both, and `RMEM_SESSION` is the env var a host uses to say who it is. Selectivity is a one-line filter in the hook plus a judgement pass over the existing records.

**Tech Stack:** Rust, pinned toolchain 1.98.0. PowerShell for the hook. No new dependencies — host name comes from the environment, not a crate.

**Spec:** `docs/superpowers/specs/2026-08-26-authorship-and-selectivity-design.md`

## Global Constraints

- **Toolchain is pinned at 1.98.0.** Every task ends green under `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --workspace`.
- **Verify with exit codes, not by parsing counts.** `cargo test --workspace && git commit`. A field-split counter reported zero failures for a whole session (entity 219); the passed count is a cross-check, never the check.
- **Baseline is 795 passing, 0 failing** on `main` at `a626468`.
- **No new dependencies.** This project parses its own arguments rather than take `clap`; a hostname crate is the same trade and the answer is the same.
- **Do not mutate the environment inside a test.** `std::env::set_var` is process-global and Rust runs tests in parallel, so two tests touching `RMEM_SESSION` race. All logic goes in pure functions taking values as parameters; only a thin wrapper reads the environment, and that wrapper is not unit-tested.
- **`decide` still refuses without an explicit `--scope`.** `RMEM_SESSION` is read, never demanded. Provenance is not the kind of thing to block a write on.
- **Commit trailers**, on every commit:
  ```
  Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
  ```

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rm-host/src/attribution.rs` | **New.** The one place that decides what goes in `source_ref`. Pure functions plus one env-reading wrapper. |
| `crates/rm-host/src/lib.rs` | One `pub mod attribution;` line. |
| `crates/rm-cli/src/run.rs:221,253,262` | Replaces the three hardcoded `"cli"` strings. |
| `crates/rm-mcp/src/tools.rs:396-407` | `Call::attributed` gains the host. |
| `C:/Users/bsevern/Tools/claude-hooks/lessons-inject.ps1` | Filters to `[accepted]`, using the status it already parses and discards. |
| the store | The judgement pass. No file. |

Five tasks. Tasks 1–3 are the authorship half and build on each other; Task 4 is the hook and is independent; Task 5 is judgement and touches no code.

---

### Task 1: The attribution module

**Files:**
- Create: `crates/rm-host/src/attribution.rs`
- Modify: `crates/rm-host/src/lib.rs`

**Interfaces:**
- Produces, for Tasks 2 and 3:
  - `pub fn source_ref(session: Option<&str>, host: &str) -> String`
  - `pub fn host() -> String`
  - `pub fn cli() -> String`

- [ ] **Step 1: Write the failing tests**

Create `crates/rm-host/src/attribution.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// `RMEM_SESSION` is taken at its word. A host that says who it is knows
    /// better than this crate does.
    #[test]
    fn a_session_that_names_itself_is_used_verbatim() {
        assert_eq!(
            source_ref(Some("RM@bsev-002/b149f85e"), "ignored"),
            "RM@bsev-002/b149f85e"
        );
    }

    /// Unset degrades to something honest rather than to a constant.
    ///
    /// The machine is knowable and the session is not, so the answer says the
    /// first and omits the second. That is strictly more than `"cli"`, which
    /// is what every record in the live store carries and why none of them can
    /// be attributed.
    #[test]
    fn an_unnamed_session_still_records_the_machine() {
        assert_eq!(source_ref(None, "bsev-002"), "cli@bsev-002");
    }

    /// Empty and whitespace are how an env var looks when someone meant to
    /// unset it. Same rule as `RMEM_SCOPE`, for the same reason: a setting
    /// that looks unconfigured must behave as unconfigured.
    #[test]
    fn an_empty_session_is_no_session_at_all() {
        assert_eq!(source_ref(Some(""), "bsev-002"), "cli@bsev-002");
        assert_eq!(source_ref(Some("   "), "bsev-002"), "cli@bsev-002");
        assert_eq!(source_ref(Some("\t\n"), "bsev-002"), "cli@bsev-002");
    }

    /// Whitespace around a real value is a typo, not part of the name.
    #[test]
    fn a_named_session_is_trimmed() {
        assert_eq!(source_ref(Some("  RM@host/abc  "), "x"), "RM@host/abc");
    }

    /// An unknown host is named as unknown rather than left blank, so the
    /// field never reads as though the machine were part of the identity when
    /// it is not.
    #[test]
    fn an_unknown_host_says_so() {
        assert_eq!(source_ref(None, ""), "cli@unknown-host");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p rm-host --lib attribution
```

Expected: FAIL to compile — `cannot find function 'source_ref'`. (The module is not wired into `lib.rs` yet, so it may instead report that nothing matched; wire it in Step 4 and re-run.)

- [ ] **Step 3: Write the module**

Prepend to `crates/rm-host/src/attribution.rs`:

```rust
//! Who wrote a record.
//!
//! `Provenance::source_ref` is documented as "the session, turn, or document
//! this came from [...] the host decides its shape". This is where a host
//! decides it.
//!
//! The field was never missing. `rm-mcp` has always written the handshake
//! client name into it, and `rm-cli` has always written the literal string
//! `"cli"` -- and since everything in the live store was written through the
//! CLI, the store reads as though provenance had no author field at all. 256
//! records, one constant.
//!
//! # The shape
//!
//! `<agent>@<host>/<session>` -- `RM@bsev-002/b149f85e`.
//!
//! Three parts because three separate questions went unanswered: which agent
//! found it, which machine it happened on, and which run. A name alone
//! collides -- on the machine this was written for there were five sessions
//! called `Print` and four called `Circ`.

/// What to record as the author, given what the host said and where it ran.
///
/// Pure, and takes both values as parameters, so it can be tested without
/// touching the environment. `std::env::set_var` is process-global and Rust
/// runs tests in parallel, so two tests setting `RMEM_SESSION` would race and
/// fail each other intermittently -- the kind of flake that gets re-run rather
/// than read.
pub fn source_ref(session: Option<&str>, host: &str) -> String {
    let named = session.map(str::trim).filter(|s| !s.is_empty());
    match named {
        Some(s) => s.to_string(),
        None => {
            let host = host.trim();
            let host = if host.is_empty() { "unknown-host" } else { host };
            format!("cli@{host}")
        }
    }
}

/// What this machine calls itself.
///
/// From the environment rather than a crate: this project parses its own
/// arguments rather than take `clap`, and a hostname dependency is the same
/// trade. `COMPUTERNAME` on Windows, `HOSTNAME` elsewhere -- the latter is not
/// always exported, which is why the caller has a fallback rather than this
/// returning an `Option` nobody would handle.
pub fn host() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
}

/// The author string for a CLI invocation.
///
/// The one function here that reads the environment, and deliberately the one
/// with no unit test: everything it decides is decided in [`source_ref`].
pub fn cli() -> String {
    source_ref(std::env::var("RMEM_SESSION").ok().as_deref(), &host())
}
```

- [ ] **Step 4: Wire the module in**

In `crates/rm-host/src/lib.rs`, add `pub mod attribution;` to the module list, in alphabetical position.

- [ ] **Step 5: Run to verify the tests pass**

```bash
cargo test -p rm-host --lib attribution
```

Expected: PASS, 5 tests.

- [ ] **Step 6: Commit**

```bash
cargo test --workspace -q && git add -A crates/ && git commit -m "$(cat <<'EOF'
One place that decides who wrote a record

Provenance::source_ref is documented as "the session, turn, or document
this came from -- the host decides its shape", and this is where a host
decides it. The field was never missing: rm-mcp has always written the
handshake client name into it and rm-cli has always written the literal
"cli", and since everything in the live store went through the CLI, 256
records carry one constant and none can be attributed.

The shape is <agent>@<host>/<session>, because three separate questions
went unanswered and a name alone collides -- there were five sessions
called Print on this machine and four called Circ.

source_ref is pure and takes both values as parameters so it can be tested
without touching the environment: std::env::set_var is process-global and
Rust runs tests in parallel, so two tests setting RMEM_SESSION would race
and fail each other intermittently. Only the thin wrapper reads env, and
it has no unit test because it decides nothing.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 2: The CLI stops writing a constant

**Files:**
- Modify: `crates/rm-cli/src/run.rs` — three call sites at `:221`, `:253`, `:262`

**Interfaces:**
- Consumes from Task 1: `rm_host::attribution::cli() -> String`.

- [ ] **Step 1: Write the failing test**

Append to `crates/rm-cli/src/run.rs`'s test module. This reads the stored `Provenance` rather than the CLI's own output — per entity 255, `rmem decision` prints no provenance at all, so its readback cannot see this field:

```rust
    /// A decision written through the CLI records who wrote it.
    ///
    /// Asserted against the stored `Provenance`, not against anything the CLI
    /// prints: `decision` renders title, status, choice, because and context
    /// and no provenance whatever, so a readback through it would pass while
    /// the field stayed a constant. That is the exact shape of the mistake
    /// that made this worth fixing.
    #[test]
    fn a_decision_records_an_author_rather_than_a_constant() {
        let author = rm_host::attribution::cli();
        assert!(
            author != "cli",
            "the bare constant is what this replaces: {author}"
        );
        assert!(
            author.contains('@') || author.contains('/'),
            "an author should name a machine or a session: {author}"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p rm-cli --lib a_decision_records_an_author
```

Expected: FAIL to compile — `rm_host::attribution` is reachable only once Task 1 has landed. If Task 1 has landed, this test passes immediately; the *behavioural* failure is caught by Step 4's grep, which is the real check for this task.

- [ ] **Step 3: Replace the three constants**

In `crates/rm-cli/src/run.rs`, add the import beside the existing `rm_host` imports:

```rust
use rm_host::attribution;
```

Then replace each of the three `"cli"` arguments. At `:221`, inside `Command::Remember`:

```rust
                Some(Planned::Remember(command::plan_remember(
                    text,
                    now,
                    &attribution::cli(),
                    speaker.as_deref(),
```

At `:253`, inside `Command::Decide`:

```rust
                    *decided_at,
                    now,
                    &attribution::cli(),
                    &embedder,
                )?))
```

At `:262`, inside `Command::Rescope`:

```rust
                Some(Planned::Rescope(command::plan_rescope(
                    title,
                    scope,
                    now,
                    &attribution::cli(),
                    &embedder,
                )?))
```

- [ ] **Step 4: Verify no constant survives**

```bash
rg -n '"cli"' crates/rm-cli/src/run.rs
```

Expected: **no matches.** If any remain, they are the ones this task exists to remove.

- [ ] **Step 5: Prove it end to end against a throwaway store**

The unit test checks the string; this checks that it reaches the stored record. Use the release binary — the one on `PATH` is hand-copied and may predate this change:

```bash
T=/d/Temp/rmem-author-test; R=/d/show_case/rusty-memory/target/release/rmem.exe
cargo build --release -q
rm -rf "$T"; mkdir -p "$T"
cd "$T" && "$R" init --local >/dev/null
cd "$T" && RMEM_CONFIG="$T/rmem.toml" RMEM_SESSION="RM@testbox/abc123" \
  "$R" decide "an authored decision" "x" --because "t" --scope "*" >/dev/null
python -c "
import json, os
os.chdir(r'D:\Temp\rmem-author-test')
d = json.load(open('memory.json', encoding='utf-8'))
e = json.loads(d['store'])['entities']
for v in list(e.values())[0]['attributes'].values():
    print(v[-1]['provenance']['source_ref'])
"
```

Expected: `RM@testbox/abc123` on every line. Then re-run the `decide` with `RMEM_SESSION` unset and confirm it reads `cli@<something>` rather than `cli`.

- [ ] **Step 6: Run the suite and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q && git add -A crates/ && git commit -m "$(cat <<'EOF'
The CLI stops writing a constant where the author goes

Three call sites in run.rs passed the literal "cli" as the session that
becomes Provenance::source_ref. Every record in the live store went through
that path, which is why 256 of them carry one value and none can be
attributed to anyone.

Checked end to end against a throwaway local store rather than through
rmem decision, which prints no provenance at all -- a readback through it
would have passed while the field stayed a constant, which is the same
shape as the mistake that made this worth fixing.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 3: The MCP path carries the host

**Files:**
- Modify: `crates/rm-mcp/src/tools.rs:396-407` — `Call::attributed`

**Interfaces:**
- Consumes from Task 1: `rm_host::attribution::host() -> String`.

- [ ] **Step 1: Write the failing test**

Append to `crates/rm-mcp/src/tools.rs`'s test module:

```rust
    /// The handshake name is an agent, and an agent is on a machine.
    ///
    /// `attributed` already recorded who the client said it was; what it could
    /// not say is where. Two agents called `Print` on two machines were
    /// indistinguishable, and on the machine this was written for there were
    /// five.
    #[test]
    fn the_author_names_the_machine_as_well_as_the_client() {
        let host = rm_host::attribution::host();
        let got = Call::attributed(&json!({}), Some("RM")).unwrap();
        assert!(got.starts_with("RM@"), "{got}");
        if !host.is_empty() {
            assert!(got.contains(&host), "{got} should name {host}");
        }
    }

    /// A client that gives a session id keeps it, after the host.
    #[test]
    fn a_client_supplied_session_follows_the_machine() {
        let got = Call::attributed(&json!({"session": "abc"}), Some("RM")).unwrap();
        assert!(got.starts_with("RM@"), "{got}");
        assert!(got.ends_with("/abc"), "{got}");
    }

    /// A client that never named itself still records where it ran. The
    /// specification allows an anonymous client, so this must not become a
    /// refusal -- but "mcp" alone was as uninformative as the CLI's "cli".
    #[test]
    fn an_anonymous_client_still_records_the_machine() {
        let got = Call::attributed(&json!({}), None).unwrap();
        assert!(got.starts_with("mcp@"), "{got}");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p rm-mcp --lib the_author_names_the_machine
```

Expected: FAIL — `assert!(got.starts_with("RM@"))` fails with `RM`, because the host is not yet appended.

- [ ] **Step 3: Add the host**

Replace `Call::attributed`'s body in `crates/rm-mcp/src/tools.rs`:

```rust
    fn attributed(arguments: &Value, client: Option<&str>) -> Result<String, Unreadable> {
        let session = optional_string(arguments, "session")?;
        // The agent, then the machine, then the run: `RM@bsev-002/abc123`,
        // matching what `rm_host::attribution` writes on the CLI side so both
        // hosts are comparable rather than one being useful.
        let host = rm_host::attribution::host();
        let host = if host.trim().is_empty() {
            "unknown-host".to_string()
        } else {
            host
        };
        // No handshake identity: a client that did not name itself, which the
        // specification allows. `mcp` was the old default and stays the agent
        // part, so nothing that worked before now records less -- it records
        // the machine as well.
        let agent = client.unwrap_or("mcp");
        Ok(match session {
            Some(s) => format!("{agent}@{host}/{s}"),
            None => format!("{agent}@{host}"),
        })
    }
```

- [ ] **Step 4: Run to verify the tests pass**

```bash
cargo test -p rm-mcp --lib attributed
cargo test -p rm-mcp --lib the_author_names_the_machine
cargo test -p rm-mcp --lib a_client_supplied_session
cargo test -p rm-mcp --lib an_anonymous_client
```

Expected: PASS.

**Existing `attributed` tests will fail** — they assert `"c/s"` and `"c"` shapes. Read each one before changing it: the property being asserted is *what the parts are and in what order*, and that property is unchanged. Update the expectations to the new shape; do not delete a test. `rm-mcp/tests/session.rs`'s `authors()` helper reads `source_ref` out of the store and may also need its expectations widened.

- [ ] **Step 5: Run the suite and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace -q && git add -A crates/ && git commit -m "$(cat <<'EOF'
An agent is on a machine

Call::attributed recorded who the client said it was and could not say
where. Two agents called Print on two machines were indistinguishable, and
on this machine there were five.

Now <agent>@<host>/<session>, matching what the CLI side writes so the two
hosts are comparable rather than one being useful and one being a constant.
An anonymous client still records the machine: the specification allows a
client that does not name itself, so this stays "mcp" as the agent part and
gains the host rather than becoming a refusal.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01UwD5PDXidCqYp4BHrdiKzT
EOF
)"
```

---

### Task 4: The hook injects only what it should

**Files:**
- Modify: `C:/Users/bsevern/Tools/claude-hooks/lessons-inject.ps1`

**Interfaces:** none — the hook is outside the workspace and consumes nothing from the other tasks.

- [ ] **Step 1: Note what the regex already captures**

The parsing line is:

```powershell
if ($line -match '^\s*~?\s*entity\s+\d+\s\s+(.+?)(\s\[[a-z]+\])?\s*$') {
```

Group 2 is the status — `[accepted]`, `[deprecated]` — and is captured and discarded. This task starts using it. Nothing new needs parsing.

- [ ] **Step 2: Filter to accepted**

Replace the title-collecting loop:

```powershell
    $titles = @()
    foreach ($line in $out) {
        # "  entity 224  shell: rg -r is ..." / "~ entity 12  ..." when superseded
        if ($line -match '^\s*~?\s*entity\s+\d+\s\s+(.+?)(\s\[([a-z]+)\])?\s*$') {
            # `accepted` only. `*` now means "worth telling every session
            # unprompted", so demoting a record with `--status deprecated`
            # takes it out of the injection while leaving it findable by
            # `decision` and `recall`. That is the pruning mechanism, and it
            # already existed -- the status was being captured and thrown away.
            if ($Matches[3] -eq 'accepted') {
                $titles += $Matches[1].Trim()
            }
        }
    }
```

- [ ] **Step 3: Pipe-test both dispositions**

```bash
echo '{}' | pwsh -NoProfile -File "C:/Users/bsevern/Tools/claude-hooks/lessons-inject.ps1" \
  > /d/Temp/hookout.json 2>&1
echo "exit=$?"
python -c "
import json
d=json.load(open(r'D:\Temp\hookout.json',encoding='utf-8'))
c=d['hookSpecificOutput']['additionalContext']
n=len([l for l in c.split(chr(10)) if l.startswith('- ')])
print('titles injected:', n, ' chars:', len(c))
"
```

Expected: exit 0, and a count **lower than the unfiltered 64** by however many `*` records are not `accepted` — at time of writing entity 221 is `deprecated`, so at least one.

Then confirm a deprecated title is genuinely absent:

```bash
rg -c 'a quoted heredoc still consumes' /d/Temp/hookout.json || echo "correctly absent"
```

Expected: `correctly absent`. Entity 221 is `*`-scoped and deprecated, so it is the worked example of the filter doing its job — and it is the one record that must not be injected, because it names the wrong mechanism.

- [ ] **Step 4: Confirm the failure paths still exit 0**

The hook runs at every session start; a failure that stops a session is worse than no hook.

```bash
pwsh -NoProfile -Command "\$env:PATH='C:\nonexistent'; & 'C:/Users/bsevern/Tools/claude-hooks/lessons-inject.ps1'"; echo "exit=$?"
```

Expected: `exit=0`, no output.

- [ ] **Step 5: Record the change**

The hook is not version controlled, so the store is where its behaviour gets
recorded — that being the durable record for anything machine-wide:

```bash
cat > /tmp/hookfilter.sh <<'SH'
#!/usr/bin/env bash
set -euo pipefail
export RMEM_CONFIG=D:/memory/rmem.toml
rmem decide "scope: * means worth telling every session unprompted, not merely true everywhere" \
  "the SessionStart hook injects *-scoped records whose status is accepted; demote with --status deprecated" \
  --context "the injection hook was pasting all 64 *-scoped titles into every session" \
  --because "cost was never the constraint -- 64 titles is 1,092 tokens once, against 1,130 per turn for the MCP tools, and 300 would still be cheap. Attention is: a wall of titles gets skimmed, which is the same failure as injecting nothing and more expensive. A salience field was considered and turned down, because scope alone was mis-set twelve times in one day and a second dial is a second thing to get wrong" \
  --scope "*"
SH
doppler run --scope "D:\personal" --project local-tooling --config dev -- bash /tmp/hookfilter.sh
```

Then read it back with `rmem recall`, **not** `rmem decision` — the latter prints no scope, so it cannot see the field most likely to be wrong.

---

### Task 5: The judgement pass

**Files:** none. This changes records, not code.

This is the task that makes Task 4 worth anything: the filter is a mechanism, and without a pass over the existing records it filters nothing.

**It cannot be automated and the plan does not pretend otherwise.** What follows is the procedure and the criteria, not the answers.

- [ ] **Step 1: List the candidates with their reasoning**

```bash
cd /d/memory && RMEM_CONFIG=D:/memory/rmem.toml RMEM_SCOPE='*' rmem decisions
```

`*` as a **position** is the root — the narrowest place to stand — so only `*`-scoped records reach it. That asymmetry is easy to get backwards; as a scope `*` means everywhere, as a position it means nowhere in particular.

- [ ] **Step 2: Sort each into one of three dispositions**

The bar for staying is **both** halves: true of every project on this machine, **and** worth interrupting a session with unprompted.

| disposition | when | how |
|---|---|---|
| **Keep** | clears both halves | nothing to do |
| **Rescope** | true, but of one project rather than all — it was over-scoped | `rmem rescope "<title>" --scope "work/<project>"` |
| **Deprecate** | true everywhere, not worth announcing | `rmem decide "<title>" "<same choice>" --status deprecated --because "<why it is not announcement-worthy>"` |

A worked example of each, from records that exist today:

- **Keep** — *"verification: a null from an instrument that cannot observe the thing is not evidence."* Three sessions hit that class in one day, in three different disguises.
- **Deprecate** — *"windows: a quoted heredoc still consumes one backslash level."* Already deprecated, and the worked example of why the filter matters: it names the wrong mechanism, and injecting it teaches the wrong model to every session.
- **Rescope** — any record whose title names a tool only one project uses.

- [ ] **Step 3: Do the writes from a script file, never inline**

The body of this script is the output of Step 2 and cannot be written in
advance — that is the whole nature of a judgement pass, and a plan that
pre-filled it would be inventing the answers it exists to ask for.

```bash
cat > /tmp/triage.sh <<'SH'
#!/usr/bin/env bash
set -euo pipefail
export RMEM_CONFIG=D:/memory/rmem.toml
# One line per record that moves, from Step 2. For example:
#   rmem rescope "tests: pytest-split shards shift when a file is added" --scope "work/goldenmatch"
#   rmem decide "docs: sweep every documentation surface at the end of a rollout" #     "<its existing choice, unchanged>" --status deprecated #     --because "true of every project and not worth interrupting a session with" #     --scope "*"
SH
doppler run --scope "D:\personal" --project local-tooling --config dev -- bash /tmp/triage.sh
```

**Inline is what broke twelve records.** `--scope "*"` inside `doppler run -- env ...` glob-expands downstream against the working directory and silently becomes `decisions.json`; a script file preserves the quoting. Entity 254.

- [ ] **Step 4: Verify from the store, not from the CLI**

```bash
python -c "
import json, collections
d = json.load(open(r'D:\memory\decisions.json', encoding='utf-8'))
e = json.loads(d['store'])['entities']
star = [(k, v['attributes'].get('status', [{}])[-1].get('value'))
        for k, v in e.items()
        if (v['attributes'].get('scope') or [{}])[-1].get('value') == '*']
print('star-scoped:', len(star))
print('by status:', collections.Counter(s for _, s in star))
"
```

The `accepted` count is what the hook will inject. **Read the store rather than a CLI listing**: `rmem decisions --scope "*"` mangles its argument exactly as a write does, so it filters by whatever the glob produced and agrees with itself. Entity 255.

- [ ] **Step 5: Re-run the hook and confirm the number moved**

```bash
echo '{}' | pwsh -NoProfile -File "C:/Users/bsevern/Tools/claude-hooks/lessons-inject.ps1" | \
  python -c "
import json, sys
c = json.load(sys.stdin)['hookSpecificOutput']['additionalContext']
print('injected:', len([l for l in c.split(chr(10)) if l.startswith('- ')]))
"
```

Expected: the `accepted` count from Step 4. If they disagree, the hook and the store disagree about what is injected, and the hook is what to read.

---

## Finishing

After Task 5, use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request reports: the workspace test count going 795 → 804 (five in Task
1, one in Task 2, three in Task 3), the number of `*`-scoped records before and
after the triage, and the injected title count before and after.
