//! What each command does. Data out; rendering lives in `format`.

use std::path::{Path, PathBuf};

use rm_engine::{
    Believed, Embedder, Engine, Ingested, Interval, Observation, Provenance, Query, Recalled,
    Record, Remembered, ReviewId, Source, StableId, Supersession, Timestamp,
};
use rm_extract::{Completer, Turn};

/// Re-exported because [`Outcome::Remembered`] carries these and a host has to
/// be able to name them.
///
/// `rm-cli` depends on this crate and `rm-engine`, and on nothing else — that
/// narrowing is the point of `rm-host` existing. A host forced to add
/// `rm-extract` to its manifest just to read a field of an `Outcome` this crate
/// hands it would give that back.
pub use rm_extract::Dropped;

use crate::config::TEMPLATE;
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
    Recalled(Vec<Recalled>),
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
}

/// One decision as a caller sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionLine {
    pub entity: StableId,
    pub title: String,
    pub status: String,
    pub choice: String,
    pub because: Option<String>,
    /// Whether anything later replaced this decision's `choice`.
    ///
    /// Read from the store rather than from `status`, so a decision retired by
    /// being re-decided under the same title reads as superseded even though
    /// nobody set a status field.
    pub still_stands: bool,
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
const DECISION_FIELDS: [&str; 4] = ["status", "choice", "because", "context"];

/// Write a config, with the embedding dimension taken from the model.
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
    replaced_unparsable: Option<String>,
    probe: &dyn Fn() -> Result<usize, String>,
) -> Result<Outcome, HostError> {
    if replaced_unparsable.is_none() && config_path.exists() && !force {
        return Err(HostError::Config(format!(
            "{} already exists, and it may have been edited -- pass --force to replace it",
            config_path.display()
        )));
    }

    let dimension = probe().map_err(HostError::Refused)?;

    // The template's own value is an example. Substituting rather than
    // formatting keeps the file one literal, so the test that parses it is
    // testing the bytes a user receives.
    let contents = TEMPLATE.replace("dimension = 1536", &format!("dimension = {dimension}"));

    std::fs::write(config_path, contents).map_err(|e| {
        HostError::Config(format!("could not write {}: {e}", config_path.display()))
    })?;

    Ok(Outcome::Initialised {
        path: config_path.to_path_buf(),
        dimension,
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
    let turn = Turn {
        text: text.to_string(),
        speaker: speaker.map(str::to_string),
        observed_at,
        session: session.to_string(),
    };

    let extraction =
        rm_engine::extract(&turn, completer).map_err(|e| HostError::Refused(e.to_string()))?;

    // Which entities existed before, so the landings can say "recognised"
    // rather than only naming an id.
    let before: Vec<StableId> = engine.entity_ids();

    let ingested = engine
        .ingest(&turn, &extraction, embedder)
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
) -> Result<Outcome, HostError> {
    let embedding = embedder
        .embed(query)
        .map_err(|e| HostError::Refused(e.to_string()))?;
    let hits = engine
        .recall(&Query::new(embedding, k))
        .map_err(|e| HostError::Refused(e.to_string()))?;
    Ok(Outcome::Recalled(hits))
}

/// What the store believes an attribute held.
pub fn about(
    engine: &Engine,
    entity: StableId,
    attribute: &str,
    valid_t: rm_engine::Timestamp,
    tx_t: rm_engine::Timestamp,
) -> Result<Outcome, HostError> {
    engine
        .about(entity, attribute, valid_t, tx_t)
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
    because: Option<&str>,
    context: Option<&str>,
    supersedes: Option<&str>,
    observed_at: Timestamp,
    session: &str,
    embedder: &impl Embedder,
) -> Result<Outcome, HostError> {
    if title.trim().is_empty() || choice.trim().is_empty() {
        return Err(HostError::Refused(
            "a decision needs a title and a choice: the title is how it is found again, and the choice is what was decided".into(),
        ));
    }

    // Retire the old one first. If this fails, nothing has been written, and a
    // caller who re-runs the command gets one attempt rather than a duplicate
    // decision beside a still-standing predecessor.
    let mut superseded = None;
    let mut supersedes_unknown = None;
    if let Some(old_title) = supersedes {
        match find_decision(engine, old_title) {
            None => supersedes_unknown = Some(old_title.to_string()),
            Some(old) => {
                write_field(
                    engine,
                    old_title,
                    "status",
                    "superseded",
                    observed_at,
                    session,
                    embedder,
                )?;
                superseded = Some((old, old_title.to_string()));
            }
        }
    }

    let fields = [
        ("status", Some("accepted")),
        ("choice", Some(choice)),
        ("because", because),
        ("context", context),
    ];
    let mut entity = None;
    for (name, value) in fields {
        let Some(value) = value.filter(|v| !v.trim().is_empty()) else {
            continue;
        };
        let landed = write_field(engine, title, name, value, observed_at, session, embedder)?;
        entity.get_or_insert(landed);
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

/// Every decision the store holds, most recently recorded first.
pub fn decisions(engine: &Engine) -> Result<Outcome, HostError> {
    let mut out: Vec<DecisionLine> = Vec::new();
    for id in engine.entity_ids() {
        let Some(record) = engine.identity_of(id) else {
            continue;
        };
        if record.get("kind") != Some("decision") {
            continue;
        }
        let latest = |attr: &str| {
            engine
                .store_history(id, attr)
                .iter()
                .filter_map(|v| v.value.clone())
                .next_back()
        };
        let Some(choice) = latest("choice") else {
            continue;
        };
        out.push(DecisionLine {
            entity: id,
            title: record.get("name").unwrap_or_default().to_string(),
            status: latest("status").unwrap_or_else(|| "accepted".into()),
            choice,
            because: latest("because"),
            // A decision stands while nothing later replaced its choice. One
            // version is the common case and stands trivially; a second means
            // it was re-decided.
            still_stands: engine.store_history(id, "choice").len() == 1,
        });
    }
    out.sort_by_key(|d| std::cmp::Reverse(d.entity));
    Ok(Outcome::Decisions(out))
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
fn write_field(
    engine: &mut Engine,
    title: &str,
    attribute: &str,
    value: &str,
    observed_at: Timestamp,
    session: &str,
    embedder: &impl Embedder,
) -> Result<StableId, HostError> {
    debug_assert!(DECISION_FIELDS.contains(&attribute));
    let embedding = embedder
        .embed(&format!("decision {title}: {attribute} is {value}"))
        .map_err(|e| HostError::Refused(e.to_string()))?;
    let landed = engine
        .remember(Observation {
            kind: "decision".to_string(),
            mention: Record::new().with("name", title).with("kind", "decision"),
            attribute: attribute.to_string(),
            value: Some(value.to_string()),
            valid: Interval::since(observed_at),
            // `UserAssertion`, not `ToolOutput`: nobody inferred this from a
            // sentence. Somebody decided it and said so.
            provenance: Provenance::new(Source::UserAssertion, observed_at, session),
            supersession: Supersession::Corrects,
            embedding,
        })
        .map_err(|e| HostError::Refused(e.to_string()))?;
    Ok(match landed {
        Remembered::Merged { entity, .. }
        | Remembered::Created { entity, .. }
        | Remembered::CreatedPendingReview { entity, .. } => entity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn init_writes_a_config_whose_dimension_came_from_the_model() {
        // Not from a default and not from the user. A dimension that disagrees
        // with the embedding model makes every distance meaningless, and
        // nothing reports it.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let out = init(&path, false, None, &|| Ok(1536)).unwrap();

        assert_eq!(
            out,
            Outcome::Initialised {
                path: path.clone(),
                dimension: 1536,
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
        init(&path, false, None, &|| Ok(3072)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("dimension = 3072"), "{written}");
        assert!(!written.contains("dimension = 1536"), "{written}");
    }

    #[test]
    fn init_refuses_to_overwrite_an_existing_config() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "# hand-edited, do not lose").unwrap();

        let err = init(&path, false, None, &|| Ok(1536)).unwrap_err();
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

        let err = init(&path, false, None, &|| panic!("the probe must not run")).unwrap_err();
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
        init(&path, true, None, &|| Ok(768)).unwrap();
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
        let out = init(&path, false, Some(notice.to_string()), &|| Ok(1536)).unwrap();

        assert_eq!(
            out,
            Outcome::Initialised {
                path: path.clone(),
                dimension: 1536,
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
        let err = init(&path, false, None, &|| Err("quota exceeded".to_string())).unwrap_err();
        assert!(err.to_string().contains("quota exceeded"), "{err}");
        assert!(!path.exists(), "no config may be left behind");
    }

    #[test]
    fn what_init_writes_is_what_the_config_loader_reads() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        init(&path, false, None, &|| Ok(1536)).unwrap();
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

    fn engine() -> Engine {
        let config: crate::config::Config = toml::from_str(crate::config::TEMPLATE).unwrap();
        Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            config.ruleset().unwrap(),
            config.policy_for_engine().unwrap(),
        )
    }

    // ---- decisions ---------------------------------------------------------

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
            Some("A single axis makes a stale answer indistinguishable from a bug"),
            Some("Choosing a storage model for the memory store"),
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
            Some("we know it"),
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
            Some("simpler ops"),
            None,
            None,
            200,
            "cli",
            &stub,
        )
        .unwrap();

        let Outcome::Decisions(lines) = decisions(&e).unwrap() else {
            panic!()
        };
        let queue: Vec<_> = lines.iter().filter(|l| l.title == "Pick a queue").collect();
        assert_eq!(queue.len(), 1, "one decision, two versions of its choice");
        assert_eq!(queue[0].choice, "NATS", "the latest choice is the answer");
        assert!(
            !queue[0].still_stands,
            "and the store knows the earlier one was replaced"
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
            Some("whole-file rewrites do not survive a real corpus"),
            None,
            Some("Store as JSON"),
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
            None,
            None,
            Some("Teh Old Way"),
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
                decide(&mut e, title, choice, None, None, None, 100, "cli", &stub).is_err(),
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
            None,
            None,
            None,
            200,
            "cli",
            &stub,
        )
        .unwrap();

        let Outcome::Decisions(lines) = decisions(&e).unwrap() else {
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

        let out = recall(&e, "Ben works at Globex", 5, &stub).unwrap();
        let Outcome::Recalled(hits) = out else {
            panic!("{out:?}")
        };
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| !h.attribute.is_empty()));
    }

    #[test]
    fn recalling_an_empty_store_is_an_empty_answer_not_a_failure() {
        let e = engine();
        let stub = StubProvider::new(vec![]);
        let out = recall(&e, "anything", 5, &stub).unwrap();
        let Outcome::Recalled(hits) = out else {
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

        let out = about(&e, ingested.entities[0], "spouse", 1000, 1000).unwrap();
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
}
