//! Applying an extraction to the store.
//!
//! `rm_extract` describes a turn and knows nothing about entities; this is
//! where a description becomes writes. The mapping from a mention's local index
//! to a `StableId` lives here because here is where the ids are born — inside
//! `remember`, which resolves the mention against everything already known.

use rm_core::{Interval, Provenance, Source};
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
            // `fact.subject` cannot be out of bounds: `rm_extract::extract`
            // refuses any fact naming a mention index `>= mentions.len()`
            // before an `Extraction` is ever produced. `out.entities` has one
            // entry per mention, filled by the loop above in mention order, so
            // `out.entities[fact.subject]` is the entity that mention already
            // resolved to.
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
            // `write` only reads `attribute`, `value`, `valid`, `provenance`
            // and `embedding` -- `kind` and `mention` exist because
            // `Observation` is the one shape both `remember` and `write` take,
            // not because a fact's write needs an identity to resolve.
            let assertion = self.write(
                entity,
                &Observation {
                    kind: extraction.mentions[fact.subject].kind.clone(),
                    mention: Record::new()
                        .with("name", extraction.mentions[fact.subject].name.clone()),
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
            // `relation.subject` and `relation.object` cannot be out of bounds
            // for the same reason as `fact.subject` above, and `out.entities`
            // has exactly one entry per mention -- built by the loop just
            // above, in the same order `extract` assigned local indices.
            self.relate(
                out.entities[relation.subject],
                relation.predicate.clone(),
                out.entities[relation.object],
                Interval::since(relation.valid_from),
                prov.clone(),
            )?;
        }

        Ok(out)
    }
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
