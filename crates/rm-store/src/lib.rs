//! A bi-temporal store for agent memory, where **attributes** are bi-temporal.
//!
//! # Why this crate exists at all
//!
//! Golden Suite's `goldengraph-core` already ships a bi-temporal store, and it
//! is good: stable ids, `as_of(valid_t, tx_t)`, merge/split history, portable
//! snapshots. But it draws its scope line here:
//!
//! > identity (which id) and edge facts are bi-temporal; entity *attributes*
//! > (canonical_name / surface_names) reflect the latest state, not their value
//! > as-of `tx_t`.
//!
//! For master data that line is reasonable — you care which customer record is
//! which, and the display name is cosmetic. For agent memory it is exactly
//! backwards. The attributes *are* the payload. "The user's employer" is an
//! attribute, and its history is the entire product.
//!
//! So this store keeps the shape of that design and moves the time axes down
//! onto attribute values.
//!
//! # The two axes, and why one is not enough
//!
//! - **Valid time** — when the fact was true in the world.
//! - **Transaction time** — when this agent learned it.
//!
//! They come apart constantly in conversation. In September a user mentions they
//! changed jobs back in July. The valid time starts in July; the transaction
//! time starts in September. A single-axis store has to choose:
//!
//! - Record it as of July, and the agent's August answers become retroactively
//!   wrong — you can no longer reconstruct what it knew when it said them, so
//!   you cannot tell a stale answer from a bug.
//! - Record it as of September, and the store now claims the user worked
//!   somewhere in August that they did not.
//!
//! Both are lies. Keeping both axes means neither is needed: [`MemoryStore::as_of`]
//! answers "what did we believe at *T_tx* about what was true at *T_valid*",
//! and every one of those questions has a correct answer.
//!
//! # Absent is not unknown
//!
//! [`Known`] distinguishes three states, because memory needs all three. "The
//! user has no employer" is a fact; "we have never discussed the user's
//! employer" is a gap. Collapsing them to `Option<&str>` teaches an agent to
//! state the first when it only has the second, which is how a memory store
//! starts confabulating.

use std::collections::BTreeMap;

use rm_core::{Interval, Provenance, Timestamp};
use rm_survivor::{merge, Candidate, Outcome, Refused, Strategy};
use serde::{Deserialize, Serialize};

/// A durable entity id: assigned once, monotonic, never reused.
///
/// Never reused even after an entity is forgotten, so a dangling reference in a
/// log or an exported transcript can always be recognised as dangling rather
/// than silently resolving to whatever took its place.
pub type StableId = u64;

/// Something went wrong that the caller has to decide about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No entity with this id. Not created, or created in a different store.
    UnknownEntity(StableId),
    /// Survivorship declined to guess. Carries its explanation.
    Refused(Refused),
    /// A snapshot could not be read.
    Parse(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::UnknownEntity(id) => write!(f, "no entity with id {id}"),
            StoreError::Refused(r) => write!(f, "{r}"),
            StoreError::Parse(m) => write!(f, "could not read snapshot: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<Refused> for StoreError {
    fn from(r: Refused) -> Self {
        StoreError::Refused(r)
    }
}

/// What the store knows about one attribute at one point on each axis.
///
/// Three states, not two. See the module docs: conflating [`Known::Absent`]
/// with [`Known::Unknown`] is how a memory store learns to confabulate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Known<'a> {
    /// A value was asserted and held.
    Value(&'a str),
    /// Asserted to have no value — a positive claim of absence.
    Absent,
    /// Nothing covers this point. The store has no opinion.
    Unknown,
}

impl<'a> Known<'a> {
    /// The value, if one is known. Both `Absent` and `Unknown` give `None`, so
    /// only reach for this where the distinction genuinely does not matter.
    pub fn value(self) -> Option<&'a str> {
        match self {
            Known::Value(v) => Some(v),
            _ => None,
        }
    }

    /// Whether the store has any opinion here, including a claim of absence.
    pub fn is_known(self) -> bool {
        !matches!(self, Known::Unknown)
    }
}

/// One assertion about an attribute, positioned on both time axes.
///
/// Versions are never mutated or deleted — a correction is a new version with a
/// later `ingested_at`. That is what makes "what did we believe last Tuesday"
/// answerable at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// `None` asserts the attribute had no value over `valid` — a tombstone,
    /// distinct from having said nothing.
    pub value: Option<String>,
    /// When this held in the world.
    pub valid: Interval,
    /// Who said so and when we heard it. `provenance.observed_at` *is* the
    /// transaction time; keeping one field rather than two means they cannot
    /// drift apart.
    pub provenance: Provenance,
}

impl Version {
    /// Transaction time: when this assertion entered the store.
    pub fn ingested_at(&self) -> Timestamp {
        self.provenance.observed_at
    }
}

/// An entity and everything ever asserted about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub id: StableId,
    pub kind: String,
    pub created_at: Timestamp,
    /// Attribute name to its append-only version log. `BTreeMap` so snapshots
    /// are byte-stable and diffable across runs.
    pub attributes: BTreeMap<String, Vec<Version>>,
}

/// The store.
///
/// Append-only by construction: there is no API to modify or remove a
/// [`Version`]. Forgetting is a future operation on entities, not an edit to
/// history.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStore {
    entities: BTreeMap<StableId, Entity>,
    next_id: StableId,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an entity and return its durable id.
    pub fn create_entity(&mut self, kind: impl Into<String>, created_at: Timestamp) -> StableId {
        let id = self.next_id;
        self.next_id += 1;
        self.entities.insert(
            id,
            Entity {
                id,
                kind: kind.into(),
                created_at,
                attributes: BTreeMap::new(),
            },
        );
        id
    }

    pub fn entity(&self, id: StableId) -> Option<&Entity> {
        self.entities.get(&id)
    }

    pub fn entity_ids(&self) -> impl Iterator<Item = StableId> + '_ {
        self.entities.keys().copied()
    }

    /// Record one assertion.
    ///
    /// Nothing is overwritten: this appends. Asserting a different value over
    /// the same valid span is a *correction*, and both versions survive so the
    /// earlier belief stays reconstructible.
    pub fn assert(
        &mut self,
        id: StableId,
        attribute: impl Into<String>,
        value: Option<String>,
        valid: Interval,
        provenance: Provenance,
    ) -> Result<(), StoreError> {
        let entity = self
            .entities
            .get_mut(&id)
            .ok_or(StoreError::UnknownEntity(id))?;
        entity
            .attributes
            .entry(attribute.into())
            .or_default()
            .push(Version {
                value,
                valid,
                provenance,
            });
        Ok(())
    }

    /// Resolve competing assertions with `strategy`, then record the result.
    ///
    /// This is where survivorship and bi-temporality meet, and the two outcome
    /// shapes land differently on purpose:
    ///
    /// - A single survivor is written as one open-ended version, valid from the
    ///   earliest observation among the candidates. The strategy concluded there
    ///   is one answer, so the store records one.
    /// - A timeline is written as one version per span. The strategy concluded
    ///   the value *changed*, and flattening that back to a winner would throw
    ///   away the distinction the store exists to keep.
    /// - No survivor writes nothing. `UnanimousOrNull` on a disagreement means
    ///   "no answer", which is a gap ([`Known::Unknown`]), not a tombstone.
    ///   Writing `Absent` here would convert "we could not tell" into "there is
    ///   none".
    ///
    /// A refusal propagates. The store does not fall back to a looser strategy:
    /// a memory chosen by a rule the caller did not ask for is exactly the
    /// plausible-looking wrong answer the refusals exist to prevent.
    pub fn assert_resolved(
        &mut self,
        id: StableId,
        attribute: impl Into<String>,
        candidates: &[Candidate<'_>],
        strategy: &Strategy,
    ) -> Result<(), StoreError> {
        if !self.entities.contains_key(&id) {
            return Err(StoreError::UnknownEntity(id));
        }
        let outcome = merge(candidates, strategy)?;
        let attribute = attribute.into();

        // The resolution is known as of the newest thing it considered, and
        // speaks about valid time from the oldest.
        let asserted: Vec<&Candidate<'_>> =
            candidates.iter().filter(|c| c.value.is_some()).collect();
        let (Some(earliest), Some(latest)) = (
            asserted.iter().map(|c| c.provenance.observed_at).min(),
            asserted
                .iter()
                .max_by_key(|c| c.provenance.observed_at)
                .map(|c| c.provenance),
        ) else {
            return Ok(()); // nothing was asserted; nothing to record
        };

        match outcome {
            Outcome::Survivor(None) => Ok(()),
            Outcome::Survivor(Some(value)) => self.assert(
                id,
                attribute,
                Some(value),
                Interval::since(earliest),
                latest.clone(),
            ),
            Outcome::Timeline(facts) => {
                for fact in facts {
                    self.assert(
                        id,
                        attribute.clone(),
                        Some(fact.value),
                        fact.valid,
                        latest.clone(),
                    )?;
                }
                Ok(())
            }
        }
    }

    /// What we believed at `tx_t` about what was true at `valid_t`.
    ///
    /// The one query the whole crate is built around. Among versions we had by
    /// `tx_t` whose valid span covers `valid_t`, the latest-ingested wins:
    /// later knowledge supersedes earlier knowledge *about the same moment*,
    /// which is what a correction is.
    ///
    /// Ingestion ties break toward the later append. Deterministic, but a tie
    /// means two contradictory versions arrived at the same instant, which
    /// [`Self::assert_resolved`] refuses to create — reaching it implies raw
    /// [`Self::assert`] calls the caller has not reconciled.
    pub fn as_of(
        &self,
        id: StableId,
        attribute: &str,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Known<'_> {
        let Some(versions) = self
            .entities
            .get(&id)
            .and_then(|e| e.attributes.get(attribute))
        else {
            return Known::Unknown;
        };

        let winner = versions
            .iter()
            .enumerate()
            .filter(|(_, v)| v.ingested_at() <= tx_t && v.valid.contains(valid_t))
            .max_by_key(|(i, v)| (v.ingested_at(), *i))
            .map(|(_, v)| v);

        match winner {
            None => Known::Unknown,
            Some(v) => match &v.value {
                Some(s) => Known::Value(s),
                None => Known::Absent,
            },
        }
    }

    /// What the store believes *now* about what is true *now*, given a clock
    /// reading. Sugar for the common diagonal query.
    pub fn current(&self, id: StableId, attribute: &str, now: Timestamp) -> Known<'_> {
        self.as_of(id, attribute, now, now)
    }

    /// Every version ever asserted for an attribute, in append order.
    ///
    /// The audit trail. Empty for an unknown entity or attribute — asking about
    /// something never discussed is not an error.
    pub fn history(&self, id: StableId, attribute: &str) -> &[Version] {
        self.entities
            .get(&id)
            .and_then(|e| e.attributes.get(attribute))
            .map_or(&[], |v| v.as_slice())
    }

    /// Serialise to canonical JSON.
    ///
    /// `BTreeMap` ordering makes this byte-stable for a given state, so two
    /// stores can be compared by diffing snapshots.
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(self).expect("MemoryStore is always serialisable")
    }

    /// Restore from a snapshot.
    pub fn open(snapshot: &str) -> Result<Self, StoreError> {
        serde_json::from_str(snapshot).map_err(|e| StoreError::Parse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_core::Source;

    // A readable timeline. Units are arbitrary; only order matters.
    const JAN: Timestamp = 1;
    const MAR: Timestamp = 3;
    const JUL: Timestamp = 7;
    const AUG: Timestamp = 8;
    const SEP: Timestamp = 9;
    const OCT: Timestamp = 10;

    fn user_said(at: Timestamp) -> Provenance {
        Provenance::new(Source::UserAssertion, at, format!("session-{at}"))
    }

    fn store_with_user() -> (MemoryStore, StableId) {
        let mut s = MemoryStore::new();
        let id = s.create_entity("person", JAN);
        (s, id)
    }

    // ---- the reason both axes exist ---------------------------------------

    #[test]
    fn a_retroactive_correction_does_not_rewrite_what_we_knew_before() {
        // March: "I work at Acme" (and have since January).
        // September: "actually I moved to Globex back in July."
        let (mut s, id) = store_with_user();
        s.assert(
            id,
            "employer",
            Some("Acme".into()),
            Interval::since(JAN),
            user_said(MAR),
        )
        .unwrap();
        s.assert(
            id,
            "employer",
            Some("Globex".into()),
            Interval::since(JUL),
            user_said(SEP),
        )
        .unwrap();

        // In August we had not been told yet, so we believed Acme. An agent
        // asked in August was not wrong, and the store can still show that.
        assert_eq!(s.as_of(id, "employer", AUG, AUG), Known::Value("Acme"));

        // In October we know better about that same August.
        assert_eq!(s.as_of(id, "employer", AUG, OCT), Known::Value("Globex"));

        // February is untouched by the correction: the fix was about July on.
        assert_eq!(s.as_of(id, "employer", 2, OCT), Known::Value("Acme"));

        // And nothing was destroyed to achieve any of it.
        assert_eq!(s.history(id, "employer").len(), 2);
    }

    #[test]
    fn knowledge_does_not_leak_backwards_along_the_transaction_axis() {
        let (mut s, id) = store_with_user();
        s.assert(
            id,
            "employer",
            Some("Globex".into()),
            Interval::since(JAN),
            user_said(SEP),
        )
        .unwrap();
        // True since January, but we did not hear it until September.
        assert_eq!(s.as_of(id, "employer", MAR, MAR), Known::Unknown);
        assert_eq!(s.as_of(id, "employer", MAR, OCT), Known::Value("Globex"));
    }

    // ---- absent is not unknown --------------------------------------------

    #[test]
    fn a_tombstone_is_a_fact_and_silence_is_not() {
        let (mut s, id) = store_with_user();
        assert_eq!(s.current(id, "employer", OCT), Known::Unknown);

        s.assert(id, "employer", None, Interval::since(JUL), user_said(JUL))
            .unwrap();
        // "I'm unemployed" is an answer; it must not read as "never discussed".
        assert_eq!(s.current(id, "employer", OCT), Known::Absent);
        assert!(s.current(id, "employer", OCT).is_known());
        assert!(!s.current(id, "spouse", OCT).is_known());
        // Both yield None when the caller genuinely does not care which.
        assert_eq!(s.current(id, "employer", OCT).value(), None);
    }

    #[test]
    fn a_tombstone_can_itself_be_superseded() {
        let (mut s, id) = store_with_user();
        s.assert(id, "employer", None, Interval::since(JAN), user_said(MAR))
            .unwrap();
        s.assert(
            id,
            "employer",
            Some("Globex".into()),
            Interval::since(JUL),
            user_said(SEP),
        )
        .unwrap();
        assert_eq!(s.as_of(id, "employer", AUG, MAR), Known::Absent);
        assert_eq!(s.as_of(id, "employer", AUG, OCT), Known::Value("Globex"));
    }

    // ---- the survivorship bridge ------------------------------------------

    #[test]
    fn valid_interval_lands_as_separate_versions_not_one_winner() {
        let (mut s, id) = store_with_user();
        let (p_mar, p_sep) = (user_said(MAR), user_said(SEP));
        s.assert_resolved(
            id,
            "employer",
            &[
                Candidate::new(Some("Acme"), &p_mar),
                Candidate::new(Some("Globex"), &p_sep),
            ],
            &Strategy::ValidInterval,
        )
        .unwrap();

        assert_eq!(s.history(id, "employer").len(), 2, "the change was kept");
        assert_eq!(s.as_of(id, "employer", 5, OCT), Known::Value("Acme"));
        assert_eq!(s.as_of(id, "employer", OCT, OCT), Known::Value("Globex"));
    }

    #[test]
    fn a_survivor_strategy_lands_as_one_open_ended_version() {
        let (mut s, id) = store_with_user();
        let (p_mar, p_sep) = (user_said(MAR), user_said(SEP));
        s.assert_resolved(
            id,
            "employer",
            &[
                Candidate::new(Some("Acme"), &p_mar),
                Candidate::new(Some("Globex"), &p_sep),
            ],
            &Strategy::MostRecent,
        )
        .unwrap();

        let history = s.history(id, "employer");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].valid, Interval::since(MAR));
        // One answer, at every valid time it covers -- that is what the caller
        // asked for by choosing a survivor strategy over ValidInterval.
        assert_eq!(s.as_of(id, "employer", 5, OCT), Known::Value("Globex"));
    }

    #[test]
    fn no_survivor_writes_a_gap_not_a_tombstone() {
        let (mut s, id) = store_with_user();
        let (p_mar, p_sep) = (user_said(MAR), user_said(SEP));
        s.assert_resolved(
            id,
            "employer",
            &[
                Candidate::new(Some("Acme"), &p_mar),
                Candidate::new(Some("Globex"), &p_sep),
            ],
            &Strategy::UnanimousOrNull,
        )
        .unwrap();
        // "We could not tell" must not become "there is none".
        assert_eq!(s.current(id, "employer", OCT), Known::Unknown);
        assert!(s.history(id, "employer").is_empty());
    }

    #[test]
    fn a_refusal_propagates_and_writes_nothing() {
        let (mut s, id) = store_with_user();
        // Contradictory assertions at the same instant: no order between them.
        let (a, b) = (user_said(SEP), user_said(SEP));
        let err = s
            .assert_resolved(
                id,
                "employer",
                &[
                    Candidate::new(Some("Acme"), &a),
                    Candidate::new(Some("Globex"), &b),
                ],
                &Strategy::ValidInterval,
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::Refused(_)), "{err}");
        assert!(
            s.history(id, "employer").is_empty(),
            "a refused resolution must not half-write"
        );
    }

    #[test]
    fn asserting_only_nulls_records_nothing() {
        let (mut s, id) = store_with_user();
        let p = user_said(MAR);
        s.assert_resolved(
            id,
            "employer",
            &[Candidate::new(None, &p)],
            &Strategy::ValidInterval,
        )
        .unwrap();
        assert!(s.history(id, "employer").is_empty());
    }

    // ---- identity and errors ----------------------------------------------

    #[test]
    fn ids_are_monotonic_and_never_reused() {
        let mut s = MemoryStore::new();
        let a = s.create_entity("person", JAN);
        let b = s.create_entity("company", JAN);
        assert_ne!(a, b);
        assert!(b > a);
    }

    #[test]
    fn writing_to_an_unknown_entity_is_an_error_not_a_silent_create() {
        let mut s = MemoryStore::new();
        let err = s
            .assert(
                99,
                "employer",
                Some("Acme".into()),
                Interval::since(JAN),
                user_said(MAR),
            )
            .unwrap_err();
        assert_eq!(err, StoreError::UnknownEntity(99));
    }

    #[test]
    fn reading_something_never_discussed_is_unknown_not_an_error() {
        let (s, id) = store_with_user();
        assert_eq!(s.current(id, "employer", OCT), Known::Unknown);
        assert_eq!(s.current(404, "employer", OCT), Known::Unknown);
        assert!(s.history(404, "employer").is_empty());
    }

    // ---- persistence -------------------------------------------------------

    #[test]
    fn a_snapshot_round_trips_including_history() {
        let (mut s, id) = store_with_user();
        s.assert(
            id,
            "employer",
            Some("Acme".into()),
            Interval::since(JAN),
            user_said(MAR),
        )
        .unwrap();
        s.assert(
            id,
            "employer",
            Some("Globex".into()),
            Interval::since(JUL),
            user_said(SEP),
        )
        .unwrap();

        let restored = MemoryStore::open(&s.snapshot()).unwrap();
        assert_eq!(restored, s);
        // The whole point: the reconstructed store can still answer about the past.
        assert_eq!(
            restored.as_of(id, "employer", AUG, AUG),
            Known::Value("Acme")
        );
    }

    #[test]
    fn snapshots_are_byte_stable() {
        let (mut s, id) = store_with_user();
        s.assert(
            id,
            "role",
            Some("engineer".into()),
            Interval::since(JAN),
            user_said(MAR),
        )
        .unwrap();
        s.assert(
            id,
            "employer",
            Some("Acme".into()),
            Interval::since(JAN),
            user_said(MAR),
        )
        .unwrap();
        // Attributes inserted out of order still serialise identically, so
        // snapshots can be diffed rather than merely parsed.
        assert_eq!(
            s.snapshot(),
            MemoryStore::open(&s.snapshot()).unwrap().snapshot()
        );
    }

    #[test]
    fn a_malformed_snapshot_is_reported_not_panicked_on() {
        let err = MemoryStore::open("{ not json").unwrap_err();
        assert!(matches!(err, StoreError::Parse(_)), "{err:?}");
    }
}
