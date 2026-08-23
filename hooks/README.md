# rmem as a hook

An agent that *chooses* to remember forgets. `decide` and `remember` are tools
the model may or may not reach for, and the turns it does not reach for are
gone. A hook is not a tool: it fires on the event, and nothing in the model's
reasoning can skip it.

`rmem-hook.sh` wires `UserPromptSubmit` to both directions of the store.

| mode | when | what it does | cost per prompt |
|---|---|---|---|
| `recall` | synchronously, before the model sees the prompt | injects what the store already holds near the prompt as `additionalContext` | one embedding, ~0.6s |
| `spool` | synchronously, then detaches | queues the prompt and forks a drainer | one `flock` and an `echo`, ~20ms |
| `drain` | detached, one at a time | `rmem remember` on each queued turn | one completion + embeddings, ~5s |

## What this makes automatic, and what it does not

It makes **capture** automatic: every prompt you type above
`RMEM_HOOK_MIN_CHARS` reaches the store whether or not the agent thought to
record it, and every prompt is answered against the store before the model
reads it.

It does **not** make `decide` automatic. A decision is something the two of you
reach in conversation, and no hook on your prompt can see it — the hook reads
what you typed, not what was concluded. `rmem decide` stays a tool the agent
calls. What the hook changes is that the decision is *found again*: `recall`
runs on every prompt, so a question that comes near a recorded decision surfaces
it without anyone remembering to look.

## Install

The hook is not wired by default and the repo does not ship a
`.claude/settings.json` that turns it on. It spends money on every prompt, and
that is not a choice to make on someone else's behalf by their cloning a repo.

Copy `settings.example.json` into `.claude/settings.local.json` (gitignored),
fill in the two absolute paths, and open `/hooks` once so the settings are
reloaded:

```sh
mkdir -p .claude
sed -e "s#/absolute/path/to/your/.rmem#$PWD/.rmem#" \
    -e "s#/absolute/path/to/rmem#$(command -v rmem)#" \
    hooks/settings.example.json > .claude/settings.local.json
```

The `.rmem` directory is an ordinary rmem store — `mkdir .rmem && cd .rmem &&
rmem init`. Point `RMEM_HOOK_DIR` at the same directory your MCP server uses
and the hook and the agent share one memory.

## Configuration

Every one of these is read from the environment, so the `env` block in
`settings.local.json` is the place to set them.

| variable | default | |
|---|---|---|
| `RMEM_HOOK_DIR` | `$CLAUDE_PROJECT_DIR/.rmem` | directory holding `rmem.toml` and the store |
| `RMEM_BIN` | `rmem` | path to the binary |
| `RMEM_HOOK_SPEAKER` | `user` | `--speaker` for remembered turns |
| `RMEM_HOOK_MIN_CHARS` | `24` | shorter prompts are skipped entirely |
| `RMEM_HOOK_K` | `5` | how many hits `recall` injects |
| `RMEM_HOOK_OFF` | unset | `1` makes every mode a no-op |

If `RMEM_HOOK_DIR` has no `rmem.toml`, or `jq` is missing, or the binary is not
found, every mode exits 0 having done nothing. A machine without rmem set up
should not see errors at its prompt.

## The two things worth knowing before you turn it on

**It costs a completion per prompt.** `remember` is an extraction call plus
embeddings. At `MIN_CHARS = 24` a working session is a few hundred of them.
Raise the threshold if that is not worth it.

**Turns can still fail to extract, and the log is where you find out.**
`$RMEM_HOOK_DIR/hook.log` holds every `remember`'s own output, including the
`not remembered from this turn:` lines. A hook that captured the turn and an
extraction that dropped it are different failures, and only the log separates
them.

## Why a queue and not just `rmem remember`

`remember` holds the store's exclusive lock across its extraction and its
embeddings — seconds, because the model is across a network. The lock waits 5s
and then refuses. Two prompts close together, or one arriving while the MCP
server is mid-write, would lose a turn outright.

So the part that must not fail is separated from the part that is slow. The
spool append is a `flock` and an `echo`; it cannot lose a turn to contention.
The drainer is serialised against itself by a second lock taken *non-blocking*:
a second drainer exits rather than queues, because the one already holding the
lock will reach the line it would have taken.
