//! What each command does. Data out; rendering lives in `format`.

use std::path::{Path, PathBuf};

use rm_engine::{
    Believed, Embedder, Engine, Ingested, Interval, Metric, Observation, Prepared, Provenance,
    Query, Recalled, Record, ReviewId, Source, StableId, Supersession, Timestamp, Version,
};
use rm_extract::{Completer, Extraction, Turn};

/// Re-exported because [`Outcome::Remembered`] carries these and a host has to
/// be able to name them.
///
/// `rm-cli` depends on this crate and `rm-engine`, and on nothing else — that
/// narrowing is the point of `rm-host` existing. A host forced to add
/// `rm-extract` to its manifest just to read a field of an `Outcome` this crate
/// hands it would give that back.
pub use rm_extract::Dropped;

use crate::config::TEMPLATE;
use crate::time::At;
use crate::HostError;

/// One mention and where it ended up.
///
/// `Ingested` records which entity each mention resolved to but not whether
/// that entity was created, and "recognised" versus "new" is the most useful
/// thing on the screen — it is the difference between the store learning about
/// someone and the store recognising them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionLanding {
    pub name: String,
    pub entity: StableId,
    pub was_new: bool,
}

/// One open question, as a caller sees it.
///
/// Carries what each side is *called*, not just its id. "review 4: entity 3 vs
/// entity 11 (5.03 bits)" is not a question anyone can answer -- answering it
/// meant running `about` twice per pair first, which is enough friction that
/// the queue goes unread, and an unread queue is the same as no queue.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewLine {
    pub id: ReviewId,
    pub a: StableId,
    pub b: StableId,
    /// What `a` resolved on, when it still has an identity to read.
    pub a_name: Option<String>,
    pub b_name: Option<String>,
    /// What each side is, which is often the whole answer: measured on a real
    /// corpus, about half the band pairs an entity with one of a different
    /// kind, and "a person and a place" settles the question on sight.
    pub a_kind: String,
    pub b_kind: String,
    pub score: f64,
}

/// What a command did.
/// How much of each hit a recall should return.
///
/// No level summarises: a deeper one is a superset of a shallower one, byte
/// for byte. `Stated` is what `recall` has always returned and is the
/// default, so tiering is opt-in in the direction that saves money.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Depth {
    /// What was found and whether it stands. No assertion text.
    Located,
    /// ...and what it says. Today's behaviour.
    #[default]
    Stated,
    /// ...and who asserted it, and what it stands against. Expensive:
    /// measured at roughly 8x `Stated` over twenty hits, so it is for one
    /// answer rather than for a result set. See `docs/tiering-cost.md`.
    Traced,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Initialised {
        path: PathBuf,
        dimension: usize,
        /// Set when `init --force` replaced a file that existed but did not
        /// parse, so the reader learns their old file is gone and why rather
        /// than only seeing a fresh one appear. Carries the same words
        /// [`HostError::Config`] would have refused with -- built entirely by
        /// this crate, so it names a location and never a value out of the
        /// file it describes.
        replaced_unparsable: Option<String>,
        /// Whether the dimension came from `--local` rather than from the
        /// model.
        ///
        /// Carried so the message can say which. `rmem init --local` reported
        /// "taken from the model" while asking no model anything -- true of
        /// the http path, false here, and exactly the shape of claim this
        /// project keeps finding.
        local: bool,
    },
    /// A fact recorded about someone the store may or may not have known.
    Noted {
        entity: StableId,
        attribute: String,
        /// The fact asserted there is no value.
        absent: bool,
        /// It landed on an entity that already existed.
        merged: bool,
        /// Whose view it was recorded as, if anybody's.
        ///
        /// Reported because the flag's *absence* is the risk: two
        /// commands differing by one argument produce records that
        /// never meet, and a forgotten flag silently promotes an
        /// opinion to a fact.
        according_to: Option<StableId>,
        /// Every pair it scored inside the review band against. A vector
        /// rather than an option because one mention can be ambiguous against
        /// several entities at once, and reporting only the first would hide
        /// the rest. Empty is the ordinary case.
        ///
        /// The fact is recorded either way; what is open is only whose it is.
        reviews: Vec<rm_engine::PendingReview>,
    },
    Remembered {
        ingested: Ingested,
        landings: Vec<MentionLanding>,
        /// How many relationships the turn asserted.
        ///
        /// Carried rather than derived: `Ingested` records entities,
        /// assertions, reviews and closures, but nothing about relations --
        /// `relate` returns no id -- and the spec's worked output counts them.
        relations: usize,
        /// What the model described that `rm_extract` would not keep.
        ///
        /// Carried all the way to the surface on purpose. `extract` salvages a
        /// turn rather than refusing it whole, and the only thing that makes
        /// that defensible instead of silent data loss is that the loss is
        /// reported. A field nothing renders would leave it silent for every
        /// person actually using this, whatever the type says.
        dropped: Vec<rm_extract::Dropped>,
    },
    /// Hits, nearest first, with the bar below which the nearest one is worth
    /// calling weak.
    ///
    /// The bar travels with the answer rather than being looked up by whoever
    /// renders it: it comes from `rmem.toml`, and a renderer that had to read
    /// config would need one threaded through every call for the sake of one
    /// number.
    Recalled {
        hits: Vec<Recalled>,
        /// From `[retrieval] weak_below`. Zero turns the notice off.
        weak_below: f32,
    },

    /// What a document ingest wrote.
    Ingested(crate::ingest::Read),
    /// What a document ingest *would* write, having called nothing.
    Surveyed(crate::ingest::Read),
    /// A recall at [`Depth::Located`]: locators, no assertion text.
    LocatedHits {
        hits: Vec<rm_engine::Located>,
    },
    /// A recall at [`Depth::Traced`]: each hit with the versions it stands
    /// against.
    TracedHits {
        hits: Vec<rm_engine::Traced>,
        weak_below: f32,
    },
    About(Believed),
    Reviews(Vec<ReviewLine>),
    Confirmed {
        survivor: StableId,
    },
    Rejected,
    Decided {
        entity: StableId,
        /// The decision this one replaces, if it named one and it was found.
        superseded: Option<(StableId, String)>,
        /// Named but not found. Reported rather than silently ignored: a
        /// caller who mistyped the title of the decision they meant to retire
        /// has left it standing, and will not learn that from a success.
        supersedes_unknown: Option<String>,
    },
    Decisions(Vec<DecisionLine>),
    /// The index was rebuilt: how many assertions, and under what shape.
    Reindexed {
        assertions: usize,
        dimension: usize,
    },
    /// One decision read in full. `None` when no decision has that title --
    /// reported rather than empty, because "no such decision" and "a decision
    /// with nothing in it" are different answers.
    Decision(Found),
    /// A decision's reach was corrected, and nothing else about it changed.
    Rescoped {
        entity: StableId,
        title: String,
        scope: String,
        /// What it reached before. `None` when it had no scope at all, which
        /// is the backfill case -- and worth distinguishing, because
        /// "widened from work/goldenmatch" and "had none" are different
        /// things to have done.
        previous: Option<String>,
    },
}

/// One decision as a caller sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionLine {
    pub entity: StableId,
    pub title: String,
    pub status: String,
    pub choice: String,
    pub because: Option<String>,
    /// Whether this decision is in force — safe to act on as it reads.
    ///
    /// True only for an `accepted` decision that nothing has superseded. Every
    /// other status is a reason not to act: `proposed` is not settled,
    /// `rejected` records an option declined, `deprecated` is on its way out,
    /// and `superseded` means another decision replaced this one.
    ///
    /// About the value being displayed, not about the title's whole past. A
    /// title re-decided under itself shows its *latest* choice, and that choice
    /// is in force -- marking it replaced said the opposite of what the line
    /// shows. The count of earlier choices is [`Self::revisions`], a different
    /// question that now reads as one.
    pub still_stands: bool,
    /// How many times this title has been decided. One unless it was
    /// re-decided under itself.
    pub revisions: usize,
    /// The decision that replaced this one, when the chain records it.
    ///
    /// Marking a decision retired without naming its successor tells a reader
    /// that the answer they are holding is wrong and leaves them no way to the
    /// right one. `None` on a decision that stands, and also on one retired
    /// before supersession was recorded as an edge.
    pub superseded_by: Option<(StableId, String)>,
}

/// What looking for one decision found.
///
/// Three answers rather than two, because the store holds the difference and
/// collapsing it loses information a reader needs. `find_decision` matches on
/// the identity record's `name`, which is not versioned, so a decision recorded
/// after `at.tx` still resolves by title -- and answering "no such decision"
/// for it would read as a spelling mistake and send the reader looking for one.
///
/// The same distinction `Believed` draws between `Absent` ("someone said there
/// is none") and `Unknown` ("it has never come up").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// The decision, as it stood at the time asked about.
    Decision(Box<DecisionDetail>),
    /// The title resolves, but nothing of it stood at `at`.
    ///
    /// Both days are carried because either clock can be the one that excluded
    /// it and they are not the same question. A decision backdated to March and
    /// typed up in August is invisible before March on the valid axis and
    /// before August on the transaction axis, and a reader told only "first
    /// recorded August" would not understand why asking about April also came
    /// back empty.
    NotYetRecorded {
        title: String,
        /// The first moment the store heard of this decision.
        first_recorded: Timestamp,
        /// The first moment it claims to have held.
        first_held: Timestamp,
    },
    /// The title resolves, and the decision does not reach where it was asked
    /// from.
    ///
    /// Distinct from `Unknown` for the same reason `NotYetRecorded` is: the
    /// title matched, so "no decision by that title" would read as a spelling
    /// mistake. You named it exactly, so you are told where it lives.
    NotHere {
        title: String,
        /// The reach the decision states.
        scope: String,
        /// The position it was asked from.
        asked_from: String,
    },
    /// No decision by that title.
    Unknown,
}

/// One decision, in full, with the chain it sits in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionDetail {
    pub entity: StableId,
    pub title: String,
    pub status: String,
    pub choice: String,
    pub because: Option<String>,
    pub context: Option<String>,
    pub still_stands: bool,
    /// What this decision replaced, most recent first, following the chain back
    /// as far as it goes.
    pub supersedes: Vec<(StableId, String)>,
    /// What replaced this decision, and what replaced that, ending at whatever
    /// stands now. Empty when this is the one that stands.
    pub superseded_by: Vec<(StableId, String)>,
    /// Every `choice` this title has held, oldest first, with the day it was
    /// decided -- valid time, so a backdated decision reads under the day it
    /// was made rather than the day it was typed up. One entry is a decision
    /// made once; more is a decision re-decided under the same title.
    pub history: Vec<(Timestamp, String)>,
    /// The clock this answer was given under, or `None` for what stands now.
    ///
    /// A `DecisionDetail` is an answer at a time, not a timeless fact, and
    /// renderers need to know which. `still_stands` is evaluated at this clock,
    /// so a reader shown "this is what stands" under a past `as_of` would be
    /// told the present tense about the past.
    pub answered_at: Option<At>,
}

/// The attributes a decision is recorded under, and whether each admits one
/// value at a time.
///
/// Fixed, which is the whole point. `remember` sends a turn through a model and
/// gets back whatever attribute names it invents -- measured on a real corpus,
/// 81% of them are used exactly once, which makes them unreferenceable and
/// makes supersession unreachable. A decision has a known shape, so this writes
/// the shape directly: no completion call, no invented vocabulary, and
/// `Supersession` filled exactly rather than guessed.
///
/// Every one of these corrects. A decision has one status, one choice, one
/// stated reason at a time; re-deciding under the same title is a correction
/// and that is precisely what `Standing::Corrected` should say about the old
/// one.
const DECISION_FIELDS: [&str; 5] = ["status", "choice", "because", "context", "scope"];

/// What a decision's `status` may be, and the whole of it.
///
/// A closed vocabulary because the point of a status is that a reader can
/// branch on it. An open one would let `rejected`, `Rejected` and `declined`
/// mean the same thing to a person and three different things to a program,
/// which is the same failure as the 81% singleton attribute names that made
/// `decide` skip extraction in the first place.
///
/// `superseded` is deliberately not here. It says a *specific other decision*
/// replaced this one, and writing it without naming that decision produces the
/// state the supersession edge exists to prevent -- retired, with no way to
/// reach what retired it. [`plan_decide`] refuses it and points at
/// `--supersedes`, which writes both ends.
pub const DECISION_STATUSES: [&str; 4] = ["proposed", "accepted", "rejected", "deprecated"];

/// The status a decision takes when nobody says otherwise.
pub const DEFAULT_STATUS: &str = "accepted";

/// The status written on a decision that another one replaced.
///
/// Set by `--supersedes` and never by a caller directly; see
/// [`DECISION_STATUSES`].
pub const SUPERSEDED: &str = "superseded";

/// The edge a decision draws to the one it replaced.
///
/// An edge rather than a field on either side, because a supersession is a
/// fact about the *pair*. Written on the old decision it would be a
/// `superseded_by` that the next supersession has to correct; written on the
/// new one it would be a `supersedes` that says nothing when you are holding
/// the old one and asking what happened to it. As an edge it is one write and
/// both directions read it -- `edges_from` for what this replaced,
/// `edges_into` for what replaced this.
pub const SUPERSEDES: &str = "supersedes";

/// Write a config, with the embedding dimension taken from the model unless
/// `local` is set, in which case nothing is asked and no key is needed.
///
/// `probe` is a closure rather than a provider so this is testable without a
/// socket; the binary passes one that calls `HttpProvider::probe_dimension`.
///
/// Probing before writing is deliberate. Half a config is worse than none: the
/// next command would read it and fail somewhere further from the cause.
///
/// `replaced_unparsable` says whether `run` already decided, before calling
/// this, that an existing file could not be parsed and is being replaced
/// under `--force`. When it is `Some`, the existence check below is skipped
/// deliberately rather than merely made moot by `force` being `true`: a file
/// that failed to parse can still fail `Path::exists` on some errors it does
/// not represent (permission, a directory in the way), and this function has
/// no way to tell those apart from `force` alone. The caller already did that
/// work in `Config::read_for_init`, and repeating a weaker
/// version of it here would be the same drift `deny_unknown_fields` exists to
/// stop elsewhere in this crate.
pub fn init(
    config_path: &Path,
    force: bool,
    local: bool,
    replaced_unparsable: Option<String>,
    probe: &dyn Fn() -> Result<usize, String>,
) -> Result<Outcome, HostError> {
    if replaced_unparsable.is_none() && config_path.exists() && !force {
        return Err(HostError::Config(format!(
            "{} already exists, and it may have been edited -- pass --force to replace it",
            config_path.display()
        )));
    }

    // The short-circuit lives here rather than in the caller's closure, so
    // "--local demands no key" is a property of this function's contract and
    // not of one call site's discipline. A caller that forgot the branch would
    // otherwise reintroduce the probe silently.
    let dimension = if local {
        crate::config::TEMPLATE_DIMENSION
    } else {
        probe().map_err(HostError::Refused)?
    };

    // The template's own value is an example. Substituting rather than
    // formatting keeps the file one literal, so the test that parses it is
    // testing the bytes a user receives.
    let mut contents = TEMPLATE.replace("dimension = 1536", &format!("dimension = {dimension}"));
    if local {
        // The `[provider]` fields stay in the file even though the local
        // embedder never dials. Making them optional would weaken validation
        // on the http path -- the one where a wrong `api_key_env` costs money
        // -- to tidy four inert lines here. Left required, they are also a
        // safety property: `api_key_env` naming a variable nobody sets means
        // an accidental fall back to `http` fails loudly rather than quietly
        // spending someone's key.
        contents = contents.replace("embedder = \"http\"", "embedder = \"local\"");
    }

    // An absolute store path, so a config this crate writes needs no rule to
    // interpret. `Config::parse` anchors a relative path against the config's
    // own directory, which is what makes hand-written and older configs behave
    // sensibly; writing it out in full is what stops the question arising at
    // all. `TEMPLATE` keeps the relative example so the committed file stays
    // readable and portable -- the two differ on purpose.
    if let Some(dir) = config_path.parent() {
        let store = dir.join("memory.json");
        // Forward slashes: TOML would read a Windows backslash as an escape.
        let store = store.to_string_lossy().replace('\\', "/");
        contents = contents.replace("path = \"memory.json\"", &format!("path = \"{store}\""));
    }

    std::fs::write(config_path, contents).map_err(|e| {
        HostError::Config(format!("could not write {}: {e}", config_path.display()))
    })?;

    Ok(Outcome::Initialised {
        path: config_path.to_path_buf(),
        dimension,
        local,
        replaced_unparsable,
    })
}

/// Extract a turn and apply it.
///
/// # The speaker is worth passing
///
/// This was hardcoded to `None`, so neither `rmem` nor the MCP server could
/// supply a speaker however much the caller knew. Measured on a real corpus,
/// that is expensive: dialogue is mostly first person, and without a speaker
/// "I moved to Chicago" names nobody to attach anything to. Supplying it took
/// responses listing no mentions at all from 45% to 1%, and what a 419-turn
/// conversation yielded from about 576 assertions to about 1494.
///
/// The failure is worse than a missing subject. Run through the built binary,
/// "I moved to Chicago last month and started at Globex." with no speaker
/// records `employer = Globex` and `location = Chicago` on the entity *Chicago*
/// -- the facts are real, and the only thing near enough to hang them on is a
/// city. A wrong subject is not a smaller error than no subject; it is a
/// confident one.
///
/// Still `Option`, because a turn may genuinely have no identified speaker — a
/// log line, a document, a note someone left. `rm_extract`'s prompt states that
/// case explicitly rather than leaving a blank for the model to fill, so `None`
/// is a supported answer rather than a missing argument.
pub fn remember(
    engine: &mut Engine,
    text: &str,
    observed_at: rm_engine::Timestamp,
    session: &str,
    speaker: Option<&str>,
    completer: &impl Completer,
    embedder: &impl Embedder,
) -> Result<Outcome, HostError> {
    let (dimension, metric) = engine.index_shape();
    let plan = plan_remember(
        text,
        observed_at,
        session,
        speaker,
        completer,
        embedder,
        dimension,
        metric,
    )?;
    commit_remember(engine, plan)
}

/// Everything [`commit_remember`] will need from the network, and nothing else.
///
/// Built without an [`Engine`], which is the entire point: a host that calls
/// this before taking its lock pays for the extraction and the embeddings on
/// its own time rather than on every other writer's. See [`rm_engine::prepare`]
/// for why none of this depends on the store.
pub struct RememberPlan {
    turn: Turn,
    extraction: Extraction,
    prepared: Prepared,
}

impl RememberPlan {
    /// What the model read out of the turn, before any of it was written.
    ///
    /// Exposed so a caller can refuse a plan without committing it -- an empty
    /// extraction is a completion spent for nothing, and a host that queues
    /// writes may prefer to know that before it queues one.
    pub fn extraction(&self) -> &Extraction {
        &self.extraction
    }
}

/// The half of [`remember`] that talks to models. No store, no lock.
///
/// `dimension` and `metric` describe the index the vectors are destined for.
/// A caller has them already: they come from `rmem.toml`, and the store
/// refuses to load against a config that disagrees with them.
#[allow(clippy::too_many_arguments)]
pub fn plan_remember(
    text: &str,
    observed_at: rm_engine::Timestamp,
    session: &str,
    speaker: Option<&str>,
    completer: &impl Completer,
    embedder: &impl Embedder,
    dimension: usize,
    metric: Metric,
) -> Result<RememberPlan, HostError> {
    let turn = Turn {
        text: text.to_string(),
        speaker: speaker.map(str::to_string),
        observed_at,
        session: session.to_string(),
    };

    let extraction =
        rm_engine::extract(&turn, completer).map_err(|e| HostError::Refused(e.to_string()))?;
    let prepared = rm_engine::prepare(&extraction, embedder, dimension, metric)
        .map_err(|e| HostError::Refused(e.to_string()))?;

    Ok(RememberPlan {
        turn,
        extraction,
        prepared,
    })
}

/// The half of [`remember`] that needs the store. Touches no network.
pub fn commit_remember(engine: &mut Engine, plan: RememberPlan) -> Result<Outcome, HostError> {
    let RememberPlan {
        turn,
        extraction,
        prepared,
    } = plan;

    // Which entities existed before, so the landings can say "recognised"
    // rather than only naming an id. Read here rather than in the plan: it is
    // a fact about the store at the moment of the write, and a copy taken
    // before the lock could name an entity another writer has since merged.
    let before: Vec<StableId> = engine.entity_ids();

    let ingested = engine
        .ingest_prepared(&turn, &extraction, prepared)
        .map_err(|e| HostError::Refused(e.to_string()))?;

    let landings = extraction
        .mentions
        .iter()
        .zip(&ingested.entities)
        .map(|(mention, &entity)| MentionLanding {
            name: mention.name.clone(),
            entity,
            was_new: !before.contains(&entity),
        })
        .collect();

    Ok(Outcome::Remembered {
        ingested,
        landings,
        relations: extraction.relations.len(),
        dropped: extraction.dropped,
    })
}

/// Search for assertions near a query.
pub fn recall(
    engine: &Engine,
    query: &str,
    k: usize,
    embedder: &impl Embedder,
    weak_below: f32,
    here: Option<&str>,
) -> Result<Outcome, HostError> {
    // `Stated` because this convenience wrapper is what the CLI's one-shot
    // path uses, and its behaviour must not change when a depth exists.
    commit_recall(
        engine,
        plan_recall(query, embedder)?,
        k,
        weak_below,
        here,
        Depth::Stated,
    )
}

/// Embed a query, touching no store.
///
/// `recall` takes a shared lock, so two readers never queue behind each other
/// -- but a shared lock still holds a writer off, and holding one across a
/// network round trip made every reader a brake on every writer.
pub fn plan_recall(query: &str, embedder: &impl Embedder) -> Result<Vec<f32>, HostError> {
    embedder
        .embed(query)
        .map_err(|e| HostError::Refused(e.to_string()))
}

/// Search with an embedding [`plan_recall`] already produced.
pub fn commit_recall(
    engine: &Engine,
    embedding: Vec<f32>,
    k: usize,
    weak_below: f32,
    here: Option<&str>,
    depth: Depth,
) -> Result<Outcome, HostError> {
    // The position filters inside the index scan rather than over a fetched
    // page, so `k` still means "k results that apply" rather than "k
    // candidates, some of which survive".
    let mut query = Query::new(embedding, k);
    if let Some(here) = here {
        query = query.at(here);
    }
    // Dispatched here rather than in each host, so the CLI and the MCP server
    // cannot drift on what a depth means.
    match depth {
        Depth::Located => {
            let hits = engine
                .recall_located(&query)
                .map_err(|e| HostError::Refused(e.to_string()))?;
            Ok(Outcome::LocatedHits { hits })
        }
        Depth::Stated => {
            let hits = engine
                .recall(&query)
                .map_err(|e| HostError::Refused(e.to_string()))?;
            Ok(Outcome::Recalled { hits, weak_below })
        }
        Depth::Traced => {
            let hits = engine
                .recall_traced(&query)
                .map_err(|e| HostError::Refused(e.to_string()))?;
            Ok(Outcome::TracedHits { hits, weak_below })
        }
    }
}

/// What the store believes an attribute held.
///
/// `valid_at` is an `Option` rather than a resolved timestamp because the two
/// cases are different questions and only the caller can tell them apart.
/// "What held in March" and "what holds now" arrive here identically once a
/// default has been applied, and that collapse is precisely why `--valid-at`
/// could be accepted, do nothing, and say nothing: the information needed to
/// refuse was destroyed one layer above the check.
pub fn about(
    engine: &Engine,
    entity: StableId,
    attribute: &str,
    valid_at: Option<Timestamp>,
    as_of: Option<Timestamp>,
    now: Timestamp,
    according_to: Option<StableId>,
) -> Result<Outcome, HostError> {
    // Asked about a moment, under a strategy that has no moments. Refused
    // rather than warned, because a warning on stderr is a wrong answer with a
    // note attached -- and this codebase refuses everywhere else it has faced
    // the same choice.
    if valid_at.is_some() {
        let strategy = engine.policy().for_attribute(attribute);
        if !strategy.keeps_a_timeline() {
            return Err(HostError::Refused(format!(
                "{attribute:?} is resolved by {strategy:?}, which picks one winner rather than keeping a timeline, so there is no moment to ask about -- every date would answer the same. Set `{attribute} = \"valid_interval\"` under [policy.attribute] in rmem.toml to keep one, or drop --valid-at to read what stands."
            )));
        }
    }
    // Whose view, or the store's own. A holder-less read never returns a
    // view and a holder's read never returns a fact, so these are two
    // questions rather than one with a filter.
    match according_to {
        None => engine.about(
            entity,
            attribute,
            valid_at.unwrap_or(now),
            as_of.unwrap_or(now),
        ),
        Some(holder) => engine.about_according_to(
            entity,
            attribute,
            holder,
            valid_at.unwrap_or(now),
            as_of.unwrap_or(now),
        ),
    }
    .map(Outcome::About)
    .map_err(|e| HostError::Refused(e.to_string()))
}

/// What an entity currently says it is.
///
/// The kind is stored as an ordinary attribute -- `ingest` asserts it like any
/// other fact -- so reading it means reading the latest version, not a field.
fn kind_of(engine: &Engine, entity: StableId) -> String {
    engine
        .store_history(entity, "kind")
        .last()
        .and_then(|v| v.value.clone())
        .unwrap_or_else(|| "?".to_string())
}

/// The open questions.
pub fn review_list(engine: &Engine) -> Result<Outcome, HostError> {
    Ok(Outcome::Reviews(
        engine
            .pending_review()
            .into_iter()
            .map(|p| {
                let name = |e| {
                    engine
                        .identity_of(e)
                        .and_then(|r| r.get("name").map(str::to_string))
                };
                ReviewLine {
                    id: p.id,
                    a: p.a,
                    b: p.b,
                    a_name: name(p.a),
                    b_name: name(p.b),
                    a_kind: kind_of(engine, p.a),
                    b_kind: kind_of(engine, p.b),
                    score: p.score,
                }
            })
            .collect(),
    ))
}

/// Answer a review with "the same".
pub fn review_confirm(engine: &mut Engine, id: ReviewId) -> Result<Outcome, HostError> {
    engine
        .confirm(id)
        .map(|survivor| Outcome::Confirmed { survivor })
        .map_err(|e| HostError::Refused(e.to_string()))
}

/// Answer a review with "different".
pub fn review_reject(engine: &mut Engine, id: ReviewId) -> Result<Outcome, HostError> {
    engine
        .reject(id)
        .map(|()| Outcome::Rejected)
        .map_err(|e| HostError::Refused(e.to_string()))
}

/// Record a decision, and optionally retire the one it replaces.
///
/// # Why this does not go through `remember`
///
/// `remember` is for dialogue: it hands a turn to a model and stores whatever
/// the model found. That is right when the shape of what is said is unknown,
/// and wrong here. A decision has a known shape, and the value of recording one
/// is being able to ask for it again later -- which needs the attribute names
/// to be stable. The extractor does not give stable names: on a real corpus 81%
/// of the names it invents are used exactly once.
///
/// So this writes the four fields directly. It costs one embedding per field
/// and no completion at all, and it fills `Supersession` exactly rather than
/// asking a model to guess it. `rm_extract::arity` documents this as the case
/// the design is actually for; this is the first caller to be it.
///
/// # Superseding
///
/// A decision that replaces another names it by title. The old decision's
/// `status` becomes `superseded`, written as a correction, so
/// `Standing::still_stands` on its `choice` is the question "is this still what
/// we do" and the store answers it without anyone maintaining a status field by
/// hand.
///
/// A title that matches nothing is reported, not ignored. Silently accepting it
/// would leave the decision the caller meant to retire standing, and they would
/// have no way to know.
#[allow(clippy::too_many_arguments)]
pub fn decide(
    engine: &mut Engine,
    title: &str,
    choice: &str,
    scope: &str,
    status: Option<&str>,
    because: Option<&str>,
    context: Option<&str>,
    supersedes: Option<&str>,
    decided_at: Option<Timestamp>,
    observed_at: Timestamp,
    session: &str,
    embedder: &impl Embedder,
) -> Result<Outcome, HostError> {
    let plan = plan_decide(
        title,
        choice,
        scope,
        status,
        because,
        context,
        supersedes,
        decided_at,
        observed_at,
        session,
        embedder,
    )?;
    commit_decide(engine, plan)
}

/// One attribute of one decision, with its embedding already produced.
struct FieldWrite {
    title: String,
    attribute: String,
    value: String,
    embedding: Vec<f32>,
}

/// Everything [`commit_decide`] will need from the embedder, and nothing else.
///
/// A decision costs no completion -- the shape is known -- but it did cost four
/// sequential embedding calls with the store's exclusive lock held, which made
/// one agent recording a decision a three-second outage for every other writer.
pub struct DecidePlan {
    /// The title of the decision being recorded. Its identity, not a hint --
    /// see [`commit_decide`].
    title: String,
    /// When the decision was made: valid time.
    decided_at: Timestamp,
    /// When the store was told: transaction time.
    observed_at: Timestamp,
    session: String,
    /// The retirement of the decision this one replaces, embedded whether or
    /// not a decision by that title turns out to exist.
    ///
    /// Prepared unconditionally because whether it exists is a question about
    /// the store, and asking it here would need the lock this type exists to
    /// avoid taking. One wasted embedding on a mistyped `--supersedes` is the
    /// cost, and only when `--supersedes` was passed at all.
    retire: Option<(String, FieldWrite)>,
    /// The fields of the new decision, in write order.
    fields: Vec<FieldWrite>,
}

/// Everything [`commit_note`] will need from the embedder, and nothing else.
///
/// The same split [`DecidePlan`] makes and for the same reason: the
/// embedding happens before the store's exclusive lock is taken, so a slow
/// or failing embedder never holds it.
#[derive(Debug)]
pub struct NotePlan {
    who: String,
    kind: String,
    attribute: String,
    /// `None` is a tombstone -- an asserted absence, which is a claim and
    /// not a gap. `rm_store` keeps the two apart and so does this.
    value: Option<String>,
    /// Extra mention fields. They reach the identity record, which is what
    /// the resolver compares, and are written once per entity.
    fields: Vec<(String, String)>,
    valid_from: Timestamp,
    observed_at: Timestamp,
    session: String,
    /// Whose view this is. `None` makes it the store's own fact,
    /// which is what a caller who says nothing is asserting.
    according_to: Option<StableId>,
    /// The scope and its embedding together, because a scope is a second
    /// attribute and therefore a second vector -- taken here so the store's
    /// lock is never held while an embedder is called.
    scope: Option<(String, Vec<f32>)>,
    embedding: Vec<f32>,
}

/// Record a fact someone already knows.
///
/// # No completer, stated as a type
///
/// This signature cannot name a `Completer`, which is the whole point:
/// [`plan_remember`] takes one, so every fact in this store would have cost
/// a completion call, and the cheapest way to record something you already
/// knew was to write prose about it and pay a model to read the prose back.
/// [`plan_decide`] made the opposite bargain for a decision; this makes it
/// for a fact.
///
/// # `scope` is optional here and required by [`plan_decide`]
///
/// Not an inconsistency. An entity with no `scope` attribute already reaches
/// every position, so omitting it is the correct answer rather than an unset
/// field -- and a fact about a person is usually true whichever project the
/// asker is standing in. `plan_decide` refuses without one because a
/// decision's reach genuinely varies.
#[allow(clippy::too_many_arguments)]
pub fn plan_note(
    who: &str,
    kind: &str,
    attribute: &str,
    value: Option<&str>,
    fields: &[(String, String)],
    valid_from: Option<Timestamp>,
    observed_at: Timestamp,
    session: &str,
    scope: Option<&str>,
    according_to: Option<StableId>,
    embedder: &impl Embedder,
) -> Result<NotePlan, HostError> {
    if who.trim().is_empty() {
        return Err(HostError::Refused(
            "a note needs to say who or what it is about: that name is how the store decides whether this is someone it already knows".into(),
        ));
    }
    if attribute.trim().is_empty() {
        return Err(HostError::Refused(
            "a note needs an attribute: the name of the thing being recorded, so it can be asked about later".into(),
        ));
    }

    // Before the embedder, so a typo costs nothing.
    if let Some(scope) = scope {
        crate::scope::validate(scope).map_err(HostError::Refused)?;
    }

    // One embedding, in the same shape `plan_decide` uses for a field.
    let text = match value {
        Some(v) => format!("{who}: {attribute} is {v}"),
        None => format!("{who}: {attribute} is not set"),
    };
    let embedding = embedder
        .embed(&text)
        .map_err(|e| HostError::Refused(e.to_string()))?;

    // A scope is a second attribute, so it needs its own vector -- taken
    // here with the first, before the lock, rather than inside `commit_note`.
    let scope = match scope {
        Some(sc) => {
            let v = embedder
                .embed(&format!("{who}: scope is {sc}"))
                .map_err(|e| HostError::Refused(e.to_string()))?;
            Some((sc.to_string(), v))
        }
        None => None,
    };

    Ok(NotePlan {
        who: who.trim().to_string(),
        kind: kind.trim().to_string(),
        attribute: attribute.trim().to_string(),
        value: value.map(str::to_string),
        fields: fields.to_vec(),
        // Valid time defaults to when the store was told, which is the
        // honest answer when nobody said otherwise.
        valid_from: valid_from.unwrap_or(observed_at),
        observed_at,
        session: session.to_string(),
        scope,
        according_to,
        embedding,
    })
}

/// Write the fact, resolving who it is about.
///
/// [`Engine::remember`], never `remember_as`. `remember_as` takes an entity
/// the caller has already identified -- which is what `decide` does, and is
/// why this store reached 265 entities with an empty review queue and a
/// resolver that had never been asked to judge anything. Naming a person and
/// letting the ruleset decide whether that is someone already known is the
/// whole of what this adds.
pub fn commit_note(engine: &mut Engine, plan: NotePlan) -> Result<Outcome, HostError> {
    let NotePlan {
        who,
        kind,
        attribute,
        value,
        fields,
        valid_from,
        observed_at,
        session,
        scope,
        according_to,
        embedding,
    } = plan;

    let mut mention = Record::new()
        .with("name", who.as_str())
        .with("kind", kind.as_str());
    for (k, v) in &fields {
        mention = mention.with(k.as_str(), v.as_str());
    }

    let absent = value.is_none();
    let observation = Observation {
        kind: kind.clone(),
        mention,
        attribute: attribute.clone(),
        value,
        valid: Interval::since(valid_from),
        provenance: Provenance::new(Source::UserAssertion, observed_at, session.clone()),
        supersession: Supersession::Corrects,
        according_to,
        embedding,
    };

    let remembered = engine
        .remember(observation)
        .map_err(|e| HostError::Refused(e.to_string()))?;

    let (entity, merged, review_ids) = match remembered {
        rm_engine::Remembered::Merged { entity, .. } => (entity, true, Vec::new()),
        rm_engine::Remembered::Created { entity, .. } => (entity, false, Vec::new()),
        rm_engine::Remembered::CreatedPendingReview { entity, review, .. } => {
            (entity, false, review)
        }
    };

    // The variant carries ids; a caller needs the pair and the score to say
    // anything useful, and `pending_review` is where those live. Looked up
    // here rather than left to the host, so both hosts report the same thing.
    let reviews: Vec<rm_engine::PendingReview> = engine
        .pending_review()
        .into_iter()
        .filter(|r| review_ids.contains(&r.id))
        .cloned()
        .collect();

    // `remember_as`, not `remember`: the entity was identified by the fact
    // above, and re-resolving the same mention would ask a question already
    // answered -- and could answer it differently, leaving a fact on one
    // entity and its scope on another.
    if let Some((sc, scope_embedding)) = scope {
        engine
            .remember_as(
                Some(entity),
                Observation {
                    kind: kind.clone(),
                    mention: Record::new()
                        .with("name", who.as_str())
                        .with("kind", kind.as_str()),
                    attribute: "scope".to_string(),
                    value: Some(sc),
                    valid: Interval::since(valid_from),
                    provenance: Provenance::new(Source::UserAssertion, observed_at, session),
                    supersession: Supersession::Corrects,
                    according_to: None,
                    embedding: scope_embedding,
                },
            )
            .map_err(|e| HostError::Refused(e.to_string()))?;
    }

    Ok(Outcome::Noted {
        entity,
        attribute,
        absent,
        merged,
        according_to,
        reviews,
    })
}
/// The half of [`decide`] that talks to the embedder. No store, no lock.
#[allow(clippy::too_many_arguments)]
pub fn plan_decide(
    title: &str,
    choice: &str,
    // How far this decision reaches. Required, and never defaulted: reach
    // varies per decision, so neither the session nor the store can supply it.
    scope: &str,
    status: Option<&str>,
    because: Option<&str>,
    context: Option<&str>,
    supersedes: Option<&str>,
    // `decided_at` is valid time and only that. The transaction time stays at
    // `observed_at`, because the store did not know this in March however true
    // it was then, and moving both would make every answer it gave in between
    // retroactively wrong -- you could no longer tell a stale answer from a
    // bug, which is the whole reason `rm_store` carries two axes.
    decided_at: Option<Timestamp>,
    observed_at: Timestamp,
    session: &str,
    embedder: &impl Embedder,
) -> Result<DecidePlan, HostError> {
    if title.trim().is_empty() || choice.trim().is_empty() {
        return Err(HostError::Refused(
            "a decision needs a title and a choice: the title is how it is found again, and the choice is what was decided".into(),
        ));
    }

    // Before the embedder, so a typo costs nothing -- the same bargain the
    // status checks below make.
    crate::scope::validate(scope).map_err(HostError::Refused)?;

    let status = status.unwrap_or(DEFAULT_STATUS);
    // Refused before the embedder is called, so a typo costs nothing. Named
    // separately from an unknown status because the answer is different: this
    // one is not a mistake about the vocabulary, it is a request for the one
    // thing the vocabulary deliberately withholds.
    if status == SUPERSEDED {
        return Err(HostError::Refused(format!(
            "a decision is not marked {SUPERSEDED:?} directly -- that says another decision replaced this one, and written on its own it leaves no way to reach the decision that did. Record the new decision with `--supersedes {title:?}`, which writes both ends."
        )));
    }
    if !DECISION_STATUSES.contains(&status) {
        return Err(HostError::Refused(format!(
            "{status:?} is not a decision status. It is one of: {}",
            DECISION_STATUSES.join(", ")
        )));
    }

    let retire = match supersedes {
        None => None,
        Some(old_title) => Some((
            old_title.to_string(),
            embed_field(old_title, "status", SUPERSEDED, embedder)?,
        )),
    };

    let mut fields = Vec::with_capacity(DECISION_FIELDS.len());
    for (name, value) in [
        ("status", Some(status)),
        ("choice", Some(choice)),
        ("because", because),
        ("context", context),
        ("scope", Some(scope)),
    ] {
        let Some(value) = value.filter(|v| !v.trim().is_empty()) else {
            continue;
        };
        fields.push(embed_field(title, name, value, embedder)?);
    }

    Ok(DecidePlan {
        title: title.to_string(),
        decided_at: decided_at.unwrap_or(observed_at),
        observed_at,
        session: session.to_string(),
        retire,
        fields,
    })
}

/// The half of [`decide`] that needs the store. Touches no network.
///
/// # A title is an identifier, so it is matched exactly
///
/// Every write below goes through [`Engine::remember_as`] against an entity
/// found by exact title, rather than through `Engine::remember`, which would
/// resolve the title against every similar one already in the store.
///
/// That resolver is right for a name somebody said in a sentence and wrong for
/// a title somebody chose. Measured before this: `Adopt SQLite` followed by
/// `Adopt SQLite WAL` scored above the match threshold and became one entity,
/// keeping the first title, so the second decision existed nowhere while the
/// command reported success. Worse with `--supersedes`, because the two halves
/// of this function disagreed about what "the same title" meant --
/// `find_decision` exactly, the write fuzzily -- so a decision could retire an
/// old one by exact title and then land its own fields on that same retired
/// entity. The store was left holding one decision, under the old title,
/// marked superseded, and the new one was gone.
pub fn commit_decide(engine: &mut Engine, plan: DecidePlan) -> Result<Outcome, HostError> {
    let DecidePlan {
        title,
        decided_at,
        observed_at,
        session,
        retire,
        fields,
    } = plan;

    // Retire the old one first. If this fails, nothing has been written, and a
    // caller who re-runs the command gets one attempt rather than a duplicate
    // decision beside a still-standing predecessor.
    let mut superseded = None;
    let mut supersedes_unknown = None;
    if let Some((old_title, write)) = retire {
        match find_decision(engine, &old_title) {
            None => supersedes_unknown = Some(old_title),
            Some(old) => {
                // Retired as of the new decision's valid time: what replaced
                // it is what says when it stopped standing.
                write_field(engine, Some(old), &write, decided_at, observed_at, &session)?;
                superseded = Some((old, old_title));
            }
        }
    }

    // Resolved once, and by exact title. `None` means the store has no
    // decision under this title and the first write below creates one; every
    // write after that names the entity the first returned, so the fields of
    // one decision cannot scatter across two entities.
    let mut entity = find_decision(engine, &title);
    for write in &fields {
        let landed = write_field(engine, entity, write, decided_at, observed_at, &session)?;
        entity = Some(landed);
    }

    // The link, once both ends exist. Retiring the old decision above set its
    // status and nothing else, which left the chain unrecoverable: a reader
    // holding a decision marked `superseded` could see that something replaced
    // it and had no way to find out what. That is the one question a decision
    // log exists to answer.
    //
    // Written after the fields because the new decision may not have had an
    // entity until the loop above created one, and an edge needs both ends.
    if let (Some((old, _)), Some(new)) = (&superseded, entity) {
        // A decision that supersedes itself is not a chain, it is a loop, and
        // `rm_store::relate` refuses it -- which would abort a `decide` whose
        // fields are already written. It arises from `--supersedes` naming the
        // decision being written, so it is checked rather than risked.
        if new != *old {
            engine
                .relate(
                    new,
                    SUPERSEDES,
                    *old,
                    Interval::since(decided_at),
                    Provenance::new(Source::UserAssertion, observed_at, &session),
                )
                .map_err(|e| HostError::Refused(e.to_string()))?;
        }
    }

    Ok(Outcome::Decided {
        // `status` is always written, so the loop above always lands at least
        // once and this cannot be `None` -- but saying so with an error beats
        // an `expect` that would panic if a later edit made `status` optional.
        entity: entity.ok_or_else(|| {
            HostError::Refused(
                "a decision recorded no fields, which should not be reachable".into(),
            )
        })?,
        superseded,
        supersedes_unknown,
    })
}

/// The versions of one attribute both clocks admit, oldest first.
///
/// The raw version log filtered rather than `Engine::about`, deliberately. A
/// decision's timeline is built here from the versions themselves, so a
/// valid-time question is answered without a survivorship strategy -- and
/// therefore without depending on `[policy]`, where the shipped default is
/// `most_recent` and a valid time has nothing to index into.
/// A correction to one decision's reach, embedded and ready to write.
pub struct RescopePlan {
    title: String,
    scope: String,
    observed_at: Timestamp,
    session: String,
    field: FieldWrite,
}

/// Embed a new reach for an existing decision. No store, no lock.
///
/// # Why this is not `decide` with one argument changed
///
/// `decide` writes every field it is given, and `choice` is one of them. Two
/// of them, after a re-decide: `revisions` counts the visible versions of
/// `choice`, so re-deciding a decision purely to attach a scope leaves it
/// reading "revised 2 times" when nothing about the choice was revised. Across
/// a backfill that is every decision in the log claiming a revision that never
/// happened -- a decision log that lies about its own history is worse than
/// one missing an attribute.
///
/// This writes `scope` and touches nothing else, so the count stays true.
pub fn plan_rescope(
    title: &str,
    scope: &str,
    observed_at: Timestamp,
    session: &str,
    embedder: &impl Embedder,
) -> Result<RescopePlan, HostError> {
    if title.trim().is_empty() {
        return Err(HostError::Refused(
            "rescope needs the title of the decision to correct".into(),
        ));
    }
    // Before the embedder, so a typo costs nothing -- the same bargain
    // `plan_decide` makes.
    crate::scope::validate(scope).map_err(HostError::Refused)?;
    Ok(RescopePlan {
        title: title.to_string(),
        scope: scope.to_string(),
        observed_at,
        session: session.to_string(),
        field: embed_field(title, "scope", scope, embedder)?,
    })
}

/// Write the new reach. Touches no network.
///
/// # An unknown title is refused, never created
///
/// `decide` creates a decision when the title is new. This must not: a
/// decision holding a scope and no choice is not a decision, and the case
/// where a title does not resolve is overwhelmingly a typo -- which, during a
/// backfill of hundreds, is precisely when a silent create is most expensive
/// and least visible.
///
/// # Backfill and correction want opposite valid times, and are told apart by
/// whether a scope was already recorded
///
/// Two callers, two right answers:
///
/// * **Backfill** -- a reach that was always true and never written down. Its
///   valid time is the decision's own start, because that is when it started
///   being true.
/// * **Correction** -- the reach genuinely changed today, because an effort was
///   renamed or absorbed. Its valid time is now. Dating it from the decision's
///   start would assert the decision always reached somewhere it did not, which
///   is the rewrite-history door that putting scope on a bi-temporal attribute
///   exists to keep shut.
///
/// No flag distinguishes them, because the store already knows: a decision with
/// no scope recorded is a backfill, and one with a scope is a correction. That
/// cannot be passed wrong during a run of hundreds.
///
/// Note what dating from now does NOT do. A scope invisible at some earlier
/// clock does not hide the decision from a query at that clock -- `held`
/// returns `None`, the applicability check does not run, and the decision is
/// INCLUDED, because a decision with no scope recorded reaches everywhere. So
/// the failure a backfill dated from now would cause is over-reach in history:
/// every past query would see it as universal. That is the costly direction,
/// which is why the backfill case takes the decision's valid time.
pub fn commit_rescope(engine: &mut Engine, plan: RescopePlan) -> Result<Outcome, HostError> {
    let RescopePlan {
        title,
        scope,
        observed_at,
        session,
        field,
    } = plan;

    let Some(entity) = find_decision(engine, &title) else {
        return Err(HostError::Refused(format!(
            "no decision is titled {title:?}. rescope corrects the reach of a decision that already exists; it does not create one -- a scope with no choice under it is not a decision. Check the title against `rmem decisions`."
        )));
    };

    let at = At {
        valid: observed_at,
        tx: observed_at,
    };
    let previous = held(engine, entity, "scope", at);

    // Writing nothing is the honest answer to "set it to what it already is".
    // A no-op write would land a second identical version and make the
    // attribute's history claim a correction that never happened -- the same
    // falsified-history problem, one attribute over, that keeps this command
    // out of `decide`.
    if previous.as_deref() == Some(scope.as_str()) {
        return Ok(Outcome::Rescoped {
            entity,
            title,
            scope,
            previous,
        });
    }

    let valid_from = match previous {
        // Correction: the reach changed today, and only today.
        Some(_) => observed_at,
        // Backfill: it always reached this far. Read at the latest clock, not
        // at `at` -- reading the decision's start through a narrower clock
        // would make it depend on when the backfill happened to run.
        None => visible(engine, entity, "choice", At::latest())
            .first()
            .map_or(observed_at, |v| v.valid.from),
    };

    write_field(
        engine,
        Some(entity),
        &field,
        valid_from,
        observed_at,
        &session,
    )?;

    Ok(Outcome::Rescoped {
        entity,
        title,
        scope,
        previous,
    })
}

fn visible<'a>(engine: &'a Engine, id: StableId, attr: &str, at: At) -> Vec<&'a Version> {
    engine
        .store_history(id, attr)
        .iter()
        .filter(|v| v.provenance.observed_at <= at.tx && v.valid.from <= at.valid)
        .collect()
}

/// The value standing at `at`, or `None` if there is none.
///
/// Replaces two `latest()` closures that disagreed. `decisions` read the last
/// *non-tombstone* version and `decision` read the last version and then its
/// value, so a tombstoned `choice` showed the old choice in the list and an
/// empty one in the detail. This is `decision`'s reading: a tombstone asserts
/// the attribute has no value, and stepping past it to report a superseded one
/// would answer with something the store has been told is no longer true.
fn held(engine: &Engine, id: StableId, attr: &str, at: At) -> Option<String> {
    visible(engine, id, attr, at)
        .last()
        .and_then(|v| v.value.clone())
}

/// Every decision the store holds, most recently recorded first.
pub fn decisions(
    engine: &Engine,
    only: Option<&str>,
    at: At,
    here: Option<&str>,
) -> Result<Outcome, HostError> {
    // Checked before the scan rather than silently matching nothing. A status
    // nobody uses and a status that does not exist both return an empty list,
    // and the difference between "we have never rejected anything" and "you
    // typed `declined`" is the whole answer.
    if let Some(want) = only {
        if want != SUPERSEDED && !DECISION_STATUSES.contains(&want) {
            return Err(HostError::Refused(format!(
                "{want:?} is not a decision status. It is one of: {}, {SUPERSEDED}",
                DECISION_STATUSES.join(", ")
            )));
        }
    }
    let mut out: Vec<DecisionLine> = Vec::new();
    for id in engine.entity_ids() {
        let Some(record) = engine.identity_of(id) else {
            continue;
        };
        if record.get("kind") != Some("decision") {
            continue;
        }
        // A decision with no scope recorded reaches everywhere. That is not a
        // default for new writes -- those are refused without one -- it is how
        // records written before scopes existed read, so nothing vanishes.
        if let (Some(here), Some(reach)) = (here, held(engine, id, "scope", at)) {
            if !crate::scope::applies_at(&reach, here) {
                continue;
            }
        }
        // Not visible at `at` means the store had not heard of it yet, and the
        // existing `continue` is exactly the "absent from the list" answer.
        let Some(choice) = held(engine, id, "choice", at) else {
            continue;
        };
        let status = held(engine, id, "status", at).unwrap_or_else(|| DEFAULT_STATUS.into());
        // Read once: it decides the mark and it is shown on the line.
        let superseded_by = engine
            .edges_into(id, at.valid, at.tx)
            .iter()
            .find(|e| e.predicate == SUPERSEDES)
            .map(|e| (e.subject, title_of(engine, e.subject)));
        out.push(DecisionLine {
            entity: id,
            title: record.get("name").unwrap_or_default().to_string(),
            // A decision stands while nothing later replaced it, and there are
            // two ways to be replaced. Re-deciding under the same title writes
            // a second `choice`, and nobody sets a status when they do it --
            // which is why the history is read at all rather than trusting the
            // field. Being named by another decision's `--supersedes` writes
            // the status and leaves the choice alone.
            //
            // Reading only the first made a decision retired by `--supersedes`
            // print as standing while printing `[superseded]` beside itself,
            // which is the one combination that cannot be true.
            still_stands: superseded_by.is_none() && status == DEFAULT_STATUS,
            revisions: visible(engine, id, "choice", at).len(),
            superseded_by,
            status,
            choice,
            because: held(engine, id, "because", at),
        });
    }
    if let Some(want) = only {
        out.retain(|d| d.status == want);
    }
    out.sort_by_key(|d| std::cmp::Reverse(d.entity));
    Ok(Outcome::Decisions(out))
}

/// Everything the index needs rebuilding with, embedded before the lock.
pub struct ReindexPlan {
    vectors: Vec<(rm_engine::AssertionId, Vec<f32>)>,
    dimension: usize,
    metric: Metric,
}

/// Re-embed every assertion in the store under the current provider.
///
/// # What this is for
///
/// The store keeps a value, an interval and a provenance. The text that was
/// *embedded* is not among them -- it goes to the embedder and is dropped -- so
/// the vectors are the only surviving representation of it, and changing
/// embedding model strands every one of them. That makes choosing an embedder
/// a one-way door, which is a poor position from which to try a different one.
///
/// This is the way back, where the text can be worked out again. A decision's
/// is `"decision {title}: {attribute} is {value}"`, and title, attribute and
/// value are all in the store, so a decision log can be re-embedded by anything
/// at any time.
///
/// # Why it refuses on a mixed store
///
/// A fact that came from a conversation was embedded on a sentence the
/// extractor wrote, and that sentence is gone. Re-embedding around it would
/// leave two models' output in one index, where the distances between them are
/// not wrong but meaningless -- the failure this workspace refuses everywhere
/// else it appears. So a store holding anything but decisions is refused, and
/// says what it found.
pub fn reindex_texts(engine: &Engine) -> Result<Vec<(rm_engine::AssertionId, String)>, HostError> {
    let mut texts = Vec::new();
    let mut unreachable: Vec<String> = Vec::new();
    for (id, entity, attribute) in engine.assertion_ids() {
        let is_decision = engine
            .identity_of(entity)
            .is_some_and(|r| r.get("kind") == Some("decision"));
        if !is_decision || !DECISION_FIELDS.contains(&attribute.as_str()) {
            if unreachable.len() < 3 {
                unreachable.push(format!("entity {entity}'s {attribute}"));
            }
            continue;
        }
        // Rebuilt exactly as `embed_field` composes it. The two have to agree
        // or a rebuilt vector lands somewhere its original never was; the test
        // `a_rebuilt_decision_vector_matches_the_one_decide_wrote` is what
        // holds them together.
        let title = title_of(engine, entity);
        let value = engine
            .store_history(entity, &attribute)
            .last()
            .and_then(|v| v.value.clone())
            .unwrap_or_default();
        texts.push((id, format!("decision {title}: {attribute} is {value}")));
    }

    if !unreachable.is_empty() {
        return Err(HostError::Refused(format!(
            "this store holds assertions that cannot be re-embedded, so rebuilding the index would leave two models' output in it and every distance between them meaningless. Only decisions carry text that can be worked out again -- a fact from a conversation was embedded on a sentence the extractor wrote, and that sentence is not kept. Found, for example: {}.",
            unreachable.join(", ")
        )));
    }

    Ok(texts)
}

/// Embed what [`reindex_texts`] worked out. No store, no lock.
///
/// Separate so the read that enumerates the store and the network calls that
/// re-embed it do not happen under one lock -- the same split `remember` and
/// `decide` already make, for the same reason.
///
/// Between the read and the commit another writer may add an assertion. Nothing
/// here notices, and nothing needs to: the commit covers N of N+1 and
/// [`Engine::rebuild_index`] refuses it by count, so the failure is a message
/// and a re-run rather than an index missing a vector.
pub fn plan_reindex(
    texts: Vec<(rm_engine::AssertionId, String)>,
    embedder: &impl Embedder,
    dimension: usize,
    metric: Metric,
) -> Result<ReindexPlan, HostError> {
    let mut vectors = Vec::with_capacity(texts.len());
    for (id, text) in texts {
        let v = embedder
            .embed(&text)
            .map_err(|e| HostError::Refused(e.to_string()))?;
        vectors.push((id, v));
    }
    Ok(ReindexPlan {
        vectors,
        dimension,
        metric,
    })
}

/// Swap in the index [`plan_reindex`] built. Touches no network.
pub fn commit_reindex(engine: &mut Engine, plan: ReindexPlan) -> Result<Outcome, HostError> {
    let assertions = plan.vectors.len();
    engine
        .rebuild_index(plan.dimension, plan.metric, plan.vectors)
        .map_err(|e| HostError::Refused(e.to_string()))?;
    Ok(Outcome::Reindexed {
        assertions,
        dimension: plan.dimension,
    })
}

/// One decision in full, found by exact title.
///
/// The command the log exists for. `decisions` says *that* something was
/// replaced; this says by what, and walks the chain to whatever stands now, so
/// a reader who arrives at a retired decision is carried to the live one
/// rather than left to guess.
pub fn decision(
    engine: &Engine,
    title: &str,
    at: At,
    here: Option<&str>,
) -> Result<Outcome, HostError> {
    let Some(id) = find_decision(engine, title) else {
        return Ok(Outcome::Decision(Found::Unknown));
    };
    // `status` is always written by `commit_decide`, so its absence at `at`
    // means the store had not heard of this decision at all -- not that a field
    // is missing. The same fact the `Outcome::Decided` construction relies on.
    if held(engine, id, "status", at).is_none() {
        let versions = engine.store_history(id, "status");
        let first_recorded = versions
            .iter()
            .map(|v| v.provenance.observed_at)
            .min()
            .ok_or_else(|| {
                HostError::Refused(
                    "a decision recorded no status, which should not be reachable".into(),
                )
            })?;
        let first_held = versions
            .iter()
            .map(|v| v.valid.from)
            .min()
            .unwrap_or(first_recorded);
        return Ok(Outcome::Decision(Found::NotYetRecorded {
            title: title.to_string(),
            first_recorded,
            first_held,
        }));
    }
    // Existence first, reach second. A decision the store had not heard of yet
    // is a different answer from one it has heard of and that does not apply.
    if let (Some(here), Some(reach)) = (here, held(engine, id, "scope", at)) {
        if !crate::scope::applies_at(&reach, here) {
            return Ok(Outcome::Decision(Found::NotHere {
                title: title.to_string(),
                scope: reach,
                asked_from: here.to_string(),
            }));
        }
    }
    let history: Vec<(Timestamp, String)> = visible(engine, id, "choice", at)
        .iter()
        // The day it was decided, not the day the store was told. They are
        // the same unless the decision was backdated, and when they differ the
        // decided day is what a log is a log of -- "we chose this in March" is
        // the entry, and "we typed it up in August" is not.
        .filter_map(|v| Some((v.valid.from, v.value.clone()?)))
        .collect();
    let status = held(engine, id, "status", at).unwrap_or_else(|| DEFAULT_STATUS.into());
    let superseded_by = chain(engine, id, Direction::Forward, at);

    Ok(Outcome::Decision(Found::Decision(Box::new(
        DecisionDetail {
            entity: id,
            title: title.to_string(),
            choice: held(engine, id, "choice", at).unwrap_or_default(),
            because: held(engine, id, "because", at),
            context: held(engine, id, "context", at),
            still_stands: superseded_by.is_empty() && status == DEFAULT_STATUS,
            status,
            supersedes: chain(engine, id, Direction::Back, at),
            superseded_by,
            history,
            answered_at: (at != At::latest()).then_some(at),
        },
    ))))
}

/// Which way along the supersession edges to walk.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Towards what this decision replaced.
    Back,
    /// Towards whatever stands now.
    Forward,
}

/// Follow the supersession edges from `start`, not including it.
///
/// Cycle-guarded. `commit_decide` refuses to link a decision to itself, but a
/// longer loop -- A supersedes B, B supersedes A, written by two commands that
/// each looked reasonable -- is reachable, and a walk that trusted the data
/// would hang rather than report it. The visited set costs nothing on a chain
/// of realistic length and turns an infinite loop into a short answer.
fn chain(engine: &Engine, start: StableId, dir: Direction, at: At) -> Vec<(StableId, String)> {
    let mut out = Vec::new();
    let mut seen = vec![start];
    let mut cursor = start;
    loop {
        let edges = match dir {
            Direction::Back => engine.edges_from(cursor, at.valid, at.tx),
            Direction::Forward => engine.edges_into(cursor, at.valid, at.tx),
        };
        let next = edges
            .iter()
            .find(|e| e.predicate == SUPERSEDES)
            .map(|e| match dir {
                Direction::Back => e.object,
                Direction::Forward => e.subject,
            });
        let Some(next) = next else { return out };
        if seen.contains(&next) {
            return out;
        }
        seen.push(next);
        out.push((next, title_of(engine, next)));
        cursor = next;
    }
}

/// The title a decision entity carries, or a placeholder naming the entity.
fn title_of(engine: &Engine, id: StableId) -> String {
    engine
        .identity_of(id)
        .and_then(|r| r.get("name").map(str::to_string))
        .unwrap_or_else(|| format!("(untitled entity {id})"))
}

/// The entity a decision with this title lives on, if the store has one.
fn find_decision(engine: &Engine, title: &str) -> Option<StableId> {
    engine.entity_ids().into_iter().find(|id| {
        engine.identity_of(*id).is_some_and(|r| {
            r.get("kind") == Some("decision") && r.get("name").is_some_and(|n| n == title)
        })
    })
}

/// One field of one decision, embedded so it can be found again.
///
/// The embedded text names the decision as well as the field, because "because
/// = the other one locks us into their release cycle" is not findable on its
/// own -- a search for why a decision was made has to be able to reach the
/// reason through the decision's own title.
#[allow(clippy::too_many_arguments)]
/// Embed one decision field. The embedder call, and nothing else.
fn embed_field(
    title: &str,
    attribute: &str,
    value: &str,
    embedder: &impl Embedder,
) -> Result<FieldWrite, HostError> {
    debug_assert!(DECISION_FIELDS.contains(&attribute));
    let embedding = embedder
        .embed(&format!("decision {title}: {attribute} is {value}"))
        .map_err(|e| HostError::Refused(e.to_string()))?;
    Ok(FieldWrite {
        title: title.to_string(),
        attribute: attribute.to_string(),
        value: value.to_string(),
        embedding,
    })
}

/// Write one decision field onto `entity`, or onto a new one when `None`.
///
/// [`Engine::remember_as`] rather than `Engine::remember`: the caller has
/// already decided which entity this is, by exact title. See
/// [`commit_decide`] for what happens when that decision is left to the
/// resolver instead.
fn write_field(
    engine: &mut Engine,
    entity: Option<StableId>,
    write: &FieldWrite,
    decided_at: Timestamp,
    observed_at: Timestamp,
    session: &str,
) -> Result<StableId, HostError> {
    let FieldWrite {
        title,
        attribute,
        value,
        embedding,
    } = write;
    let (landed, _) = engine
        .remember_as(
            entity,
            Observation {
                kind: "decision".to_string(),
                mention: Record::new().with("name", title).with("kind", "decision"),
                attribute: attribute.to_string(),
                value: Some(value.to_string()),
                // Valid from when it was decided; observed when the store was
                // told. The same for a decision recorded as it is made, and
                // apart for one recorded later -- which is the case the two
                // axes exist for.
                valid: Interval::since(decided_at),
                // `UserAssertion`, not `ToolOutput`: nobody inferred this from
                // a sentence. Somebody decided it and said so.
                provenance: Provenance::new(Source::UserAssertion, observed_at, session),
                supersession: Supersession::Corrects,
                according_to: None,
                embedding: embedding.clone(),
            },
        )
        .map_err(|e| HostError::Refused(e.to_string()))?;
    Ok(landed)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::TempDir;
    use rm_embed::Hashed;

    #[test]
    fn init_writes_a_config_whose_dimension_came_from_the_model() {
        // Not from a default and not from the user. A dimension that disagrees
        // with the embedding model makes every distance meaningless, and
        // nothing reports it.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let out = init(&path, false, false, None, &|| Ok(1536)).unwrap();

        assert_eq!(
            out,
            Outcome::Initialised {
                path: path.clone(),
                dimension: 1536,
                local: false,
                replaced_unparsable: None,
            }
        );
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("dimension = 1536"), "{written}");
    }

    #[test]
    fn init_writes_the_dimension_the_probe_reported_not_the_one_in_the_template() {
        // The template carries 1536 as an example. If init copied it verbatim
        // the whole probe would be theatre, and a 3072-dimension model would
        // silently produce a broken store.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        init(&path, false, false, None, &|| Ok(3072)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("dimension = 3072"), "{written}");
        assert!(!written.contains("dimension = 1536"), "{written}");
    }

    #[test]
    fn init_refuses_to_overwrite_an_existing_config() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "# hand-edited, do not lose").unwrap();

        let err = init(&path, false, false, None, &|| Ok(1536)).unwrap_err();
        assert!(err.to_string().contains("--force"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# hand-edited, do not lose",
            "the existing file must be untouched"
        );
    }

    #[test]
    fn init_refuses_an_existing_config_without_ever_calling_the_probe() {
        // The doc comment on `init` says the existence check comes before the
        // probe: a user who already has a config should not need a working API
        // key and a live model just to be told the file exists. Nothing had
        // pinned that ordering -- a reviewer once swapped the probe ahead of
        // the existence check and the rest of the suite still passed, because
        // every other test's probe happens to succeed either way. A probe that
        // panics if it is ever called is the only way to catch the swap.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "# hand-edited, do not lose").unwrap();

        let err = init(&path, false, false, None, &|| {
            panic!("the probe must not run")
        })
        .unwrap_err();
        assert!(err.to_string().contains("--force"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# hand-edited, do not lose",
            "the existing file must be untouched"
        );
    }

    #[test]
    fn init_force_overwrites_and_says_it_did() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "# old").unwrap();
        init(&path, true, false, None, &|| Ok(768)).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("dimension = 768"));
    }

    #[test]
    fn a_replaced_unparsable_notice_skips_the_exists_check_and_rides_along_in_the_outcome() {
        // `run` is what decides a file could not be parsed and passes that
        // decision down as `Some(..)`; this only has to honour it once it
        // arrives, both by writing over the file without demanding `force`
        // *and* by carrying the notice into the `Outcome` so `format` has
        // something to show.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "# not valid toml at all [[[").unwrap();

        let notice = "rmem.toml is not valid: that is not valid TOML (line 1, column 1)";
        let out = init(&path, false, false, Some(notice.to_string()), &|| Ok(1536)).unwrap();

        assert_eq!(
            out,
            Outcome::Initialised {
                path: path.clone(),
                dimension: 1536,
                local: false,
                replaced_unparsable: Some(notice.to_string()),
            }
        );
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("dimension = 1536"));
    }

    #[test]
    fn init_writes_nothing_when_the_probe_fails() {
        // Half a config is worse than none: the next command would read it and
        // fail somewhere less obvious.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let err = init(&path, false, false, None, &|| {
            Err("quota exceeded".to_string())
        })
        .unwrap_err();
        assert!(err.to_string().contains("quota exceeded"), "{err}");
        assert!(!path.exists(), "no config may be left behind");
    }

    #[test]
    fn what_init_writes_is_what_the_config_loader_reads() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        init(&path, false, false, None, &|| Ok(1536)).unwrap();
        let config = crate::config::Config::load(&path).unwrap();
        assert_eq!(config.provider.dimension, 1536);
        config.ruleset().unwrap();
        config.policy_for_engine().unwrap();
    }

    use crate::testing::StubProvider;
    use rm_engine::{Metric, VectorIndex};

    const EXTRACTION: &str = r#"{"mentions":[
        {"kind":"person","name":"Ben Severn","text":"Ben"},
        {"kind":"organisation","name":"Globex","text":"Globex"}],
      "facts":[{"subject":0,"attribute":"employer","value":"Globex",
                "text":"Ben works at Globex","days_ago":null}],
      "relations":[{"subject":0,"predicate":"employed_by","object":1,"days_ago":null}],
      "closures":[]}"#;

    pub(crate) fn engine() -> Engine {
        let config: crate::config::Config = toml::from_str(crate::config::TEMPLATE).unwrap();
        Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            config.ruleset().unwrap(),
            config.policy_for_engine().unwrap(),
        )
    }

    // ---- a fact you already know -----------------------------------------
    //
    // These use `engine()`, which builds from the shipped TEMPLATE -- so the
    // ruleset and its thresholds are the ones a real store uses. A test
    // ruleset of its own would prove the resolver works on a ruleset nobody
    // runs.

    /// A name nobody has mentioned creates an entity.
    #[test]
    fn a_note_about_someone_new_creates_them() {
        let mut e = engine();
        let plan = plan_note(
            "Jon Severn",
            "person",
            "role",
            Some("leads circ"),
            &[],
            None,
            100,
            "test",
            None,
            None,
            &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted {
            entity,
            merged,
            reviews,
            ..
        } = commit_note(&mut e, plan).unwrap()
        else {
            panic!("expected Noted")
        };
        assert!(!merged, "nothing was there to merge onto");
        assert!(reviews.is_empty(), "{reviews:?}");
        assert_eq!(
            e.about(entity, "role", Timestamp::MAX, Timestamp::MAX)
                .unwrap(),
            Believed::Value("leads circ".into())
        );
    }

    /// The same name again lands on the same entity rather than a second one.
    ///
    /// This is the resolver doing its job, and it is the first time anything
    /// in this store has asked it to.
    #[test]
    fn a_second_note_about_the_same_name_joins_the_first() {
        let mut e = engine();
        let first = plan_note(
            "Jon Severn",
            "person",
            "role",
            Some("leads circ"),
            &[],
            None,
            100,
            "test",
            None,
            None,
            &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity: a, .. } = commit_note(&mut e, first).unwrap() else {
            panic!("expected Noted")
        };

        let second = plan_note(
            "Jon Severn",
            "person",
            "team",
            Some("circulation"),
            &[],
            None,
            200,
            "test",
            None,
            None,
            &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted {
            entity: b, merged, ..
        } = commit_note(&mut e, second).unwrap()
        else {
            panic!("expected Noted")
        };

        assert_eq!(a, b, "the same person twice is one entity");
        assert!(merged, "and the second write should say so");
        assert_eq!(e.entity_count(), 1);
    }

    /// `--absent` asserts there is no value, which is not the same as never
    /// having been asked. The store's own instructions open with this
    /// distinction and no write path could express it before.
    #[test]
    fn an_absence_is_asserted_rather_than_left_unknown() {
        let mut e = engine();
        let plan = plan_note(
            "Jon Severn",
            "person",
            "reports",
            None,
            &[],
            None,
            100,
            "test",
            None,
            None,
            &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, absent, .. } = commit_note(&mut e, plan).unwrap() else {
            panic!("expected Noted")
        };
        assert!(absent);

        // Asserted absence, and an attribute nobody mentioned. Two different
        // answers, and collapsing them is the failure this guards.
        assert_eq!(
            e.about(entity, "reports", Timestamp::MAX, Timestamp::MAX)
                .unwrap(),
            Believed::Absent
        );
        assert_eq!(
            e.about(entity, "spouse", Timestamp::MAX, Timestamp::MAX)
                .unwrap(),
            Believed::Unknown
        );
    }

    /// `--valid-from` is valid time and only that: the store learned it now,
    /// and it was true earlier.
    #[test]
    fn a_backdated_note_is_true_from_when_it_started_being_true() {
        let mut e = engine();
        let plan = plan_note(
            "Jon Severn",
            "person",
            "role",
            Some("leads circ"),
            &[],
            Some(50),
            100,
            "test",
            None,
            None,
            &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, .. } = commit_note(&mut e, plan).unwrap() else {
            panic!("expected Noted")
        };
        // True at 60, which is before the store was told at 100.
        assert_eq!(
            e.about(entity, "role", 60, Timestamp::MAX).unwrap(),
            Believed::Value("leads circ".into())
        );
    }

    /// Mention fields reach the identity record, so a later ruleset can
    /// compare them without every record being rewritten.
    #[test]
    fn a_mention_field_lands_on_the_identity_not_the_attributes() {
        let mut e = engine();
        let plan = plan_note(
            "Jon Severn",
            "person",
            "role",
            Some("leads circ"),
            &[("email".to_string(), "j@example.com".to_string())],
            None,
            100,
            "test",
            None,
            None,
            &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, .. } = commit_note(&mut e, plan).unwrap() else {
            panic!("expected Noted")
        };
        let identity = e
            .identity_of(entity)
            .expect("a noted entity has an identity");
        assert_eq!(identity.get("email"), Some("j@example.com"));
        assert_eq!(identity.get("name"), Some("Jon Severn"));
        // And it is not an attribute: `email` was never noted as one.
        assert_eq!(
            e.about(entity, "email", Timestamp::MAX, Timestamp::MAX)
                .unwrap(),
            Believed::Unknown
        );
    }

    /// An empty name is refused before the embedder, so a typo costs nothing
    /// -- the same bargain `plan_decide` makes.
    #[test]
    fn a_note_about_nobody_is_refused_before_it_costs_an_embedding() {
        let err = plan_note(
            "   ",
            "person",
            "role",
            Some("x"),
            &[],
            None,
            100,
            "test",
            None,
            None,
            &Hashed::new(3),
        )
        .unwrap_err();
        assert!(format!("{err}").contains("who"), "{err}");
    }

    /// A scoped note reaches only where it says, and an unscoped one reaches
    /// everywhere -- the same applicability rule the decision reads use.
    #[test]
    fn a_note_can_be_scoped_and_is_otherwise_everywhere() {
        let mut e = engine();
        let plan = plan_note(
            "Jon Severn",
            "person",
            "oncall",
            Some("tuesdays"),
            &[],
            None,
            100,
            "test",
            Some("work/circ-tools"),
            None,
            &Hashed::new(3),
        )
        .unwrap();
        let Outcome::Noted { entity, .. } = commit_note(&mut e, plan).unwrap() else {
            panic!("expected Noted")
        };
        assert_eq!(
            e.about(entity, "scope", Timestamp::MAX, Timestamp::MAX)
                .unwrap(),
            Believed::Value("work/circ-tools".into())
        );
    }
    // ---- the lock and the network ------------------------------------------

    /// Can an exclusive lock on this store be taken right now?
    ///
    /// A second handle on the same file: `flock` conflicts between open file
    /// descriptions, so this contends with a lock the same process holds
    /// exactly as another process would.
    fn lock_is_free(store: &std::path::Path) -> bool {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(crate::store::lock_path(store))
            .unwrap();
        f.try_lock().is_ok()
    }

    /// A provider that checks, on every call, whether the store is locked.
    struct ProbesTheLock<'a> {
        inner: StubProvider,
        store: &'a std::path::Path,
        calls: std::cell::Cell<usize>,
        always_free: std::cell::Cell<bool>,
    }

    impl<'a> ProbesTheLock<'a> {
        fn new(store: &'a std::path::Path, responses: Vec<&str>) -> Self {
            ProbesTheLock {
                inner: StubProvider::new(responses),
                store,
                calls: std::cell::Cell::new(0),
                always_free: std::cell::Cell::new(true),
            }
        }

        fn probe(&self) {
            self.calls.set(self.calls.get() + 1);
            if !lock_is_free(self.store) {
                self.always_free.set(false);
            }
        }
    }

    impl Embedder for ProbesTheLock<'_> {
        fn embed(&self, text: &str) -> Result<Vec<f32>, rm_engine::EmbedderError> {
            self.probe();
            self.inner.embed(text)
        }
    }

    impl Completer for ProbesTheLock<'_> {
        fn complete(&self, prompt: &str) -> Result<String, rm_extract::CompleterError> {
            self.probe();
            self.inner.complete(prompt)
        }
    }

    /// The store's lock is free the whole time a model is being called.
    ///
    /// This is what the plan/commit split is *for*, stated as something
    /// observable rather than as a shape. Every model call used to happen
    /// inside `with_write`, so an extraction and a set of embeddings -- seconds
    /// each, across a network -- were held against every other writer, and
    /// `Lock::acquire` gives up after five. Measured on a live store before
    /// this change, the fourth concurrent writer was refused.
    ///
    /// The compiler already enforces most of it: `commit_remember` and
    /// `commit_decide` take no embedder and no completer, so nothing reached
    /// from inside the lock *can* call one. This covers the other half -- that
    /// the plan really is built before the lock is taken -- which no signature
    /// can state.
    #[test]
    fn the_store_lock_is_free_while_a_model_is_being_called() {
        let dir = TempDir::new();
        let store = dir.path().join("memory.json");
        let config: crate::config::Config = toml::from_str(crate::config::TEMPLATE).unwrap();
        let (ruleset, policy) = (
            config.ruleset().unwrap(),
            config.policy_for_engine().unwrap(),
        );
        let shape = || {
            (
                config.ruleset().unwrap(),
                config.policy_for_engine().unwrap(),
                3,
                Metric::Cosine,
            )
        };

        // A turn: one completion for the extraction, then one embedding per
        // mention and per fact.
        let probe = ProbesTheLock::new(&store, vec![EXTRACTION]);
        let plan = plan_remember(
            "Ben moved to Chicago.",
            100,
            "test",
            Some("Ben"),
            &probe,
            &probe,
            3,
            Metric::Cosine,
        )
        .unwrap();
        // Not vacuous: the probe has to have actually run. A plan that called
        // no model would pass the assertion below while proving nothing.
        assert!(
            probe.calls.get() > 1,
            "the probe saw {} calls -- it must see the completion and at least one embedding, or it is asserting nothing",
            probe.calls.get()
        );
        assert!(
            probe.always_free.get(),
            "the store was locked while a model was being called"
        );

        let (r, p, d, m) = shape();
        crate::store::with_write(&store, r, p, d, m, |engine| commit_remember(engine, plan))
            .unwrap();

        // The same again for a decision, which reaches no completer but did
        // hold the lock across four sequential embeddings.
        let probe = ProbesTheLock::new(&store, vec![]);
        let plan = plan_decide(
            "Pin the toolchain",
            "rust-toolchain.toml names the version",
            "work",
            None,
            Some("CI and a working copy were answering different questions"),
            None,
            None,
            None,
            200,
            "test",
            &probe,
        )
        .unwrap();
        assert_eq!(
            probe.calls.get(),
            4,
            "status, choice, because and scope -- one embedding each"
        );
        assert!(
            probe.always_free.get(),
            "the store was locked while a decision was being embedded"
        );

        let (r, p, d, m) = shape();
        crate::store::with_write(&store, r, p, d, m, |engine| commit_decide(engine, plan)).unwrap();

        // And the guard that makes the two assertions above mean something:
        // the probe *can* see a held lock. Without this the test would pass
        // just as happily against a `lock_is_free` that always said yes.
        let mut saw_locked = None;
        crate::store::with_write(&store, ruleset, policy, 3, Metric::Cosine, |_| {
            saw_locked = Some(lock_is_free(&store));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            saw_locked,
            Some(false),
            "the probe cannot detect a held lock, so it proves nothing about an unheld one"
        );
    }

    // ---- decisions ---------------------------------------------------------

    /// Three decisions at three reaches, asked from one position.
    #[test]
    fn a_read_returns_what_applies_where_it_is_asked_from() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        let mut t = 1_000;
        for (title, scope) in [
            ("Machine wide", "*"),
            ("Work wide", "work"),
            ("This project", "work/goldenmatch"),
            ("A sibling", "work/other"),
            ("Personal", "personal"),
        ] {
            decide(
                &mut e, title, "a choice", scope, None, None, None, None, None, t, "t", &stub,
            )
            .unwrap();
            t += 10;
        }

        let titles = |here: Option<&str>| {
            let Outcome::Decisions(ds) = decisions(&e, None, At::latest(), here).unwrap() else {
                panic!("decisions did not return decisions")
            };
            let mut out: Vec<String> = ds.into_iter().map(|d| d.title).collect();
            out.sort();
            out
        };

        assert_eq!(
            titles(Some("work/goldenmatch")),
            vec![
                "Machine wide".to_string(),
                "This project".to_string(),
                "Work wide".to_string()
            ],
            "ancestor-or-self, and nothing beside it"
        );
        assert_eq!(
            titles(Some("personal")),
            vec!["Machine wide".to_string(), "Personal".to_string()]
        );
        // No position, no filtering. This is `--all`, and it is also every
        // caller that never set RMEM_SCOPE.
        assert_eq!(titles(None).len(), 5);
    }

    /// An exact title that exists but does not reach here is its own answer,
    /// for the same reason `NotYetRecorded` is: the title resolved, so "no
    /// decision by that title" would read as a spelling mistake.
    #[test]
    fn a_title_out_of_reach_says_where_it_lives() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "A sibling",
            "a choice",
            "work/other",
            None,
            None,
            None,
            None,
            None,
            1_000,
            "t",
            &stub,
        )
        .unwrap();

        assert_eq!(
            decision(&e, "A sibling", At::latest(), Some("work/goldenmatch")).unwrap(),
            Outcome::Decision(Found::NotHere {
                title: "A sibling".to_string(),
                scope: "work/other".to_string(),
                asked_from: "work/goldenmatch".to_string(),
            })
        );

        // Asked from where it lives, or from nowhere, it is just a decision.
        for here in [Some("work/other"), None] {
            assert!(matches!(
                decision(&e, "A sibling", At::latest(), here).unwrap(),
                Outcome::Decision(Found::Decision(_))
            ));
        }
    }

    /// The precondition the legacy rule rests on: an unwritten scope reads as
    /// `None`, and `None` means the rule does not apply rather than that the
    /// decision fails it.
    #[test]
    fn a_decision_with_no_scope_recorded_reaches_everywhere() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e, "Legacy", "a choice", "work", None, None, None, None, None, 1_000, "t", &stub,
        )
        .unwrap();
        let id = find_decision(&e, "Legacy").expect("recorded");
        let before = At {
            valid: Timestamp::MAX,
            tx: 999,
        };
        assert_eq!(
            held(&e, id, "scope", before),
            None,
            "no scope at that clock"
        );
    }

    /// A scope is required and validated before the embedder is called, so a
    /// typo costs nothing -- the same bargain the status check already makes.
    #[test]
    fn a_decision_states_its_reach_or_is_refused() {
        let stub = StubProvider::new(vec![]);
        let plan = |scope: &str| {
            plan_decide(
                "Pin the compiler",
                "rust-toolchain.toml names the version",
                scope,
                None,
                None,
                None,
                None,
                None,
                1_000,
                "t",
                &stub,
            )
        };

        assert!(plan("work/goldenmatch").is_ok());
        assert!(plan(crate::scope::UNIVERSAL).is_ok());

        let Err(HostError::Refused(why)) = plan("") else {
            panic!("an unscoped decision should be refused")
        };
        assert!(why.contains("how far"), "{why}");

        let Err(HostError::Refused(why)) = plan("work/*") else {
            panic!("a wildcard segment should be refused")
        };
        assert!(why.contains('*'), "{why}");
    }

    /// The scope is stored like any other field, so it is versioned, readable
    /// at a past clock, and rebuilt by `reindex`.
    #[test]
    fn a_scope_is_an_attribute_like_the_others() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Pin the compiler",
            "a choice",
            "work/goldenmatch",
            None,
            None,
            None,
            None,
            None,
            1_000,
            "t",
            &stub,
        )
        .unwrap();
        let id = find_decision(&e, "Pin the compiler").expect("recorded");
        assert_eq!(
            held(&e, id, "scope", At::latest()),
            Some("work/goldenmatch".to_string())
        );
    }

    /// Three answers, not two. A title that resolves but was recorded later is
    /// its own case: reporting "no such decision" would read as a typo.
    #[test]
    fn a_decision_not_yet_recorded_is_distinguished_from_one_that_does_not_exist() {
        const MARCH: Timestamp = 1_772_236_800_000;
        const AUGUST: Timestamp = 1_787_532_411_419;
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Pin the compiler",
            "a choice",
            "work",
            None,
            None,
            None,
            None,
            Some(MARCH),
            AUGUST,
            "t",
            &stub,
        )
        .unwrap();

        // Backdated to March but recorded in August: as of March the store knew
        // nothing, even though the decision claims to have held then.
        let Outcome::Decision(found) = decision(
            &e,
            "Pin the compiler",
            At {
                valid: Timestamp::MAX,
                tx: MARCH,
            },
            None,
        )
        .unwrap() else {
            panic!("not a decision outcome")
        };
        assert_eq!(
            found,
            Found::NotYetRecorded {
                title: "Pin the compiler".to_string(),
                first_recorded: AUGUST,
                first_held: MARCH,
            },
            "both clocks are reported: it was typed up in August and claims March"
        );

        // The other axis excludes it too, and for a different reason. Asking
        // what held in January, with everything the store knows, still finds
        // nothing -- and the two days above are what tell those cases apart.
        assert!(matches!(
            decision(
                &e,
                "Pin the compiler",
                At {
                    valid: 1,
                    tx: Timestamp::MAX
                },
                None
            )
            .unwrap(),
            Outcome::Decision(Found::NotYetRecorded { .. })
        ));

        // A title nobody ever used is a different answer.
        assert_eq!(
            decision(&e, "Never decided", At::latest(), None).unwrap(),
            Outcome::Decision(Found::Unknown)
        );

        // And now, it is there.
        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Pin the compiler", At::latest(), None).unwrap()
        else {
            panic!("expected a decision")
        };
        assert_eq!(d.choice, "a choice");
    }

    /// The chain is walked at the clock too: a supersession recorded later does
    /// not reach back and retire a decision in the past.
    #[test]
    fn a_later_supersession_does_not_reach_back() {
        const MARCH: Timestamp = 1_772_236_800_000;
        const AUGUST: Timestamp = 1_787_532_411_419;
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "First",
            "the old way",
            "work",
            None,
            None,
            None,
            None,
            None,
            MARCH,
            "t",
            &stub,
        )
        .unwrap();
        decide(
            &mut e,
            "Second",
            "the new way",
            "work",
            None,
            None,
            None,
            Some("First"),
            None,
            AUGUST,
            "t",
            &stub,
        )
        .unwrap();

        let stands = |at: At| {
            let Outcome::Decision(Found::Decision(d)) = decision(&e, "First", at, None).unwrap()
            else {
                panic!("expected a decision")
            };
            (d.still_stands, d.superseded_by.len())
        };

        assert_eq!(
            stands(At {
                valid: Timestamp::MAX,
                tx: MARCH
            }),
            (true, 0),
            "in March nothing had replaced it"
        );
        assert_eq!(stands(At::latest()), (false, 1), "August replaced it");
    }

    /// A decision the store had not yet heard of is not in the list, and the
    /// count of revisions is the count it had then.
    #[test]
    fn the_list_is_answered_as_of_a_transaction_time() {
        const MARCH: Timestamp = 1_772_236_800_000;
        const AUGUST: Timestamp = 1_787_532_411_419;
        let mut e = engine();
        let stub = StubProvider::new(vec![]);

        decide(
            &mut e,
            "Early",
            "chosen in March",
            "work",
            None,
            None,
            None,
            None,
            Some(MARCH),
            MARCH,
            "t",
            &stub,
        )
        .unwrap();
        decide(
            &mut e,
            "Late",
            "chosen in August",
            "work",
            None,
            None,
            None,
            None,
            Some(AUGUST),
            AUGUST,
            "t",
            &stub,
        )
        .unwrap();
        decide(
            &mut e,
            "Early",
            "revised in August",
            "work",
            None,
            None,
            None,
            None,
            Some(AUGUST),
            AUGUST,
            "t",
            &stub,
        )
        .unwrap();

        let titles = |at: At| {
            let Outcome::Decisions(ds) = decisions(&e, None, at, None).unwrap() else {
                panic!("decisions did not return decisions")
            };
            ds.into_iter()
                .map(|d| (d.title, d.choice, d.revisions))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            titles(At {
                valid: Timestamp::MAX,
                tx: MARCH
            }),
            vec![("Early".to_string(), "chosen in March".to_string(), 1)],
            "August had not happened yet"
        );

        let now = titles(At::latest());
        assert_eq!(now.len(), 2, "both decisions exist now");
        let early = now.iter().find(|(t, ..)| t == "Early").unwrap();
        assert_eq!(early.1, "revised in August");
        assert_eq!(early.2, 2, "revised once, so two choices");
    }

    /// Both axes bite, and a tombstone is an answer rather than something to
    /// skip past.
    #[test]
    fn a_visible_version_is_one_both_clocks_admit() {
        const MARCH: Timestamp = 1_772_236_800_000; // 2026-02-28
        const AUGUST: Timestamp = 1_787_532_411_419; // 2026-08-24
        let mut e = engine();
        let stub = StubProvider::new(vec![]);

        // Decided in March, recorded in March.
        decide(
            &mut e,
            "Pin the compiler",
            "first choice",
            "work",
            None,
            None,
            None,
            None,
            Some(MARCH),
            MARCH,
            "t",
            &stub,
        )
        .unwrap();
        // Re-decided in August under the same title.
        decide(
            &mut e,
            "Pin the compiler",
            "second choice",
            "work",
            None,
            None,
            None,
            None,
            Some(AUGUST),
            AUGUST,
            "t",
            &stub,
        )
        .unwrap();

        let id = find_decision(&e, "Pin the compiler").expect("recorded");

        // As of March the store had heard only the first.
        assert_eq!(
            held(
                &e,
                id,
                "choice",
                At {
                    valid: Timestamp::MAX,
                    tx: MARCH
                }
            ),
            Some("first choice".to_string())
        );
        // As of now it has both, and the later one is the answer.
        assert_eq!(
            held(&e, id, "choice", At::latest()),
            Some("second choice".to_string())
        );
        // Valid time alone: in March the second had not begun to hold.
        assert_eq!(
            held(
                &e,
                id,
                "choice",
                At {
                    valid: MARCH,
                    tx: Timestamp::MAX
                }
            ),
            Some("first choice".to_string())
        );
        // Before either clock, nothing at all.
        assert_eq!(held(&e, id, "choice", At { valid: 1, tx: 1 }), None);
        assert!(visible(&e, id, "choice", At { valid: 1, tx: 1 }).is_empty());
        assert_eq!(visible(&e, id, "choice", At::latest()).len(), 2);
    }

    /// Every decision in the store, as (title, entity, still standing).
    fn recorded(e: &mut Engine) -> Vec<(String, StableId, bool)> {
        let Outcome::Decisions(ds) = decisions(e, None, At::latest(), None).unwrap() else {
            panic!("decisions did not return decisions")
        };
        ds.iter()
            .map(|d| (d.title.clone(), d.entity, d.still_stands))
            .collect()
    }

    /// Two decisions whose titles are nearly the same are still two decisions.
    ///
    /// They were one. `decide` wrote through `Engine::remember`, which scores a
    /// title against every similar one already stored, and these two land above
    /// the match threshold: the second decision's fields went onto the first
    /// entity, which kept the *first* title, so the second existed nowhere and
    /// the command reported success.
    ///
    /// The pair is not contrived -- it is the shape a real log takes, a
    /// decision and a later refinement of it. `Pin the toolchain` against
    /// `Pin the toolchain version` stays under the threshold and always did,
    /// so it would not have caught this.
    #[test]
    fn a_decision_is_never_merged_into_one_whose_title_merely_resembles_it() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Adopt SQLite",
            "yes",
            "work",
            None,
            None,
            None,
            None,
            None,
            100,
            "t",
            &stub,
        )
        .unwrap();
        decide(
            &mut e,
            "Adopt SQLite WAL",
            "also yes",
            "work",
            None,
            None,
            None,
            None,
            None,
            200,
            "t",
            &stub,
        )
        .unwrap();

        let mut titles: Vec<String> = recorded(&mut e).into_iter().map(|d| d.0).collect();
        titles.sort();
        assert_eq!(titles, ["Adopt SQLite", "Adopt SQLite WAL"]);
    }

    /// The same title twice is the same decision, re-decided.
    ///
    /// The other half of the rule above, and the one that makes supersession
    /// work at all: exact means exact, so it still matches.
    #[test]
    fn deciding_again_under_the_same_title_lands_on_the_same_entity() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Adopt SQLite",
            "yes",
            "work",
            None,
            None,
            None,
            None,
            None,
            100,
            "t",
            &stub,
        )
        .unwrap();
        decide(
            &mut e,
            "Adopt SQLite",
            "on reflection, no",
            "work",
            None,
            None,
            None,
            None,
            None,
            200,
            "t",
            &stub,
        )
        .unwrap();

        let all = recorded(&mut e);
        assert_eq!(all.len(), 1, "one title is one decision: {all:?}");
        let Outcome::About(Believed::Value(choice)) = about(
            &e,
            all[0].1,
            "choice",
            None,
            Some(Timestamp::MAX),
            Timestamp::MAX,
            None,
        )
        .unwrap() else {
            panic!("no choice on the re-decided entity")
        };
        assert_eq!(choice, "on reflection, no");
    }

    /// Superseding a decision does not consume the one doing the superseding.
    ///
    /// The sharpest form of the bug, because the two halves of `commit_decide`
    /// disagreed about what "the same title" meant: `find_decision` matched
    /// exactly and the write matched fuzzily. So this call retired `Adopt
    /// SQLite` by exact title and then wrote its own fields onto that same
    /// retired entity. The store was left holding one decision, under the old
    /// title, marked superseded -- the new one was gone, and the command said
    /// it had worked.
    #[test]
    fn the_decision_doing_the_superseding_survives_it() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Adopt SQLite",
            "yes",
            "work",
            None,
            None,
            None,
            None,
            None,
            100,
            "t",
            &stub,
        )
        .unwrap();
        let out = decide(
            &mut e,
            "Adopt SQLite WAL",
            "also yes",
            "work",
            None,
            None,
            None,
            Some("Adopt SQLite"),
            None,
            200,
            "t",
            &stub,
        )
        .unwrap();

        let Outcome::Decided {
            entity, superseded, ..
        } = out
        else {
            panic!("not a decision")
        };
        let (old, _) = superseded.expect("it should have retired the old one");
        assert_ne!(
            entity, old,
            "the new decision landed on the entity it just retired"
        );

        let all = recorded(&mut e);
        assert_eq!(all.len(), 2, "both decisions should exist: {all:?}");
        let stands: Vec<&(String, StableId, bool)> = all.iter().filter(|d| d.2).collect();
        assert_eq!(
            stands.len(),
            1,
            "exactly one should still stand: {stands:?}"
        );
        assert_eq!(stands[0].0, "Adopt SQLite WAL");
    }

    #[test]
    fn a_decision_is_recorded_without_asking_a_model_anything() {
        // The point of the separate path. `StubProvider` errors if asked to
        // complete, so a completion call here would fail the test rather than
        // pass quietly -- which is the assertion, not a side effect of it.
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        let out = decide(
            &mut e,
            "Use bi-temporal storage",
            "Keep valid time and transaction time on every attribute value",
            "work",
            None,
            Some("A single axis makes a stale answer indistinguishable from a bug"),
            Some("Choosing a storage model for the memory store"),
            None,
            None,
            100,
            "cli",
            &stub,
        )
        .unwrap();

        let Outcome::Decided {
            entity,
            superseded,
            supersedes_unknown,
        } = out
        else {
            panic!("{out:?}")
        };
        assert!(superseded.is_none());
        assert!(supersedes_unknown.is_none());
        assert_eq!(
            e.store_history(entity, "choice")[0].value.as_deref(),
            Some("Keep valid time and transaction time on every attribute value")
        );
        assert_eq!(
            e.store_history(entity, "status")[0].value.as_deref(),
            Some("accepted")
        );
    }

    #[test]
    fn every_field_of_a_decision_corrects_rather_than_accumulating() {
        // A decision has one status, one choice and one stated reason at a
        // time. Writing these as `Unstated` would leave a re-decided choice
        // reading as "a later assertion exists and nobody said what it meant",
        // which is exactly the thing a decision record exists to settle.
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        let Outcome::Decided { entity, .. } = decide(
            &mut e,
            "Pick a queue",
            "RabbitMQ",
            "work",
            None,
            Some("we know it"),
            None,
            None,
            None,
            100,
            "cli",
            &stub,
        )
        .unwrap() else {
            panic!()
        };
        for attr in ["status", "choice", "because"] {
            assert_eq!(
                e.store_history(entity, attr)[0].supersession,
                Supersession::Corrects,
                "{attr}"
            );
        }
    }

    #[test]
    fn re_deciding_under_one_title_retires_the_old_choice() {
        // No status field maintained by hand: the second `choice` corrects the
        // first, so the store itself answers "is this still what we do".
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Pick a queue",
            "RabbitMQ",
            "work",
            None,
            None,
            None,
            None,
            None,
            100,
            "cli",
            &stub,
        )
        .unwrap();
        decide(
            &mut e,
            "Pick a queue",
            "NATS",
            "work",
            None,
            Some("simpler ops"),
            None,
            None,
            None,
            200,
            "cli",
            &stub,
        )
        .unwrap();

        let Outcome::Decisions(lines) = decisions(&e, None, At::latest(), None).unwrap() else {
            panic!()
        };
        let queue: Vec<_> = lines.iter().filter(|l| l.title == "Pick a queue").collect();
        assert_eq!(queue.len(), 1, "one decision, two versions of its choice");
        assert_eq!(queue[0].choice, "NATS", "the latest choice is the answer");
        // `still_stands` is about the choice on the line, and the line shows
        // NATS. This used to assert the opposite -- the field then counted any
        // earlier choice as a replacement -- and the rendering that came out of
        // it marked "Pick a queue -> NATS" as replaced, which is the answer
        // being current and told otherwise. The fact that assertion cared about
        // is the revision count, which says it without contradicting the line.
        assert!(
            queue[0].still_stands,
            "NATS is the current choice and nothing supersedes this decision"
        );
        assert_eq!(
            queue[0].revisions, 2,
            "and the store still knows there was an earlier choice"
        );
    }

    /// A rebuilt vector is the one `decide` wrote.
    ///
    /// Two places compose the text a decision is embedded from -- `embed_field`
    /// on the way in, `reindex_texts` on the way back -- and nothing but this
    /// keeps them saying the same thing. If they drift, a rebuilt store still
    /// works, still returns hits, and quietly ranks everything a little wrong:
    /// the failure this project keeps finding and keeps refusing to ship.
    #[test]
    fn a_rebuilt_decision_vector_matches_the_one_decide_wrote() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Pin the compiler",
            "rust-toolchain.toml names the version",
            "work",
            None,
            Some("CI took whatever stable had become"),
            None,
            None,
            None,
            100,
            "t",
            &stub,
        )
        .unwrap();

        // What the store now holds, keyed by assertion.
        let before: Vec<(rm_engine::AssertionId, Vec<f32>)> = reindex_texts(&e)
            .unwrap()
            .into_iter()
            .map(|(id, text)| (id, stub.embed(&text).unwrap()))
            .collect();
        assert!(
            !before.is_empty(),
            "the probe must actually cover something"
        );

        // Every recalled hit is reachable at the same score after a rebuild,
        // which is only true if the text was composed identically.
        let query = stub
            .embed("decision Pin the compiler: choice is rust-toolchain.toml names the version")
            .unwrap();
        let Outcome::Recalled { hits: was, .. } =
            commit_recall(&e, query.clone(), 5, 0.0, None, Depth::Stated).unwrap()
        else {
            panic!()
        };

        let plan = plan_reindex(reindex_texts(&e).unwrap(), &stub, 3, Metric::Cosine).unwrap();
        commit_reindex(&mut e, plan).unwrap();

        let Outcome::Recalled { hits: now, .. } =
            commit_recall(&e, query, 5, 0.0, None, Depth::Stated).unwrap()
        else {
            panic!()
        };
        assert_eq!(was.len(), now.len());
        for (a, b) in was.iter().zip(&now) {
            assert_eq!(a.assertion, b.assertion, "the order must not move");
            assert_eq!(
                a.score, b.score,
                "a rebuilt vector that scores differently is a different vector"
            );
        }
    }

    /// A store holding anything but decisions is refused.
    ///
    /// A conversational fact was embedded on a sentence the extractor wrote,
    /// and that sentence is not kept. Re-embedding around it would leave two
    /// models' output in one index, where the distances between them are not
    /// wrong but meaningless.
    #[test]
    fn reindexing_a_store_it_cannot_fully_cover_is_refused() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        // A decision, which is reachable...
        decide(
            &mut e, "Kept", "a choice", "work", None, None, None, None, None, 100, "t", &stub,
        )
        .unwrap();
        // ...and something that is not, written the way `ingest` writes.
        e.remember(Observation {
            kind: "person".into(),
            mention: Record::new().with("name", "Ben").with("kind", "person"),
            attribute: "employer".into(),
            value: Some("Globex".into()),
            valid: Interval::since(100),
            provenance: Provenance::new(Source::ToolOutput, 100, "t"),
            supersession: Supersession::Unstated,
            according_to: None,
            embedding: stub.embed("Ben works at Globex").unwrap(),
        })
        .unwrap();

        let Err(HostError::Refused(why)) = reindex_texts(&e) else {
            panic!("a store it cannot fully cover must be refused")
        };
        assert!(
            why.contains("employer"),
            "it should say what it found: {why}"
        );
        assert!(why.contains("meaningless"), "and why it matters: {why}");
    }

    /// A rebuild that does not cover everything is refused by the engine too,
    /// not only by the host that usually catches it first.
    #[test]
    fn a_partial_rebuild_leaves_the_index_alone() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e, "One", "a", "work", None, None, None, None, None, 100, "t", &stub,
        )
        .unwrap();
        decide(
            &mut e, "Two", "b", "work", None, None, None, None, None, 100, "t", &stub,
        )
        .unwrap();
        let before = e.index_len();

        let mut half = plan_reindex(reindex_texts(&e).unwrap(), &stub, 3, Metric::Cosine).unwrap();
        half.vectors.truncate(1);
        let Err(HostError::Refused(why)) = commit_reindex(&mut e, half) else {
            panic!("a partial rebuild must be refused")
        };
        assert!(why.contains("could never be recalled"), "{why}");
        assert_eq!(
            e.index_len(),
            before,
            "and the index it came in with must survive"
        );
    }

    /// A decision made in March, recorded in August, is valid from March and
    /// known from August.
    ///
    /// The two axes, and the reason there are two. Moving the transaction time
    /// back with the valid time would say the store knew this in March, which
    /// would make every answer it gave between March and August retroactively
    /// wrong -- you could no longer tell a stale answer from a bug, which is
    /// what `rm_store`'s module docs give as the whole point of the second
    /// axis. So `--at` moves one and leaves the other alone.
    #[test]
    fn a_backdated_decision_holds_from_then_and_is_known_from_now() {
        const MARCH: Timestamp = 1_772_236_800_000; // 2026-02-28, well before
        const AUGUST: Timestamp = 1_787_532_411_419; // 2026-08-24
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Pin the compiler",
            "rust-toolchain.toml names the version",
            "work",
            None,
            None,
            None,
            None,
            Some(MARCH),
            AUGUST,
            "t",
            &stub,
        )
        .unwrap();

        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Pin the compiler", At::latest(), None).unwrap()
        else {
            panic!()
        };
        // The history reads back the day it was decided, not the day it was
        // typed. That is the whole user-visible point.
        assert_eq!(
            crate::time::format_day(d.history[0].0),
            "2026-02-28",
            "the log should show when it was decided"
        );

        // Valid time is refused here, and this assertion used to be its
        // opposite. It read "Valid time: it held in March" and asserted the
        // value came back -- which it did, because `choice` resolves under
        // `most_recent`, whose outcome is a single winner with no time
        // dimension, so `held_at` returned the same value for every instant.
        // The test passed because the flag was ignored: a test endorsing the
        // defect rather than catching it.
        let Err(HostError::Refused(why)) =
            about(&e, d.entity, "choice", Some(MARCH), None, AUGUST, None)
        else {
            panic!("a valid-time question under most_recent should be refused")
        };
        assert!(why.contains("valid_interval"), "{why}");

        // Transaction time still bites, and works under any strategy: the
        // store did not know this in March, so asking what it believed then
        // gives the answer it would have given then, which is nothing.
        assert_eq!(
            about(&e, d.entity, "choice", None, Some(MARCH), MARCH, None).unwrap(),
            Outcome::About(Believed::Unknown),
            "backdating must not rewrite what the store knew when"
        );
    }

    /// Without `--at` the two axes coincide, so nothing changes for a decision
    /// recorded as it is made.
    #[test]
    fn a_decision_recorded_now_is_valid_now_and_known_now() {
        const NOW: Timestamp = 1_787_532_411_419;
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e, "Plain", "a choice", "work", None, None, None, None, None, NOW, "t", &stub,
        )
        .unwrap();
        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Plain", At::latest(), None).unwrap()
        else {
            panic!()
        };
        assert_eq!(d.history[0].0, NOW);
        assert_eq!(
            about(&e, d.entity, "choice", None, Some(NOW), NOW, None).unwrap(),
            Outcome::About(Believed::Value("a choice".into()))
        );
    }

    /// The log can be read one status at a time.
    #[test]
    fn decisions_can_be_filtered_to_one_status() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        for (title, status) in [
            ("Kept", None),
            ("Turned down", Some("rejected")),
            ("Still weighing", Some("proposed")),
            ("Also turned down", Some("rejected")),
        ] {
            decide(
                &mut e, title, "a choice", "work", status, None, None, None, None, 100, "t", &stub,
            )
            .unwrap();
        }

        let titles = |only: Option<&str>| {
            let Outcome::Decisions(l) = decisions(&e, only, At::latest(), None).unwrap() else {
                panic!()
            };
            let mut t: Vec<String> = l.iter().map(|d| d.title.clone()).collect();
            t.sort();
            t
        };
        assert_eq!(titles(None).len(), 4);
        assert_eq!(
            titles(Some("rejected")),
            ["Also turned down", "Turned down"]
        );
        assert_eq!(titles(Some("proposed")), ["Still weighing"]);
        assert_eq!(titles(Some("accepted")), ["Kept"]);
        assert!(titles(Some("deprecated")).is_empty());
    }

    /// A status the vocabulary does not have is refused rather than matching
    /// nothing.
    ///
    /// "we have never rejected anything" and "you typed `declined`" both
    /// produce an empty list, and telling them apart is the entire answer.
    #[test]
    fn filtering_by_a_status_that_does_not_exist_is_refused() {
        let e = engine();
        let Err(HostError::Refused(why)) = decisions(&e, Some("declined"), At::latest(), None)
        else {
            panic!("a bad status should be refused")
        };
        assert!(
            why.contains("proposed") && why.contains("superseded"),
            "{why}"
        );
        // `superseded` is not settable by `decide`, but it is a real status a
        // decision can hold, so filtering by it has to work.
        assert!(decisions(&e, Some("superseded"), At::latest(), None).is_ok());
    }

    /// An option considered and turned down is recordable as one.
    ///
    /// The entry a decision log is most useful for, and `decide` could not
    /// write it: every decision was `accepted`, so the only way to record a
    /// rejection was to accept the word "no", which loses what was rejected
    /// and why. The reason is the whole point -- it is what stops the same
    /// question being reopened in six months.
    #[test]
    fn an_option_can_be_recorded_as_considered_and_rejected() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Rerank the recall results",
            "a cross-encoder over the top 200",
            "work",
            Some("rejected"),
            Some("the k-curve is still 0.926 at k=200, so there is nothing to rerank into"),
            None,
            None,
            None,
            100,
            "t",
            &stub,
        )
        .unwrap();

        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Rerank the recall results", At::latest(), None).unwrap()
        else {
            panic!()
        };
        assert_eq!(d.status, "rejected");
        assert!(!d.still_stands, "a rejected option is not in force");
        assert!(
            d.superseded_by.is_empty(),
            "and nothing replaced it -- it was never adopted"
        );
        assert!(d.because.is_some(), "the reason is the record");
    }

    /// Only the four statuses, and a typo is refused rather than stored.
    ///
    /// An open vocabulary would let `rejected`, `Rejected` and `declined` read
    /// the same to a person and differently to a program, which is the failure
    /// that made `decide` skip extraction in the first place.
    #[test]
    fn a_status_outside_the_vocabulary_is_refused() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        for status in ["declined", "Rejected", "wontfix", ""] {
            let out = decide(
                &mut e,
                "Some title",
                "some choice",
                "work",
                Some(status),
                None,
                None,
                None,
                None,
                100,
                "t",
                &stub,
            );
            let Err(HostError::Refused(why)) = out else {
                panic!("{status:?} should have been refused, got {out:?}")
            };
            assert!(
                why.contains("proposed") && why.contains("deprecated"),
                "the refusal should list the vocabulary: {why}"
            );
        }
        // And nothing was written on the way through.
        assert_eq!(
            decision(&e, "Some title", At::latest(), None).unwrap(),
            Outcome::Decision(Found::Unknown)
        );
    }

    /// `superseded` is not a status a caller sets.
    ///
    /// It claims a specific other decision replaced this one. Written on its
    /// own it produces exactly the state the supersession edge exists to
    /// prevent: retired, with no way to reach whatever retired it.
    #[test]
    fn superseded_is_refused_and_points_at_the_flag_that_does_it_properly() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        let out = decide(
            &mut e,
            "Store as one file",
            "whole-file rewrite",
            "work",
            Some("superseded"),
            None,
            None,
            None,
            None,
            100,
            "t",
            &stub,
        );
        let Err(HostError::Refused(why)) = out else {
            panic!("expected a refusal, got {out:?}")
        };
        assert!(
            why.contains("--supersedes"),
            "the refusal has to name the thing that does work: {why}"
        );
    }

    /// The default is unchanged, so every existing caller means what it did.
    #[test]
    fn a_decision_with_no_status_is_accepted_and_in_force() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e, "Plain", "a choice", "work", None, None, None, None, None, 100, "t", &stub,
        )
        .unwrap();
        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Plain", At::latest(), None).unwrap()
        else {
            panic!()
        };
        assert_eq!(d.status, "accepted");
        assert!(d.still_stands);
    }

    /// A retired decision names what replaced it, and the chain leads to the
    /// one that stands.
    ///
    /// The question a decision log exists to answer, and it had no answer:
    /// `--supersedes` wrote `status = superseded` on the old decision and
    /// nothing else, so a reader holding it could see that something replaced
    /// it and had no way to find out what. Checked over two hops, because one
    /// hop can be faked by reading the status.
    #[test]
    fn a_retired_decision_leads_to_the_one_that_stands() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        for (title, choice, replaces) in [
            ("Store as one JSON file", "whole-file rewrite", None),
            (
                "Store in SQLite",
                "incremental",
                Some("Store as one JSON file"),
            ),
            ("Store in Postgres", "a server", Some("Store in SQLite")),
        ] {
            decide(
                &mut e, title, choice, "work", None, None, None, replaces, None, 100, "t", &stub,
            )
            .unwrap();
        }

        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Store as one JSON file", At::latest(), None).unwrap()
        else {
            panic!("the oldest decision should be readable")
        };
        assert!(!d.still_stands);
        assert_eq!(
            d.superseded_by
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>(),
            ["Store in SQLite", "Store in Postgres"],
            "the walk should reach the decision that stands, not stop at the first hop"
        );

        // And back the other way, from the live one.
        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Store in Postgres", At::latest(), None).unwrap()
        else {
            panic!()
        };
        assert!(d.still_stands);
        assert!(d.superseded_by.is_empty());
        assert_eq!(
            d.supersedes
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>(),
            ["Store in SQLite", "Store as one JSON file"]
        );
    }

    /// A cycle in the chain is reported, not walked forever.
    ///
    /// `commit_decide` refuses to link a decision to itself, so the short loop
    /// cannot be written. A longer one can: two commands that each looked
    /// reasonable leave A superseding B and B superseding A, and a walk that
    /// trusted the data would hang instead of answering.
    #[test]
    fn a_supersession_cycle_terminates() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e, "A", "first", "work", None, None, None, None, None, 100, "t", &stub,
        )
        .unwrap();
        decide(
            &mut e,
            "B",
            "second",
            "work",
            None,
            None,
            None,
            Some("A"),
            None,
            200,
            "t",
            &stub,
        )
        .unwrap();
        // Closes the loop: A now supersedes B, which already supersedes A.
        decide(
            &mut e,
            "A",
            "third",
            "work",
            None,
            None,
            None,
            Some("B"),
            None,
            300,
            "t",
            &stub,
        )
        .unwrap();

        let Outcome::Decision(Found::Decision(d)) = decision(&e, "A", At::latest(), None).unwrap()
        else {
            panic!()
        };
        // Exactly one hop each way, then the revisit stops it. Asserted
        // exactly rather than as a bound: a walk that returned nothing would
        // also satisfy "did not loop forever", and would be a different bug
        // wearing this test as cover.
        assert_eq!(
            d.supersedes
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>(),
            ["B"],
            "one hop back, then the cycle is noticed"
        );
        assert_eq!(
            d.superseded_by
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>(),
            ["B"],
            "and one hop forward"
        );
    }

    /// Naming yourself in `--supersedes` does not abort the write.
    ///
    /// `rm_store::relate` refuses a self-edge, and the fields are already
    /// written by the time the edge is drawn -- so an unchecked `relate` would
    /// fail a command whose work had landed.
    #[test]
    fn a_decision_may_not_supersede_itself_and_is_still_recorded() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e, "Only one", "first", "work", None, None, None, None, None, 100, "t", &stub,
        )
        .unwrap();
        decide(
            &mut e,
            "Only one",
            "second",
            "work",
            None,
            None,
            None,
            Some("Only one"),
            None,
            200,
            "t",
            &stub,
        )
        .expect("naming itself must not fail the write");

        let Outcome::Decision(Found::Decision(d)) =
            decision(&e, "Only one", At::latest(), None).unwrap()
        else {
            panic!()
        };
        assert_eq!(d.choice, "second");
        assert!(d.supersedes.is_empty(), "no self-edge was drawn");
    }

    /// A title the store does not have is said so, not answered emptily.
    #[test]
    fn reading_a_decision_that_does_not_exist_says_so() {
        let e = engine();
        assert_eq!(
            decision(&e, "never recorded", At::latest(), None).unwrap(),
            Outcome::Decision(Found::Unknown)
        );
    }

    #[test]
    fn superseding_another_decision_retires_it_by_name() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decide(
            &mut e,
            "Store as JSON",
            "One file per store",
            "work",
            None,
            None,
            None,
            None,
            None,
            100,
            "cli",
            &stub,
        )
        .unwrap();
        let out = decide(
            &mut e,
            "Store as SQLite",
            "One database per store",
            "work",
            None,
            Some("whole-file rewrites do not survive a real corpus"),
            None,
            Some("Store as JSON"),
            None,
            200,
            "cli",
            &stub,
        )
        .unwrap();

        let Outcome::Decided {
            superseded: Some((old, title)),
            supersedes_unknown: None,
            ..
        } = out
        else {
            panic!("{out:?}")
        };
        assert_eq!(title, "Store as JSON");
        assert_eq!(
            e.store_history(old, "status")
                .iter()
                .filter_map(|v| v.value.clone())
                .next_back()
                .as_deref(),
            Some("superseded")
        );
    }

    #[test]
    fn superseding_something_that_is_not_there_is_reported_not_ignored() {
        // The caller mistyped the title of the decision they meant to retire.
        // It is still standing, and a plain success would never tell them.
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        let out = decide(
            &mut e,
            "New way",
            "Do it differently",
            "work",
            None,
            None,
            None,
            Some("Teh Old Way"),
            None,
            100,
            "cli",
            &stub,
        )
        .unwrap();
        let Outcome::Decided {
            supersedes_unknown: Some(missing),
            superseded: None,
            ..
        } = out
        else {
            panic!("{out:?}")
        };
        assert_eq!(missing, "Teh Old Way");
    }

    #[test]
    fn a_decision_without_a_title_or_a_choice_is_refused() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        for (title, choice) in [("", "something"), ("something", ""), ("  ", "x")] {
            assert!(
                decide(
                    &mut e, title, choice, "work", None, None, None, None, None, 100, "cli", &stub
                )
                .is_err(),
                "{title:?}/{choice:?}"
            );
        }
    }

    #[test]
    fn decisions_are_not_confused_with_anything_else_in_the_store() {
        // The store holds people and organisations too; `decisions` must list
        // only what was decided.
        let mut e = engine();
        let stub = StubProvider::new(vec![EXTRACTION]);
        remember(&mut e, "I work at Globex", 100, "cli", None, &stub, &stub).unwrap();
        decide(
            &mut e,
            "Pick a queue",
            "NATS",
            "work",
            None,
            None,
            None,
            None,
            None,
            200,
            "cli",
            &stub,
        )
        .unwrap();

        let Outcome::Decisions(lines) = decisions(&e, None, At::latest(), None).unwrap() else {
            panic!()
        };
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].title, "Pick a queue");
    }

    #[test]
    fn remembering_a_turn_names_every_mention_and_where_it_landed() {
        let mut e = engine();
        let stub = StubProvider::new(vec![EXTRACTION]);
        let out = remember(&mut e, "I work at Globex", 100, "cli", None, &stub, &stub).unwrap();

        let Outcome::Remembered {
            ingested, landings, ..
        } = out
        else {
            panic!("{out:?}")
        };
        assert_eq!(landings.len(), 2);
        assert_eq!(landings[0].name, "Ben Severn");
        assert!(landings[0].was_new, "the first turn creates everyone");
        assert_eq!(ingested.entities.len(), 2);
    }

    #[test]
    fn remembering_carries_what_extraction_would_not_keep() {
        // The wiring the whole salvage rests on. `rm_extract` drops a bad item
        // instead of refusing the turn, which is only defensible because the
        // drop is reported -- and a field that reaches `Outcome` but no
        // further would be a report nobody receives.
        let mut e = engine();
        let stub = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"I"}],
                "facts":[{"subject":0,"attribute":"employer","value":"Globex","text":"Ben works at Globex","days_ago":null},
                         {"subject":9,"attribute":"city","value":"London","text":"x","days_ago":null}],
                "relations":[],"closures":[]}"#,
        ]);
        let out = remember(&mut e, "I work at Globex", 100, "cli", None, &stub, &stub).unwrap();

        let Outcome::Remembered {
            landings, dropped, ..
        } = out
        else {
            panic!("{out:?}")
        };
        assert_eq!(landings.len(), 1, "the turn was kept, not refused");
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].what, "fact");
        assert!(dropped[0].why.contains("names mention 9"), "{}", dropped[0]);
    }

    #[test]
    fn a_mention_seen_before_is_reported_as_recognised_not_new() {
        // The distinction is the most useful thing on the screen: it is the
        // difference between the store learning about someone and the store
        // recognising them.
        let mut e = engine();
        let first = StubProvider::new(vec![EXTRACTION]);
        remember(&mut e, "I work at Globex", 100, "cli", None, &first, &first).unwrap();

        let second = StubProvider::new(vec![EXTRACTION]);
        let out = remember(
            &mut e,
            "I still work at Globex",
            200,
            "cli",
            None,
            &second,
            &second,
        )
        .unwrap();
        let Outcome::Remembered { landings, .. } = out else {
            panic!("{out:?}")
        };
        assert!(!landings[0].was_new, "Ben was already known");
    }

    #[test]
    fn a_turn_the_model_answered_with_nonsense_is_refused_with_the_reason() {
        let mut e = engine();
        let stub = StubProvider::new(vec!["I'm afraid I can't do that"]);
        let err = remember(&mut e, "anything", 100, "cli", None, &stub, &stub).unwrap_err();
        assert!(matches!(err, HostError::Refused(_)), "{err:?}");
        assert!(err.to_string().len() > 30, "the reason must survive: {err}");
    }

    #[test]
    fn recalling_reports_the_entity_behind_every_hit() {
        // Without the entity id a user cannot then ask `about` anything.
        let mut e = engine();
        let stub = StubProvider::new(vec![EXTRACTION]);
        remember(&mut e, "I work at Globex", 100, "cli", None, &stub, &stub).unwrap();

        let out = recall(&e, "Ben works at Globex", 5, &stub, 0.0, None).unwrap();
        let Outcome::Recalled { hits, .. } = out else {
            panic!("{out:?}")
        };
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| !h.attribute.is_empty()));
    }

    #[test]
    fn recalling_an_empty_store_is_an_empty_answer_not_a_failure() {
        let e = engine();
        let stub = StubProvider::new(vec![]);
        let out = recall(&e, "anything", 5, &stub, 0.0, None).unwrap();
        let Outcome::Recalled { hits, .. } = out else {
            panic!("{out:?}")
        };
        assert!(hits.is_empty());
    }

    #[test]
    fn asking_about_an_attribute_nobody_mentioned_is_not_an_error() {
        let mut e = engine();
        let stub = StubProvider::new(vec![EXTRACTION]);
        let out = remember(&mut e, "I work at Globex", 100, "cli", None, &stub, &stub).unwrap();
        let Outcome::Remembered { ingested, .. } = out else {
            panic!()
        };

        let out = about(
            &e,
            ingested.entities[0],
            "spouse",
            None,
            Some(1000),
            1000,
            None,
        )
        .unwrap();
        assert_eq!(out, Outcome::About(rm_engine::Believed::Unknown));
    }

    #[test]
    fn confirming_a_review_names_the_surviving_entity() {
        let mut e = engine();
        // Two turns naming a person who scores in the review band against the
        // first, rather than merging or separating outright. "Ben Severn"
        // against "Ben Sanderson" scores about 4.98 bits under the template's
        // name-only rule, between review_at = 4.0 and match_at = 6.0. The
        // brief's own suggestion, "Ben Severne", scores 6.31 -- above
        // match_at -- and would merge outright, so the fixture name is chosen
        // to land in the band rather than the template being retuned to fit
        // the fixture.
        let a = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"Ben"}],
                "facts":[],"relations":[],"closures":[]}"#,
        ]);
        remember(&mut e, "Ben", 100, "cli", None, &a, &a).unwrap();
        let b = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Sanderson","text":"Ben"}],
                "facts":[],"relations":[],"closures":[]}"#,
        ]);
        let out = remember(&mut e, "Ben again", 200, "cli", None, &b, &b).unwrap();
        let Outcome::Remembered { ingested, .. } = out else {
            panic!()
        };
        assert_eq!(
            ingested.reviews.len(),
            1,
            "the near-miss has to be asked about"
        );

        let out = review_confirm(&mut e, ingested.reviews[0]).unwrap();
        let Outcome::Confirmed { survivor } = out else {
            panic!("{out:?}")
        };
        assert_eq!(e.entity_count(), 1);
        assert!(e.pending_review().is_empty());
        let _ = survivor;
    }

    #[test]
    fn listing_reviews_reports_what_would_be_merged_and_how_confident_it_was() {
        let mut e = engine();
        let a = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"Ben"}],
                "facts":[],"relations":[],"closures":[]}"#,
        ]);
        remember(&mut e, "Ben", 100, "cli", None, &a, &a).unwrap();
        let b = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Sanderson","text":"Ben"}],
                "facts":[],"relations":[],"closures":[]}"#,
        ]);
        remember(&mut e, "Ben again", 200, "cli", None, &b, &b).unwrap();

        let Outcome::Reviews(lines) = review_list(&e).unwrap() else {
            panic!()
        };
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].score > 0.0,
            "a review pair has real evidence behind it"
        );
    }

    #[test]
    fn a_review_line_says_what_the_two_entities_are_called_and_what_they_are() {
        let mut e = engine();
        let a = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"Ben"}],
                "facts":[],"relations":[],"closures":[]}"#,
        ]);
        remember(&mut e, "Ben", 100, "cli", None, &a, &a).unwrap();
        let b = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Sanderson","text":"Ben"}],
                "facts":[],"relations":[],"closures":[]}"#,
        ]);
        remember(&mut e, "Ben again", 200, "cli", None, &b, &b).unwrap();

        let Outcome::Reviews(lines) = review_list(&e).unwrap() else {
            panic!()
        };
        let line = &lines[0];
        // The names are the question. Without them the line asks whether two
        // integers are the same thing, which nobody can answer.
        let mut names = [line.a_name.clone(), line.b_name.clone()];
        names.sort();
        assert_eq!(
            names,
            [
                Some("Ben Sanderson".to_string()),
                Some("Ben Severn".to_string())
            ]
        );
        assert_eq!(
            (line.a_kind.as_str(), line.b_kind.as_str()),
            ("person", "person")
        );
    }
    /// `--local` writes a config without asking any model anything.
    ///
    /// The probe panics here rather than returning a value: the property is
    /// not that the dimension is right, it is that **nothing is asked**. A
    /// probe that ran and happened to succeed would pass a weaker test while
    /// leaving the key requirement exactly where it was.
    #[test]
    fn a_local_init_never_reaches_the_probe() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let out = init(&path, false, true, None, &|| {
            panic!("--local must not probe a model")
        })
        .unwrap();
        assert_eq!(
            out,
            Outcome::Initialised {
                path: path.clone(),
                dimension: crate::config::TEMPLATE_DIMENSION,
                local: true,
                replaced_unparsable: None,
            }
        );
    }

    /// And what it writes is a config this build can read back, with the
    /// local embedder selected.
    ///
    /// The `[provider]` fields stay in the file. They are inert under `local`,
    /// and left required deliberately -- see `init`. The check that matters is
    /// that the file parses, because the defect this fixes was a documented
    /// path that produced nothing loadable.
    #[test]
    fn a_local_init_writes_a_config_that_loads_and_selects_the_local_embedder() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        init(&path, false, true, None, &|| panic!("no probe")).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"embedder = "local""#), "{text}");
        assert!(
            !text.contains(r#"embedder = "http""#),
            "the http line must be replaced, not joined: {text}"
        );
        crate::config::Config::load(&path).expect("a config init wrote must load");
    }

    /// Without `--local` the probe still runs and still decides the dimension.
    /// The keyless path is an addition, not a replacement.
    #[test]
    fn a_default_init_still_asks_the_model() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        init(&path, false, false, None, &|| Ok(3072)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("dimension = 3072"), "{text}");
        assert!(text.contains(r#"embedder = "http""#), "{text}");
    }
}

#[cfg(test)]
mod rescope_tests {
    use super::tests::engine;
    use super::*;
    use crate::testing::StubProvider;

    const MARCH: Timestamp = 1_772_236_800_000;
    const AUGUST: Timestamp = 1_787_532_411_419;

    fn rescope(
        e: &mut Engine,
        title: &str,
        scope: &str,
        observed_at: Timestamp,
        stub: &StubProvider,
    ) -> Result<Outcome, HostError> {
        let plan = plan_rescope(title, scope, observed_at, "t", stub)?;
        commit_rescope(e, plan)
    }

    fn decided(e: &mut Engine, title: &str, scope: &str, at: Timestamp, stub: &StubProvider) {
        decide(
            e,
            title,
            "the choice",
            scope,
            None,
            None,
            None,
            None,
            Some(at),
            at,
            "t",
            stub,
        )
        .unwrap();
    }

    /// The reason this command exists rather than being `decide` with one
    /// argument changed.
    #[test]
    fn rescoping_does_not_count_as_revising_the_choice() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decided(&mut e, "D", "work", MARCH, &stub);

        let before = visible(&e, find_decision(&e, "D").unwrap(), "choice", At::latest()).len();
        rescope(&mut e, "D", "work/goldenmatch", AUGUST, &stub).unwrap();
        let after = visible(&e, find_decision(&e, "D").unwrap(), "choice", At::latest()).len();

        assert_eq!(before, 1);
        assert_eq!(
            after, 1,
            "a scope-only write must leave the choice history alone -- \
             `revisions` counts choice versions, so a second one here would make \
             every backfilled decision read as revised when none was"
        );
    }

    /// A record written before scopes existed: status and choice, no scope.
    /// This is the shape the 219-decision backfill actually operates on, and
    /// `decide` cannot produce it -- it refuses without a scope.
    fn decided_without_a_scope(e: &mut Engine, title: &str, at: Timestamp, stub: &StubProvider) {
        for (attr, value) in [("status", "accepted"), ("choice", "the choice")] {
            let w = embed_field(title, attr, value, stub).unwrap();
            let entity = find_decision(e, title);
            write_field(e, entity, &w, at, at, "t").unwrap();
        }
    }

    /// Backfill: the reach was always true, so it takes the decision's own
    /// valid time -- not the day the backfill happened to run.
    #[test]
    fn a_backfilled_scope_is_valid_from_when_the_decision_was() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decided_without_a_scope(&mut e, "D", MARCH, &stub);
        let id = find_decision(&e, "D").unwrap();
        assert!(
            held(&e, id, "scope", At::latest()).is_none(),
            "precondition: this is a pre-scope record"
        );

        rescope(&mut e, "D", "work/goldenmatch", AUGUST, &stub).unwrap();

        // March, long before the backfill ran, already reads the new reach.
        let march = At {
            valid: MARCH,
            tx: Timestamp::MAX,
        };
        assert_eq!(
            held(&e, id, "scope", march).as_deref(),
            Some("work/goldenmatch"),
            "a backfilled scope dated from now would leave every historical              query reading the decision as unscoped -- and an unscoped decision              reaches EVERYWHERE, so the failure is over-reach in history"
        );
    }

    /// Correction: the reach changed today, so today is when it changed. The
    /// old reach must still stand in the past, or the store has been made to
    /// claim the decision always reached somewhere it did not.
    #[test]
    fn a_corrected_scope_starts_today_and_leaves_the_past_alone() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decided(&mut e, "D", "*", MARCH, &stub);
        let id = find_decision(&e, "D").unwrap();

        rescope(&mut e, "D", "work/goldenmatch", AUGUST, &stub).unwrap();

        let march = At {
            valid: MARCH,
            tx: Timestamp::MAX,
        };
        assert_eq!(
            held(&e, id, "scope", march).as_deref(),
            Some("*"),
            "March still reached everywhere; only August narrowed it"
        );
        assert_eq!(
            held(&e, id, "scope", At::latest()).as_deref(),
            Some("work/goldenmatch")
        );
    }

    /// Setting a scope to what it already is writes nothing: a second
    /// identical version would make the attribute's own history claim a
    /// correction that never happened.
    #[test]
    fn rescoping_to_the_same_scope_writes_nothing() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        decided(&mut e, "D", "work", MARCH, &stub);
        let id = find_decision(&e, "D").unwrap();
        let before = visible(&e, id, "scope", At::latest()).len();

        let out = rescope(&mut e, "D", "work", AUGUST, &stub).unwrap();

        assert_eq!(visible(&e, id, "scope", At::latest()).len(), before);
        let Outcome::Rescoped { previous, .. } = out else {
            panic!("not a rescope")
        };
        assert_eq!(previous.as_deref(), Some("work"));
    }

    /// An unknown title is a typo, and during a backfill of hundreds a silent
    /// create is the most expensive and least visible thing that could happen.
    #[test]
    fn an_unknown_title_is_refused_and_creates_nothing() {
        let mut e = engine();
        let stub = StubProvider::new(vec![]);
        let before = e.entity_ids().len();

        let err = rescope(&mut e, "never decided", "work", AUGUST, &stub).unwrap_err();

        assert!(matches!(err, HostError::Refused(_)), "{err:?}");
        assert_eq!(e.entity_ids().len(), before, "nothing was created");
    }

    /// A typo costs no embedding, the same bargain `plan_decide` makes.
    #[test]
    fn an_invalid_scope_is_refused_before_the_embedder() {
        let stub = StubProvider::new(vec![]);
        assert!(matches!(
            plan_rescope("D", "", AUGUST, "t", &stub),
            Err(HostError::Refused(_))
        ));
    }
    /// `init` writes a path that needs no rule to interpret.
    ///
    /// `Config::parse` anchoring a relative path is what makes hand-written
    /// and older configs behave sensibly. Writing the path out in full is
    /// what stops the question arising for configs this crate produces, and
    /// it is checked through `Config::load` -- the path a real command takes
    /// -- rather than by reading the bytes back for a substring.
    #[test]
    fn init_writes_a_store_path_that_needs_no_anchoring() {
        let dir = crate::testing::TempDir::new();
        let config_path = dir.path().join("rmem.toml");
        init(&config_path, false, true, None, &|| Ok(1536)).unwrap();

        let written = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            !written.contains("path = \"memory.json\""),
            "init left the template's relative example in the file"
        );
        assert_eq!(
            crate::config::Config::load(&config_path)
                .unwrap()
                .store
                .path,
            dir.path().join("memory.json")
        );
    }
}
