//! Applying an extraction to the store.
//!
//! `rm_extract` describes a turn and knows nothing about entities; this is
//! where a description becomes writes. The mapping from a mention's local index
//! to a `StableId` lives here because here is where the ids are born — inside
//! `remember`, which resolves the mention against everything already known.

use rm_core::{Interval, Provenance, Source};
use rm_extract::Extraction;
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
    /// Every embedding is produced and validated before anything is written, so
    /// a failing embedder costs nothing. That is the guarantee `Engine::remember`
    /// already makes and for the same reason: a fact in the store with no vector
    /// to find it is undetectable from outside — no query reports it and no
    /// error names it.
    pub fn ingest(
        &mut self,
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
                valid: Interval::since(0),
                provenance: Provenance::new(Source::ToolOutput, 0, "extraction"),
                embedding,
            })?;
            record(&mut out, remembered);
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
