//! Reading the engine: survivorship at query time.

use std::collections::BTreeSet;

use rm_core::{Interval, Provenance, Source, Supersession, Timestamp};
use rm_store::StableId;
use rm_survivor::{merge, Candidate, Held};

use crate::{AssertionId, AssertionRef, Engine, EngineError, Policy};

/// What the engine concluded an attribute held.
///
/// Owned rather than borrowed: survivorship on read builds its answer from an
/// outcome computed inside the call, so when the strategy produces a value no
/// single stored version carried, there is nothing in the store to borrow from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Believed {
    Value(String),
    Absent,
    Unknown,
}

/// A recall request.
#[derive(Clone, Debug)]
pub struct Query {
    pub embedding: Vec<f32>,
    pub k: usize,
    /// `(valid_t, tx_t)`. Both axes, because "what did I believe last Tuesday
    /// about what was true in May" is a different question from either half.
    pub as_of: Option<(Timestamp, Timestamp)>,
    pub entity: Option<StableId>,
    pub source: Option<Source>,
    pub session: Option<String>,
    /// Entities whose assertions are worth more for this query, and how much.
    ///
    /// A *boost*, not a filter, and the distinction is the whole design.
    /// [`Query::entity`] answers "only tell me about Alice", which is right
    /// when the caller knows the subject. A question does not come with that
    /// knowledge — it comes with a name, and turning a name into an entity is
    /// exactly the fallible step. Measured on this corpus the store separates
    /// the right subject from the wrong one at J = 0.33, so filtering on it
    /// would discard the answer outright every time it guessed wrong, while a
    /// boost only costs the guess its advantage.
    ///
    /// Empty and zero mean an unboosted query, which is what
    /// [`Query::new`] builds.
    ///
    /// # It did not help on LoCoMo, and the reason is the corpus
    ///
    /// This was built because 98% of that benchmark's remaining recall gap is
    /// ranking rather than storage -- the store holds an assertion from an
    /// evidence turn for 99.3% of questions and gets one into the top ten for
    /// 70.9%. Boosting the subject looked like the lever.
    ///
    /// Swept over three conversations it is flat and then falls:
    ///
    /// ```text
    ///   boost      +0.00   +0.02   +0.05   +0.10   +0.20   +0.50
    ///   recall     0.671   0.671   0.671   0.664   0.651   0.651
    /// ```
    ///
    /// Because a LoCoMo conversation has two speakers who between them own
    /// **93-98% of every assertion in the store**. Boosting one of them adds a
    /// constant to more than half the index, which reorders almost nothing at
    /// small weights and, at large ones, promotes the subject's irrelevant
    /// facts over the other speaker's relevant ones. "Is this about the
    /// subject" carries almost no information when everything is about one of
    /// two people.
    ///
    /// So the null is about the corpus, not the mechanism: a store whose
    /// assertions are spread over hundreds of entities is the case this is for,
    /// and LoCoMo cannot show it either way. Kept, tested, and documented as
    /// unproven rather than quietly deleted or quietly shipped as a win.
    pub boost: BTreeSet<StableId>,
    /// Added to the similarity of a boosted entity's assertions.
    ///
    /// Cosine similarity lives in `[-1, 1]`, so this is on that scale: 0.05 is
    /// a nudge between near-ties, 0.5 outranks almost anything unboosted. The
    /// right value is not obvious and is not guessed here — `benches/locomo`
    /// sweeps it.
    pub boost_by: f32,
}

/// One recalled assertion.
#[derive(Clone, Debug, PartialEq)]
pub struct Recalled {
    pub entity: StableId,
    /// What the entity this is about is called, when it has a name.
    ///
    /// Resolved here rather than left to the caller because every caller wants
    /// it and the engine already holds it. A hit reading `entity 14  because =
    /// the k-curve is still 0.926 at k=200` is the right answer with the
    /// question missing: what the assertion says, with no way to tell what it
    /// is about short of a second lookup per hit.
    ///
    /// `None` for an entity whose identity carries no `name`. Nothing requires
    /// one -- an entity exists as soon as something is asserted about it, and
    /// the mention that created it may have had only a kind.
    pub name: Option<String>,
    pub assertion: AssertionId,
    pub attribute: String,
    /// `None` is a tombstone — this assertion claimed the attribute had no
    /// value. It is never "we have nothing": an assertion that says nothing is
    /// not stored and cannot be recalled.
    pub value: Option<String>,
    pub valid: Interval,
    pub provenance: Provenance,
    pub score: f32,
    /// Where this assertion stands against the later ones in its slot, as of
    /// the query's `tx_t`.
    pub standing: Standing,
}

/// Where a recalled assertion stands against the later ones in its slot.
///
/// This used to be a `bool` named `superseded`, and it was set by asking
/// whether anything later existed. Arrival order is not contradiction: over ten
/// LoCoMo conversations that rule flagged 26% of every assertion in the store as
/// replaced, and the sample is dominated by facts that are all still true --
/// three things someone attended, five things they appreciate, two pets. An
/// agent reading `superseded` on "she has a dog" forgets the dog.
///
/// So the question is now put to the party that can answer it. `rm_extract`
/// asks the model, per fact, whether a later one of these makes the earlier one
/// untrue, and [`Supersession`] carries the answer down to here. What is left
/// is the case where nobody answered, and that gets its own variant rather than
/// being folded into either of the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Nothing later in this slot. The latest thing said.
    Latest,
    /// Later assertions exist and every one of them said it was one more of the
    /// same thing. This is still true; it is simply not the only answer.
    ///
    /// Distinct from [`Standing::Latest`] because a caller reading out "she has
    /// a dog" is better off knowing there is also a cat, and distinct from
    /// [`Standing::Unsettled`] because here somebody actually answered.
    Joined,
    /// A later assertion in this slot claimed to replace what came before.
    Corrected,
    /// A later assertion exists, and none of them said whether it replaced
    /// this. Both may be true at once. Reported rather than resolved: a store
    /// that guesses here is wrong a quarter of the time and never says so.
    Unsettled,
}

impl Standing {
    /// Whether this assertion may still be stated as current.
    ///
    /// True for everything but [`Standing::Corrected`], which is the point: an
    /// unanswered question is not a correction, and a caller that only wants to
    /// know whether it can say the fact out loud should not have to reason
    /// about why nobody answered it.
    pub fn still_stands(&self) -> bool {
        !matches!(self, Standing::Corrected)
    }
}

impl Query {
    /// A recall over everything, as of now.
    ///
    /// `as_of` defaults to `None`, meaning unbounded on both axes rather than
    /// "now" — this crate takes no clock, and inventing one here would make the
    /// result depend on a wall clock the caller cannot control in a test.
    pub fn new(embedding: Vec<f32>, k: usize) -> Self {
        Query {
            embedding,
            k,
            as_of: None,
            entity: None,
            source: None,
            session: None,
            boost: BTreeSet::new(),
            boost_by: 0.0,
        }
    }

    /// Prefer assertions attached to these entities, by `weight`.
    ///
    /// See [`Query::boost`] for why this is a preference and not a filter.
    /// An empty set or a zero weight leaves the query unchanged, so a caller
    /// that failed to identify a subject can pass what it found without
    /// branching.
    pub fn boosting(mut self, entities: impl IntoIterator<Item = StableId>, weight: f32) -> Self {
        self.boost = entities.into_iter().collect();
        self.boost_by = weight;
        self
    }

    pub fn as_of(mut self, valid_t: Timestamp, tx_t: Timestamp) -> Self {
        self.as_of = Some((valid_t, tx_t));
        self
    }

    pub fn about_entity(mut self, entity: StableId) -> Self {
        self.entity = Some(entity);
        self
    }

    pub fn in_session(mut self, session: impl Into<String>) -> Self {
        self.session = Some(session.into());
        self
    }

    pub fn from_source(mut self, source: Source) -> Self {
        self.source = Some(source);
        self
    }
}

impl Engine {
    /// What we believed at `tx_t` about what was true at `valid_t`.
    ///
    /// This is where survivorship runs. `remember` appends without resolving,
    /// so the strategy is applied to the whole history on every read — which is
    /// what makes it swappable, and what makes `Strategy::ValidInterval` need
    /// no special handling: its outcome is a timeline and the question is
    /// answered by asking the timeline.
    pub fn about(
        &self,
        entity: StableId,
        attribute: &str,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Result<Believed, EngineError> {
        self.about_under(&self.policy, entity, attribute, valid_t, tx_t)
    }

    /// The same question, answered under a policy the engine does not hold.
    ///
    /// This is the crate's central claim made callable: the *same* stored
    /// history reads as one winner under [`Strategy::MostRecent`] and as a
    /// timeline under [`Strategy::ValidInterval`], and a caller can ask both
    /// ways in successive lines without anything being rewritten between them.
    ///
    /// [`Engine::with_policy`] can express the same thing but consumes the
    /// engine, so demonstrating the contrast means moving it there and back —
    /// which reads as though the engine were being reconfigured, when the point
    /// is that it is not. Nothing about the store changes here; only the
    /// question does.
    ///
    /// [`Strategy::MostRecent`]: rm_survivor::Strategy::MostRecent
    /// [`Strategy::ValidInterval`]: rm_survivor::Strategy::ValidInterval
    pub fn about_under(
        &self,
        policy: &Policy,
        entity: StableId,
        attribute: &str,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Result<Believed, EngineError> {
        // Only what we had by tx_t. Later knowledge does not leak backwards.
        let versions: Vec<_> = self
            .store
            .history(entity, attribute)
            .iter()
            .filter(|v| v.ingested_at() <= tx_t)
            .collect();
        if versions.is_empty() {
            // Covers three cases that are all the same answer: an unknown
            // entity, an attribute never discussed, and every version having
            // arrived after tx_t. None of them is an error — the store simply
            // has no opinion yet.
            return Ok(Believed::Unknown);
        }

        let candidates: Vec<Candidate<'_>> = versions
            .iter()
            .map(|v| {
                let c = match &v.value {
                    Some(s) => Candidate::new(Some(s.as_str()), &v.provenance),
                    // A stored `None` is a tombstone — a positive claim of
                    // absence — and has to compete as one. `Candidate::new(
                    // None, ..)` would instead read as the source saying
                    // nothing, which drops the tombstone out of the comparison
                    // entirely and lets an earlier value win by default.
                    None => Candidate::absent(&v.provenance),
                };
                // Over the span it actually held, rather than the default of
                // "valid from when it was heard". Without this the store
                // records both axes and the read path sees one: a job change
                // mentioned in September and true from July answered with the
                // old employer for August, which is the case `rm_store`'s
                // module docs open with.
                c.over(v.valid)
            })
            .collect();

        // A refusal propagates rather than falling back to a looser strategy:
        // a memory chosen by a rule the caller did not ask for is exactly the
        // plausible-looking wrong answer the refusals exist to prevent.
        let outcome = merge(&candidates, policy.for_attribute(attribute))?;
        Ok(match outcome.held_at(valid_t) {
            // `held_at`, not `as_of`: `as_of` collapses an asserted absence
            // into `None`, the same shape as no coverage at all. `Believed`
            // exists to keep exactly that distinction, so the precise
            // accessor is the only one that can feed it.
            Some(Held::Value(v)) => Believed::Value(v.clone()),
            Some(Held::Absent) => Believed::Absent,
            None => Believed::Unknown,
        })
    }

    /// The same engine reading under a different policy from now on.
    ///
    /// Cheap because nothing was resolved on write: changing the rule changes
    /// the answer without touching a single stored version.
    ///
    /// Consumes the engine, so it suits setting a default once. To ask one
    /// history two ways without moving the engine anywhere, use
    /// [`Engine::about_under`].
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// The policy this engine reads under.
    ///
    /// Exposed so a host can ask what governs an attribute *before* asking
    /// about it -- `Policy::for_attribute` then names the strategy, and
    /// `Strategy::keeps_a_timeline` says whether a valid-time question has an
    /// answer at all. Without this the caller can only ask and be told
    /// something plausible.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The `k` nearest assertions matching the query's scope.
    ///
    /// Scoping is handed to `rm_index::VectorIndex::search_filtered` as a
    /// closure, so it runs *during* the scan rather than afterwards. Fetching a
    /// top-`k` and filtering it after the fact silently returns two results for
    /// "what do I know about Alice in this session" whenever eight
    /// better-scoring assertions belong to other sessions — the caller sees a
    /// short list with no way to tell it was truncated by the filter rather
    /// than by the data. `rm_index` was built specifically to avoid that
    /// failure, and reintroducing it one layer up would waste the work.
    pub fn recall(&self, q: &Query) -> Result<Vec<Recalled>, EngineError> {
        let keep = |id: rm_index::EntryId| self.in_scope(id, q);
        // The boost runs inside the scan for the same reason the filter does.
        // Re-ranking a fetched top-`k` could only ever promote assertions that
        // raw similarity had already surfaced, and the one this exists to
        // rescue is the assertion about the right person sitting at rank 40.
        let boost = |id: rm_index::EntryId, score: f32| {
            if q.boost.is_empty() {
                return score;
            }
            match self.assertions.get(&id) {
                Some(entry) if q.boost.contains(&entry.entity) => score + q.boost_by,
                _ => score,
            }
        };
        let hits = self.index.search_adjusted(&q.embedding, q.k, keep, boost)?;

        let recalled: Vec<Recalled> = hits
            .into_iter()
            .filter_map(|hit| {
                // A hit that resolves to nothing is dropped rather than
                // reported, and that is not a shrug. `in_scope` already
                // performed both of these lookups during the scan and returned
                // false for anything that failed them, so reaching here with a
                // missing assertion or version would mean the index and the
                // assertion map disagreed *within a single call* — a state
                // `Engine::open` rejects at the door and no `&mut self` method
                // can produce. Reporting it would mean adding an error variant
                // for a condition that cannot arise, and every caller would
                // then have to handle it; `?` on an `Option` keeps `recall`
                // total instead. The reverse direction — a vector no assertion
                // claims — is the one that used to slip through a restore, and
                // it is checked in `open` where it can actually be caught.
                let entry = self.assertions.get(&hit.id)?;
                let version = self
                    .store
                    .history(entry.entity, &entry.attribute)
                    .get(entry.version)?;
                Some(Recalled {
                    entity: entry.entity,
                    name: self
                        .identity_of(entry.entity)
                        .and_then(|r| r.get("name").map(str::to_string)),
                    assertion: hit.id,
                    attribute: entry.attribute.clone(),
                    value: version.value.clone(),
                    valid: version.valid,
                    provenance: version.provenance.clone(),
                    score: hit.score,
                    standing: self.standing(entry, version, q),
                })
            })
            .collect();
        Ok(demote_replaced(recalled))
    }
}

/// Put a corrected assertion below the live one that replaced it, when both
/// came back.
///
/// Measured. Over ten conversations, 3.2% of questions came back led by an
/// assertion something later had replaced, and **30% of those had the live
/// value sitting further down the same result list** -- the store held the
/// current answer and offered the dead one first. With `rm_extract::arity`
/// filling `Supersession` that share rose to 61% on conversation 0, because
/// more corrections are now known to be corrections. An agent that reads the
/// first hit and states it will state the stale one, and the mark it ignored
/// was all that stood between that and a confident wrong answer.
///
/// **Same slot only.** A corrected fact is demoted below a live one for the
/// same attribute on the same entity, and nowhere else. A superseded employer
/// should not outrank the current employer; it may perfectly well outrank an
/// unrelated fact that happens to still stand, because relevance is what the
/// score is for and this is not trying to be a second opinion about relevance.
///
/// **The returned set does not change**, only its order, so recall@k is
/// untouched by construction -- this cannot flatter the metric it sits beside.
/// The sort is stable, so anything not demoted keeps its score order exactly.
fn demote_replaced(mut hits: Vec<Recalled>) -> Vec<Recalled> {
    // A slot has a live answer in these results if any hit on it still stands.
    let live: BTreeSet<(StableId, &str)> = hits
        .iter()
        .filter(|h| h.standing.still_stands())
        .map(|h| (h.entity, h.attribute.as_str()))
        .collect();
    // Collected before sorting because the borrow above ends here; the keys are
    // owned so the comparator can consult them while `hits` is mutable.
    let demoted: Vec<bool> = hits
        .iter()
        .map(|h| !h.standing.still_stands() && live.contains(&(h.entity, h.attribute.as_str())))
        .collect();

    let mut order: Vec<usize> = (0..hits.len()).collect();
    order.sort_by_key(|&i| demoted[i]);
    let mut out: Vec<Option<Recalled>> = hits.drain(..).map(Some).collect();
    order
        .into_iter()
        .map(|i| out[i].take().expect("each index taken once"))
        .collect()
}

impl Engine {
    /// Whether one assertion passes the query's non-vector filters.
    ///
    /// Called from inside `search_filtered`'s scan rather than after it — see
    /// [`Engine::recall`] for why that ordering is the point.
    fn in_scope(&self, id: rm_index::EntryId, q: &Query) -> bool {
        let Some(entry) = self.assertions.get(&id) else {
            return false;
        };
        if q.entity.is_some_and(|e| e != entry.entity) {
            return false;
        }
        let Some(version) = self
            .store
            .history(entry.entity, &entry.attribute)
            .get(entry.version)
        else {
            return false;
        };
        if let Some(session) = &q.session {
            if &version.provenance.source_ref != session {
                return false;
            }
        }
        if let Some(source) = &q.source {
            if &version.provenance.source != source {
                return false;
            }
        }
        if let Some((valid_t, tx_t)) = q.as_of {
            if version.ingested_at() > tx_t || !version.valid.contains(valid_t) {
                return false;
            }
        }
        true
    }

    /// Where an assertion stands against the later ones in its slot, as of the
    /// query's `tx_t` (or unbounded, if the query has none).
    ///
    /// Reported rather than filtered: semantic recall of a fact that *was* true
    /// is often exactly what was wanted ("what did I believe about her employer
    /// in May"), and dropping a corrected fact would make that unanswerable.
    /// Returning it unmarked is worse — it lets a caller state a stale fact as
    /// current. Marking it is the only option that does neither.
    ///
    /// Strictly later on the transaction axis, so two assertions that arrived
    /// at the same instant never rank each other. `MemoryStore::as_of` breaks
    /// that tie by arrival because it has to return one value; nothing here has
    /// to, and reporting an arbitrary tie-break as a correction would be a
    /// claim about the world made out of a `Vec`'s index.
    ///
    /// One [`Supersession::Corrects`] among the later assertions settles it.
    /// The rest have to be unanimous to be believed: a slot holding a
    /// correction and an addition has been corrected, because the correction
    /// spoke about everything under it.
    fn standing(&self, entry: &AssertionRef, version: &rm_store::Version, q: &Query) -> Standing {
        let horizon = q.as_of.map(|(_, tx)| tx).unwrap_or(Timestamp::MAX);
        let mut later = self
            .store
            .history(entry.entity, &entry.attribute)
            .iter()
            .filter(|other| {
                other.ingested_at() <= horizon && other.ingested_at() > version.ingested_at()
            })
            .peekable();
        if later.peek().is_none() {
            return Standing::Latest;
        }
        let mut unsettled = false;
        for other in later {
            match other.supersession {
                Supersession::Corrects => return Standing::Corrected,
                Supersession::Unstated => unsettled = true,
                Supersession::Joins => {}
            }
        }
        if unsettled {
            Standing::Unsettled
        } else {
            Standing::Joined
        }
    }
}
