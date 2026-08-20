//! What each command does. Data out; rendering lives in `format`.

use std::path::{Path, PathBuf};

use rm_engine::{Believed, Embedder, Engine, Ingested, Query, Recalled, ReviewId, StableId};
use rm_extract::{Completer, Turn};

use crate::config::TEMPLATE;
use crate::CliError;

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
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewLine {
    pub id: ReviewId,
    pub a: StableId,
    pub b: StableId,
    pub score: f64,
}

/// What a command did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Initialised { path: PathBuf, dimension: usize },
    Remembered(Ingested, Vec<MentionLanding>),
    Recalled(Vec<Recalled>),
    About(Believed),
    Reviews(Vec<ReviewLine>),
    Confirmed { survivor: StableId },
    Rejected,
}

/// Write a config, with the embedding dimension taken from the model.
///
/// `probe` is a closure rather than a provider so this is testable without a
/// socket; the binary passes one that calls `HttpProvider::probe_dimension`.
///
/// Probing before writing is deliberate. Half a config is worse than none: the
/// next command would read it and fail somewhere further from the cause.
pub fn init(
    config_path: &Path,
    force: bool,
    probe: &dyn Fn() -> Result<usize, String>,
) -> Result<Outcome, CliError> {
    if config_path.exists() && !force {
        return Err(CliError::Config(format!(
            "{} already exists, and it may have been edited -- pass --force to replace it",
            config_path.display()
        )));
    }

    let dimension = probe().map_err(CliError::Refused)?;

    // The template's own value is an example. Substituting rather than
    // formatting keeps the file one literal, so the test that parses it is
    // testing the bytes a user receives.
    let contents = TEMPLATE.replace("dimension = 1536", &format!("dimension = {dimension}"));

    std::fs::write(config_path, contents)
        .map_err(|e| CliError::Config(format!("could not write {}: {e}", config_path.display())))?;

    Ok(Outcome::Initialised {
        path: config_path.to_path_buf(),
        dimension,
    })
}

/// Extract a turn and apply it.
pub fn remember(
    engine: &mut Engine,
    text: &str,
    observed_at: rm_engine::Timestamp,
    session: &str,
    completer: &impl Completer,
    embedder: &impl Embedder,
) -> Result<Outcome, CliError> {
    let turn = Turn {
        text: text.to_string(),
        speaker: None,
        observed_at,
        session: session.to_string(),
    };

    let extraction =
        rm_engine::extract(&turn, completer).map_err(|e| CliError::Refused(e.to_string()))?;

    // Which entities existed before, so the landings can say "recognised"
    // rather than only naming an id.
    let before: Vec<StableId> = engine.entity_ids();

    let ingested = engine
        .ingest(&turn, &extraction, embedder)
        .map_err(|e| CliError::Refused(e.to_string()))?;

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

    Ok(Outcome::Remembered(ingested, landings))
}

/// Search for assertions near a query.
pub fn recall(
    engine: &Engine,
    query: &str,
    k: usize,
    embedder: &impl Embedder,
) -> Result<Outcome, CliError> {
    let embedding = embedder
        .embed(query)
        .map_err(|e| CliError::Refused(e.to_string()))?;
    let hits = engine
        .recall(&Query::new(embedding, k))
        .map_err(|e| CliError::Refused(e.to_string()))?;
    Ok(Outcome::Recalled(hits))
}

/// What the store believes an attribute held.
pub fn about(
    engine: &Engine,
    entity: StableId,
    attribute: &str,
    valid_t: rm_engine::Timestamp,
    tx_t: rm_engine::Timestamp,
) -> Result<Outcome, CliError> {
    engine
        .about(entity, attribute, valid_t, tx_t)
        .map(Outcome::About)
        .map_err(|e| CliError::Refused(e.to_string()))
}

/// The open questions.
pub fn review_list(engine: &Engine) -> Result<Outcome, CliError> {
    Ok(Outcome::Reviews(
        engine
            .pending_review()
            .into_iter()
            .map(|p| ReviewLine {
                id: p.id,
                a: p.a,
                b: p.b,
                score: p.score,
            })
            .collect(),
    ))
}

/// Answer a review with "the same".
pub fn review_confirm(engine: &mut Engine, id: ReviewId) -> Result<Outcome, CliError> {
    engine
        .confirm(id)
        .map(|survivor| Outcome::Confirmed { survivor })
        .map_err(|e| CliError::Refused(e.to_string()))
}

/// Answer a review with "different".
pub fn review_reject(engine: &mut Engine, id: ReviewId) -> Result<Outcome, CliError> {
    engine
        .reject(id)
        .map(|()| Outcome::Rejected)
        .map_err(|e| CliError::Refused(e.to_string()))
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
        let out = init(&path, false, &|| Ok(1536)).unwrap();

        assert_eq!(
            out,
            Outcome::Initialised {
                path: path.clone(),
                dimension: 1536
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
        init(&path, false, &|| Ok(3072)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("dimension = 3072"), "{written}");
        assert!(!written.contains("dimension = 1536"), "{written}");
    }

    #[test]
    fn init_refuses_to_overwrite_an_existing_config() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "# hand-edited, do not lose").unwrap();

        let err = init(&path, false, &|| Ok(1536)).unwrap_err();
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

        let err = init(&path, false, &|| panic!("the probe must not run")).unwrap_err();
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
        init(&path, true, &|| Ok(768)).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("dimension = 768"));
    }

    #[test]
    fn init_writes_nothing_when_the_probe_fails() {
        // Half a config is worse than none: the next command would read it and
        // fail somewhere less obvious.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let err = init(&path, false, &|| Err("quota exceeded".to_string())).unwrap_err();
        assert!(err.to_string().contains("quota exceeded"), "{err}");
        assert!(!path.exists(), "no config may be left behind");
    }

    #[test]
    fn what_init_writes_is_what_the_config_loader_reads() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        init(&path, false, &|| Ok(1536)).unwrap();
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

    #[test]
    fn remembering_a_turn_names_every_mention_and_where_it_landed() {
        let mut e = engine();
        let stub = StubProvider::new(vec![EXTRACTION]);
        let out = remember(&mut e, "I work at Globex", 100, "cli", &stub, &stub).unwrap();

        let Outcome::Remembered(ingested, landings) = out else {
            panic!("{out:?}")
        };
        assert_eq!(landings.len(), 2);
        assert_eq!(landings[0].name, "Ben Severn");
        assert!(landings[0].was_new, "the first turn creates everyone");
        assert_eq!(ingested.entities.len(), 2);
    }

    #[test]
    fn a_mention_seen_before_is_reported_as_recognised_not_new() {
        // The distinction is the most useful thing on the screen: it is the
        // difference between the store learning about someone and the store
        // recognising them.
        let mut e = engine();
        let first = StubProvider::new(vec![EXTRACTION]);
        remember(&mut e, "I work at Globex", 100, "cli", &first, &first).unwrap();

        let second = StubProvider::new(vec![EXTRACTION]);
        let out = remember(
            &mut e,
            "I still work at Globex",
            200,
            "cli",
            &second,
            &second,
        )
        .unwrap();
        let Outcome::Remembered(_, landings) = out else {
            panic!("{out:?}")
        };
        assert!(!landings[0].was_new, "Ben was already known");
    }

    #[test]
    fn a_turn_the_model_answered_with_nonsense_is_refused_with_the_reason() {
        let mut e = engine();
        let stub = StubProvider::new(vec!["I'm afraid I can't do that"]);
        let err = remember(&mut e, "anything", 100, "cli", &stub, &stub).unwrap_err();
        assert!(matches!(err, CliError::Refused(_)), "{err:?}");
        assert!(err.to_string().len() > 30, "the reason must survive: {err}");
    }

    #[test]
    fn recalling_reports_the_entity_behind_every_hit() {
        // Without the entity id a user cannot then ask `about` anything.
        let mut e = engine();
        let stub = StubProvider::new(vec![EXTRACTION]);
        remember(&mut e, "I work at Globex", 100, "cli", &stub, &stub).unwrap();

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
        let out = remember(&mut e, "I work at Globex", 100, "cli", &stub, &stub).unwrap();
        let Outcome::Remembered(ingested, _) = out else {
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
        remember(&mut e, "Ben", 100, "cli", &a, &a).unwrap();
        let b = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Sanderson","text":"Ben"}],
                "facts":[],"relations":[],"closures":[]}"#,
        ]);
        let out = remember(&mut e, "Ben again", 200, "cli", &b, &b).unwrap();
        let Outcome::Remembered(ingested, _) = out else {
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
        remember(&mut e, "Ben", 100, "cli", &a, &a).unwrap();
        let b = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Sanderson","text":"Ben"}],
                "facts":[],"relations":[],"closures":[]}"#,
        ]);
        remember(&mut e, "Ben again", 200, "cli", &b, &b).unwrap();

        let Outcome::Reviews(lines) = review_list(&e).unwrap() else {
            panic!()
        };
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].score > 0.0,
            "a review pair has real evidence behind it"
        );
    }
}
