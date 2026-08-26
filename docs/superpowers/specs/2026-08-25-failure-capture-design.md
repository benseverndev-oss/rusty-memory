# Capture at the moment, record on judgement

> **Superseded before implementation, the same day. Not built.** The evidence
> below is kept because the reasoning was sound and the conclusion was wrong,
> which is worth more than a deleted file. See *What killed it* at the end.

A hook that preserves what was tried and what happened when a tool fails, so a
lesson can still be written afterwards — and a nudge, at the end of the turn,
when the same failure has happened more than once.

## Why this spec lives in this repo

The implementation lands in `~/.claude/settings.json` and
`~/Tools/claude-hooks/`, neither of which is version controlled. This repo
builds `rmem`, which is what a captured lesson eventually becomes a record in,
and `docs/superpowers/specs/` is the only versioned home available. The spec is
here; the code will not be.

## The problem, from one day's evidence

Two distinct failures, on the same day, that look the same and are not.

**Nothing captured the intermediate state.** A peer session running its own
store made a decision, shipped it, found it wanting within hours, and made a
different one. It recorded a single record holding the final state, because it
wrote up at the end of the day. The first choice — and the reason it was wrong,
which was the useful half — never reached the store.

**And nothing surfaced a record that already existed.** In this repo, with no
`rmem` server configured, entity 61 was unreachable: *"rust: verify each crate's
build exit code explicitly; piped tails mask failures."* An `awk` counter that
could not report a failure was written anyway, and cost most of a session's
confidence in its own verification.

The second is now fixed — the MCP entry exists. This spec is the first.

## Capture is mechanical. Recording is judged.

The hook's job is **not** to decide what a lesson is. It cannot: a
`PostToolUseFailure` hook supports only the `command` type — `prompt` and
`agent` are limited to `PreToolUse`, `PostToolUse` and `PermissionRequest` — so
it has no model and no context.

Its job is to make sure the raw material still exists when someone can judge.

## The signal is repetition

"Was that worth learning from?" is unanswerable in a shell script. **"Have I hit
this exact failure before?"** is arithmetic, and on the day's evidence it
separates the two classes cleanly:

| failure | times | worth recording |
|---|---|---|
| a quoted heredoc eats a backslash level | 4 | yes |
| `rg -r` is ripgrep's replace flag | 2 | yes |
| `cargo fmt` collapses `\` continuations in a literal | 2 | yes |
| a test failing red on purpose | many | no |
| a compile error fixed in ten seconds | many | no |

The first three are now entities 221, 224 and 222. The last two are noise, and
no rule involving judgement was needed to tell them apart — only counting.

## Three pieces

### 1. The capture hook

A `PostToolUseFailure` command hook appends **one line** to
`~/.claude/lessons/failures.jsonl`:

```json
{"at":"2026-08-25T23:41:07Z","session":"b149f85e","project":"D:/show_case/rusty-memory",
 "tool":"Bash","key":"assertionerror-block-not-found","error":"AssertionError: table_hint body not found"}
```

`error` is the raw first line, kept because it is what a human reads. `key` is
the normalised form, kept because it is what the counting groups on. Both,
because collapsing them would make the log either ungroupable or unreadable.

No model. No network. No judgement.

### 2. The normaliser

The only part with a right answer, and therefore the only part that gets tested.

`key` is derived from `(tool, error)` by lowercasing and removing what varies
between two occurrences of the same mistake: digits, absolute paths, temp
directory names, hex ids, and quoted string contents. Two occurrences of the
same mistake must produce the same key; two different mistakes must not.

Getting this wrong fails in both directions and neither is silent-safe:
over-normalising merges unrelated failures and manufactures repeats that were
never repeated; under-normalising means the same mistake never groups and the
nudge never fires. So it is tested against recorded pairs from this session —
the four heredoc failures must group, and the heredoc failures must not group
with the ripgrep one.

### 3. The nudge

A `Stop` hook counts this session's entries by `key` and, where any count is 2
or more, emits a `systemMessage`:

```
3x this session: Bash / heredoc backslash eaten before the interpreter
2x this session: Bash / rg -r consumed the next argument

Worth a `decide`? The mechanism is still in context; tomorrow only the
symptom will be.
```

**At the end of the turn, deliberately.** A lesson written a day later keeps the
symptom and loses the mechanism, which is most of its value — the difference
between "heredocs are fiddly" and "`<<'PY'` suppresses expansion but not the
backslash pass, so `\\n` arrives as a real newline."

## What it must never do

**Never write to `rmem` on its own.** A decision store full of
machine-generated entries stops being worth reading, and being worth reading is
the entire advantage it has over a directory of markdown. A peer session put it
exactly right: markdown is "a cheaper memory that cannot express a rejected
option" — a store that can, but is 90% noise, has thrown that away.

The session writes the entry, in the house style, or nobody does.

## Constraints inherited from what is already there

**One writer per file.** aiTrak registers its event buffer twice — once via
`pwsh.exe`, once via `powershell.exe` — on both `PostToolUse` and `Stop`. Both
fire. Its log holds 44 events with **one** duplicated signature, where double
registration should have produced roughly 44: two processes are racing to append
and writes are being lost. This hook registers once, and appends atomically.

**Never near aiTrak.** Its buffer syncs off-machine. Tool inputs contain file
contents, shell commands and paths. This log stays local, is written by nothing
that ships, and is not wired into any sync.

**Merge, do not replace.** `~/.claude/settings.json` already carries three
`PostToolUse` groups and two `Stop` groups. The existing arrays are appended to;
replacing them would silently disable the ruff-on-edit formatter and both
aiTrak registrations.

## Testing

The hook scripts are PowerShell and are not reachable from `cargo test`, so each
piece is exercised by piping a synthesised event into it and reading the result,
the way the `update-config` skill describes:

- **Capture** — pipe a `PostToolUseFailure` payload, assert exactly one line is
  appended and that it parses as JSON.
- **Normalisation** — the pairs above, asserted to group and not to group.
- **Nudge** — seed a log with a key at count 1 and at count 3; assert silence
  for the first and a `systemMessage` naming the second.
- **Merge** — after editing settings, `jq -e` the specific event and matcher
  back out, and confirm the pre-existing hooks are all still present. A
  malformed `settings.json` silently disables every setting in it, for every
  session, so this is checked rather than assumed.

## Out of scope

- **No auto-recording**, per above.
- **No cross-session aggregation.** The nudge counts this session only. Repeats
  across days are a real signal and a different feature; counting them needs a
  retention policy this does not have.
- **No capture of successes.** The interesting thing is what went wrong; a log
  of everything is aiTrak's job and it already exists.
- **No change to aiTrak.** Its duplicate registration and apparent lost writes
  are reported, not fixed. It is corporate tooling with a sync agent, and its
  owner should decide.

## What killed it

Two findings arrived while the first script was being written.

**It would have captured almost nothing.** Of six lessons worth recording that
day, exactly one produced a tool failure at all. The rest were silent wrong
answers: an `awk` counter returning a confident zero, `rg -rn` returning
plausible nonsense, `cargo fmt` quietly reformatting a string literal, `git add
-A` succeeding while sweeping in two embedded worktrees. A
`PostToolUseFailure` hook never sees any of those.

And the one that *did* fail produced four different messages -- `table_hint body
not found`, `session block not found`, and two more -- so the normaliser would
not have grouped them either, unless it were loosened to the point of merging
unrelated assertions. The test written to pin that grouping failed, and the test
was right.

**Capture was never the bottleneck.** A peer session had the relevant record,
had `decisions` and `decision` in its tool list all day, and hit the same
failure three times without ever looking. Two others hit it twice and once.
Four sessions, one day, one family of failure, and the lesson was already
written down and reachable by at least one of them.

So the gap was recall, not capture. **A tool you have to choose to call is not
memory.** An opt-in that fires on "I suspect there is a record" cannot fire on
the case where you suspect nothing, which is every case that matters.

## What was built instead

A `SessionStart` hook that injects every `*`-scoped lesson title as
`additionalContext`. Measured: 49 titles, ~1,092 tokens, **once per session** --
against ~1,130 tokens **per turn** for the three MCP tools it partly replaces.
Cheaper than a single turn of the alternative, and it works, because it is in
front of the session rather than behind a call it has to decide to make.

`~/Tools/claude-hooks/lessons-inject.ps1`, wired into `~/.claude/settings.json`.
It reads through `rmem decisions` rather than parsing the snapshot, needs no API
key (only `recall` and `decide` embed), and exits 0 silently on every failure
path -- a memory aid that can stop you working is worse than none.

The one piece of this spec that survived intact is the constraint list: one
writer per file, and nothing near aiTrak's buffer.
