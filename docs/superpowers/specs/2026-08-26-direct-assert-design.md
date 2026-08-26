# A fact you already know

`rmem note` — record a fact without asking a model to find one.

## The door that was never built

Every fact in this store today would have to arrive through `remember`, and
`plan_remember` takes a `Completer`. So a fact costs a completion call, and the
cheapest way to record something you already know is to write prose about it
and pay a model to read the prose back.

`decide` makes the opposite bargain and says so: *"a decision has a known shape,
so it costs embeddings and no completion at all — and where the embedder is
local, no credential and no socket either."*

**This is that bargain applied to a fact**, and the absence of it is the best
explanation for what the store actually contains:

```
entities: 265      identities: 265
edges:    0        review queue: 0    rejected pairs: 0
attribute names in use:  because, choice, context, scope, status
```

Five attribute names, all of them decision fields. No edges. A review queue
that has never held anything, because the resolver has never been asked to
judge. `rm-resolve`, `rm-extract` and `rm-graph` have never touched real data —
not for want of a use case, but because the only entrance charges a completion
model per fact.

## What it does

```sh
# an attribute of the person
rmem note "Jon Severn" role "leads the circ team"

# the same attribute, later, from when it became true
rmem note "Jon Severn" role "leads circ + print" --valid-from 2026-06-01

# an asserted absence, which is not the same as never having been asked
rmem note "Jon Severn" reports --absent

# --field adds to the MENTION, not the attributes: it is what the resolver
# compares and what the identity record keeps, so this is how a person gets
# an identifier stronger than their name
rmem note "Jon Severn" role "leads circ" --field email=jsevern@mjhassoc.com
```

The last one is worth reading twice. `role` is a fact *about* Jon and lands in
his attributes; `--field email=...` is part of *who Jon is* and lands in the
identity record the resolver scores against. They are different halves and the
command takes both because a real import has both.

One embedding, no completion, no key when the embedder is local. It builds an
`Observation` by hand and hands it to `Engine::remember` — the resolving path,
not `remember_as`.

**Resolving is the point.** `remember_as` takes an entity the caller already
identified, which is what `decide` uses and why the resolver has never run.
`note` names a person and lets the ruleset decide whether that is someone
already known. That is the whole reason this wakes anything.

## Three decisions worth stating

### It resolves by name, and may create, merge, or ask

`Engine::remember` answers three ways, and the third is the one this exists to
reach:

- `Merged` — the name matched something known. The fact joins that entity.
- `Created` — nothing matched. A new entity.
- `CreatedPendingReview` — it scored inside the review band. **The fact is
  recorded and the identity question is queued**, which is the engine's own
  position: *"The fact is remembered either way; what is uncertain is only whose
  it is."*

`note` exits 0 on all three and reports the third rather than swallowing it:

```
recorded as entity 271

open question: this scored 6.1 against entity 88 "Jon Severn", inside the
review band (5.24 to 7.24). Both are kept. `rmem review` lists it;
`rmem review --confirm <id>` settles it.
```

Refusing was considered and turned down: it contradicts the engine, which
deliberately keeps the fact, and it would stop a bulk import dead on its first
near-miss. Succeeding quietly was turned down too — an open question nobody is
told about is one nobody settles, and the queue would fill with things no one
looks at.

### `--absent` is not a convenience

`Version.value` is an `Option<String>` and the store's three-way answer —
`Value`, `Absent`, `Unknown` — is the distinction its own MCP instructions open
with: *"absent means someone asserted there is no value. unknown means it has
never been discussed. Treating the second as the first will make you state as
fact that someone has no employer when nobody has ever mentioned their job."*

`decide` cannot express an absence. Without `--absent` neither can `note`, and
the distinction the store is proudest of stays unreachable from every write
path it has.

### `--scope` is optional, and that is not laxity

`decide` refuses without one because a decision's reach genuinely varies and
`RMEM_SCOPE` cannot supply it. A fact about a person is different: an entity
with no `scope` attribute already reaches every position, by the applicability
rule as built. So omitting it is not an unset field — it is the correct answer
for most facts, and "Jon leads the circ team" is true whichever project you are
standing in.

`--scope` stays available for a fact that really is project-bound. Requiring it
would put `*` on nearly every record, and a field that is nearly always the same
value stops being answered and starts being typed.

### `--field` exists because identity is written once

The mention is what the resolver compares and what gets stored as the identity
record. The shipped ruleset compares `name` and `kind` only, so a bare
`note` would write name-only identities — permanently, for every person.

An email address is the strongest identifier a person has. Recording it in the
mention costs nothing today, because the shipped ruleset ignores fields it does
not name, and it means a later ruleset change can use it **without rewriting
every record**. The alternative is discovering in a month that the identities
cannot be improved without a migration.

## What this deliberately does not do

- **It does not wake `rm-extract`.** Extraction harvests facts from prose; this
  receives facts someone decided to record. The store's quality today rests on
  every record being deliberate, and harvested facts have a different bar and a
  failure mode nobody would notice. Extraction can wait for a reason of its own.
- **It does not create edges.** `relate()` exists and the graph is still at
  zero; a fact and a relationship are different verbs and this is the fact one.
- **It changes no existing behaviour.** New command, new tool. `decide`,
  `remember` and every read are untouched.
- **It does not change the ruleset.** If the first real dataset wants email
  compared, that is a `rmem.toml` edit and a separate argument — the thresholds
  currently shipped were calibrated on generated names, and changing them
  deserves its own evidence.

## What it costs to be wrong

**The resolver has never judged anything.** Its thresholds — `review_at 5.2439`,
`match_at 7.2439` — were calibrated on generated names, and this is the first
time they meet real ones. If they merge two people who are not the same person,
that is silent and permanent, which is exactly why `CreatedPendingReview`
exists and why this reports it.

The honest expectation is that the first real dataset produces review-band
pairs, and that is the design working rather than failing. A run that produces
none would be worth suspicion rather than relief: on 483 lines of people with
name variants, nothing ambiguous at all suggests the blocking key never brought
candidates together to be compared.

## Testing

- **The three outcomes**, each reached deliberately: a fresh name creates, the
  same name again merges onto the same entity, and a name engineered to score
  inside the band queues a review while still recording the fact.
- **`--absent` writes a tombstone**, asserted by reading the stored `Version`
  and checking `value` is `None` — distinct from an attribute never written,
  which `about` answers as `Unknown`.
- **No completer is reachable.** The strongest available statement: `note`'s
  plan function takes no `Completer` parameter at all, so the type system says
  it rather than a test asserting a negative.
- **Scope absence reaches everywhere** — a note written without `--scope` is
  visible from a position it was not written at.
- **Mention fields survive**, read back off the stored identity record rather
  than through any CLI listing, per entity 255.
