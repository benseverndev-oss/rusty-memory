# rm-extract — design

Status: approved design, pre-implementation.

Turn → mentions, facts, relations. The crate that needs a language model, and
the one that decides whether arrival implies departure.

## What this crate is for

Everything below it works and has never seen a real conversation. `rm-engine`
and `rm-graph` have only ever been given hand-built `Observation`s and
hand-written `relate` calls. `rm-extract` is what proves those shapes survive
contact with a turn of dialogue — and it is the last piece before an MCP server
or a CLI would bake them into a protocol.

It is also where a question the rest of the design deliberately deferred comes
due. `rm-store` refuses to close an edge when a new one arrives, on the grounds
that arrival does not entail departure and the inference belongs to whoever
heard the sentence. That is this crate.

## No crate touches the network

`rm-extract` defines a port and the host implements it:

```rust
pub trait Completer {
    fn complete(&self, prompt: &str) -> Result<String, CompleterError>;
}
```

The architecture sketch called `rm-extract` "the only crate that touches the
network". A trait is the stronger version of that idea: *no* crate does. The
binary that wires an engine to a provider does, and the whole workspace stays
compilable, testable and auditable with zero third-party dependencies — which it
has managed for seven crates and should not surrender in the eighth, least of
all in the one holding the hardest domain logic.

A stub `Completer` returning a canned string is three lines, so every test in
this crate runs offline and deterministically.

The rejected alternative was an HTTP client behind a feature flag. It buys
convenience for a caller who could write the same twenty lines once, and pays
for it with TLS, an async runtime, and a dependency graph that changes shape
depending on which features are on.

## The crate owns both prompt and schema

`rm-extract` builds the prompt and parses the response against its own schema,
and exposes the prompt publicly:

```rust
pub fn prompt(turn: &Turn) -> String;
pub fn extract(turn: &Turn, completer: &impl Completer) -> Result<Extraction, ExtractError>;
```

Splitting them — the host writing a prompt against a schema the crate owns — is
the same mistake `rm-resolve` avoided by exposing `BlockingKey::keys_for`. Two
copies of a contract drift, and the drift here is silent: a prompt that has
fallen behind its schema produces a thin extraction, not an error. Nothing in
the output says "the model was asked the wrong question".

`prompt` is public rather than private so a host can read it, log it, diff it
across versions, or build a few-shot variant on top. Owning the contract does
not require hiding it.

Two-phase extraction — mentions first, relations second — was considered and
deferred. It doubles latency and cost against a reliability problem for which
there is no evidence yet, and it is purely additive later: neither the trait nor
the output type changes if a second call is added inside `extract`.

## The types

```rust
pub struct Turn {
    pub text: String,
    /// The speaker's name, so first-person references resolve to a mention.
    /// `None` when the turn has no identified speaker.
    pub speaker: Option<String>,
    pub observed_at: Timestamp,
    pub session: String,
}

/// Something the turn referred to. Its position in `Extraction::mentions` is
/// its local index, and every other part of an extraction refers to it by that.
pub struct Mention {
    pub kind: String,
    /// The identifying field, used for entity resolution.
    pub name: String,
    /// What to embed — the phrasing the turn actually used.
    pub text: String,
}

/// An attribute assertion about one mention.
pub struct Fact {
    pub subject: usize,
    pub attribute: String,
    /// `None` asserts the attribute has no value — a tombstone.
    pub value: Option<String>,
    /// What to embed for *this fact* — not the mention's text.
    ///
    /// A fact and the thing it is about are different search targets. If every
    /// fact about Ben shared Ben's embedding, "where does he work" could only
    /// find the employer by first finding Ben; the assertion itself would be
    /// unreachable. Each fact carries the phrasing that states it.
    pub text: String,
    pub valid_from: Timestamp,
}

/// A relationship between two mentions.
pub struct Relation {
    pub subject: usize,
    pub predicate: String,
    pub object: usize,
    pub valid_from: Timestamp,
}

pub struct Extraction {
    pub mentions: Vec<Mention>,
    pub facts: Vec<Fact>,
    pub relations: Vec<Relation>,
    pub closures: Vec<Closure>,
}
```

Local indices rather than `StableId`s because the ids do not exist yet:
resolution happens inside `remember`, which has not run when extraction returns.
An extraction is a description of a turn, not a set of store operations.

## Closures — and the shape they turned out to need

The design assumption going in was that a closure names the edge it ends. It
cannot. "I started at Globex" does not mention Acme at all, and `rm-extract` has
never seen the store, so the departed employer is neither in the turn nor
reachable from it.

A closure is therefore a statement about a *predicate*:

```rust
pub struct Closure {
    pub subject: usize,
    pub predicate: String,
    pub at: Timestamp,
    /// The model's stated reason, kept for the caller to log.
    pub because: String,
}
```

It means: as of `at`, this subject's `predicate` edges — other than the ones
asserted in this same extraction — have ended. Only `ingest` can resolve that,
because only `ingest` can see what the store already holds.

### The inference is recorded as an inference

Every tombstone a closure produces carries `Source::AgentInference`, which
`rm-core` already documents as the weakest source: "inferences are derived from
the others and re-deriving one does not make it more true."

That is the whole reason this is safe to do. The closure is traceable in
`edge_history`, it is distinguishable from anything the user said, and
`Strategy::SourcePriority` can rank it below a user assertion so a later
correction from the horse's mouth wins without special handling. The alternative
readings both fail: emitting nothing leaves the store permanently showing two
employers with no mechanism that will ever fix it, and emitting a closure
provenanced as a user assertion launders a guess into testimony.

## Refusals over salvage

`ExtractError` covers a response that is not JSON, a mention with no name, a
`Fact`/`Relation`/`Closure` naming an index outside `mentions`, and a relation
whose subject and object are the same mention — which `rm-store::relate` refuses
anyway, so accepting it here only moves the error later.

Each names what was wrong. A partial extraction is a turn silently
half-remembered, and nothing downstream can tell it apart from a turn that
genuinely said less.

No retry logic. The host owns the `Completer`, so retries, backoff and provider
failover are its business and it is better placed to do them.

## `Engine::ingest`

`rm-engine` gains a dependency on `rm-extract` for the `Extraction` type. The
arrow points that way and not back: `rm-extract` never learns what an `Engine`
is, so its tests need no store and no mocking.

```rust
pub trait Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError>;
}

pub struct Ingested {
    /// Local mention index → the entity it resolved to.
    pub entities: Vec<StableId>,
    pub assertions: Vec<AssertionId>,
    /// Open questions raised while resolving the mentions.
    pub reviews: Vec<ReviewId>,
    /// Edges closed by inference, with the model's stated reason.
    pub closed: Vec<Closed>,
}

/// One edge a closure ended.
///
/// A named struct rather than a tuple: `(StableId, String, StableId, String)`
/// has two same-typed ids and two same-typed strings, so every reader has to
/// go and check which is which.
pub struct Closed {
    pub subject: StableId,
    pub predicate: String,
    pub object: StableId,
    /// The model's stated reason, from the closure that ended this edge.
    pub because: String,
}

pub fn ingest(&mut self, extraction: &Extraction, embedder: &impl Embedder)
    -> Result<Ingested, EngineError>;
```

`Embedder` is a second narrow port, symmetric with `Completer` and for the same
reason. It lives in `rm-engine` because `ingest` is what needs it; a test
implementation is three lines, so the whole pipeline stays offline.

The rejected alternative was passing a parallel slice of vectors the caller
produced. The correspondence between mentions and vectors would then be
positional and unchecked, and a caller who filtered or reordered one and not the
other would write confidently wrong memories with nothing raising an error.

### Order of operations

1. **Embed every mention, and validate every vector**, before anything is
   written. This is the discipline `remember` already follows: a rejected vector
   must cost nothing, because a fact in the store with no vector to find it is
   undetectable from outside.
2. **Remember each mention**, asserting its `kind`, and record the resulting
   entity against its local index.
3. **Remember each fact**, reusing its subject's mention so resolution lands it
   on the same entity.
4. **Relate**, mapping both endpoints through the local-index table.
5. **Resolve closures** against the store.

### Every mention becomes an entity, even one with no facts

"Bristol" may appear only as the object of an edge, and an edge cannot name an
entity that does not exist. So step 2 asserts each mention's `kind` as an
attribute.

The `kind` assertion embeds the mention's own `text`, which is what makes the
entity findable by the phrasing the turn used for it. Facts about it embed their
own text instead, per `Fact::text` above.

That is not a workaround. The kind is a genuine fact about the thing, it gives
the entity something to be recalled by, and it guarantees no entity exists
without an assertion — which keeps `Engine::open`'s every-assertion-has-a-vector
invariant simple rather than needing an exception for attribute-less entities.

### Resolving a closure

For each closure: take `edges_from(subject, at, Timestamp::MAX)` filtered to
the predicate, and `unrelate` every edge whose object is **not** among this
extraction's own relations for that subject and predicate.

`Timestamp::MAX` on the transaction axis, deliberately: the question is which
edges the store holds *right now*, not what it believed at some earlier point.
The engine takes no clock, so a literal "now" is not available to it, and
reusing `at` — a valid-time value — would silently hide any edge learned after
the moment the closure speaks about, leaving it open forever.

So "I started at Globex" ends Acme and leaves Globex alone, and a turn that
re-states an existing relationship closes nothing.

### A known limitation, stated rather than smuggled

`Provenance` has no field for a rationale, so a closure's `because` is returned
in `Ingested` for the caller to log and does not land in `edge_history`. Adding
a rationale field would touch `rm-core` and every crate that constructs a
`Provenance`; encoding it into `source_ref` would abuse a field documented as
naming the session or document. Neither is worth it for a first version, and the
gap is visible here rather than discovered later.

## Errors

`ExtractError` is `rm-extract`'s own. `ingest` returns `EngineError`, gaining
one variant for an embedder failure, wrapping the host's message the way
`EngineError` already wraps its siblings' explanations rather than replacing
them.

## Testing

**`rm-extract`**, all against a stub `Completer`:
- `a_turn_naming_two_people_and_a_relationship_extracts_all_three`
- `a_response_that_is_not_json_is_refused`
- `a_relation_naming_a_mention_that_does_not_exist_is_refused`
- `a_mention_with_no_name_is_refused`
- `a_relation_from_a_mention_to_itself_is_refused`
- `the_prompt_names_the_speaker_so_first_person_resolves`
- `an_extraction_with_nothing_in_it_is_not_an_error` — a turn may say nothing
  worth remembering

**`Engine::ingest`**:
- `two_people_and_an_edge_arrive_from_one_turn`
- `a_mention_with_no_facts_still_becomes_an_entity`
- `a_closure_ends_the_prior_edge_and_says_an_agent_inferred_it`
- `a_closure_does_not_end_an_edge_asserted_in_the_same_extraction`
- `a_rejected_embedding_leaves_the_store_and_the_index_untouched`
- `an_ambiguous_mention_comes_back_as_a_review_rather_than_a_merge`
- `the_readme_story_arrives_from_a_turn_instead_of_by_hand`

## Out of scope

- **Retries, backoff, provider failover.** The host owns the `Completer`.
- **Batching turns, streaming, conversation history.** One turn per call.
- **Any provider-specific code, anywhere in the workspace.**
- **Two-phase extraction.** Additive later without changing the trait or the
  output type.
- **A rationale field on `Provenance`.** See the limitation above.
