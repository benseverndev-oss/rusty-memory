# A store path belongs to its config, not to whoever ran the command

**Status:** proposed
**Date:** 2026-08-26

## The problem

`rmem init` writes:

```toml
[store]
path = "memory.json"
```

That path is resolved against the **caller's working directory**, not against
the config file that contains it. So the same config points at a different
store depending on where the process happened to start.

This contradicts what the README says the config is for:

> `RMEM_CONFIG` is what makes it one store rather than one per project: without
> it each session reads `./rmem.toml` from its own directory, and eventually
> one of them points somewhere else — a divergence nothing reports, because two
> stores are not an error.

`RMEM_CONFIG` exists so a config can be shared. A shared config with a relative
store path is not shared at all: it names a different file for every caller.

The live store escapes this only because `D:/memory/rmem.toml` happens to
contain an absolute path. Anything written by `rmem init` does not.

## How it failed

On 2026-08-26 this silently invalidated an experiment.

Two scratch stores were created to compare blocking strategies — same corpus,
one config difference — with `RMEM_CONFIG` pointed at each in turn. Both
configs said `path = "memory.json"`, and both runs executed from the repository
root, so **both wrote to the same file**. The second run found everything the
first had written and reported 37 merges and zero questions.

The tell was not an error. It was implausibly clean data: every freshly created
entity reported as "already knew". Had the comparison been between two
configurations whose real results were merely similar, the run would have
looked like a legitimate null result and been reported as one.

It also left `memory.json`, `memory.json.lock` and `memory.vec` sitting
untracked in the repository root, which is the second symptom and the one a
person actually notices.

## The change

Resolve a relative `[store] path` against the directory containing the config
file. An absolute path is used as-is.

`Config::parse(path, &text)` already receives the config's own path, so this
has one natural home and every consumer inherits it — `rm-cli`, `rm-mcp`, and
anything later. No caller changes.

```rust
// A relative store path is relative to the config that names it, not to
// whoever happened to start the process. `RMEM_CONFIG` exists so one config
// can be shared between projects; a path resolved against the caller's cwd
// makes that config name a different file for every caller, which is the
// opposite of what sharing it was for.
let path = if raw.is_absolute() {
    raw
} else {
    config_path.parent().unwrap_or(Path::new(".")).join(raw)
};
```

## What breaks

Any existing config with a relative store path that was being run from a
directory other than the config's own. In that case the store moves.

This is worth doing anyway, and the reasoning is that such a setup is already
broken — it silently resolves to a different store per caller, which is the bug
being fixed. But it must not move data without saying so:

- The change ships with a note in the README under "Where a store lives".
- Where the new path does not exist and the old one does, the error names both
  and says the resolution rule changed. Silence here would look exactly like
  the empty-store case, which is the failure this whole spec is about.

`rmem init` should also write an **absolute** path, so a freshly written config
is unambiguous regardless of this rule. The rule is what makes hand-written and
older configs behave sensibly; the absolute path is what stops the question
arising.

## What it does not do

**It does not change `RMEM_CONFIG` lookup.** How the config file is found is a
separate mechanism and works.

**It does not add store discovery.** No searching upwards for a `rmem.toml`, no
XDG default location. One config, one store, named explicitly.

## Testing

- A config in directory `A` naming `memory.json`, loaded from directory `B`,
  resolves to `A/memory.json`. This is the bug, stated directly.
- An absolute path is unchanged by the rule.
- Two configs in different directories, both naming `memory.json`, resolve to
  two different files — the property the failed experiment needed and did not
  have.
- `rmem init` writes a path that is absolute.
- The not-found error names both the resolved path and the config that named
  it. A path in an error message is only useful if you can tell which config
  produced it.

## Risks

**Silent data relocation** is the whole risk, and it is why the not-found error
matters more than usual. A user whose store appears empty after an upgrade must
be told the resolution rule changed, or they will conclude their data is gone.
The empty store and the moved store look identical otherwise.
