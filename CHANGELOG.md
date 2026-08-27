# Changelog

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
