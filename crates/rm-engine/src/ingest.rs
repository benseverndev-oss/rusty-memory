//! Applying an extraction to the store.
//!
//! `rm_extract` describes a turn and knows nothing about entities; this is
//! where a description becomes writes. The mapping from a mention's local index
//! to a `StableId` lives here because here is where the ids are born — inside
//! `remember`, which resolves the mention against everything already known.

use rm_core::{Interval, Provenance, Source, Timestamp};
use rm_extract::{Extraction, Turn};
use rm_store::StableId;

use crate::{AssertionId, Engine, EngineError, Observation, Record, Remembered, ReviewId};

/// Whatever went wrong producing an embedding. Opaque here: the host's
/// service, the host's error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedderError(pub String);

impl std::fmt::Display for EmbedderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the embedder failed: {}", self.0)
    }
}

impl std::error::Error for EmbedderError {}

/// A text embedding model, supplied by the host.
///
/// The counterpart of `rm_extract::Completer`, and a port for the same reason:
/// no crate in this workspace touches the network, so the one thing that needs
/// a remote service asks for it rather than reaching for it. A test
/// implementation is a few lines, which is what keeps the whole pipeline
/// testable offline.
pub trait Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError>;
}

/// One edge a closure ended.
///
/// A named struct rather than a tuple: `(StableId, String, StableId, String)`
/// has two same-typed ids and two same-typed strings, so every reader would
/// have to go and check which is which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Closed {
    pub subject: StableId,
    pub predicate: String,
    pub object: StableId,
    /// The reason the model gave, from the closure that ended this edge.
    pub because: String,
}

/// What one turn did to the store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ingested {
    /// Local mention index to the entity it resolved to. Same order and length
    /// as the extraction's `mentions`.
    pub entities: Vec<StableId>,
    pub assertions: Vec<AssertionId>,
    /// Open questions raised while resolving the mentions. A mention that
    /// scored in the review band created its own entity and filed a question
    /// rather than merging, exactly as `remember` does on its own.
    pub reviews: Vec<ReviewId>,
    /// Edges closed by inference, with the reason the model gave.
    pub closed: Vec<Closed>,
}

impl Engine {
    /// Apply an extracted turn.
    ///
    /// Every embedding -- for every mention and every fact -- is produced and
    /// validated before anything is written, so a failing embedder costs
    /// nothing. That is the guarantee `Engine::remember` already makes and for
    /// the same reason: a fact in the store with no vector to find it is
    /// undetectable from outside -- no query reports it and no error names it.
    /// Embedding a fact's text lazily inside its own write loop would keep that
    /// promise for mentions only, so both passes are produced up front, before
    /// the first write of either kind.
    pub fn ingest(
        &mut self,
        turn: &Turn,
        extraction: &Extraction,
        embedder: &impl Embedder,
    ) -> Result<Ingested, EngineError> {
        // Every mention index checked before anything else, embeddings
        // included: `extract` refuses these for its own output, but
        // `Extraction`'s fields are `pub` and this function is `pub`, so a
        // hand-built extraction with an out-of-range index is not a
        // hypothetical -- it is how every test in this crate exercises
        // `ingest`. Checked this early so a bad index costs nothing, the same
        // guarantee the embedder check just below gives a failing embedder.
        validate_extraction(extraction)?;

        // Every vector first, and every vector checked, before the first write.
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(extraction.mentions.len());
        for mention in &extraction.mentions {
            let v = embedder.embed(&mention.text)?;
            self.index.check(&v)?;
            vectors.push(v);
        }
        let mut fact_vectors: Vec<Vec<f32>> = Vec::with_capacity(extraction.facts.len());
        for fact in &extraction.facts {
            let v = embedder.embed(&fact.text)?;
            self.index.check(&v)?;
            fact_vectors.push(v);
        }

        // Every write this turn produces carries the turn's own provenance.
        // `ToolOutput` because an extraction is what a tool returned, not what
        // the user said in so many words -- the user said the sentence, and
        // this is a model's reading of it.
        let prov = Provenance::new(Source::ToolOutput, turn.observed_at, turn.session.clone());

        let mut out = Ingested::default();

        for (mention, embedding) in extraction.mentions.iter().zip(vectors) {
            // The kind is asserted as an attribute so a mention with no facts
            // still becomes an entity. Not a workaround: the kind is a genuine
            // fact about the thing, it gives the entity something to be
            // recalled by, and it means no entity can exist without an
            // assertion -- which is what keeps `Engine::open`'s
            // every-assertion-has-a-vector rule free of exceptions.
            let remembered = self.remember(Observation {
                kind: mention.kind.clone(),
                mention: Record::new().with("name", mention.name.clone()),
                attribute: "kind".to_string(),
                value: Some(mention.kind.clone()),
                valid: Interval::since(turn.observed_at),
                provenance: prov.clone(),
                embedding,
            })?;
            record(&mut out, remembered);
        }

        // Facts, each embedded by its own text. A fact and its subject are
        // different search targets: sharing an embedding would make "where does
        // he work" reachable only by first reaching Ben.
        for (fact, embedding) in extraction.facts.iter().zip(fact_vectors) {
            // `fact.subject` is in range: `validate_extraction` checked every
            // fact, relation and closure subject/object against
            // `extraction.mentions.len()` before the first write above.
            // `out.entities` has one entry per mention, filled by the loop
            // above in mention order, so `out.entities[fact.subject]` is the
            // entity that mention already resolved to.
            //
            // Writing straight to that entity rather than calling `remember`
            // again is deliberate, not an optimisation: `remember` resolves by
            // scoring the mention's fields against every blocked candidate, and
            // a mention built from nothing but a name can legitimately land in
            // the review band on a second look, even against the entity it
            // just came from -- `test_ruleset` requires a corroborating field
            // to place a name-only match above the line, and a `Mention` never
            // carries one. Re-resolving would then either misfile the fact
            // under a fresh, review-pending entity or -- with a more lenient
            // ruleset -- gamble on landing back on the right one. The subject
            // is not a guess here; it is the id `remember` already returned for
            // this exact mention earlier in this same call, so reusing it is
            // the only way to make the brief's own guarantee ("a fact resolves
            // to the same entity its mention already did") actually hold
            // rather than usually hold.
            let entity = out.entities[fact.subject];
            let subject_mention = &extraction.mentions[fact.subject];
            // `write` only reads `attribute`, `value`, `valid`, `provenance`
            // and `embedding` -- `kind` and `mention` exist because
            // `Observation` is the one shape both `remember` and `write` take,
            // not because a fact's write needs an identity to resolve.
            let assertion = self.write(
                entity,
                &Observation {
                    kind: subject_mention.kind.clone(),
                    mention: Record::new().with("name", subject_mention.name.clone()),
                    attribute: fact.attribute.clone(),
                    value: fact.value.clone(),
                    valid: Interval::since(fact.valid_from),
                    provenance: prov.clone(),
                    embedding,
                },
            )?;
            out.assertions.push(assertion);
        }

        for relation in &extraction.relations {
            // `relation.subject` and `relation.object` are in range for the
            // same reason as `fact.subject` above -- `validate_extraction`
            // already checked them -- and `out.entities` has exactly one
            // entry per mention, built by the loop just above in the same
            // order `extract` assigned local indices.
            self.relate(
                out.entities[relation.subject],
                relation.predicate.clone(),
                out.entities[relation.object],
                Interval::since(relation.valid_from),
                prov.clone(),
            )?;
        }

        // Closures run after relations here, but nothing below depends on
        // that order: `spared` is read straight from `extraction.relations`,
        // not from anything the store has been made to hold yet, so it
        // excludes the same same-turn objects whether closures run before or
        // after relations do -- confirmed by running the two loops in the
        // opposite order and seeing every test in this module still pass.
        // Running closures last only matches the reading order "assert what
        // is now true, then retire what it superseded"; `spared`, not the
        // sequence, is what keeps "I moved from Acme to Globex" from closing
        // Globex as fast as it opened it. Compared by resolved entity id, not
        // by local mention index: `extract` does not dedupe mentions, so the
        // same person can appear at more than one index in one extraction,
        // and a relation asserted from a different index than the closure's
        // would otherwise be missed even though both resolved to this same
        // subject.
        for closure in &extraction.closures {
            let subject = out.entities[closure.subject];

            // Edges this extraction asserted for the same subject and
            // predicate. Anything else in force is what the closure ends.
            let spared: Vec<StableId> = extraction
                .relations
                .iter()
                .filter(|r| out.entities[r.subject] == subject && r.predicate == closure.predicate)
                .map(|r| out.entities[r.object])
                .collect();

            // `Timestamp::MAX` on the transaction axis: the question is which
            // edges the store holds *now*, not what it believed earlier. The
            // engine has no clock, and reusing `closure.at` -- a valid-time
            // value -- would hide any edge learned after the moment the closure
            // speaks about, leaving it open forever.
            let doomed: Vec<(String, StableId)> = self
                .edges_from(subject, closure.at, Timestamp::MAX)
                .into_iter()
                .filter(|e| e.predicate == closure.predicate && !spared.contains(&e.object))
                .map(|e| (e.predicate.to_string(), e.object))
                .collect();

            // `AgentInference`, which `rm_core` documents as the weakest source: "inferences are derived from the others and re-deriving one does not make it more true". That is what makes this safe to do at all -- the closure is traceable in `edge_history`, it can never be mistaken for something the user said, and `Strategy::SourcePriority` can rank it below a user assertion so a later correction wins with no special handling.
            let inferred = Provenance::new(
                Source::AgentInference,
                turn.observed_at,
                turn.session.clone(),
            );

            for (predicate, object) in doomed {
                self.unrelate(subject, &predicate, object, closure.at, inferred.clone())?;
                out.closed.push(Closed {
                    subject,
                    predicate,
                    object,
                    because: closure.because.clone(),
                });
            }
        }

        Ok(out)
    }
}

/// Check every mention has a name, that every fact, relation and closure names
/// a mention this extraction actually has, and that no relation runs from a
/// mention to itself, before `ingest` writes anything.
///
/// All three of `extract`'s refusals, not two of them. The nameless-mention
/// check matters more here than it does there: `ingest` resolves a mention on a
/// `Record` carrying only `name`, and a ruleset blocking on
/// `BlockingKey::Prefix("name", 3)` turns an empty name into the key `name~`.
/// Every nameless mention therefore lands in one block, and inside it each
/// scores against every other on the one field they share, blank on both sides
/// and so in agreement. A ruleset that trusts a name match then merges distinct
/// things into one entity; a stricter one files a review for every pair. Both
/// are wrong and neither is an error, which is why this is refused here rather
/// than left to be noticed.
///
/// `rm_extract::extract` enforces this for its own output, but `Extraction`'s
/// fields are `pub` and `ingest` is `pub`, so a caller can build one with an
/// out-of-range index directly -- every test in this crate does, to exercise
/// `ingest` without going through `extract` at all. Indexing straight into
/// `out.entities` on a bad index would panic on input the type system allows,
/// which is not how any other door in this workspace treats a caller's
/// mistake: `MemoryStore::open` validates its snapshot, `Engine::relate`
/// validates its endpoints, and `extract` validates these same indices for
/// the extractions it builds itself. `ingest` owes its own callers the same
/// treatment rather than trusting a guarantee it cannot see enforced.
fn validate_extraction(extraction: &Extraction) -> Result<(), EngineError> {
    // Names first, in `extract`'s own order, so the two refuse the same
    // extraction with the same complaint rather than with whichever check
    // happened to run first.
    for (index, mention) in extraction.mentions.iter().enumerate() {
        if mention.name.trim().is_empty() {
            return Err(EngineError::NamelessMention(index));
        }
    }

    let mentions = extraction.mentions.len();
    let in_range = |what: &'static str, index: usize| -> Result<(), EngineError> {
        if index < mentions {
            Ok(())
        } else {
            Err(EngineError::BadMentionIndex {
                what,
                index,
                mentions,
            })
        }
    };
    for fact in &extraction.facts {
        in_range("fact subject", fact.subject)?;
    }
    for relation in &extraction.relations {
        in_range("relation subject", relation.subject)?;
        in_range("relation object", relation.object)?;
        // Mirrors `extract`'s own check, by local index rather than resolved
        // entity: `extract` refuses this for its own output, but a hand-built
        // `Extraction` can still carry it, and letting it through would still
        // end in an error -- `rm_store::relate` refuses a self-edge -- just
        // after the mention and fact loops above had already written. That
        // is the same guarantee `BadMentionIndex` exists to give a bad index:
        // a relation that can never be created should cost nothing, not
        // merely fail no worse than it would have anyway.
        if relation.subject == relation.object {
            return Err(EngineError::SelfRelation(relation.subject));
        }
    }
    for closure in &extraction.closures {
        in_range("closure subject", closure.subject)?;
    }
    Ok(())
}

/// Fold one `remember` result into the running record.
fn record(out: &mut Ingested, remembered: Remembered) {
    match remembered {
        Remembered::Merged { entity, assertion } | Remembered::Created { entity, assertion } => {
            out.entities.push(entity);
            out.assertions.push(assertion);
        }
        Remembered::CreatedPendingReview {
            entity,
            assertion,
            review,
        } => {
            out.entities.push(entity);
            out.assertions.push(assertion);
            out.reviews.extend(review);
        }
    }
}
