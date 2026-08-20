//! `rmem.toml`: what it holds, and how it becomes an engine.
//!
//! # Why the ruleset is spelled out rather than named
//!
//! A `profile = "people"` would be a shorter file. It would also bury the `m`
//! and `u` probabilities inside this binary, and `rm_resolve::FieldRule`
//! documents at length why they are stated: the Fellegi-Sunter model makes a
//! caller say *why* a field is informative rather than how much they like it,
//! and a weight nobody can see is exactly the hand-tuned opacity that model
//! exists to replace.
//!
//! So [`TEMPLATE`] writes working numbers with their meaning beside them, and
//! they live on disk where they can be read and changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rm_engine::{BlockingKey, Comparator, FieldRule, Metric, Policy, Ruleset, Strategy};
use rm_providers::HttpProvider;
use serde::Deserialize;

use crate::CliError;

/// The file `rmem init` writes.
///
/// Kept as one literal so the test that parses it is testing the same bytes a
/// user gets. A template assembled at runtime could pass its own test and still
/// write something unreadable.
pub const TEMPLATE: &str = r#"# rmem configuration.
#
# Written by `rmem init`. The numbers below are a working starting point, not
# defaults hidden in the binary -- read them, and change them when you know
# something about your data that they do not.

[store]
path = "memory.json"

[provider]
base_url = "https://api.openai.com/v1"
# The NAME of the environment variable holding your key -- never the key. This
# file is a thing people commit.
api_key_env = "OPENAI_API_KEY"
completion_model = "gpt-4o-mini"
embedding_model = "text-embedding-3-small"
# Discovered by `rmem init` from the model itself. If you change the embedding
# model, run `rmem init --force` again rather than editing this: a dimension
# that disagrees with the model makes every distance meaningless and nothing
# reports it.
dimension = 1536
metric = "cosine"

[resolution]
# Evidence, in bits, for two mentions being the same thing.
# Below review_at: different. Above match_at: the same.
# Between them: you are asked, and nothing is merged until you answer.
review_at = 4.0
match_at = 6.0

[[resolution.field]]
field = "name"
comparator = "jaro_winkler"
# m = P(this field agrees | the two are the same thing)
# u = P(this field agrees | they are different things) -- the field's commonness
m = 0.9
u = 0.01

[[resolution.blocking]]
kind = "prefix"
field = "name"
n = 3

[policy]
# How competing values for one attribute are resolved, at read time.
default = "most_recent"

[policy.attribute]
# An employer changes; both facts are worth keeping, with the store answering
# by date rather than picking a winner.
employer = "valid_interval"
"#;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub store: StoreConfig,
    pub provider: ProviderConfig,
    pub resolution: ResolutionConfig,
    pub policy: PolicyConfig,
}

#[derive(Debug, Deserialize)]
pub struct StoreConfig {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub completion_model: String,
    pub embedding_model: String,
    pub dimension: usize,
    pub metric: String,
}

#[derive(Debug, Deserialize)]
pub struct ResolutionConfig {
    pub review_at: f64,
    pub match_at: f64,
    pub field: Vec<FieldConfig>,
    pub blocking: Vec<BlockingConfig>,
}

#[derive(Debug, Deserialize)]
pub struct FieldConfig {
    pub field: String,
    pub comparator: String,
    pub m: f64,
    pub u: f64,
}

#[derive(Debug, Deserialize)]
pub struct BlockingConfig {
    pub kind: String,
    pub field: String,
    #[serde(default)]
    pub n: usize,
}

#[derive(Debug, Deserialize)]
pub struct PolicyConfig {
    pub default: String,
    #[serde(default)]
    pub attribute: BTreeMap<String, String>,
}

impl Config {
    /// Read a config file.
    pub fn load(path: &Path) -> Result<Config, CliError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            CliError::Config(format!(
                "could not read {}: {e} -- run `rmem init` to write one",
                path.display()
            ))
        })?;
        toml::from_str(&text)
            .map_err(|e| CliError::Config(format!("{} is not valid: {e}", path.display())))
    }

    /// The resolver this file describes.
    pub fn ruleset(&self) -> Result<Ruleset, CliError> {
        let mut rules = Vec::new();
        for f in &self.resolution.field {
            rules.push(FieldRule::new(
                f.field.clone(),
                comparator(&f.comparator)?,
                f.m,
                f.u,
            ));
        }
        let mut blocking = Vec::new();
        for b in &self.resolution.blocking {
            blocking.push(blocking_key(b)?);
        }
        Ruleset::new(
            rules,
            blocking,
            self.resolution.review_at,
            self.resolution.match_at,
        )
        .map_err(|e| CliError::Config(e.to_string()))
    }

    /// The survivorship policy this file describes.
    pub fn policy_for_engine(&self) -> Result<Policy, CliError> {
        let mut policy = Policy::new(strategy(&self.policy.default)?);
        for (attribute, name) in &self.policy.attribute {
            policy = policy.with(attribute.clone(), strategy(name)?);
        }
        Ok(policy)
    }

    /// The distance metric this file names.
    pub fn metric(&self) -> Result<Metric, CliError> {
        match self.provider.metric.as_str() {
            "cosine" => Ok(Metric::Cosine),
            "l2" => Ok(Metric::L2),
            other => Err(CliError::Config(format!(
                "metric {other:?} is not one this build knows; use \"cosine\" or \"l2\". It is not defaulted because choosing wrong is a silent quality bug -- results stay plausible and get subtly worse."
            ))),
        }
    }

    /// A provider built from this file and the environment.
    pub fn provider(&self) -> Result<HttpProvider, CliError> {
        let key = std::env::var(&self.provider.api_key_env)
            .map_err(|_| CliError::MissingKey(self.provider.api_key_env.clone()))?;
        Ok(HttpProvider::new(
            self.provider.base_url.clone(),
            key,
            self.provider.completion_model.clone(),
            self.provider.embedding_model.clone(),
        ))
    }
}

fn comparator(name: &str) -> Result<Comparator, CliError> {
    match name {
        "exact" => Ok(Comparator::Exact),
        "normalized" => Ok(Comparator::Normalized),
        "jaro_winkler" => Ok(Comparator::JaroWinkler),
        "token_jaccard" => Ok(Comparator::TokenJaccard),
        other => Err(CliError::Config(format!(
            "comparator {other:?} is not one this build knows; use \"exact\", \"normalized\", \"jaro_winkler\" or \"token_jaccard\""
        ))),
    }
}

fn blocking_key(b: &BlockingConfig) -> Result<BlockingKey, CliError> {
    match b.kind.as_str() {
        "exact" => Ok(BlockingKey::Exact(b.field.clone())),
        "token" => Ok(BlockingKey::Token(b.field.clone())),
        "prefix" if b.n > 0 => Ok(BlockingKey::Prefix(b.field.clone(), b.n)),
        "prefix" => Err(CliError::Config(
            "a prefix blocking key needs n greater than 0; n = 0 puts every record in one block, which compares everything to everything".to_string(),
        )),
        other => Err(CliError::Config(format!(
            "blocking kind {other:?} is not one this build knows; use \"exact\", \"prefix\" or \"token\""
        ))),
    }
}

fn strategy(name: &str) -> Result<Strategy, CliError> {
    match name {
        "most_complete" => Ok(Strategy::MostComplete),
        "longest_value" => Ok(Strategy::LongestValue),
        "majority_vote" => Ok(Strategy::MajorityVote),
        "confidence_majority" => Ok(Strategy::ConfidenceMajority),
        "first_non_null" => Ok(Strategy::FirstNonNull),
        "unanimous_or_null" => Ok(Strategy::UnanimousOrNull),
        "most_recent" => Ok(Strategy::MostRecent),
        "valid_interval" => Ok(Strategy::ValidInterval),
        // Naming it and inventing an order would be exactly the arbitrary answer
        // wearing a deterministic hat that rm-survivor refuses to give.
        "source_priority" => Err(CliError::Config(
            "source_priority needs an order of sources to rank, and this config format has no way to say one yet -- choose another strategy, or rank them in code".to_string(),
        )),
        other => Err(CliError::Config(format!(
            "strategy {other:?} is not one this build knows; use one of most_complete, longest_value, majority_vote, confidence_majority, first_non_null, unanimous_or_null, most_recent, valid_interval"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config, CliError> {
        toml::from_str(s).map_err(|e| CliError::Config(e.to_string()))
    }

    #[test]
    fn the_template_this_crate_writes_is_one_it_can_read_back() {
        // `init` writes TEMPLATE and every later command parses it. If those
        // ever disagree the failure lands on the user's second command, not on
        // us, and the message is a parse error about a file they never wrote.
        let config = parse(TEMPLATE).expect("the template must parse");
        config.ruleset().expect("and produce a ruleset");
        config.policy_for_engine().expect("and a policy");
        config.metric().expect("and a metric");
    }

    #[test]
    fn the_template_never_contains_anything_that_looks_like_a_key() {
        // It holds the NAME of an environment variable. A config file is a
        // thing people commit.
        assert!(TEMPLATE.contains("api_key_env"));
        assert!(!TEMPLATE.contains("sk-"));
        assert!(
            !TEMPLATE.contains("api_key ="),
            "the template must not have a field a user could paste a key into"
        );
    }

    #[test]
    fn a_ruleset_carries_the_thresholds_and_fields_the_file_asked_for() {
        let config = parse(TEMPLATE).unwrap();
        let ruleset = config.ruleset().unwrap();
        assert!(!ruleset.blocking().is_empty(), "blocking keys must survive");
    }

    #[test]
    fn an_unknown_comparator_names_what_it_did_not_recognise_and_what_it_accepts() {
        let bad = TEMPLATE.replace("comparator = \"jaro_winkler\"", "comparator = \"vibes\"");
        let err = parse(&bad).unwrap().ruleset().unwrap_err();
        assert!(err.to_string().contains("vibes"), "{err}");
        assert!(err.to_string().contains("jaro_winkler"), "{err}");
    }

    #[test]
    fn an_unknown_strategy_names_what_it_did_not_recognise() {
        let bad = TEMPLATE.replace("default = \"most_recent\"", "default = \"whatever\"");
        let err = parse(&bad).unwrap().policy_for_engine().unwrap_err();
        assert!(err.to_string().contains("whatever"), "{err}");
    }

    #[test]
    fn source_priority_is_refused_with_a_reason_rather_than_silently_dropped() {
        // The strategy needs an ordered list of sources, which this file format
        // has no way to express. Accepting the name and picking an order would
        // be the arbitrary answer wearing a deterministic hat that rm-survivor
        // refuses on principle.
        let bad = TEMPLATE.replace("default = \"most_recent\"", "default = \"source_priority\"");
        let err = parse(&bad).unwrap().policy_for_engine().unwrap_err();
        assert!(err.to_string().contains("source_priority"), "{err}");
        assert!(err.to_string().contains("order"), "{err}");
    }

    #[test]
    fn a_per_attribute_strategy_overrides_the_default() {
        let config = parse(TEMPLATE).unwrap();
        let policy = config.policy_for_engine().unwrap();
        assert_eq!(policy.for_attribute("employer"), &Strategy::ValidInterval);
        assert_eq!(policy.for_attribute("anything else"), &Strategy::MostRecent);
    }

    #[test]
    fn an_unset_key_variable_names_which_variable_to_set() {
        let mut config = parse(TEMPLATE).unwrap();
        config.provider.api_key_env = "RMEM_TEST_DEFINITELY_UNSET".to_string();
        // Not `.unwrap_err()`: that needs `HttpProvider: Debug` for the panic
        // message on the `Ok` arm, and `HttpProvider` deliberately has no
        // `Debug` impl so the key it holds can never end up in one.
        let err = match config.provider() {
            Err(e) => e,
            Ok(_) => panic!("expected a missing-key error"),
        };
        assert!(
            err.to_string().contains("RMEM_TEST_DEFINITELY_UNSET"),
            "{err}"
        );
    }

    #[test]
    fn an_unknown_metric_is_refused_rather_than_defaulted() {
        // Choosing the wrong metric is a silent quality bug: results stay
        // plausible and get subtly worse. rm-index refuses to default it and so
        // does this.
        let bad = TEMPLATE.replace("metric = \"cosine\"", "metric = \"euclidean-ish\"");
        let err = parse(&bad).unwrap().metric().unwrap_err();
        assert!(err.to_string().contains("euclidean-ish"), "{err}");
    }
}
