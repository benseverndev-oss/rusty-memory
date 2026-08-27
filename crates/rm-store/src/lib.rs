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

use std::collections::{BTreeMap, BTreeSet};

use rm_core::{Interval, Provenance, Supersession, Timestamp};
use rm_survivor::{merge, Candidate, Held, Outcome, Refused, Span, Strategy};
use serde::{Deserialize, Serialize};

/// A durable entity id: assigned once, monotonic, never reused.
///
/// Never reused even after an entity is forgotten, so a dangling reference in a
/// log or an exported transcript can always be recognised as dangling rather
/// than silently resolving to whatever took its place.
pub type StableId = u64;

/// Where a snapshot's JSON failed to parse, and what kind of failure it was —
/// never what `serde_json` said about it.
///
/// This used to carry `serde_json::Error::to_string()`, and that string quotes
/// whatever the parser did not like: `invalid type: string "sk-proj-...",
/// expected u64` for a wrong-typed field. `line()`, `column()` and
/// `classify()` are structural facts about *where* and *what kind* of failure
/// it was, not text read out of the snapshot, so a type built only from those
/// three cannot carry a secret regardless of what malformed snapshot produced
/// it — the guarantee is in the type, not in anything that inspects the
/// message afterward. The cost: `expected u64` is gone, and that is the only
/// part of the old message that could ever have been one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotParseError {
    pub line: usize,
    pub column: usize,
    pub category: ParseCategory,
}

impl std::fmt::Display for SnapshotParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.category.reason(),
            self.line,
            self.column
        )
    }
}

impl From<serde_json::Error> for SnapshotParseError {
    fn from(e: serde_json::Error) -> Self {
        SnapshotParseError {
            line: e.line(),
            column: e.column(),
            category: e.classify().into(),
        }
    }
}

/// What kind of thing went wrong while reading the JSON, per
/// [`serde_json::error::Category`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseCategory {
    /// The reader itself failed -- not applicable to a snapshot already held
    /// as a `&str`, but kept so this mirrors `serde_json`'s own categories
    /// rather than inventing a smaller set that a future `serde_json` could
    /// outgrow.
    Io,
    /// The bytes are not well-formed JSON at all.
    Syntax,
    /// The JSON is well-formed but does not match the shape a store snapshot
    /// takes -- a field of the wrong type, an unknown enum variant, a missing
    /// field.
    Data,
    /// The input ended before a complete value was read.
    Eof,
}

impl ParseCategory {
    fn reason(self) -> &'static str {
        match self {
            ParseCategory::Io => "the snapshot could not be read as bytes",
            ParseCategory::Syntax => "the snapshot is not well-formed JSON",
            ParseCategory::Data => "the JSON there does not match the shape a store snapshot takes",
            ParseCategory::Eof => "the snapshot ends before its JSON is complete",
        }
    }
}

impl From<serde_json::error::Category> for ParseCategory {
    fn from(c: serde_json::error::Category) -> Self {
        match c {
            serde_json::error::Category::Io => ParseCategory::Io,
            serde_json::error::Category::Syntax => ParseCategory::Syntax,
            serde_json::error::Category::Data => ParseCategory::Data,
            serde_json::error::Category::Eof => ParseCategory::Eof,
        }
    }
}

/// Something went wrong that the caller has to decide about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No entity with this id. Not created, or created in a different store.
    UnknownEntity(StableId),
    /// Survivorship declined to guess. Carries its explanation.
    Refused(Refused),
    /// A snapshot could not be read.
    Parse(SnapshotParseError),
    /// A snapshot was read, and describes a store that cannot exist.
    ///
    /// Distinct from [`StoreError::Parse`] because the two ask different things
    /// of a caller: `Parse` means these bytes are not a store, which a truncated
    /// write or a wrong file produces and a retry can fix. This means the bytes
    /// *are* a store, and one whose own invariants contradict each other — a
    /// retry writes the same thing again. `rm_index` and `rm_engine` already
    /// draw that line with a variant of this name; drawing it differently here
    /// would make the three doors read as three unrelated designs.
    CorruptSnapshot(String),
    /// An edge from an entity to itself.
    SelfEdge(StableId),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::UnknownEntity(id) => write!(f, "no entity with id {id}"),
            StoreError::Refused(r) => write!(f, "{r}"),
            StoreError::Parse(e) => write!(f, "could not read snapshot: {e}"),
            StoreError::CorruptSnapshot(why) => {
                write!(
                    f,
                    "snapshot parsed but describes an impossible store: {why}"
                )
            }
            StoreError::SelfEdge(id) => write!(f, "entity {id} cannot relate to itself; a self-edge carries nothing a traversal can use, and a merge drops the ones it creates"),
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
    /// What this assertion claims about the ones already in the slot.
    ///
    /// The store does not read it -- [`MemoryStore::as_of`] still answers with
    /// one value and still breaks ties by arrival, because a caller asking for
    /// one value has to be given one. It is carried so that a reader who wants
    /// to know whether the later assertion *meant* to replace this one can find
    /// out, instead of inferring it from the order and being wrong a quarter of
    /// the time. `rm_engine`'s recall path is that reader.
    ///
    /// Written only when it says something, so a snapshot from before the field
    /// existed round-trips byte for byte.
    #[serde(default, skip_serializing_if = "Supersession::is_unstated")]
    pub supersession: Supersession,

    /// Whose view this is, when it is a view rather than a fact.
    ///
    /// An entity, not a label: a holder is somebody the store already knows,
    /// so a holder can be asked about like anyone else and two spellings of
    /// one person cannot become two holders.
    ///
    /// `None` is the store's own assertion, which is what every version
    /// written before this field existed is. Survivorship partitions a slot
    /// by this before resolving, so one holder correcting themselves is a
    /// correction and two holders differing is not -- the latter used to be
    /// settled by arrival order, reporting a change where nothing changed.
    ///
    /// Written only when it says something, so a snapshot from before the
    /// field existed round-trips byte for byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub according_to: Option<StableId>,
}

impl Version {
    /// Transaction time: when this assertion entered the store.
    pub fn ingested_at(&self) -> Timestamp {
        self.provenance.observed_at
    }
}

/// One assertion about a relationship, positioned on both time axes.
///
/// The edge counterpart of [`Version`], and deliberately the same shape:
/// `present: false` is a tombstone asserting the relationship did *not* hold
/// over `valid`, exactly as a `Version` with `value: None` asserts an attribute
/// had none. Keeping the two models identical is what lets edges inherit
/// bi-temporality, provenance and the restore path without a second
/// implementation to keep honest against the first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeVersion {
    pub present: bool,
    pub valid: Interval,
    pub provenance: Provenance,
}

impl EdgeVersion {
    /// Transaction time: when this assertion entered the store.
    pub fn ingested_at(&self) -> Timestamp {
        self.provenance.observed_at
    }
}

/// A relationship in force at the queried point on both axes.
///
/// Borrowed rather than owned, and carrying its endpoints, so a caller reading
/// a neighbourhood never has to reassemble the key it asked about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge<'a> {
    pub subject: StableId,
    pub predicate: &'a str,
    pub object: StableId,
    pub valid: Interval,
    pub provenance: &'a Provenance,
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
/// Append-only but for the handful of operations that say otherwise. Nothing
/// modifies a [`Version`] or an [`EdgeVersion`] in place; [`MemoryStore::erase`]
/// and [`MemoryStore::erase_edges`] destroy them, and
/// [`MemoryStore::repoint_edges`] moves them onto another entity. All three are
/// deliberately narrow, deliberately hard to reach for, and each documented as
/// doing something to history the rest of the crate exists to prevent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStore {
    entities: BTreeMap<StableId, Entity>,
    next_id: StableId,
    /// subject -> predicate -> object -> version log.
    ///
    /// Nested rather than keyed on a `(subject, predicate, object)` tuple
    /// because `serde_json` cannot use a tuple as a JSON object key — only
    /// scalars stringify. Nesting keeps every level a `BTreeMap`, so snapshots
    /// stay byte-stable, and makes `edges_from` one lookup rather than a scan.
    #[serde(default)]
    edges: BTreeMap<StableId, BTreeMap<String, BTreeMap<StableId, Vec<EdgeVersion>>>>,
    /// object -> the `(subject, predicate)` pairs pointing at it.
    ///
    /// Derived from `edges`, so it is never persisted and is rebuilt inside
    /// [`MemoryStore::open`]. This workspace has learned that rule twice the
    /// hard way: `VectorIndex::positions` shipped persisted and let a restored
    /// index disagree with itself, and `rm_engine`'s blocking map made a
    /// snapshot round-trip change which mentions resolved together. Derived
    /// state that is persisted is derived state that can lie. `#[serde(skip)]`
    /// does not exempt this field from the derived `PartialEq` -- comparing two
    /// stores still compares `into`, which is exactly what makes a round-trip
    /// test a real proof that the rebuild reproduces the live map.
    #[serde(skip)]
    into: BTreeMap<StableId, BTreeSet<(StableId, String)>>,
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
    ///
    /// `supersession` is what this assertion claims about what the slot already
    /// holds, and it is a parameter rather than a default so that every caller
    /// has to have an answer. [`Supersession::Unstated`] is a real answer --
    /// "this host does not know" -- and saying it explicitly is the difference
    /// between a caller that has considered the question and one that has not.
    // Eight arguments, and clippy is right that this is too many. It is
    // allowed rather than fixed because the fix is a parameter struct and
    // that is a wider change than the one in hand -- every caller in the
    // workspace moves with it. Recorded here so the next person to add an
    // argument treats this as the second warning rather than the first.
    #[allow(clippy::too_many_arguments)]
    pub fn assert(
        &mut self,
        id: StableId,
        attribute: impl Into<String>,
        value: Option<String>,
        valid: Interval,
        provenance: Provenance,
        supersession: Supersession,
        according_to: Option<StableId>,
    ) -> Result<(), StoreError> {
        let value_is_absent = value.is_none();
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
                // A tombstone always corrects, whatever the caller passed. "She
                // has no pets" is not one more pet: it is a claim about the
                // whole slot, and there is no reading of it under which the
                // values it lands on top of are still true. The caller is not
                // second-guessed anywhere else -- this is the one case where
                // the value itself settles the question.
                supersession: if value_is_absent {
                    Supersession::Corrects
                } else {
                    supersession
                },
                according_to,
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
        let asserted: Vec<&Candidate<'_>> = candidates
            .iter()
            .filter(|c| c.value.is_assertion())
            .collect();
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
            // Both arms claim [`Supersession::Corrects`], for the same reason:
            // a resolution is by construction the answer that beat the others,
            // so whatever the slot held before it is no longer the store's
            // position. Within a `Timeline` the spans do not correct *each
            // other* -- they share one `observed_at`, so no reader that orders
            // by transaction time can put one after another anyway, and their
            // valid times are disjoint by construction.
            Outcome::Survivor(Some(value)) => self.assert(
                id,
                attribute,
                held_to_value(value),
                Interval::since(earliest),
                latest.clone(),
                Supersession::Corrects,
                // A materialised resolution is the store's own conclusion,
                // not anybody's view.
                None,
            ),
            Outcome::Timeline(facts) => {
                // A contested span has no representation here: `Version.value`
                // is an `Option<String>`, and there is nothing to write for
                // "two values and nothing orders them". A read can be asked
                // about one instant; a materialised resolution cannot, so this
                // refuses whole -- and scans before writing, so a refusal
                // leaves no half-written timeline behind.
                if let Some(fact) = facts
                    .iter()
                    .find(|f| matches!(f.span, Span::Contested { .. }))
                {
                    return Err(Refused(format!(
                        "the span opening at {} is contested, and a resolution written into storage has no way to record two values that nothing orders. Read it with `about` at an instant outside that span, or distinguish the observation times and resolve again.",
                        fact.valid.from
                    ))
                    .into());
                }
                for fact in facts {
                    let Span::Held(value) = fact.span else {
                        unreachable!("contested spans were refused above")
                    };
                    self.assert(
                        id,
                        attribute.clone(),
                        held_to_value(value),
                        fact.valid,
                        latest.clone(),
                        Supersession::Corrects,
                        None,
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

    /// Remove every version of an attribute, returning how many went.
    ///
    /// **This destroys history rather than superseding it**, and it is the only
    /// call that does so for attributes ([`MemoryStore::erase_edges`] is its
    /// counterpart for edges, and neither implies the other). After it, `as_of`
    /// answers `Unknown` for every point on both axes — not `Absent`, because
    /// the store no longer has an opinion rather than holding that there was
    /// none.
    ///
    /// It exists for the request that cannot be answered any other way: someone
    /// asking that a fact about them stop existing. A tombstone is the right
    /// answer to "stop telling me this" and the wrong answer to that, since it
    /// leaves the value legible in `history`.
    ///
    /// Erasing an attribute that was never discussed is not an error — it
    /// reports 0, because the caller's goal ("this must not be here") is already
    /// true. Erasing on an unknown entity *is* an error: the caller is working
    /// from an id that means nothing, and silently succeeding would let a typo
    /// read as a completed deletion.
    pub fn erase(&mut self, id: StableId, attribute: &str) -> Result<usize, StoreError> {
        let entity = self
            .entities
            .get_mut(&id)
            .ok_or(StoreError::UnknownEntity(id))?;
        Ok(entity
            .attributes
            .remove(attribute)
            .map_or(0, |versions| versions.len()))
    }

    /// Record that a relationship held over `valid`.
    ///
    /// Appends; it does not overwrite. Asserting the same triple again is a
    /// correction, and both versions survive so the earlier belief stays
    /// reconstructible — the same rule [`MemoryStore::assert`] follows.
    ///
    /// Both endpoints must exist. A dangling edge is a lie a traversal would
    /// follow without complaint, and since ids are never reused, one that names
    /// nothing today will never name anything.
    pub fn relate(
        &mut self,
        subject: StableId,
        predicate: impl Into<String>,
        object: StableId,
        valid: Interval,
        provenance: Provenance,
    ) -> Result<(), StoreError> {
        self.push_edge(
            subject,
            predicate.into(),
            object,
            EdgeVersion {
                present: true,
                valid,
                provenance,
            },
        )
    }

    /// Record that a relationship stopped holding at `at`.
    ///
    /// Appends a tombstone rather than editing the earlier assertion, so
    /// [`MemoryStore::edge_history`] still shows that it held and who said so.
    /// Ending a relationship is a fact with provenance, not an untraceable edit
    /// — the same distinction `forget` draws for attributes.
    pub fn unrelate(
        &mut self,
        subject: StableId,
        predicate: &str,
        object: StableId,
        at: Timestamp,
        provenance: Provenance,
    ) -> Result<(), StoreError> {
        self.push_edge(
            subject,
            predicate.to_string(),
            object,
            EdgeVersion {
                present: false,
                valid: Interval::since(at),
                provenance,
            },
        )
    }

    /// Validate the endpoints and append one edge version, to both the forward
    /// map and its derived reverse.
    ///
    /// Takes `predicate` by value and clones it once, up front, rather than
    /// letting each map ask for its own copy: the forward map is keyed on it
    /// and the reverse map's `(subject, predicate)` pair needs an owned copy
    /// too, and cloning twice would be paying for the same fact twice.
    fn push_edge(
        &mut self,
        subject: StableId,
        predicate: String,
        object: StableId,
        version: EdgeVersion,
    ) -> Result<(), StoreError> {
        for id in [subject, object] {
            if !self.entities.contains_key(&id) {
                return Err(StoreError::UnknownEntity(id));
            }
        }
        if subject == object {
            return Err(StoreError::SelfEdge(subject));
        }
        let predicate_for_reverse = predicate.clone();
        self.edges
            .entry(subject)
            .or_default()
            .entry(predicate)
            .or_default()
            .entry(object)
            .or_default()
            .push(version);
        self.into
            .entry(object)
            .or_default()
            .insert((subject, predicate_for_reverse));
        Ok(())
    }

    /// Edges out of `subject` in force at `valid_t`, as known by `tx_t`.
    ///
    /// Per triple, the latest-ingested version we had by `tx_t` whose validity
    /// covers `valid_t` — the same resolution [`MemoryStore::as_of`] performs
    /// for an attribute. Ordered by `(predicate, object)` so two runs agree.
    pub fn edges_from(
        &self,
        subject: StableId,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Vec<Edge<'_>> {
        let Some(by_predicate) = self.edges.get(&subject) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (predicate, by_object) in by_predicate {
            for (&object, versions) in by_object {
                if let Some(v) = in_force(versions, valid_t, tx_t) {
                    out.push(Edge {
                        subject,
                        predicate,
                        object,
                        valid: v.valid,
                        provenance: &v.provenance,
                    });
                }
            }
        }
        out
    }

    /// Edges into `object` in force at `valid_t`, as known by `tx_t`.
    ///
    /// The mirror of [`MemoryStore::edges_from`], answering "who works at Acme"
    /// rather than "where does Alice work". Walks the derived reverse map to
    /// find candidate subjects, then re-resolves each triple through
    /// `edge_history` and `in_force` -- the same resolution `edges_from` uses --
    /// rather than caching a resolved answer that could drift from it. Ordered
    /// by `(subject, predicate)` so two runs agree.
    pub fn edges_into(
        &self,
        object: StableId,
        valid_t: Timestamp,
        tx_t: Timestamp,
    ) -> Vec<Edge<'_>> {
        let Some(sources) = self.into.get(&object) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (subject, predicate) in sources {
            let versions = self.edge_history(*subject, predicate, object);
            if let Some(v) = in_force(versions, valid_t, tx_t) {
                // Borrow the predicate from the forward map, not from `into`,
                // so every `Edge` in this result borrows from the same
                // structure with one lifetime -- matching what `edges_from`
                // hands back.
                let key = self
                    .edges
                    .get(subject)
                    .and_then(|m| m.get_key_value(predicate.as_str()))
                    .map(|(k, _)| k.as_str())
                    .expect("the reverse map only ever names predicates the forward map holds");
                out.push(Edge {
                    subject: *subject,
                    predicate: key,
                    object,
                    valid: v.valid,
                    provenance: &v.provenance,
                });
            }
        }
        out
    }

    /// Every version ever asserted for one triple, in append order.
    ///
    /// Empty for a triple never discussed — asking is not an error.
    pub fn edge_history(
        &self,
        subject: StableId,
        predicate: &str,
        object: StableId,
    ) -> &[EdgeVersion] {
        self.edges
            .get(&subject)
            .and_then(|m| m.get(predicate))
            .and_then(|m| m.get(&object))
            .map_or(&[], |v| v.as_slice())
    }

    /// Remove every edge touching `entity`, in both directions, returning how
    /// many triples went.
    ///
    /// **Destructive, like [`MemoryStore::erase`].** It removes the history, not
    /// just the relationship — for "they no longer work there", use
    /// [`MemoryStore::unrelate`], which keeps the record that it once held.
    ///
    /// Deliberately separate from `erase`, and neither implies the other. A
    /// caller reaching for either is usually answering a deletion request and
    /// needs to know exactly what was removed; a convenience that quietly did
    /// both would make that question unanswerable.
    ///
    /// Erasing edges on an entity that holds none is not an error — it reports
    /// `Ok(0)`, because the caller's goal ("these must not be here") is already
    /// true. An id the store does not hold *is* an error, exactly as it is for
    /// [`MemoryStore::erase`]: the caller is working from an id that means
    /// nothing, and a bare 0 would let a typo read as a completed deletion.
    /// That distinction is the whole reason this call reports at all, so the two
    /// deletion doors answer a mistake the same way rather than two ways.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownEntity`] if `entity` names no entity in this store.
    pub fn erase_edges(&mut self, entity: StableId) -> Result<usize, StoreError> {
        if !self.entities.contains_key(&entity) {
            return Err(StoreError::UnknownEntity(entity));
        }
        let mut removed = 0;

        // Outgoing: drop the whole subtree, then unhook each object from the
        // reverse map. Dropping it first is what stops the incoming pass below
        // from finding, and counting a second time, an edge already gone.
        if let Some(by_predicate) = self.edges.remove(&entity) {
            for (predicate, by_object) in by_predicate {
                for object in by_object.into_keys() {
                    removed += 1;
                    self.unhook_reverse(entity, &predicate, object);
                }
            }
        }

        // Incoming: the reverse map names exactly who points here, so this is a
        // handful of lookups rather than a scan of every subject in the store.
        if let Some(sources) = self.into.remove(&entity) {
            for (subject, predicate) in sources {
                if let Some(by_object) = self
                    .edges
                    .get_mut(&subject)
                    .and_then(|m| m.get_mut(&predicate))
                {
                    if by_object.remove(&entity).is_some() {
                        removed += 1;
                    }
                }
                self.prune_empty(subject, &predicate);
            }
        }
        Ok(removed)
    }

    /// Move every edge touching `from` onto `to`, returning how many left
    /// `from`.
    ///
    /// For a merge: two entities turned out to be one, so everything said about
    /// the absorbed id is now said about the survivor. An edge the move would
    /// turn into a self-edge is dropped rather than stored, because
    /// [`MemoryStore::relate`] refuses to create one and two answers to the same
    /// question is worse than either.
    ///
    /// Where both entities already held the same relationship, the version logs
    /// are concatenated and re-sorted by ingestion — neither source's assertion
    /// is discarded, and the latest-ingested rule still picks the same winner it
    /// would have picked had both been asserted about one entity all along.
    ///
    /// The count includes the edges dropped as self-edges, because it answers
    /// "how many relationships stopped being `from`'s", which is what a caller
    /// reporting a merge has to say. Counting only the survivors would make
    /// "there was nothing to move" and "everything collapsed into the survivor"
    /// the same number.
    ///
    /// Merging an entity into itself is a no-op reporting `Ok(0)` rather than an
    /// error: both ids are real and the caller's goal is already true, which is
    /// the same reading `erase` gives an attribute nobody ever discussed.
    ///
    /// # Errors
    ///
    /// [`StoreError::UnknownEntity`] if either id names no entity in this store.
    /// `relate` rejects an endpoint naming no entity and [`MemoryStore::open`]
    /// rejects a snapshot holding one, so moving edges onto one would write a
    /// store that cannot be reopened — damage found at restore, long after the
    /// call that caused it. Reporting 0 instead would be no better: it is what
    /// "there was nothing to move" says, so a caller could not tell a merge that
    /// found nothing from one aimed at an id that never existed.
    pub fn repoint_edges(&mut self, from: StableId, to: StableId) -> Result<usize, StoreError> {
        for id in [from, to] {
            if !self.entities.contains_key(&id) {
                return Err(StoreError::UnknownEntity(id));
            }
        }
        // Checked after the endpoints, so merging an unknown id into itself is
        // still the error it is anywhere else. Caught here rather than left to
        // fall through: every edge on `from` would also be an edge on `to`, so
        // the self-edge rule below would read the whole neighbourhood as
        // collapsing and delete it.
        if from == to {
            return Ok(0);
        }
        let mut moved = 0;

        // Outgoing: take the subtree off `from` and re-file each triple under
        // `to`, unhooking the reverse entry that named `from` as its subject.
        if let Some(by_predicate) = self.edges.remove(&from) {
            for (predicate, by_object) in by_predicate {
                for (object, versions) in by_object {
                    self.unhook_reverse(from, &predicate, object);
                    moved += 1;
                    if object == to {
                        continue; // `to` -> `to`; see the self-edge note above
                    }
                    self.absorb_edge(to, predicate.clone(), object, versions);
                }
            }
        }

        // Incoming: the same move from the other side. Taking the reverse entry
        // for `from` out of the map first means the loop cannot re-read a
        // pairing it has already rewritten.
        if let Some(sources) = self.into.remove(&from) {
            for (subject, predicate) in sources {
                let versions = self
                    .edges
                    .get_mut(&subject)
                    .and_then(|m| m.get_mut(&predicate))
                    .and_then(|m| m.remove(&from));
                self.prune_empty(subject, &predicate);
                // A reverse entry with no forward edge behind it is a broken
                // invariant, not a move: skip it rather than counting it.
                let Some(versions) = versions else { continue };
                moved += 1;
                if subject == to {
                    continue; // `to` -> `to`; see the self-edge note above
                }
                self.absorb_edge(subject, predicate, to, versions);
            }
        }
        Ok(moved)
    }

    /// Add one triple's versions to the forward map and its reverse.
    ///
    /// Where the destination already holds the triple the two logs are
    /// concatenated and re-sorted by ingestion, because "append order" across
    /// two entities' logs is not an order anyone appended in. The sort is
    /// *stable*, so versions sharing an ingestion time keep their relative
    /// order and `in_force`'s latest-ingested tie-break stays deterministic.
    ///
    /// An empty destination takes the log exactly as it stood rather than a
    /// sorted copy of it, so a move that collides with nothing leaves
    /// [`MemoryStore::edge_history`] in the append order it documents. Sorting
    /// there would be harmless to every query — the winner is the largest
    /// `ingested_at` however the vector is ordered — but it would silently
    /// reorder an audit trail nobody asked to merge.
    fn absorb_edge(
        &mut self,
        subject: StableId,
        predicate: String,
        object: StableId,
        versions: Vec<EdgeVersion>,
    ) {
        let log = self
            .edges
            .entry(subject)
            .or_default()
            .entry(predicate.clone())
            .or_default()
            .entry(object)
            .or_default();
        if log.is_empty() {
            *log = versions;
        } else {
            log.extend(versions);
            log.sort_by_key(|v| v.ingested_at());
        }
        self.into
            .entry(object)
            .or_default()
            .insert((subject, predicate));
    }

    /// Take one `(subject, predicate)` pairing out of the reverse map, dropping
    /// the entry if it was the last one pointing at `object`.
    ///
    /// An emptied entry is pruned rather than left: `into` is compared by the
    /// derived `PartialEq`, and [`MemoryStore::open`] rebuilds it from the
    /// forward map with no empty entries at all, so leaving one makes a store
    /// unequal to its own round trip over a difference no query can see.
    fn unhook_reverse(&mut self, subject: StableId, predicate: &str, object: StableId) {
        if let Some(sources) = self.into.get_mut(&object) {
            // The set owns its predicate, so removing means building the whole
            // key. Borrowing one from the forward map instead would tie the
            // reverse map's lifetime to the forward one, which is exactly what
            // keeping `into` a plain owned field avoids.
            sources.remove(&(subject, predicate.to_string()));
            if sources.is_empty() {
                self.into.remove(&object);
            }
        }
    }

    /// Drop a predicate or subject level left empty by a removal, so an empty
    /// map never appears in a snapshot.
    ///
    /// Not cosmetic: an empty map is bytes in every later snapshot and a line in
    /// every later diff, and this crate sells snapshots as diffable. It also
    /// makes an entity that never had an edge indistinguishable from one whose
    /// edges were all removed, which is a difference nothing downstream should
    /// have to reason about.
    fn prune_empty(&mut self, subject: StableId, predicate: &str) {
        if let Some(by_predicate) = self.edges.get_mut(&subject) {
            if by_predicate.get(predicate).is_some_and(|m| m.is_empty()) {
                by_predicate.remove(predicate);
            }
            if by_predicate.is_empty() {
                self.edges.remove(&subject);
            }
        }
    }

    /// Serialise to canonical JSON.
    ///
    /// `BTreeMap` ordering makes this byte-stable for a given state, so two
    /// stores can be compared by diffing snapshots.
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(self).expect("MemoryStore is always serialisable")
    }

    /// Restore from a snapshot.
    ///
    /// `next_id` is checked rather than trusted. It names the *next* id to hand
    /// out, so a snapshot carrying it at or below an id already in use is one
    /// where the very next [`MemoryStore::create_entity`] returns a live id and
    /// `entities.insert` overwrites an existing entity — silently, since
    /// inserting over a `BTreeMap` key is not an error anywhere. An entity and
    /// every version it held would simply stop existing, with nothing returning
    /// `Err` and no count moving.
    ///
    /// Checking it here rather than in each caller is deliberate: the counter is
    /// this type's invariant, so this is the only place that can enforce it for
    /// everyone who restores a store. The bound is one-directional on purpose —
    /// a `next_id` *above* the highest live id wastes id space but cannot
    /// collide, and rejecting it would break any caller that reserves ranges.
    ///
    /// The edge map is held to everything the write path enforces: both
    /// endpoints must exist, no edge may start and end at the same entity, no
    /// triple may carry an empty version log, and no subject or predicate level
    /// may be left empty. The first three are what [`MemoryStore::relate`]
    /// refuses outright; the last is what `prune_empty` removes after every
    /// in-process removal. Accepting here what the front door rejects would
    /// make a snapshot a way to build a store no sequence of calls could, and
    /// the damage would surface as a wrong answer from a later query rather
    /// than as an error from the restore.
    ///
    /// # Errors
    ///
    /// [`StoreError::Parse`] if the bytes are not a store at all, and
    /// [`StoreError::CorruptSnapshot`] if they are a store whose own invariants
    /// contradict each other.
    pub fn open(snapshot: &str) -> Result<Self, StoreError> {
        let mut store: MemoryStore =
            serde_json::from_str(snapshot).map_err(|e| StoreError::Parse(e.into()))?;

        // Keyed on the map, not on `Entity::id`: `create_entity` inserts at
        // `next_id`, so the keys are what a reissued id would collide with.
        if let Some(&highest) = store.entities.keys().next_back() {
            if store.next_id <= highest {
                return Err(StoreError::CorruptSnapshot(format!(
                    "next_id is {}, but entity {highest} exists, so the next create_entity would overwrite it",
                    store.next_id
                )));
            }
        }

        // The edge map has to satisfy everything the write path enforces. An
        // edge that parses cleanly and then breaks an invariant is worse than
        // one that fails to parse: the store answers queries and looks healthy.
        // The restore path is a door, and a door that accepts what the front
        // door refuses is not a door -- so each check below names the call it
        // is standing in for.
        let mut reverse: BTreeMap<StableId, BTreeSet<(StableId, String)>> = BTreeMap::new();
        for (&subject, by_predicate) in &store.edges {
            if !store.entities.contains_key(&subject) {
                return Err(StoreError::CorruptSnapshot(format!(
                    "an edge starts at entity {subject}, which the snapshot does not hold"
                )));
            }
            // `prune_empty` deletes a subject whose last predicate went, so a
            // snapshot carrying one was not written by this crate's own
            // removals. Rejecting it rather than pruning it on the way in is
            // deliberate: silently repairing a snapshot means the store a
            // caller restores is not the store they saved, and the difference
            // never gets reported.
            if by_predicate.is_empty() {
                return Err(StoreError::CorruptSnapshot(format!(
                    "entity {subject} holds an edge map with no predicates in it, which prune_empty removes rather than stores"
                )));
            }
            for (predicate, by_object) in by_predicate {
                if by_object.is_empty() {
                    return Err(StoreError::CorruptSnapshot(format!(
                        "entity {subject}'s {predicate:?} names no object, which prune_empty removes rather than stores"
                    )));
                }
                for (&object, versions) in by_object {
                    if !store.entities.contains_key(&object) {
                        return Err(StoreError::CorruptSnapshot(format!(
                            "an edge from entity {subject} points at entity {object}, which the snapshot does not hold"
                        )));
                    }
                    // `push_edge` refuses a self-edge, so `relate` and
                    // `unrelate` cannot make one and `confirm` drops the ones a
                    // merge creates. Restoring one would leave `edges_from`
                    // handing a traversal an edge no call in this crate could
                    // have written, and every walk crossing it would arrive
                    // back where it started.
                    if subject == object {
                        return Err(StoreError::CorruptSnapshot(format!(
                            "an edge runs from entity {subject} to itself, which relate refuses to create"
                        )));
                    }
                    // A triple with no versions has never been asserted, so
                    // `edge_history` cannot distinguish it from one nobody has
                    // discussed -- and it still earns a reverse-map entry,
                    // which makes `edges_into` walk a triple that says nothing.
                    if versions.is_empty() {
                        return Err(StoreError::CorruptSnapshot(format!(
                            "the edge from entity {subject} to entity {object} via {predicate:?} carries no versions, so nothing was ever asserted about it"
                        )));
                    }
                    reverse
                        .entry(object)
                        .or_default()
                        .insert((subject, predicate.clone()));
                }
            }
        }
        store.into = reverse;

        Ok(store)
    }
}

/// A survived value in this store's own convention: `None` is a tombstone, not
/// a missing observation. This is the exact mapping [`Held`] exists for —
/// `Held::Absent` is a positive claim of emptiness, which is what `None` means
/// here, and `Held::Value` is a known value.
fn held_to_value(held: Held) -> Option<String> {
    match held {
        Held::Value(v) => Some(v),
        Held::Absent => None,
    }
}

/// The version in force at a point on both axes, if the relationship held.
///
/// Latest ingested wins among versions covering `valid_t`, breaking ingestion
/// ties toward the later append — the same rule, and the same tie-break, as
/// `as_of` uses for attributes. A winning tombstone yields `None`: the
/// relationship was considered and found not to hold, which is the answer.
fn in_force(versions: &[EdgeVersion], valid_t: Timestamp, tx_t: Timestamp) -> Option<&EdgeVersion> {
    versions
        .iter()
        .enumerate()
        .filter(|(_, v)| v.ingested_at() <= tx_t && v.valid.contains(valid_t))
        .max_by_key(|(i, v)| (v.ingested_at(), *i))
        .map(|(_, v)| v)
        .filter(|v| v.present)
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
            Supersession::Unstated,
            None,
        )
        .unwrap();
        s.assert(
            id,
            "employer",
            Some("Globex".into()),
            Interval::since(JUL),
            user_said(SEP),
            Supersession::Unstated,
            None,
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
            Supersession::Unstated,
            None,
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

        s.assert(
            id,
            "employer",
            None,
            Interval::since(JUL),
            user_said(JUL),
            Supersession::Unstated,
            None,
        )
        .unwrap();
        // "I'm unemployed" is an answer; it must not read as "never discussed".
        assert_eq!(s.current(id, "employer", OCT), Known::Absent);
        assert!(s.current(id, "employer", OCT).is_known());
        assert!(!s.current(id, "spouse", OCT).is_known());
        // Both yield None when the caller genuinely does not care which.
        assert_eq!(s.current(id, "employer", OCT).value(), None);
    }

    #[test]
    fn a_tombstone_corrects_whatever_the_caller_claimed() {
        // The one place the store overrules its caller. "She has no pets" is
        // not one more pet -- it is a claim about the whole slot, and there is
        // no reading of it under which the values beneath it survive. A host
        // that passed `Joins` here has made a mistake the store can see.
        let mut s = MemoryStore::new();
        let id = s.create_entity("person", JAN);
        s.assert(
            id,
            "pet",
            Some("a dog".into()),
            Interval::since(JAN),
            user_said(JAN),
            Supersession::Joins,
            None,
        )
        .unwrap();
        s.assert(
            id,
            "pet",
            None,
            Interval::since(JUL),
            user_said(JUL),
            Supersession::Joins,
            None,
        )
        .unwrap();

        let h = s.history(id, "pet");
        assert_eq!(h[0].supersession, Supersession::Joins, "the dog joins");
        assert_eq!(
            h[1].supersession,
            Supersession::Corrects,
            "the tombstone corrects, whatever it was asked to claim"
        );
    }

    #[test]
    fn a_second_value_keeps_the_claim_it_was_given() {
        // The complement of the test above, and the whole point of the field:
        // two pets are two pets. Nothing here reads the claim -- `as_of` still
        // answers with one value -- but it survives to the reader that does.
        let mut s = MemoryStore::new();
        let id = s.create_entity("person", JAN);
        for (v, t, claim) in [
            ("a dog", JAN, Supersession::Unstated),
            ("a cat", JUL, Supersession::Joins),
        ] {
            s.assert(
                id,
                "pet",
                Some(v.into()),
                Interval::since(t),
                user_said(t),
                claim,
                None,
            )
            .unwrap();
        }
        let h = s.history(id, "pet");
        assert_eq!(h[0].supersession, Supersession::Unstated);
        assert_eq!(h[1].supersession, Supersession::Joins);

        // And `as_of` is unchanged by any of it. A caller that asks for one
        // value is still given one, still the latest by arrival. The claim is
        // for readers who can hold more than one answer; this one cannot.
        assert_eq!(s.current(id, "pet", OCT).value(), Some("a cat"));
    }

    #[test]
    fn a_snapshot_written_before_the_field_existed_reads_as_unstated() {
        // Not `Joins`, which would retroactively un-correct every correction
        // ever stored, and not `Corrects`, which is the inference this field
        // exists to stop making. The stored assertion answered no question, so
        // it goes on answering none.
        let mut s = MemoryStore::new();
        let id = s.create_entity("person", JAN);
        s.assert(
            id,
            "employer",
            Some("Acme".into()),
            Interval::since(JAN),
            user_said(JAN),
            Supersession::Corrects,
            None,
        )
        .unwrap();

        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains("corrects"),
            "a claim that says something is written: {json}"
        );

        // The same snapshot with the field cut out, which is exactly what every
        // store written before this change looks like.
        let old = json.replace(r#","supersession":"corrects""#, "");
        let restored: MemoryStore = serde_json::from_str(&old).unwrap();
        assert_eq!(
            restored.history(id, "employer")[0].supersession,
            Supersession::Unstated
        );
    }

    #[test]
    fn an_unstated_claim_is_not_written_at_all() {
        // Snapshots run to tens of megabytes and are meant to be diffable, so
        // the state that means "nothing to say" says nothing. It also means a
        // pre-existing snapshot survives a round-trip through the new shape
        // byte for byte, rather than acquiring a field it never had.
        let mut s = MemoryStore::new();
        let id = s.create_entity("person", JAN);
        s.assert(
            id,
            "employer",
            Some("Acme".into()),
            Interval::since(JAN),
            user_said(JAN),
            Supersession::Unstated,
            None,
        )
        .unwrap();

        let json = serde_json::to_string(&s).unwrap();
        assert!(
            !json.contains("supersession"),
            "the default writes nothing: {json}"
        );
        let restored: MemoryStore = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, s, "and it comes back the same store");
    }

    #[test]
    fn a_tombstone_can_itself_be_superseded() {
        let (mut s, id) = store_with_user();
        s.assert(
            id,
            "employer",
            None,
            Interval::since(JAN),
            user_said(MAR),
            Supersession::Unstated,
            None,
        )
        .unwrap();
        s.assert(
            id,
            "employer",
            Some("Globex".into()),
            Interval::since(JUL),
            user_said(SEP),
            Supersession::Unstated,
            None,
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
                Supersession::Unstated,
                None,
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

    // ---- erase --------------------------------------------------------------

    #[test]
    fn erase_removes_history_where_a_tombstone_only_supersedes_it() {
        let (mut store, user) = store_with_user();
        store
            .assert(
                user,
                "employer",
                Some("Acme".into()),
                Interval::since(JAN),
                user_said(JAN),
                Supersession::Unstated,
                None,
            )
            .unwrap();
        store
            .assert(
                user,
                "employer",
                None,
                Interval::since(JUL),
                user_said(JUL),
                Supersession::Unstated,
                None,
            )
            .unwrap();

        // Before erasing: the tombstone wins now, and January is still answerable.
        assert_eq!(store.as_of(user, "employer", AUG, AUG), Known::Absent);
        assert_eq!(
            store.as_of(user, "employer", MAR, AUG),
            Known::Value("Acme")
        );

        assert_eq!(store.erase(user, "employer").unwrap(), 2);

        // After: not superseded, gone. The store has no opinion at any point.
        assert_eq!(store.as_of(user, "employer", MAR, AUG), Known::Unknown);
        assert!(store.history(user, "employer").is_empty());
    }

    #[test]
    fn erasing_something_never_discussed_reports_zero_rather_than_failing() {
        let (mut store, user) = store_with_user();
        assert_eq!(store.erase(user, "employer").unwrap(), 0);
    }

    #[test]
    fn erasing_on_an_unknown_entity_is_an_error() {
        let (mut store, _) = store_with_user();
        assert_eq!(
            store.erase(9999, "employer"),
            Err(StoreError::UnknownEntity(9999))
        );
    }

    // ---- edges --------------------------------------------------------------

    #[test]
    fn an_edge_is_bitemporal_like_an_attribute() {
        // Told in September that they joined in July: the edge is valid from
        // July, but we did not know it in August.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JUL),
                user_said(SEP),
            )
            .unwrap();

        assert_eq!(
            store.edges_from(user, AUG, AUG).len(),
            0,
            "not known in August"
        );
        assert_eq!(
            store.edges_from(user, AUG, OCT).len(),
            1,
            "known by October, true in August"
        );
        assert_eq!(
            store.edges_from(user, MAR, OCT).len(),
            0,
            "not true in March"
        );
    }

    #[test]
    fn unrelate_stops_the_edge_without_erasing_that_it_held() {
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        store
            .unrelate(user, "employed_by", acme, JUL, user_said(JUL))
            .unwrap();

        assert_eq!(
            store.edges_from(user, MAR, OCT).len(),
            1,
            "it held in March"
        );
        assert_eq!(
            store.edges_from(user, AUG, OCT).len(),
            0,
            "and not in August"
        );
        assert_eq!(
            store.edge_history(user, "employed_by", acme).len(),
            2,
            "ending a relationship is a fact, not an erasure"
        );
    }

    #[test]
    fn two_employers_at_once_both_stand() {
        // Arrival does not entail departure. Closing the first edge is an
        // inference, and the store records what was said rather than inferring.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        let globex = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        store
            .relate(
                user,
                "employed_by",
                globex,
                Interval::since(JUL),
                user_said(JUL),
            )
            .unwrap();

        assert_eq!(store.edges_from(user, AUG, OCT).len(), 2);
    }

    #[test]
    fn an_edge_naming_an_unknown_entity_is_rejected() {
        let (mut store, user) = store_with_user();
        assert_eq!(
            store.relate(
                user,
                "employed_by",
                9999,
                Interval::since(JAN),
                user_said(JAN)
            ),
            Err(StoreError::UnknownEntity(9999))
        );
        assert_eq!(
            store.relate(
                9999,
                "employed_by",
                user,
                Interval::since(JAN),
                user_said(JAN)
            ),
            Err(StoreError::UnknownEntity(9999))
        );
    }

    #[test]
    fn an_edge_from_an_entity_to_itself_is_rejected() {
        // It carries nothing a walk can use, and a merge drops the self-edges it
        // creates -- accepting one at the front door while dropping it at the
        // back would be two answers to the same question.
        let (mut store, user) = store_with_user();
        assert!(store
            .relate(user, "knows", user, Interval::since(JAN), user_said(JAN))
            .is_err());
    }

    #[test]
    fn a_later_assertion_about_the_same_edge_supersedes_the_earlier_one() {
        // Same key, and both versions cover August, so the latest ingested wins
        // -- exactly as for an attribute (see
        // `a_tombstone_can_itself_be_superseded`). A merely *narrower* `relate`
        // would not do this: presence is not read as an implicit end (see
        // `two_employers_at_once_both_stand`), so for the later version to
        // actually govern August it has to say something about August -- here,
        // that the relationship did not hold.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        store
            .unrelate(user, "employed_by", acme, JAN, user_said(JUL))
            .unwrap();

        assert_eq!(
            store.edges_from(user, AUG, OCT).len(),
            0,
            "the correction superseded it"
        );
        assert_eq!(
            store.edges_from(user, AUG, MAR).len(),
            1,
            "but we believed otherwise in March"
        );
    }

    // ---- the reverse edge index --------------------------------------------

    #[test]
    fn edges_into_finds_what_points_at_an_entity() {
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        let other = store.create_entity("person", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        store
            .relate(
                other,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        let into = store.edges_into(acme, MAR, OCT);
        assert_eq!(into.len(), 2, "who works at Acme");
        assert!(into.iter().all(|e| e.object == acme));
        assert_eq!(store.edges_into(user, MAR, OCT).len(), 0);
    }

    #[test]
    fn edges_into_respects_both_time_axes_the_same_way_edges_from_does() {
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JUL),
                user_said(SEP),
            )
            .unwrap();
        assert_eq!(
            store.edges_into(acme, AUG, AUG).len(),
            0,
            "not known in August"
        );
        assert_eq!(store.edges_into(acme, AUG, OCT).len(), 1);
        assert_eq!(
            store.edges_into(acme, MAR, OCT).len(),
            0,
            "not true in March"
        );
    }

    #[test]
    fn a_snapshot_round_trips_edges_and_rebuilds_the_reverse_map() {
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        let restored = MemoryStore::open(&store.snapshot()).unwrap();
        assert_eq!(restored.edges_from(user, MAR, OCT).len(), 1);
        assert_eq!(
            restored.edges_into(acme, MAR, OCT).len(),
            1,
            "the reverse map is derived, so it has to be rebuilt to work"
        );
        assert_eq!(restored, store);
    }

    #[test]
    fn a_snapshot_is_not_bloated_by_the_derived_reverse_map() {
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        assert!(
            !store.snapshot().contains("into"),
            "derived state that is persisted is derived state that can lie"
        );
    }

    /// Assert a restore was refused for the reason the test is about.
    ///
    /// The variant alone is not enough: `open` runs its checks in a fixed
    /// order, so a reordering could let a snapshot trip a *different* check and
    /// still satisfy `matches!(err, CorruptSnapshot(_))` -- the test would keep
    /// passing while no longer testing what its name says.
    #[track_caller]
    fn refused_because(err: &StoreError, expected: &str) {
        let StoreError::CorruptSnapshot(why) = err else {
            panic!("expected a CorruptSnapshot, got {err:?}");
        };
        assert!(
            why.contains(expected),
            "refused for the wrong reason
  expected to contain: {expected}
  actual: {why}"
        );
    }

    #[test]
    fn a_snapshot_whose_edge_names_an_unknown_entity_is_rejected() {
        // Built by mutating the parsed JSON, not by a string replace on the
        // snapshot text: entity ids are small integers, and a text replace
        // targeting `"<id>":[` can match more than the one edge-object key it
        // means to (or nothing at all if the id also appears elsewhere), which
        // would make the test pass or fail for the wrong reason. Renaming the
        // key in the parsed document is unambiguous, and asserting the removal
        // actually found something rules out the mutation being a silent no-op.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        let mut doc: serde_json::Value = serde_json::from_str(&store.snapshot()).unwrap();
        let by_object = doc["edges"][user.to_string()]["employed_by"]
            .as_object_mut()
            .expect("setup: the forward map holds subject -> predicate -> object -> versions");
        let versions = by_object.remove(&acme.to_string());
        assert!(
            versions.is_some(),
            "the mutation must actually remove a real edge-object entry, or this test is vacuous"
        );
        let clash = by_object.insert("4242".to_string(), versions.unwrap());
        assert!(clash.is_none(), "setup: 4242 must not already be a key");

        let err = MemoryStore::open(&doc.to_string()).unwrap_err();
        refused_because(&err, "points at entity");
    }

    #[test]
    fn a_snapshot_whose_edge_starts_at_an_unknown_entity_is_rejected() {
        // The mirror of the object-side test above, and it exists because the
        // two are separate `if` blocks rather than one shared check: an
        // inverted condition in the subject branch would restore a store whose
        // `edges_into` hands a walk a subject that does not exist, and the
        // object-side test would stay green throughout. Same JSON-mutation
        // approach and for the same reason -- renaming a key in the parsed
        // document cannot match more or less than the one entry it means to.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        let mut doc: serde_json::Value = serde_json::from_str(&store.snapshot()).unwrap();
        let edges = doc["edges"]
            .as_object_mut()
            .expect("setup: the forward map is keyed by subject");
        let by_predicate = edges.remove(&user.to_string());
        assert!(
            by_predicate.is_some(),
            "the mutation must actually remove a real subject entry, or this test is vacuous"
        );
        let clash = edges.insert("4242".to_string(), by_predicate.unwrap());
        assert!(clash.is_none(), "setup: 4242 must not already be a key");

        let err = MemoryStore::open(&doc.to_string()).unwrap_err();
        refused_because(&err, "starts at entity");
    }

    #[test]
    fn a_snapshot_carrying_a_self_edge_is_rejected_not_restored() {
        // `push_edge` refuses one at the front door and `confirm` drops the
        // ones a merge creates, so a restore that accepted one would be the
        // only way into a state the rest of the crate is built to prevent.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        let mut doc: serde_json::Value = serde_json::from_str(&store.snapshot()).unwrap();
        let by_object = doc["edges"][user.to_string()]["employed_by"]
            .as_object_mut()
            .expect("setup: the forward map holds subject -> predicate -> object -> versions");
        let versions = by_object
            .remove(&acme.to_string())
            .expect("setup: the edge to acme is there to re-point");
        // Re-pointed at the subject itself, which is the exact shape a merge
        // would have produced and then dropped.
        by_object.insert(user.to_string(), versions);

        let err = MemoryStore::open(&doc.to_string()).unwrap_err();
        refused_because(&err, "to itself");
    }

    #[test]
    fn a_snapshot_carrying_an_edge_with_no_versions_is_rejected_not_restored() {
        // An empty log is indistinguishable from a triple nobody ever discussed
        // as far as `edge_history` is concerned, but it still earns a
        // reverse-map entry -- so `edges_into` would walk a triple that says
        // nothing, and the two maps would disagree about what exists.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        let mut doc: serde_json::Value = serde_json::from_str(&store.snapshot()).unwrap();
        let log = &mut doc["edges"][user.to_string()]["employed_by"][acme.to_string()];
        assert!(
            log.as_array().is_some_and(|v| !v.is_empty()),
            "setup: the triple starts with a version in it"
        );
        *log = serde_json::json!([]);

        let err = MemoryStore::open(&doc.to_string()).unwrap_err();
        refused_because(&err, "carries no versions");
    }

    #[test]
    fn a_snapshot_carrying_an_empty_edge_level_is_rejected_not_restored() {
        // Both levels, because `prune_empty` removes both and a snapshot is the
        // only way either could come back. An empty level is bytes in every
        // later snapshot and a line in every later diff, and it makes an entity
        // that never had an edge indistinguishable from one whose edges all
        // went -- the two differences `prune_empty` exists to prevent.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        let snapshot = store.snapshot();

        let mut predicate_level: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        let level = &mut predicate_level["edges"][user.to_string()]["employed_by"];
        assert!(
            level.as_object().is_some_and(|m| !m.is_empty()),
            "setup: the predicate starts with an object under it, or emptying it proves nothing"
        );
        *level = serde_json::json!({});
        let err = MemoryStore::open(&predicate_level.to_string()).unwrap_err();
        refused_because(&err, "names no object");

        let mut subject_level: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        let level = &mut subject_level["edges"][user.to_string()];
        assert!(
            level.as_object().is_some_and(|m| !m.is_empty()),
            "setup: the subject starts with a predicate under it, or emptying it proves nothing"
        );
        *level = serde_json::json!({});
        let err = MemoryStore::open(&subject_level.to_string()).unwrap_err();
        refused_because(&err, "no predicates in it");
    }

    // ---- wholesale edge surgery ---------------------------------------------

    /// Assert the two edge maps name exactly the same triples, and that no
    /// level of either was left empty by a removal.
    ///
    /// The failure this exists to catch is silent. A forward edge with no
    /// reverse entry still answers `edges_from` and has simply stopped existing
    /// as far as `edges_into` is concerned; a reverse entry with no forward edge
    /// behind it trips the `expect` in `edges_into`, but only if a query happens
    /// to walk that one entry. Both survive a round of green tests easily, so
    /// every test that mutates edges wholesale ends here.
    #[track_caller]
    fn assert_maps_agree(store: &MemoryStore) {
        let mut expected: BTreeMap<StableId, BTreeSet<(StableId, String)>> = BTreeMap::new();
        for (subject, by_predicate) in &store.edges {
            assert!(
                !by_predicate.is_empty(),
                "subject {subject} kept an empty predicate map"
            );
            for (predicate, by_object) in by_predicate {
                assert!(
                    !by_object.is_empty(),
                    "{subject}'s {predicate:?} kept an empty object map"
                );
                for (object, versions) in by_object {
                    assert!(
                        !versions.is_empty(),
                        "the triple ({subject}, {predicate:?}, {object}) kept an empty log"
                    );
                    expected
                        .entry(*object)
                        .or_default()
                        .insert((*subject, predicate.clone()));
                }
            }
        }
        assert_eq!(
            store.into, expected,
            "the reverse map must name exactly the forward edges that exist"
        );
    }

    /// The ingestion timestamps of a version log, in the order the log holds
    /// them. Names what a test means by "the order", so an assertion about it
    /// reads as a claim about ordering rather than as a bare vector of numbers.
    fn ingestion_order(versions: &[EdgeVersion]) -> Vec<Timestamp> {
        versions.iter().map(|v| v.ingested_at()).collect()
    }

    #[test]
    fn erase_edges_does_not_touch_attributes_and_erase_does_not_touch_edges() {
        // A caller reaching for either is usually answering a deletion request
        // and needs to know exactly what went. A call that quietly did both
        // would make that question unanswerable.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .assert(
                user,
                "employer",
                Some("Acme".into()),
                Interval::since(JAN),
                user_said(JAN),
                Supersession::Unstated,
                None,
            )
            .unwrap();
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        assert_eq!(store.erase(user, "employer").unwrap(), 1);
        assert_eq!(
            store.edges_from(user, MAR, OCT).len(),
            1,
            "erase left the edge"
        );

        assert_eq!(store.erase_edges(user).unwrap(), 1);
        assert_eq!(store.edges_from(user, MAR, OCT).len(), 0);
        assert_maps_agree(&store);
    }

    #[test]
    fn erase_edges_removes_both_directions() {
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        assert_eq!(
            store.erase_edges(acme).unwrap(),
            1,
            "erasing the object clears it too"
        );
        assert_eq!(store.edges_from(user, MAR, OCT).len(), 0);
        assert_eq!(store.edges_into(acme, MAR, OCT).len(), 0);
        assert_maps_agree(&store);
    }

    #[test]
    fn erasing_the_last_edge_leaves_no_empty_map_behind_in_the_snapshot() {
        // The subject and predicate levels are removed with their last child,
        // so the snapshot of a store whose edges are gone is the snapshot of a
        // store that never had any -- byte-identical, and diffable against it.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        let before = store.snapshot();
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        assert_eq!(store.erase_edges(acme).unwrap(), 1);
        assert_eq!(store.snapshot(), before);
        assert_maps_agree(&store);
    }

    #[test]
    fn repointing_moves_edges_in_both_directions() {
        let (mut store, kept) = store_with_user();
        let absorbed = store.create_entity("person", JAN);
        let acme = store.create_entity("org", JAN);
        let boss = store.create_entity("person", JAN);
        store
            .relate(
                absorbed,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        store
            .relate(
                boss,
                "manages",
                absorbed,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        assert_eq!(store.repoint_edges(absorbed, kept).unwrap(), 2);
        assert_eq!(store.edges_from(kept, MAR, OCT).len(), 1);
        assert_eq!(store.edges_into(kept, MAR, OCT).len(), 1);
        assert_eq!(store.edges_from(absorbed, MAR, OCT).len(), 0);
        assert_eq!(store.edges_into(absorbed, MAR, OCT).len(), 0);
        assert_maps_agree(&store);
    }

    #[test]
    fn repointing_drops_an_edge_that_would_become_a_self_edge() {
        // The two entities turned out to be one, so "A manages B" is now "A
        // manages A" -- which relate() refuses to create, so it must not be
        // creatable this way either.
        let (mut store, kept) = store_with_user();
        let absorbed = store.create_entity("person", JAN);
        store
            .relate(
                kept,
                "manages",
                absorbed,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        store.repoint_edges(absorbed, kept).unwrap();
        assert_eq!(store.edges_from(kept, MAR, OCT).len(), 0);
        assert_eq!(store.edges_into(kept, MAR, OCT).len(), 0);
        assert_maps_agree(&store);
    }

    #[test]
    fn repointing_merges_history_when_both_entities_held_the_same_edge() {
        // Both knew about Acme. After the merge that is one relationship with
        // two sources, not one that overwrote the other.
        let (mut store, kept) = store_with_user();
        let absorbed = store.create_entity("person", JAN);
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                kept,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        store
            .relate(
                absorbed,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(MAR),
            )
            .unwrap();

        store.repoint_edges(absorbed, kept).unwrap();
        assert_eq!(store.edges_from(kept, MAR, OCT).len(), 1);
        assert_eq!(
            store.edge_history(kept, "employed_by", acme).len(),
            2,
            "neither source's assertion is discarded"
        );
        assert_maps_agree(&store);
    }

    #[test]
    fn a_merged_log_still_breaks_ingestion_ties_toward_the_later_append() {
        // Both sides heard it at the same instant, so the merged log has two
        // versions the ingestion order cannot separate. The stable sort keeps
        // them in the order they were concatenated, which is what makes
        // `in_force`'s index tie-break answer the same way on every run.
        let (mut store, kept) = store_with_user();
        let absorbed = store.create_entity("person", JAN);
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                kept,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        store
            .relate(
                absorbed,
                "employed_by",
                acme,
                Interval::since(JUL),
                user_said(JAN),
            )
            .unwrap();

        store.repoint_edges(absorbed, kept).unwrap();
        let history = store.edge_history(kept, "employed_by", acme);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].valid, Interval::since(JAN), "the survivor's own");
        assert_eq!(history[1].valid, Interval::since(JUL), "then the absorbed");
        assert_eq!(
            store.edges_from(kept, OCT, OCT)[0].valid,
            Interval::since(JUL),
            "the later append wins the tie, as it does without a merge"
        );
        assert_maps_agree(&store);
    }

    #[test]
    fn a_move_into_an_empty_destination_keeps_the_log_in_the_order_it_was_appended() {
        // `edge_history` documents itself as append order, and `push_edge`
        // appends in call order, so a log can legitimately run backwards along
        // the ingestion axis: asserted in March, then corrected by something
        // only heard about in January. A move onto an entity holding no such
        // triple has merged nothing, so it has nothing to reorder and must hand
        // the log over exactly as it stood. Sorting it anyway would be invisible
        // to every query -- `in_force` takes the maximum ingestion however the
        // vector is ordered -- which is precisely why nothing else would catch
        // it silently rewriting an audit trail nobody asked to merge.
        let (mut store, kept) = store_with_user();
        let absorbed = store.create_entity("person", JAN);
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                absorbed,
                "employed_by",
                acme,
                Interval::since(JUL),
                user_said(MAR),
            )
            .unwrap();
        store
            .relate(
                absorbed,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        assert_eq!(
            ingestion_order(store.edge_history(absorbed, "employed_by", acme)),
            vec![MAR, JAN],
            "setup: the source log has to run backwards, or this proves nothing"
        );

        assert_eq!(store.repoint_edges(absorbed, kept).unwrap(), 1);
        assert_eq!(
            ingestion_order(store.edge_history(kept, "employed_by", acme)),
            vec![MAR, JAN],
            "an empty destination merged nothing, so nothing was reordered"
        );
        assert_maps_agree(&store);
    }

    #[test]
    fn a_merged_log_keeps_versions_sharing_an_ingestion_time_in_their_relative_order() {
        // `in_force` breaks an ingestion tie on position, so where versions
        // share an ingestion time the merge's own ordering is what decides
        // which one answers. Only a stable sort makes that ordering a property
        // of the inputs rather than of whatever the sort happened to do.
        //
        // Twelve versions a side, not three: an unstable sort falls back to
        // insertion sort on short slices and preserves order there by accident,
        // so a handful of versions would pin nothing. Each is identified by its
        // valid start, which is the only thing separating versions that share a
        // provenance.
        let (mut store, kept) = store_with_user();
        let absorbed = store.create_entity("person", JAN);
        let acme = store.create_entity("org", JAN);
        for start in 200..212 {
            store
                .relate(
                    kept,
                    "employed_by",
                    acme,
                    Interval::since(start),
                    user_said(SEP),
                )
                .unwrap();
        }
        for start in 100..112 {
            store
                .relate(
                    absorbed,
                    "employed_by",
                    acme,
                    Interval::since(start),
                    user_said(JAN),
                )
                .unwrap();
        }

        assert_eq!(store.repoint_edges(absorbed, kept).unwrap(), 1);
        let expected: Vec<Interval> = (100..112).chain(200..212).map(Interval::since).collect();
        assert_eq!(
            store
                .edge_history(kept, "employed_by", acme)
                .iter()
                .map(|v| v.valid)
                .collect::<Vec<_>>(),
            expected,
            "the absorbed log sorts ahead by ingestion, and neither group is shuffled inside itself"
        );
        assert_eq!(
            store.edges_from(kept, 250, 250)[0].valid,
            Interval::since(211),
            "so the tie-break still names the last version appended before the merge"
        );
        assert_maps_agree(&store);
    }

    #[test]
    fn repointing_an_entity_onto_itself_changes_nothing() {
        // A caller resolving two references that turn out to be one id. Every
        // edge on the entity is also an edge on the survivor, so a naive move
        // reads the whole neighbourhood as collapsing into self-edges and
        // deletes it.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        let boss = store.create_entity("person", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();
        store
            .relate(boss, "manages", user, Interval::since(JAN), user_said(JAN))
            .unwrap();

        assert_eq!(store.repoint_edges(user, user).unwrap(), 0);
        assert_eq!(store.edges_from(user, MAR, OCT).len(), 1);
        assert_eq!(store.edges_into(user, MAR, OCT).len(), 1);
        assert_maps_agree(&store);
    }

    #[test]
    fn erasing_edges_on_an_unknown_entity_is_an_error() {
        // The same answer `erase` gives the same mistake, and for the same
        // reason: the caller is working from an id that means nothing, and a
        // bare 0 would let a typo read as a completed deletion. An entity that
        // exists and simply has no edges still reports Ok(0) -- what the caller
        // wanted is already true there.
        let (mut store, user) = store_with_user();
        assert_eq!(
            store.erase_edges(9999),
            Err(StoreError::UnknownEntity(9999))
        );
        assert_eq!(store.erase_edges(user).unwrap(), 0);
        assert_maps_agree(&store);
    }

    #[test]
    fn repointing_either_end_of_a_merge_onto_an_unknown_id_is_an_error() {
        // `relate` rejects an endpoint naming no entity and `open` rejects a
        // snapshot holding one, so moving edges onto one would write a store
        // that cannot be reopened -- damage found at restore, long after this
        // call. Reporting 0 would not be enough either: that is what "there was
        // nothing to move" says, so the caller could not tell the two apart.
        // The edges stay where they are, findable, and the store still opens.
        let (mut store, user) = store_with_user();
        let acme = store.create_entity("org", JAN);
        store
            .relate(
                user,
                "employed_by",
                acme,
                Interval::since(JAN),
                user_said(JAN),
            )
            .unwrap();

        assert_eq!(
            store.repoint_edges(user, 9999),
            Err(StoreError::UnknownEntity(9999))
        );
        assert_eq!(
            store.repoint_edges(9999, user),
            Err(StoreError::UnknownEntity(9999)),
            "the absorbed id is checked too, not just the survivor"
        );
        assert_eq!(
            store.repoint_edges(9999, 9999),
            Err(StoreError::UnknownEntity(9999)),
            "an unknown id merged into itself is still unknown, not a no-op"
        );

        assert_eq!(store.edges_from(user, MAR, OCT).len(), 1);
        assert!(MemoryStore::open(&store.snapshot()).is_ok());
        assert_maps_agree(&store);
    }

    #[test]
    fn a_repointed_store_equals_its_own_round_trip() {
        // `open` rebuilds the reverse map from the forward one, so a restored
        // store carries the reverse map the forward edges imply. Comparing it
        // to the live one is the strongest available statement that the move
        // left the two in step -- including the empty entries a removal can
        // leave, which the rebuild never produces.
        let (mut store, kept) = store_with_user();
        let absorbed = store.create_entity("person", JAN);
        let acme = store.create_entity("org", JAN);
        let boss = store.create_entity("person", JAN);
        for (subject, predicate, object) in [
            (absorbed, "employed_by", acme),
            (boss, "manages", absorbed),
            (kept, "manages", absorbed),
        ] {
            store
                .relate(
                    subject,
                    predicate,
                    object,
                    Interval::since(JAN),
                    user_said(JAN),
                )
                .unwrap();
        }

        assert_eq!(store.repoint_edges(absorbed, kept).unwrap(), 3);
        assert_eq!(MemoryStore::open(&store.snapshot()).unwrap(), store);
        assert_maps_agree(&store);
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
            Supersession::Unstated,
            None,
        )
        .unwrap();
        s.assert(
            id,
            "employer",
            Some("Globex".into()),
            Interval::since(JUL),
            user_said(SEP),
            Supersession::Unstated,
            None,
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
            Supersession::Unstated,
            None,
        )
        .unwrap();
        s.assert(
            id,
            "employer",
            Some("Acme".into()),
            Interval::since(JAN),
            user_said(MAR),
            Supersession::Unstated,
            None,
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
    fn a_snapshot_whose_id_counter_was_rewound_is_rejected_not_restored() {
        // The snapshot is otherwise perfect -- the entity, its attributes and
        // every version are intact. Only the counter lies, and it lies about
        // the *next* write rather than about anything stored, so without this
        // check `open` returns Ok and the damage lands on a later
        // `create_entity` that also returns normally.
        let (s, id) = store_with_user();
        let mut doc: serde_json::Value = serde_json::from_str(&s.snapshot()).unwrap();
        assert_eq!(doc["next_id"], id + 1, "setup: one entity was created");
        // Rewound to the id of a live entity, not below it: the counter names
        // the next id to hand out, so equality is already a collision.
        doc["next_id"] = serde_json::json!(id);

        let err = MemoryStore::open(&doc.to_string()).unwrap_err();
        assert!(matches!(err, StoreError::CorruptSnapshot(_)), "{err:?}");
    }

    #[test]
    fn an_empty_store_has_no_counter_to_contradict() {
        // No entities, so nothing bounds `next_id` from below and any value is
        // consistent. Guards the check against rejecting the empty case, which
        // is the one every caller starts from.
        let empty = MemoryStore::new();
        assert!(MemoryStore::open(&empty.snapshot()).is_ok());
    }

    #[test]
    fn a_malformed_snapshot_is_reported_not_panicked_on() {
        let err = MemoryStore::open("{ not json").unwrap_err();
        assert!(matches!(err, StoreError::Parse(_)), "{err:?}");
    }

    #[test]
    fn a_snapshot_parse_failure_names_a_position_never_the_text_that_broke_it() {
        // The awkward fixture: a backtick embedded in the very value
        // `serde_json` used to quote. Before this fix `StoreError::Parse`
        // carried `serde_json`'s own message -- `invalid type: string
        // "CANARY-`-...", expected u64` -- which is exactly the shape a
        // 180,000-config fuzz proved a downstream scanner cannot filter: a
        // backtick inside a backtick span is not escaped, so nothing can
        // tell content from a span's own close. `next_id` wants a `u64` and
        // this hands it a string, so the failure is real and in `Data`
        // position, not merely syntactic.
        let canary = "CANARY-`-0123456789abcdef";
        let snapshot = format!(r#"{{"entities":{{}},"next_id":"{canary}","edges":{{}}}}"#);

        let err = MemoryStore::open(&snapshot).unwrap_err();
        let rendered = err.to_string();
        let StoreError::Parse(parse) = err else {
            panic!("expected a parse failure, got {rendered}");
        };
        assert_eq!(parse.category, ParseCategory::Data);
        assert_eq!(parse.line, 1);
        assert!(!rendered.contains(canary), "{rendered}");
        assert!(!rendered.contains('`'), "{rendered}");
        assert!(!rendered.contains("CANARY"), "{rendered}");
    }
    /// A resolution containing a contested span refuses whole, and leaves
    /// nothing behind. Storage has no way to record two values that nothing
    /// orders, so unlike a read this cannot be asked about one instant.
    #[test]
    fn a_resolution_with_a_contested_span_writes_nothing_at_all() {
        let (mut s, id) = store_with_user();
        let (a, b) = (user_said(MAR), user_said(MAR));
        let refused = s.assert_resolved(
            id,
            "employer",
            &[
                Candidate::new(Some("Acme"), &a).over(Interval::since(JAN)),
                Candidate::new(Some("Globex"), &b).over(Interval::since(JAN)),
            ],
            &Strategy::ValidInterval,
        );
        assert!(
            refused.is_err(),
            "a contested resolution must not be written"
        );
        assert!(
            s.history(id, "employer").is_empty(),
            "a refused resolution left a half-written timeline behind"
        );
    }
}

#[cfg(test)]
mod holders {
    use super::*;

    /// A snapshot written before holders existed round-trips byte for byte.
    ///
    /// The same promise `supersession` makes, for the same reason: a store's
    /// whole value is that it stays reconstructible, and a field that rewrites
    /// every existing snapshot on upgrade costs more than it is worth.
    ///
    /// The literal below was captured by serialising a holder-less `Version`
    /// before the field existed, not hand-written from the struct definition.
    #[test]
    fn a_holder_less_version_serialises_exactly_as_it_did_before() {
        let v = Version {
            value: Some("Circulation".into()),
            valid: Interval::since(100),
            provenance: Provenance::new(rm_core::Source::UserAssertion, 100, "s"),
            supersession: Supersession::Unstated,
            according_to: None,
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(
            !json.contains("according_to"),
            "a holder-less version wrote the field: {json}"
        );
        assert_eq!(
            json,
            r#"{"value":"Circulation","valid":{"from":100,"to":null},"provenance":{"source":"UserAssertion","observed_at":100,"source_ref":"s"}}"#,
            "the byte-for-byte shape moved"
        );

        let back: Version = serde_json::from_str(&json).unwrap();
        assert_eq!(back.according_to, None);
    }

    /// A held version writes the field, so the two are distinguishable on disk.
    #[test]
    fn a_held_version_says_whose_view_it_is() {
        let v = Version {
            value: Some("R&A".into()),
            valid: Interval::since(100),
            provenance: Provenance::new(rm_core::Source::UserAssertion, 100, "s"),
            supersession: Supersession::Unstated,
            according_to: Some(300),
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""according_to":300"#), "{json}");
        assert_eq!(
            serde_json::from_str::<Version>(&json).unwrap().according_to,
            Some(300)
        );
    }
}
