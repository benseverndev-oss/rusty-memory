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

/// Every config struct carries `deny_unknown_fields`.
///
/// The invariant this crate is built around is that an API key never reaches
/// disk, and `TEMPLATE`'s comment says so -- "the NAME of the environment
/// variable holding your key -- never the key. This file is a thing people
/// commit." A comment is not a mechanism. Without this, `api_key = "sk-..."`
/// pasted under `[provider]` was dropped on the floor in silence: the command
/// ran, exited 0, and the user believed a key they had just written into a
/// committed file was in use when it never was.
///
/// It earns its place a second way, on ordinary mistakes. A field written
/// `embedding_model_name` -- reasonable, if you think the field is named for
/// the model's name -- used to fall through and surface later as a
/// missing-field error naming `embedding_model`, which is the one field the
/// file appears to already have.
///
/// The message names the field and never its value. That is the whole point:
/// the value is the part that may be a key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub store: StoreConfig,
    pub provider: ProviderConfig,
    pub resolution: ResolutionConfig,
    pub policy: PolicyConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreConfig {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub base_url: String,
    pub api_key_env: String,
    pub completion_model: String,
    pub embedding_model: String,
    pub dimension: usize,
    pub metric: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionConfig {
    pub review_at: f64,
    pub match_at: f64,
    pub field: Vec<FieldConfig>,
    pub blocking: Vec<BlockingConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldConfig {
    pub field: String,
    pub comparator: String,
    pub m: f64,
    pub u: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockingConfig {
    pub kind: String,
    pub field: String,
    #[serde(default)]
    pub n: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
        Self::parse(path, &text)
    }

    /// A config to probe a provider with before `rmem init` has written one.
    ///
    /// Falls back to [`TEMPLATE`]'s defaults, but only when there is no file
    /// at `path` at all -- `init` needs a working provider to discover the
    /// embedding dimension, and on a first run there is nothing else to build
    /// one from. A file that exists and fails to parse is a different
    /// problem and is refused exactly as [`Config::load`] refuses it
    /// everywhere else: silently falling back to the template's OpenAI
    /// defaults for a file that names a different provider would leave
    /// whoever wrote that file with no way to learn it never took effect,
    /// and the `--force` overwrite that follows a successful probe would
    /// then throw the broken file away without them ever seeing why.
    pub fn load_or_template(path: &Path) -> Result<Config, CliError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(path, &text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::from_template()),
            Err(e) => Err(CliError::Config(format!(
                "could not read {}: {e}",
                path.display()
            ))),
        }
    }

    fn parse(path: &Path, text: &str) -> Result<Config, CliError> {
        toml::from_str(text).map_err(|e| {
            // Not `{e}`: `Error`'s `Display` reproduces the offending source
            // line verbatim, and a config file is the one place `api_key_env`
            // makes it plausible someone pasted a real key into `api_key` by
            // mistake. `message()` and `span()` give the reason and the
            // location without echoing what triggered it -- everything a
            // reader needs to find and fix the line, and the one thing that
            // could ever leak stays out.
            CliError::Config(format!(
                "{} is not valid: {} ({})",
                path.display(),
                e.message(),
                location(text, e.span())
            ))
        })
    }

    /// A config built from [`TEMPLATE`] rather than a file.
    ///
    /// The fallback value [`Config::load_or_template`] returns when there is
    /// no config file yet. Infallible:
    /// `the_template_this_crate_writes_is_one_it_can_read_back` already pins
    /// that `TEMPLATE` parses, so this cannot fail in a way that test does
    /// not already catch.
    pub fn from_template() -> Config {
        toml::from_str(TEMPLATE).expect("TEMPLATE is tested to parse")
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
            // `[policy.attribute]` is an open map -- any attribute name, any
            // strategy name -- which makes it the one table in `rmem.toml`
            // that `deny_unknown_fields` cannot defend. It is also the last
            // table in `TEMPLATE`, so `api_key = "sk-..."` appended to the
            // end of the file, which is what appending to a file means, lands
            // here as a map entry rather than as an unknown field. Driven
            // rather than reasoned about: it put the value on the terminal
            // verbatim, through `strategy`'s catch-all arm.
            //
            // So this names the attribute and not what was written against
            // it. The attribute is the half a reader needs to find the line,
            // and the value is the half that can be a secret -- the same
            // split `Config::parse` makes between a location and a source
            // line.
            let chosen = match strategy(name) {
                Ok(s) => s,
                // `source_priority`'s refusal is a sentence written into this
                // binary rather than read out of the file, so relaying it
                // carries nothing the user typed.
                Err(e) if name == "source_priority" => return Err(e),
                Err(_) => {
                    return Err(CliError::Config(format!(
                        "the strategy for attribute {attribute:?} under [policy.attribute] is not one this build knows; use one of most_complete, longest_value, majority_vote, confidence_majority, first_non_null, unanimous_or_null, most_recent, valid_interval. What is written there is deliberately not repeated: this table accepts any name, so it is where a key pasted at the end of rmem.toml lands, and printing it would put it in your terminal and your scrollback."
                    )))
                }
            };
            policy = policy.with(attribute.clone(), chosen);
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
        // `api_key_env` holds the NAME of a variable, and the likeliest way to
        // get this file wrong is to put the key there instead -- the field is
        // called `api_key_env` and a key is what you have in your hand.
        //
        // This check is a POSIX correctness check, not a leak defence, and the
        // difference matters because it was once mistaken for one. It refuses
        // a value that cannot name an environment variable: a name is
        // `[A-Za-z_][A-Za-z0-9_]*`, so an empty string, a leading digit or any
        // other character is refused. That is a statement about the field
        // rather than a guess about the value, which is why it is safe to act
        // on.
        //
        // What it is *not* is a filter that keeps keys out of the message
        // below. Most real keys are legal variable names --
        // `gsk_aBcD1234EFgh5678IJkl9012MNop3456`, `hf_QRstUVwx7890YZab1234`, a
        // 32-character hex Azure or Mistral key -- and sail straight through.
        // Only the hyphenated `sk-...` shape is caught, which is the one shape
        // the first version of this guard was tested with, and that is why it
        // was reported closed when it was closed for one format out of
        // several. `CliError::MissingKey` no longer names the variable at all,
        // which is what actually closes it, for every key format that exists
        // and every one that does not exist yet.
        let name = &self.provider.api_key_env;
        let legal = !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !legal {
            return Err(CliError::Config(
                "rmem.toml's api_key_env must be the NAME of an environment variable -- letters, digits and underscores, not starting with a digit, like OPENAI_API_KEY -- and what is written there cannot be one. It is not repeated here: if what you pasted is the key itself, printing it would put it in your terminal, your scrollback and any log that catches them.".to_string(),
            ));
        }
        let key = std::env::var(name).map_err(|_| CliError::MissingKey)?;
        Ok(HttpProvider::new(
            self.provider.base_url.clone(),
            key,
            self.provider.completion_model.clone(),
            self.provider.embedding_model.clone(),
        ))
    }
}

/// Where a byte offset into `text` falls, as a 1-based line and *character*
/// column -- the same convention `toml`'s own `Display` uses, computed by hand
/// so the message can carry the location without carrying the line itself.
fn location(text: &str, span: Option<std::ops::Range<usize>>) -> String {
    let Some(span) = span else {
        return "location unknown".to_string();
    };
    // Clamped, then walked back to a character boundary. `min` alone keeps
    // the slice in bounds but not on a boundary, and slicing mid-character
    // panics -- with a message that prints up to 256 bytes of the string
    // being sliced, which here is the head of `rmem.toml`. That is exactly
    // the leak `Config::parse` closes, arriving as a panic instead. No
    // malformed input has been found that gets `toml` to hand back a
    // mid-character span, so this guards a mechanism rather than a live
    // defect; it is one line and the failure mode is severe.
    let mut start = span.start.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let before = &text[..start];
    let line = before.matches('\n').count() + 1;
    // Characters, not bytes. `toml_edit`, which produces the spans, counts
    // characters, so a byte column disagrees with what `toml`'s own `Display`
    // would have said the moment the line holds anything non-ASCII. Since the
    // source line is deliberately not echoed, the column is the only
    // affordance left for finding the fault -- so it has to agree with what
    // the reader will check it against.
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..start].chars().count() + 1;
    format!("line {line}, column {column}")
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
    fn from_template_reads_the_provider_block_a_first_init_needs() {
        // `main` reaches for this only when there is no `rmem.toml` yet, to
        // build the provider `init` probes before it can write the file that
        // would otherwise supply one. If this ever disagreed with TEMPLATE, a
        // first run would fail on the one command that cannot assume a config
        // exists.
        let config = Config::from_template();
        assert_eq!(config.provider.base_url, "https://api.openai.com/v1");
        assert_eq!(config.provider.api_key_env, "OPENAI_API_KEY");
        assert_eq!(config.provider.completion_model, "gpt-4o-mini");
        assert_eq!(config.provider.embedding_model, "text-embedding-3-small");
    }

    #[test]
    fn load_or_template_falls_back_to_the_template_only_when_no_file_exists() {
        use crate::testing::TempDir;

        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        // Nothing has been written yet: the template's defaults, so a first
        // `rmem init` can still reach a provider to probe with.
        let config = Config::load_or_template(&path).unwrap();
        assert_eq!(
            config.provider.base_url,
            Config::from_template().provider.base_url
        );
    }

    #[test]
    fn load_or_template_reads_an_existing_file_rather_than_the_template() {
        use crate::testing::TempDir;

        // The reviewer's repro: a file naming a different provider must
        // survive `rmem init` reaching this fallback, not be silently
        // replaced by the template's OpenAI defaults.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let custom = TEMPLATE.replace(
            "base_url = \"https://api.openai.com/v1\"",
            "base_url = \"https://internal.example/v1\"",
        );
        std::fs::write(&path, &custom).unwrap();

        let config = Config::load_or_template(&path).unwrap();
        assert_eq!(config.provider.base_url, "https://internal.example/v1");
    }

    #[test]
    fn load_or_template_refuses_a_file_that_exists_but_does_not_parse() {
        use crate::testing::TempDir;

        // The bug this guards: a file that exists and is broken is not "no
        // file yet". Treating it that way would silently probe the
        // template's OpenAI defaults instead of whatever the file named, and
        // never tell whoever wrote it that the file never took effect.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "this is not valid toml {{{").unwrap();

        let err = Config::load_or_template(&path).unwrap_err();
        assert!(err.to_string().contains("rmem.toml"), "{err}");
        assert!(err.to_string().contains("not valid"), "{err}");
    }

    #[test]
    fn a_broken_config_names_the_failing_line_without_echoing_what_is_on_it() {
        use crate::testing::TempDir;

        // toml::de::Error's own Display reproduces the offending source line
        // verbatim, and this file is the one place `api_key_env` makes it
        // plausible someone pastes a real key into `api_key` by mistake --
        // an unquoted value is an easy typo and a realistic way to land a
        // parse error on exactly that line. The location has to survive so
        // the line can be found and fixed; the line's content must not,
        // because it is the only part of this error that could ever carry a
        // secret.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let broken =
            format!("{TEMPLATE}\napi_key = sk-THIS-IS-A-FAKE-SECRET-TOKEN-LEAK-CHECK-1234567890\n");
        std::fs::write(&path, &broken).unwrap();

        let err = Config::load(&path).unwrap_err();
        let text = err.to_string();
        assert!(
            !text.contains("sk-THIS-IS-A-FAKE-SECRET-TOKEN"),
            "the secret-looking value leaked into the error: {text}"
        );
        assert!(text.contains("line"), "{text}");
    }

    #[test]
    fn a_key_pasted_into_the_config_is_refused_by_name_rather_than_ignored() {
        use crate::testing::TempDir;

        // `[provider]` names `api_key_env`, so `api_key` is the obvious
        // wrong guess, and a user who makes it has just written a live
        // credential into a file this template's own comment calls "a thing
        // people commit". Before `deny_unknown_fields` serde dropped the
        // field silently: `rmem review` printed "no open questions" and
        // exited 0, and nothing anywhere ever said the key was not in use.
        //
        // The error names the field and must never name its value -- the
        // value is the part that is a secret, which is the same reason
        // `Config::parse` reports a location instead of the source line.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let pasted = TEMPLATE.replace(
            "api_key_env = \"OPENAI_API_KEY\"",
            "api_key_env = \"OPENAI_API_KEY\"
api_key = \"sk-PASTED-FAKE-SECRET-LEAK-CHECK-1234\"",
        );
        std::fs::write(&path, pasted).unwrap();

        let err = Config::load(&path).unwrap_err().to_string();
        assert!(err.contains("api_key"), "the field has to be named: {err}");
        assert!(
            !err.contains("sk-PASTED-FAKE-SECRET"),
            "the value must never be echoed: {err}"
        );
    }

    #[test]
    fn a_field_that_is_not_one_is_refused_naming_the_word_that_was_written() {
        use crate::testing::TempDir;

        // The ordinary-mistake half of the same guard. A field written
        // `embedding_model_name` used to fall through and reappear later as
        // "missing field `embedding_model`" -- a message naming the one field
        // the file appears to already have, which sends the reader looking in
        // the wrong place.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(
            &path,
            TEMPLATE.replace("embedding_model =", "embedding_model_name ="),
        )
        .unwrap();

        let err = Config::load(&path).unwrap_err().to_string();
        assert!(err.contains("embedding_model_name"), "{err}");
    }

    #[test]
    fn the_template_this_crate_writes_survives_deny_unknown_fields() {
        // `deny_unknown_fields` is exactly the kind of change that can make
        // the file this crate writes one it can no longer read. Parsed here
        // through `Config::load`, the path a real command takes, rather than
        // through this module's own `parse` helper.
        use crate::testing::TempDir;

        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, TEMPLATE).unwrap();
        Config::load(&path).expect("the template must still parse");
    }

    #[test]
    fn a_location_column_counts_characters_the_way_toml_does() {
        // The source line is deliberately not echoed, so the column is the
        // only affordance left for finding the fault -- and whoever checks it
        // will check against `toml` itself, or an editor that agrees with it.
        // `toml_edit` counts characters; this counted bytes, and the two
        // disagree the moment the line holds anything non-ASCII.
        let text = "note = \"\u{1F600}\u{1F600}\u{1F600}\\q\"\n";
        let err = toml::from_str::<toml::Value>(text).unwrap_err();
        let ours = location(text, err.span());
        let theirs = err.to_string();
        assert!(
            theirs.contains(&ours),
            "toml reports {theirs:?}; this reports {ours:?}"
        );
    }

    #[test]
    fn a_span_that_lands_mid_character_does_not_panic() {
        // `location` is handed a byte range and clamps it with `min`, which
        // keeps the slice in bounds but not on a character boundary. Slicing
        // mid-character panics, and Rust's slice-error message prints up to
        // 256 bytes of the string being sliced -- the head of `rmem.toml`,
        // which is the very thing `Config::parse` stopped echoing. Driven
        // directly rather than through the parser because no malformed input
        // has been found that makes `toml` hand back such a span.
        let text = "\u{1F600} = 1\n";
        for start in 0..=text.len() + 4 {
            let _ = location(text, Some(start..text.len()));
        }
        assert_eq!(location(text, Some(2..3)), "line 1, column 1");
    }

    #[test]
    fn a_key_pasted_at_the_end_of_the_file_is_refused_without_being_printed() {
        // Found by driving the binary, not by reading it. `deny_unknown_fields`
        // catches `api_key = "sk-..."` under `[provider]` -- but appending to
        // a file means appending at the end, and the end of TEMPLATE is
        // `[policy.attribute]`, an open map. So the paste is a valid map
        // entry, not an unknown field, and `strategy`'s catch-all arm quoted
        // it straight to the terminal.
        let pasted = format!("{TEMPLATE}api_key = \"sk-PASTED-FAKE-SECRET-AT-THE-END-99\"\n");
        let err = parse(&pasted)
            .unwrap()
            .policy_for_engine()
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("sk-PASTED-FAKE-SECRET"),
            "the pasted value reached the message: {err}"
        );
        assert!(
            err.contains("api_key"),
            "the attribute has to be named so the line can be found: {err}"
        );
        assert!(
            err.contains("most_recent"),
            "and what a strategy may be: {err}"
        );
    }

    #[test]
    fn a_variable_name_posix_forbids_is_refused_as_a_config_error_not_as_a_missing_key() {
        // The POSIX check is a correctness check and nothing more. Its whole
        // claim is that these values cannot name an environment variable, so
        // it refuses them as a broken config rather than reporting the
        // variable as unset -- which would be a message about the environment
        // for a fault that is in the file.
        //
        // Both directions, because a check that refuses everything would pass
        // half of this and a check that refuses nothing would pass the other.
        for illegal in ["", "9LEADING_DIGIT", "HAS-A-HYPHEN", "HAS SPACE", "H$"] {
            let mut config = parse(TEMPLATE).unwrap();
            config.provider.api_key_env = illegal.to_string();
            match config.provider() {
                Err(CliError::Config(_)) => {}
                Err(other) => panic!("{illegal:?} should be a config error, got {other:?}"),
                Ok(_) => panic!("{illegal:?} cannot name an environment variable"),
            }
        }
        for legal in ["_UNDERSCORE_FIRST", "A9", "RMEM_TEST_DEFINITELY_UNSET"] {
            let mut config = parse(TEMPLATE).unwrap();
            config.provider.api_key_env = legal.to_string();
            match config.provider() {
                Err(CliError::MissingKey) => {}
                Err(other) => panic!("{legal:?} is a legal name, got {other:?}"),
                Ok(_) => panic!("{legal:?} is not a variable anything sets"),
            }
        }
    }

    #[test]
    fn a_key_written_where_its_variable_name_belongs_is_never_printed_in_any_format() {
        // The likeliest way to get this file wrong: the field is called
        // `api_key_env` and a key is what you have in your hand.
        //
        // The previous version of this test used one hyphenated `sk-...`
        // fixture, and passed -- because the POSIX name check happens to
        // refuse a hyphen. Every other key format in wide use is a legal
        // environment-variable name and sailed through to a message that
        // printed it. So the fixtures here are deliberately several shapes,
        // and the assertion is on the message rather than on which branch
        // produced it: whether a value is refused as an illegal name or
        // accepted and then found unset, it must not come back out.
        for key in [
            // Groq, and the general `prefix_alnum` shape: a legal name.
            "gsk_aBcD1234EFgh5678IJkl9012MNop3456",
            // Hugging Face: also a legal name.
            "hf_QRstUVwx7890YZab1234",
            // Azure OpenAI and Mistral: 32 hex characters, a legal name.
            "0123456789abcdef0123456789abcdef",
            // A leading digit makes this one illegal as a name, which POSIX
            // already says; it must still not be echoed.
            "9sk0000FAKE1111SECRET2222LEAKCHECK",
            // OpenAI's hyphenated shape, the only one the first guard caught.
            "sk-proj-FAKE-SECRET-LEAK-CHECK-9999",
            // An OpenAI project key at its real length, ~164 characters.
            "sk-proj-FAKESECRETLEAKCHECK0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        ] {
            let mut config = parse(TEMPLATE).unwrap();
            config.provider.api_key_env = key.to_string();
            // Not `.unwrap_err()`: that needs `HttpProvider: Debug` for the
            // panic message on the `Ok` arm, and `HttpProvider` deliberately
            // has no `Debug` impl so the key it holds can never end up in one.
            let err = match config.provider() {
                Err(e) => e,
                Ok(_) => panic!("a variable named after a key cannot be set"),
            };
            let text = err.to_string();
            assert!(
                !text.contains(key),
                "the value written in api_key_env came back out: {text}"
            );
            // And not through `Debug` either, which is where a payload on the
            // error variant would have leaked whatever `Display` withheld.
            let debug = format!("{err:?}");
            assert!(
                !debug.contains(key),
                "the value reached the error's Debug: {debug}"
            );
            assert!(text.contains("api_key_env"), "{text}");
        }
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
        // `Ruleset` keeps its rules and thresholds private, so the only honest
        // way to pin what the file asked for is through the behaviour it
        // exposes -- decide()'s boundaries and score()'s value -- not
        // `!ruleset.blocking().is_empty()`, which the name promised to check
        // but the body never did, and which `Ruleset::new` already guarantees
        // on its own by rejecting an empty rule list.
        //
        // This does not, on its own, catch a swap of review_at and match_at:
        // for this file's actual numbers a swap always makes review_at >
        // match_at, which `Ruleset::new` rejects outright, so no assertion
        // downstream of a successful `ruleset()` call could ever observe one.
        // `a_swapped_review_at_and_match_at_is_refused_naming_both` pins that
        // rejection directly, since this test cannot.
        use rm_engine::{Decision, Record};

        let config = parse(TEMPLATE).unwrap();
        let ruleset = config.ruleset().unwrap();

        assert_eq!(
            ruleset.blocking(),
            &[BlockingKey::Prefix("name".to_string(), 3)]
        );

        // review_at = 4.0, match_at = 6.0: read straight off the boundaries
        // decide() draws, since that is the only way to observe them.
        assert_eq!(ruleset.decide(3.999), Decision::NonMatch);
        assert_eq!(ruleset.decide(4.0), Decision::Review);
        assert_eq!(ruleset.decide(5.999), Decision::Review);
        assert_eq!(ruleset.decide(6.0), Decision::Match);

        // m = 0.9, u = 0.01 on "name": two records that agree completely score
        // exactly the field's agreement weight, log2(m / u).
        let a = Record::new().with("name", "Ben Severn");
        let b = Record::new().with("name", "Ben Severn");
        let expected = (0.9_f64 / 0.01).log2();
        let got = ruleset.score(&a, &b);
        assert!(
            (got - expected).abs() < 1e-9,
            "score = {got}, expected {expected}"
        );
    }

    #[test]
    fn a_swapped_review_at_and_match_at_is_refused_naming_both() {
        // What actually pins the review_at/match_at swap: not the test above,
        // which cannot reach it (see its comment), but `Ruleset::new`'s own
        // validation, exercised directly here with the file's own numbers
        // swapped -- review_at above match_at leaves no review band at all.
        let swapped = TEMPLATE
            .replace("review_at = 4.0", "review_at = 6.0")
            .replace("match_at = 6.0", "match_at = 4.0");
        let err = parse(&swapped).unwrap().ruleset().unwrap_err();
        assert!(err.to_string().contains('6'), "{err}");
        assert!(err.to_string().contains('4'), "{err}");
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
    fn an_unset_key_variable_names_the_field_to_look_at_rather_than_its_contents() {
        // It used to name the variable, which is the obviously helpful thing
        // to do and is why it took six leaks to stop doing it: the name comes
        // out of `rmem.toml`, so whenever someone had written the key there
        // instead, the refusal printed the key. Naming the field sends the
        // reader to the one line they need, and they can read their own file.
        let mut config = parse(TEMPLATE).unwrap();
        config.provider.api_key_env = "RMEM_TEST_DEFINITELY_UNSET".to_string();
        // Not `.unwrap_err()`: that needs `HttpProvider: Debug` for the panic
        // message on the `Ok` arm, and `HttpProvider` deliberately has no
        // `Debug` impl so the key it holds can never end up in one.
        let err = match config.provider() {
            Err(e) => e,
            Ok(_) => panic!("expected a missing-key error"),
        };
        assert_eq!(err, CliError::MissingKey);
        let text = err.to_string();
        assert!(text.contains("api_key_env"), "{text}");
        assert!(
            !text.contains("RMEM_TEST_DEFINITELY_UNSET"),
            "even an innocuous name is a value out of the file: {text}"
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
