# Changelog

## Unreleased

### Reading documents

`rmem ingest <dir>` reads every `.md` under a directory, splitting each on its
own headings. A section is identified by path, heading path and a hash of its
text, so a re-run reads only what changed. Sections that yielded nothing are
still recorded as read: a store that remembers only what it wrote forgets it
ever looked, which was measured at 21 sections in 30.

`--dry-run` reports what a run would cost without calling anything. A deleted
file writes nothing -- a section that disappeared is not an assertion that its
subject has no value.

Ingest writes to a scratch store only, refusing any store that holds a fact
which did not come from a document. That stands until an extractor can decline
a reading it is unsure of.

### Extraction no longer returns its own example

`extract` drops a mention whose name comes from the prompt's worked example
when the turn does not contain that name, and reports the drop in `dropped`
like any other. A model given text with nothing extractable in it answers with
the example it was shown rather than with nothing: measured at 16 of 213 facts
across arrow's API reference, all from sections as short as `Null type`.

A turn that really does name Alex Chen or Globex is unaffected -- the guard is
on the name being absent from the turn, never on the name itself.

### A code fence is not a heading

The chunker split on any line starting with `#`, which is a heading in prose
and an attribute in Rust. `#[derive(Debug)]` became a heading and detached the
rest of an item's documentation from the item it described. Fenced blocks are
now passed over: 206 lines in this repository's own `docs/` were affected, and
6% of documented items in `arrow-schema`.

The size of it is easier to see in the chunk count. `docs/` split into 705
chunks before and 534 after, so **a quarter of what a run would have paid for
was a spurious split on a line of code** -- and each one handed the model a
fragment attached to the wrong subject.

### Smaller

- `rmem ingest --dry-run <dir>` works; the directory no longer has to be the
  first argument.
- A run survives one unusable response instead of discarding every completion
  it had already paid for. Failed sections are named, counted separately from
  read ones, and not marked read, so the next run retries exactly them.
  Tolerance stops after five consecutive failures, which is a broken setup
  rather than an unlucky one.

## 0.2.0

**Breaking.** Three public signatures changed, so anything built against 0.1.0
needs an edit. Each is one argument or one field:

- `Observation` gains `according_to: Option<StableId>`. A struct literal needs
  it; `None` is what every 0.1.0 observation meant.
- `MemoryStore::assert` takes the holder as a final argument. `None` again.
- `commit_recall` takes a `Depth` as a final argument. `Depth::Stated` is what
  0.1.0 did.

Nothing on disk changed. A store written by 0.1.0 reads unchanged, and a
holder-less version still serialises byte for byte as it did — the new field is
written only when it says something.

### Whose view a fact is

An assertion can name a holder, and survivorship partitions a slot by it, so
one person correcting themselves is a correction and two people differing is
not. A holder-less read sees only holder-less assertions and a holder's read
sees only theirs, in both directions: a holder who has said nothing reads
`Unknown` rather than falling back to the fact.

`Engine::about_according_to` asks one person's view. `Engine::holders_of` names
everyone with one — deliberately a call rather than a fourth `Believed`
variant, which would have changed what every existing read can return.

### Recall depths

`Engine::recall_located` returns what was found without the assertion text;
`Engine::recall_traced` adds provenance and the versions a hit stands against.
`Engine::recall` is unchanged and remains the default.

Nothing is summarised at any level — a deeper one is a superset of a shallower
one, byte for byte, and no level calls a model. Measured over twenty hits:
`located` is 63% cheaper than `stated`, and `traced` is roughly 8x more
expensive, which makes it a tool for one answer rather than a result set. See
`docs/tiering-cost.md`.

## 0.1.0

First release. Bi-temporal store, three-state answers, entity resolution with a
review band, survivorship at read time, and a vector index.
