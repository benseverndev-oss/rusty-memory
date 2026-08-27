# A store that refuses to fabricate consensus

**Status:** proposed
**Date:** 2026-08-27

**Positioning:** `docs/positioning.md` — the sharing boundary, the last of the
six at which the store's principle has not been carried.

## The problem

The store answers *what is true*. In a shared store the question is often
*what does Jon think is true*, and Jon and Divya can hold different answers
without either being wrong or stale.

Today those two writes are indistinguishable from a contradiction. They land in
one slot and survivorship settles them — most-recent picks a winner, valid
interval splits them by time. Both are wrong: nothing was corrected and nothing
changed; two people simply disagree.

That is the third case of a shape this project has handled twice already:

| disagreement across | what the store does |
|---|---|
| **time** | keeps both, resolves at read |
| **identity** | files a question, merges nothing |
| **holders** | **settles it by arrival order** |

`Provenance` records who *wrote* an assertion — `source_ref`, the session and
host, from the authorship work. It does not record whose *view* it is. A fact
Ben typed on Jon's behalf and a fact Ben believes himself are identical in the
store.

## The design

An observation gains an optional holder, and it is an **entity id**.

```rust
pub struct Observation {
    // ... unchanged ...
    /// Whose view this is, when it is a view rather than a fact.
    ///
    /// An entity, not a label: a holder is somebody the store already knows,
    /// so `about(holder, "role")` works on them like anyone else and two
    /// spellings of one person cannot become two holders. `None` means the
    /// assertion is the store's own, which is what every assertion written
    /// before this field existed is.
    pub according_to: Option<StableId>,
}
```

Reads take it the same way:

```rust
engine.about(entity, attribute, valid_t, tx_t)                  // holder-less
engine.about_according_to(entity, attribute, holder, valid_t, tx_t)
```

**A holder-less read returns only holder-less assertions.** This is the whole
compatibility story and it is worth stating plainly: nothing written before
this feature changes meaning, the 327 entities in the live store keep answering
exactly as they do now, and a caller who never passes a holder never sees one.
Views and facts do not mix, in either direction.

### Survivorship runs per holder

A holder's assertions form their own slot. Jon correcting himself is a
correction; Jon and Divya differing is not. Concretely: `(entity, attribute,
according_to)` is the survivorship key where `(entity, attribute)` is today.

That single change is most of the feature. Everything else is plumbing.

### Disagreement is not a review

The resolver files a question when it cannot tell whether two mentions are one
thing, because that is a fact about the world with one right answer nobody
knows. Two people disagreeing has no right answer and nothing to settle. It is
recorded, and a caller who wants to see it asks for it.

`Engine::holders_of(entity, attribute) -> Vec<StableId>` is how they ask. That
is deliberately a separate call rather than a fourth `Believed` variant: adding
`Contested` would change what every existing read can return, and the
compatibility promise above is worth more than the convenience.

## What this buys, concretely

- **"Whose priority is that?"** A team's goals are held, not true. `note "cut
  the circ backlog" --according-to <Divya>` records a drive with an owner, and
  bi-temporality already handles it changing.
- **A shared store stops laundering opinions into facts.** Today an agent
  reading a colleague's stated view and writing it back cannot mark it as
  theirs; tomorrow's reader sees a fact.
- **Disagreement becomes visible rather than resolved.** Two engineers holding
  different views of an interface is information, and the store currently
  destroys it on write.

## What it does not do

**It does not decide who is right.** No trust weighting, no precedence between
holders, no seniority. A store that ranked holders would be fabricating
consensus with extra steps.

**It does not infer a holder.** An assertion with no `according_to` stays the
store's own. Guessing that a fact "belongs to" whoever wrote it would make
every historical assertion retroactively somebody's opinion.

**It does not add a `Contested` answer.** See above.

**It does not do per-holder access control.** Holders say whose view a fact is,
not who may read it. Anyone with the store has all of it, exactly as now.

## Risks

**Two write paths that look alike.** `note X role Y` and `note X role Y
--according-to Z` differ by one flag and produce records that never meet. An
agent that forgets the flag silently promotes an opinion to a fact. The CLI and
tool descriptions have to make the flag's absence mean something, which is the
same problem `--absent` has and the same solution: say what the default asserts.

**Holder resolution.** A holder is an entity, so naming one by string means
resolving it — with the same review band, and now on the write path for views.
The first version should take an entity id only, and let the host resolve names
to ids before calling. That keeps a resolution failure out of the middle of a
write.

**It is the largest change to the core model so far.** The survivorship key is
the load-bearing part, and `rm-survivor` is 0.1 and additive-only. Adding a
third component to the key is not additive. This wants the conformance suite
and the resolution corpus green before and after, and probably a `0.2`.
