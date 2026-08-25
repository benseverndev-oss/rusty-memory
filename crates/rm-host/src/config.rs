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

use crate::HostError;

/// The file `rmem init` writes.
///
/// Kept as one literal so the test that parses it is testing the same bytes a
/// user gets. A template assembled at runtime could pass its own test and still
/// write something unreadable.
/// The dimension [`TEMPLATE`] ships with.
///
/// Named because `rmem init --local` writes it without probing a model, and a
/// second copy of the number would be free to drift from the file. The test
/// `the_named_dimension_is_the_one_the_template_carries` reads it back out of
/// the template so the two cannot disagree.
///
/// It is the template's value rather than a number chosen for subword hashing:
/// `benches/locomo` measured the local embedder at whatever the config said,
/// and nothing in the documentation tells a reader to change it, so this is the
/// configuration the published 6/12 recall figure actually describes.
pub const TEMPLATE_DIMENSION: usize = 1536;

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
# Where vectors come from: "http" asks the service above, "local" computes them
# here with no socket and no model file.
#
# Local is subword hashing -- see `rm_embed`. It has no semantics: "car" and
# "automobile" land orthogonal, where a real model puts them together. What it
# has is morphology and overlap, which is a bet that your queries reuse the
# words your records were written in. That is often true of a decision log whose
# titles were chosen to be findable, and rarely true of conversation.
#
# Switching this is not free: vectors from the two are not comparable, so the
# index has to be rebuilt. `rmem reindex` does that where the text can be worked
# out again, and refuses where it cannot.
embedder = "http"
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
#
# These are calibrated against the fields below and are not portable away from
# them. They were 4.0 and 6.0 when `name` was the only field; adding `kind`
# adds log2(0.9/0.38) = 1.2439256 bits to every pair whose kinds agree, so both
# rose by that much and a pair that agrees on kind lands where it landed
# before. Written to four places, which leaves each boundary 0.000026 bits
# below the exact figure -- a pair inside that sliver really would decide
# differently, and nothing rounder gets closer. Delete the `kind` rule
# and these want to come back down, or every threshold is 1.24 bits stricter
# than it reads.
#
# The shift also makes a kind disagreement final rather than merely expensive.
# A name can contribute at most log2(0.9/0.01) = 6.49 bits, a kind
# disagreement costs 2.63, and 6.49 - 2.63 = 3.86 is below review_at -- so two
# entities whose kinds differ are never asked about however identical their
# names. "Paris" the city is not "Paris" the person, and no spelling makes it
# so. The cost is that extraction's own inconsistencies become final too: it
# called the same pets "animal" one run and "thing" the next, and those two
# will now never be offered for merging. That is a threshold policy, not
# something the probabilities below imply -- lower these two and the veto
# becomes a penalty again.
review_at = 5.2439
match_at = 7.2439

[[resolution.field]]
field = "name"
# jaro_winkler, plus one rule: a thing is never confused with what it belongs
# to. "Melanie's son" shares all of "Melanie" and sits where the prefix bonus
# rewards it, so plain jaro_winkler scores the pair 0.92 and asks whether a
# woman is her own child. Use "jaro_winkler" instead for fields holding only
# proper names, where the two behave identically anyway.
comparator = "possessive_aware"
# m = P(this field agrees | the two are the same thing)
# u = P(this field agrees | they are different things) -- the field's commonness
m = 0.9
u = 0.01

# What a thing is, which is often the whole answer. The kind is asserted on
# every entity anyway, and withholding it from resolution meant a person and a
# place were compared on their names alone -- over half the review band was
# pairs whose kinds already disagreed.
#
# "exact" because these are drawn from a closed vocabulary: "person" is not a
# near miss for "place", and a fuzzy comparator over category labels would
# invent agreement between "organisation" and "animal" that nothing means.
#
# u is high because there are only a handful of kinds and the distribution is
# skewed: 0.38 is the rate at which two entities that share a name prefix --
# the pairs blocking actually compares -- happen to share a kind, measured
# across four stores from a real corpus. m is lower than for a name because
# extraction is not perfectly consistent about the boundary cases; it called
# the same pets "animal" once and "thing" the next run.
[[resolution.field]]
field = "kind"
comparator = "exact"
m = 0.9
u = 0.38

[[resolution.blocking]]
kind = "prefix"
field = "name"
n = 3

[retrieval]
# Below this similarity, `recall` says the nearest thing it found is a weak
# match. It still returns every hit -- this labels, it never filters.
#
# Off by default, and the reason is worth reading before turning it on.
#
# # It is a weak signal
#
# Measured against LoCoMo's adversarial questions, whose premise the
# conversation does not support: 382 answerable against 112 unanswerable over
# three conversations. Of six candidate signals the top hit's raw score
# separated best (Youden's J = 0.494). Every shape-based signal -- the gap to
# the tenth hit, the ratio to the mean, the spread -- did worse, which is the
# opposite of what you would guess.
#
# J hides the trade, and the trade is bad:
#
#     keep 99% of answerable questions  ->  refuses  4.5% of unanswerable
#     keep 95%                          ->  refuses 14.3%
#     keep 90%                          ->  refuses 36.6%
#     best J (cutoff 0.706)             ->  keeps 62.8%, refuses 86.6%
#
# That is why it labels rather than filters. Dropping enough unanswerable
# queries to matter costs a tenth to a third of the real answers, and a memory
# that loses a third of what it knows to look confident is worse than one that
# answers and says how sure it is.
#
# # And it does not travel
#
# 0.62 was picked off the table above and then tried on this project's own
# decision log, where it marked a question with a perfect answer -- "should we
# add a reranker", nearest hit 0.531 -- as having nothing near it. Same
# embedding model, different corpus, and the scale moved out from under the
# number. Decision text and conversational turns simply do not land in the same
# range.
#
# So there is no default worth shipping. A cutoff that has not been measured
# against the corpus it will run on produces confident warnings on good answers,
# which is worse than the silence it was meant to fix. 0.0 turns it off.
#
# To set it, run your own questions past the store and look at where the
# answers land against where the misses do. The bench in `benches/locomo` is
# the worked example.
weak_below = 0.0

[policy]
# How competing values for one attribute are resolved, at read time.
default = "most_recent"

[policy.attribute]
# An employer changes; both facts are worth keeping, with the store answering
# by date rather than picking a winner.
employer = "valid_interval"
"#;

/// Whichever embedder the config named.
///
/// An enum rather than a boxed trait object: there are two, both are known
/// here, and the call sites want a concrete `impl Embedder` to hand to the
/// plan functions.
pub enum Embedding {
    /// Vectors from the service in `[provider]`.
    ///
    /// Boxed: an `HttpProvider` carries an agent and its TLS config and is two
    /// orders of magnitude larger than the local one, so an unboxed enum would
    /// make every `Embedding` that size.
    Http(Box<HttpProvider>),
    /// Vectors computed here, with no socket and no model file.
    Local(rm_embed::Hashed),
}

impl rm_engine::Embedder for Embedding {
    fn embed(&self, text: &str) -> Result<Vec<f32>, rm_engine::EmbedderError> {
        match self {
            Embedding::Http(p) => p.embed(text),
            Embedding::Local(h) => h.embed(text),
        }
    }
}

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
    /// Defaulted, so a config written before this section existed still loads
    /// rather than being refused for a field its author could not have known
    /// about.
    #[serde(default)]
    pub retrieval: RetrievalConfig,
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
    /// `"http"` or `"local"`. Defaulted, so a config written before this
    /// existed keeps asking the service it was written for.
    #[serde(default = "default_embedder")]
    pub embedder: String,
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
pub struct RetrievalConfig {
    /// Below this similarity, `recall` notes that its nearest hit is weak.
    ///
    /// Defaulted rather than required, so a config written before this section
    /// existed still loads. The default is the measured one; see `TEMPLATE`.
    #[serde(default = "default_weak_below")]
    pub weak_below: f32,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        RetrievalConfig {
            weak_below: default_weak_below(),
        }
    }
}

/// Off.
///
/// Not a placeholder for a number nobody measured: a cutoff that has not been
/// calibrated against the corpus it runs on marks good answers as weak, which
/// is worse than the silence it replaces. See `TEMPLATE` for the measurement
/// and for why 0.62 -- the best figure from the LoCoMo sweep -- was withdrawn
/// after it fired on a question this project can answer perfectly well.
fn default_embedder() -> String {
    "http".to_string()
}

fn default_weak_below() -> f32 {
    0.0
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyConfig {
    pub default: String,
    #[serde(default)]
    pub attribute: BTreeMap<String, String>,
}

/// What [`Config::read_for_init`] found at `rmem.toml`'s path.
pub enum InitConfig {
    /// No file was there yet -- the ordinary first run.
    Absent(Config),
    /// A file was there, and it parsed.
    Loaded(Config),
    /// A file was there and did not parse. Carries the same
    /// [`HostError::Config`] [`Config::load`] would have returned, so a caller
    /// that decides to refuse can hand it straight back, and a caller that
    /// decides to proceed anyway -- `init --force` -- still has its words to
    /// show rather than inventing new ones.
    Unparsable(HostError),
}

impl Config {
    /// Read a config file.
    pub fn load(path: &Path) -> Result<Config, HostError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            HostError::Config(format!(
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
    ///
    /// `init --force` is the one caller that may want the fallback anyway --
    /// see `Config::read_for_init`, which this is now built on top of and
    /// which draws that distinction explicitly rather than leaving it to a
    /// second, diverging copy of this match.
    pub fn load_or_template(path: &Path) -> Result<Config, HostError> {
        match Self::read_for_init(path)? {
            InitConfig::Absent(config) | InitConfig::Loaded(config) => Ok(config),
            InitConfig::Unparsable(e) => Err(e),
        }
    }

    /// What trying to read a config for `rmem init` turned up.
    ///
    /// Three outcomes rather than two, because `init` and `init --force` need
    /// to tell them apart and a plain `Result` cannot: [`InitConfig::Absent`]
    /// and [`InitConfig::Unparsable`] both end in a template-backed
    /// [`Config`], but only one of them is silent about it, and which one is
    /// silent depends on whether the caller passed `--force`. That decision
    /// belongs to the caller, not here -- this only reports what was found.
    pub fn read_for_init(path: &Path) -> Result<InitConfig, HostError> {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::parse(path, &text) {
                Ok(config) => Ok(InitConfig::Loaded(config)),
                Err(e) => Ok(InitConfig::Unparsable(e)),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(InitConfig::Absent(Config::from_template()))
            }
            Err(e) => Err(HostError::Config(format!(
                "could not read {}: {e}",
                path.display()
            ))),
        }
    }

    /// Read a config, saying what is wrong with it in this crate's own words.
    ///
    /// Nothing `toml` produced reaches the message. Not its `Display`, which
    /// reproduces the offending source line; not its `message()`, which quotes
    /// whatever it did not like — and what it did not like came out of the
    /// file, in key position (`unknown field` ...) and in value position
    /// (`invalid type: string` ...) alike.
    ///
    /// The version before this one filtered `message()`, dropping quoted spans
    /// it did not recognise. That cannot be made correct: a backtick inside a
    /// backtick span is not escaped, so no scanner can tell a delimiter from
    /// content, and a fuzz of 180,000 configs found 3,455 payloads that walked
    /// straight through. See [`our_reason`].
    ///
    /// What is left is built from three things this crate already holds: the
    /// error's position, which becomes a line and column; its kind, which
    /// selects one of [`our_reason`]'s literals; and [`SCHEMA`], which supplies
    /// the fields legal in whichever table the fault fell inside. Between them
    /// they say where, why, and what would have been valid.
    fn parse(path: &Path, text: &str) -> Result<Config, HostError> {
        toml::from_str(text).map_err(|e| {
            let span = e.span();
            HostError::Config(format!(
                "{} is not valid: {} ({}){}",
                path.display(),
                our_reason(e.message()),
                location(text, span.clone()),
                table_hint(text, span),
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
    ///
    /// The `m`, `u` and threshold checks are made here rather than left to
    /// `Ruleset::new`, and that is the whole point of them. `FieldRule`'s own
    /// validation names the field it was given — `field {:?}: m = ...` — and
    /// that string came out of `[[resolution.field]]`, so `field =
    /// "sk-proj-SECRET"` with `m = 0.9, u = 0.95` printed a credential the
    /// moment its error was relayed. Naming its input is reasonable for a
    /// library; relaying it is not reasonable here.
    ///
    /// So the same conditions are checked first, in this crate's words, naming
    /// the entry by index the way `where_` does elsewhere. The numbers *are*
    /// interpolated: they deserialise as `f64`, so no string from the file can
    /// reach them whatever is written there.
    ///
    /// `Ruleset::new` is still called and its refusal still honoured — it is
    /// the authority on what a valid ruleset is, and duplicating that judgement
    /// rather than deferring to it would be the real mistake. What is dropped
    /// is its *text*, so a rule added there later cannot start printing the
    /// file through this path. The cost, stated: such a rule would arrive here
    /// as the last message below, which says where to look and not what is
    /// wrong.
    pub fn ruleset(&self) -> Result<Ruleset, HostError> {
        let mut rules = Vec::new();
        // Entries are numbered rather than named. `field = "name"` is itself
        // text out of the file, so quoting it to say which rule went wrong
        // would be the same mistake in a smaller place; a 1-based index is a
        // location, which is what a reader needs and what nothing can hide a
        // secret in.
        for (n, f) in self.resolution.field.iter().enumerate() {
            let where_ = format!("[[resolution.field]] entry {}", n + 1);
            for (name, p) in [("m", f.m), ("u", f.u)] {
                if !(p > 0.0 && p < 1.0) {
                    return Err(HostError::Config(format!(
                        "{where_} in rmem.toml gives {name} = {p}, but it is a probability and must be strictly between 0 and 1 -- 0 or 1 would make this one field's agreement decide every comparison on its own, whatever the other evidence says"
                    )));
                }
            }
            if f.m <= f.u {
                return Err(HostError::Config(format!(
                    "{where_} in rmem.toml gives m = {} and u = {}, but m must be greater than u -- otherwise agreement on this field is evidence *against* a match. Either the two are swapped, or the field does not discriminate and should be left out.",
                    f.m, f.u
                )));
            }
            rules.push(FieldRule::new(
                f.field.clone(),
                comparator(&f.comparator, &where_)?,
                f.m,
                f.u,
            ));
        }
        if rules.is_empty() {
            return Err(HostError::Config(
                "rmem.toml has no [[resolution.field]] entries, so every pair scores 0 and the thresholds decide everything uniformly. Add at least one field to compare.".to_string(),
            ));
        }
        let mut blocking = Vec::new();
        for (n, b) in self.resolution.blocking.iter().enumerate() {
            blocking.push(blocking_key(
                b,
                &format!("[[resolution.blocking]] entry {}", n + 1),
            )?);
        }

        let (review_at, match_at) = (self.resolution.review_at, self.resolution.match_at);
        if !(review_at.is_finite() && match_at.is_finite()) {
            return Err(HostError::Config(
                "rmem.toml's [resolution] review_at and match_at must both be finite numbers of bits".to_string(),
            ));
        }
        if review_at > match_at {
            return Err(HostError::Config(format!(
                "rmem.toml's [resolution] gives review_at = {review_at}, which is above match_at = {match_at}. That leaves no review band and makes some matches unreachable; review_at must be at or below match_at."
            )));
        }

        Ruleset::new(rules, blocking, review_at, match_at).map_err(|_| {
            HostError::Config(
                "rmem.toml's [resolution] section is not one this build can turn into a resolver, for a reason the checks above did not anticipate. Its own words are not repeated here: they name the fields they were given, and a field name comes out of the file.".to_string(),
            )
        })
    }

    /// The survivorship policy this file describes.
    pub fn policy_for_engine(&self) -> Result<Policy, HostError> {
        let mut policy = Policy::new(strategy(&self.policy.default, "[policy] default")?);
        for (attribute, name) in &self.policy.attribute {
            // Neither half of an entry here is named on refusal. In
            // `[policy.attribute]` the attribute *and* the strategy are both
            // text out of the file -- it is an open map, which is why
            // `deny_unknown_fields` cannot defend it, and it is the last table
            // in `TEMPLATE`, so `api_key = "sk-..."` appended to the end of the
            // file lands here as an entry rather than as an unknown field.
            //
            // An earlier version of this named the attribute while saying in
            // the same sentence that what was written was not repeated. That
            // was true of one half and false of the other:
            // `"sk-proj-..." = "nonsense"` printed the key as the attribute.
            let chosen = strategy(name, "an entry under [policy.attribute]")?;
            policy = policy.with(attribute.clone(), chosen);
        }
        Ok(policy)
    }

    /// The distance metric this file names.
    pub fn metric(&self) -> Result<Metric, HostError> {
        match self.provider.metric.as_str() {
            "cosine" => Ok(Metric::Cosine),
            "l2" => Ok(Metric::L2),
            _ => Err(HostError::Config(format!(
                "rmem.toml's [provider] metric is not one this build knows; use \"cosine\" or \"l2\". It is not defaulted because choosing wrong is a silent quality bug -- results stay plausible and get subtly worse. {NOT_REPEATED}"
            ))),
        }
    }

    /// A provider built from this file and the environment.
    /// Where vectors come from, per `[provider] embedder`.
    ///
    /// Separate from [`Config::provider`] because the answer can be "here". A
    /// local embedder needs no base URL and no API key, so a store configured
    /// for one can record and read decisions with no credential in the
    /// environment and no socket opened -- which is the whole point of having
    /// the option.
    pub fn embedder(&self) -> Result<Embedding, HostError> {
        match self.provider.embedder.as_str() {
            "local" => Ok(Embedding::Local(rm_embed::Hashed::new(
                self.provider.dimension,
            ))),
            "http" => Ok(Embedding::Http(Box::new(self.provider()?))),
            other => Err(HostError::Config(format!(
                "rmem.toml's [provider] embedder is {other:?}, which this build does not know; use \"http\" to ask the service, or \"local\" to compute vectors here. It is not defaulted because the two are not comparable: a store built under one cannot be searched under the other without `rmem reindex`."
            ))),
        }
    }

    pub fn provider(&self) -> Result<HttpProvider, HostError> {
        validate_base_url(&self.provider.base_url)?;

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
        // several. `HostError::MissingKey` no longer names the variable at all,
        // which is what actually closes it, for every key format that exists
        // and every one that does not exist yet.
        let name = &self.provider.api_key_env;
        let legal = !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !legal {
            return Err(HostError::Config(
                "rmem.toml's api_key_env must be the NAME of an environment variable -- letters, digits and underscores, not starting with a digit, like OPENAI_API_KEY -- and what is written there cannot be one. It is not repeated here: if what you pasted is the key itself, printing it would put it in your terminal, your scrollback and any log that catches them.".to_string(),
            ));
        }
        let key = std::env::var(name).map_err(|_| HostError::MissingKey)?;
        Ok(HttpProvider::new(
            self.provider.base_url.clone(),
            key,
            self.provider.completion_model.clone(),
            self.provider.embedding_model.clone(),
        ))
    }
}

/// The tables `rmem.toml` may hold, and the fields legal in each.
///
/// Written by hand because it has to be *ours*. Every word of a parse refusal
/// is now built from this table and from the error's position, so that not one
/// character of the message came out of the user's file.
/// `every_key_the_template_uses_is_one_the_schema_lists` guards it against
/// drift as fields are added.
///
/// `[policy.attribute]` is deliberately absent. It is an open map, so it has no
/// legal-field list to offer, and inventing one would mean printing attribute
/// names a user chose.
const SCHEMA: &[(&str, &[&str])] = &[
    ("[store]", &["path"]),
    (
        "[provider]",
        &[
            "base_url",
            "embedder",
            "api_key_env",
            "completion_model",
            "embedding_model",
            "dimension",
            "metric",
        ],
    ),
    ("[resolution]", &["review_at", "match_at"]),
    ("[[resolution.field]]", &["field", "comparator", "m", "u"]),
    ("[[resolution.blocking]]", &["kind", "field", "n"]),
    ("[retrieval]", &["weak_below"]),
    ("[policy]", &["default"]),
];

/// This crate's own sentence for what went wrong, chosen by looking at
/// `message` and never by copying any of it.
///
/// # Why the filter it replaced could not work
///
/// The previous version scanned `toml`'s message and dropped quoted spans it
/// did not recognise. A review fuzzed 180,000 configs against it and found
/// 3,455 distinct leaking payloads: every one in key position, every one
/// containing a backtick. `toml` renders a field name inside backticks using
/// `Display`, and a backtick inside a backtick span is not escaped — so
/// nothing reading that string can tell a backtick that is *content* from the
/// one that closes the span. That is a property of the string, not a bug in
/// the scanner, and no refinement of the scanner removes it.
///
/// So the message is no longer filtered; it is not used. The return type is
/// `&'static str`, which is the point: every value this can return is a
/// literal below, so it is not possible for anything read out of the file to
/// reach a caller. `message` is looked at only to decide *which* of our
/// sentences to print, and a wrong guess picks a wrong sentence rather than
/// leaking anything.
///
/// The cost, and it is now small: a kind not listed here degrades to the last
/// line, so a reader gets the location and "not valid TOML" rather than a
/// reason. Every message kind this version of `toml` can produce is listed --
/// its parser's twenty labels, its four custom errors, serde's six, and
/// winnow's bare "expected" -- so the fallback is reached only by a kind a
/// future version adds. `every_toml_message_kind_has_a_sentence_of_our_own`
/// drives one input per kind and fails if any of them reaches it.
fn our_reason(message: &str) -> &'static str {
    // Ordered only where one prefix contains another: "invalid time offset"
    // has to be tried before "invalid time", or an offset is reported as a
    // time. Everything else is grouped by what a reader is looking at.
    //
    // The wording is ours throughout, deliberately: the parser's own phrasing
    // is the thing that cannot be relayed, so reaching for it here — even by
    // paraphrase close enough to copy — would be re-introducing the habit that
    // this table exists to replace.
    const KINDS: &[(&str, &str)] = &[
        // serde, about the shape of the document rather than its syntax.
        (
            "unknown field",
            "that line sets a field this build does not know",
        ),
        (
            "missing field",
            "a field this build requires is not set in that table",
        ),
        (
            "duplicate key",
            "that key is set more than once in the same table",
        ),
        (
            "dotted key",
            "that dotted key tries to extend something that is not a table",
        ),
        (
            "invalid type",
            "the value there is not the type that field takes",
        ),
        (
            "invalid value",
            "the value there is not one that field accepts",
        ),
        (
            "invalid length",
            "the value there is not the length that field takes",
        ),
        // Strings and what can go inside one.
        (
            "invalid escape sequence",
            "a backslash escape inside that string is not one TOML defines; TOML allows \\b, \\t, \\n, \\f, \\r, \\\", \\\\, \\uXXXX and \\UXXXXXXXX",
        ),
        (
            "invalid unicode",
            "a unicode escape inside that string does not have the digits it needs; TOML wants exactly four hex digits after \\u and exactly eight after \\U",
        ),
        (
            "invalid multiline",
            "that multiline string is never closed; it opens and closes with three quotes",
        ),
        (
            "invalid basic string",
            "that string is never closed, or holds a character that has to be escaped; a basic string opens and closes with a double quote",
        ),
        (
            "invalid literal string",
            "that string is never closed; a literal string opens and closes with a single quote",
        ),
        ("invalid string", "that string is not well formed"),
        // Structure.
        (
            "invalid table header",
            "that table header is not well formed; a header is a name in square brackets, like [provider], or double brackets for an array of tables, like [[resolution.field]]",
        ),
        (
            "invalid key",
            "that line does not begin with a key TOML can read; a bare key is letters, digits, underscores and dashes, and anything else has to be quoted",
        ),
        (
            "invalid array",
            "that array is never closed, or holds something TOML cannot put in one",
        ),
        (
            "invalid inline table",
            "that inline table is never closed, or holds something TOML cannot put in one",
        ),
        // Numbers.
        (
            "invalid hexadecimal integer",
            "that is not a hexadecimal integer; after 0x TOML wants hex digits and nothing else",
        ),
        (
            "invalid octal integer",
            "that is not an octal integer; after 0o TOML wants digits 0 to 7",
        ),
        (
            "invalid binary integer",
            "that is not a binary integer; after 0b TOML wants 0 or 1",
        ),
        (
            "invalid integer",
            "that is not an integer TOML can read; it wants digits, optionally separated by single underscores, and no leading zero",
        ),
        (
            "invalid floating-point number",
            "that is not a number TOML can read; it wants digits, at most one decimal point, and an optional exponent",
        ),
        (
            "number too large",
            "that number is too large for the type this field takes",
        ),
        (
            "value is out of range",
            "that value is outside the range TOML allows for its type",
        ),
        // Dates and times. "invalid time offset" precedes "invalid time".
        (
            "invalid date-time",
            "that is not a date-time TOML can read; it wants RFC 3339, like 1979-05-27T07:32:00Z",
        ),
        (
            "invalid time offset",
            "that time offset is not one TOML can read; it wants Z, or +HH:MM, or -HH:MM",
        ),
        (
            "invalid time",
            "that is not a time TOML can read; it wants HH:MM:SS",
        ),
        (
            "recursion limit exceeded",
            "that value nests deeper than this build will follow",
        ),
        // Syntax, where the parser had no label to give. `unterminated` and
        // `trailing` used to sit here and matched nothing in this version of
        // `toml`: an unclosed string is reported against its own kind above,
        // and text after a value comes out as "expected newline".
        ("expected", "the syntax there is not valid TOML"),
        ("unexpected", "the syntax there is not valid TOML"),
    ];
    for (prefix, ours) in KINDS {
        if message.starts_with(prefix) {
            return ours;
        }
    }
    "it is not valid TOML"
}

/// The fields legal in the table the fault fell inside, if it is one of ours.
///
/// Read out of this crate's own copy of the file, and matched against
/// [`SCHEMA`] rather than printed: the table name and the field names that
/// reach the message are the constants above, so a header spelled anything
/// else simply yields no hint. This is the useful half of what `toml`'s
/// "expected one of ..." used to give, rebuilt from a source that cannot carry
/// a secret.
fn table_hint(text: &str, span: Option<std::ops::Range<usize>>) -> String {
    let Some(span) = span else {
        return String::new();
    };
    let mut start = span.start.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    // Search back from the END of the line `start` falls on, so that line is
    // included. A *missing field* error carries the span of the table header
    // itself, and searching `text[..start]` excluded it: the second table
    // onward named the table *before* the one at fault, and the first table
    // got no hint at all, because there was nothing before it to find. Both
    // on exactly the errors where the hint is the useful part. Reported by a
    // session that hand-wrote a minimal config and was told its `[provider]`
    // problem was about `[store] takes path`.
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let Some(header) = text[..line_end]
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with('['))
    else {
        return String::new();
    };
    for (table, fields) in SCHEMA {
        if *table == header {
            return format!(". {table} takes {}", fields.join(", "));
        }
    }
    String::new()
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

/// The sentence every refusal below ends with, in one place so it cannot drift
/// between them.
///
/// Six credential leaks on this branch came out of error messages echoing a
/// value read from the config, each closed one shape at a time while the class
/// stayed open. The rule that replaced that: a refusal names a field, a
/// location, or nothing — never a value out of the file. A user who wrote a
/// value can read it back in their own editor, which is worth almost nothing
/// to them and was the whole attack surface.
const NOT_REPEATED: &str = "What is written there is not repeated here: any value in this file may turn out to be a pasted credential, so a refusal names the field and leaves you to read the value in your own copy.";

/// `where_` names the field or the entry, in words the reader can find in the
/// file. It is built by the caller from table names and 1-based indices, never
/// from the file's own text.
fn comparator(name: &str, where_: &str) -> Result<Comparator, HostError> {
    match name {
        "exact" => Ok(Comparator::Exact),
        "normalized" => Ok(Comparator::Normalized),
        "jaro_winkler" => Ok(Comparator::JaroWinkler),
        "token_jaccard" => Ok(Comparator::TokenJaccard),
        "possessive_aware" => Ok(Comparator::PossessiveAware),
        _ => Err(HostError::Config(format!(
            "{where_} in rmem.toml names a comparator this build does not know; use \"exact\", \"normalized\", \"jaro_winkler\", \"token_jaccard\" or \"possessive_aware\". {NOT_REPEATED}"
        ))),
    }
}

fn blocking_key(b: &BlockingConfig, where_: &str) -> Result<BlockingKey, HostError> {
    match b.kind.as_str() {
        "exact" => Ok(BlockingKey::Exact(b.field.clone())),
        "token" => Ok(BlockingKey::Token(b.field.clone())),
        "prefix" if b.n > 0 => Ok(BlockingKey::Prefix(b.field.clone(), b.n)),
        "prefix" => Err(HostError::Config(format!(
            "{where_} in rmem.toml is a prefix key and needs n greater than 0; n = 0 puts every record in one block, which compares everything to everything"
        ))),
        _ => Err(HostError::Config(format!(
            "{where_} in rmem.toml names a blocking kind this build does not know; use \"exact\", \"prefix\" or \"token\". {NOT_REPEATED}"
        ))),
    }
}

fn strategy(name: &str, where_: &str) -> Result<Strategy, HostError> {
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
        //
        // "source_priority" appears in this message as a literal written into
        // the binary, on a branch reached only when the file already said
        // exactly that. It is not the file's text being echoed back, so the
        // rule above is not bent here.
        "source_priority" => Err(HostError::Config(format!(
            "{where_} in rmem.toml asks for source_priority, which needs an order of sources to rank, and this config format has no way to say one yet -- choose another strategy, or rank them in code"
        ))),
        _ => Err(HostError::Config(format!(
            "{where_} in rmem.toml names a strategy this build does not know; use one of most_complete, longest_value, majority_vote, confidence_majority, first_non_null, unanimous_or_null, most_recent, valid_interval. {NOT_REPEATED}"
        ))),
    }
}

/// Reject a `base_url` that cannot be a base URL to append a path onto.
///
/// Hand-rolled rather than pulled in from the `url` crate, which is already in
/// the dependency tree via `ureq` but not a direct dependency of this crate --
/// adding it as one would be the minimal-dependency discipline the rest of
/// this workspace holds to giving way the moment a real parser looked
/// convenient. What is checked is narrow on purpose: the scheme is `http://`
/// or `https://`, the authority names a host and carries no userinfo, and
/// nothing after the scheme carries a query or a fragment.
///
/// # What this closes
///
/// `HttpProvider` builds every request as `format!("{base_url}/{path}")`
/// after trimming one trailing slash, so `base_url =
/// "https://host/v1?key=X"` silently becomes
/// `https://host/v1?key=X/embeddings` -- the query string swallows the path
/// this crate meant to append, so the request never reaches `/embeddings` at
/// all, and nothing about that fails loudly: the URL is well-formed and
/// `ureq` sends it exactly as written. A fragment does the same thing one
/// character later, and userinfo (`user:pass@host`) is accepted by an HTTP
/// client but is not a shape any provider's base URL legitimately takes, so
/// there is no cost to refusing it alongside the other two.
///
/// # What this does not close
///
/// `rm_providers::ProviderError`'s own doc comment names the residual this
/// workspace has left open on purpose: a provider that echoes the request
/// path back in an error body -- a 404 naming the route it could not find is
/// ordinary -- can return a credential that was pasted into `base_url` as a
/// *path segment* rather than as userinfo or a query. `https://host/v1` with
/// `sk-proj-XXXX` spliced into the path is structurally indistinguishable
/// from `https://host/openai/v1` or an Azure deployment path -- both are
/// ordinary, load-bearing shapes for a provider's base URL -- so telling them
/// apart means guessing at the *value*, which is the thing six leaks on this
/// branch have already shown cannot be done reliably. This function narrows
/// that residual by closing the two shapes that do not require guessing at a
/// value; it does not close it.
fn validate_base_url(base_url: &str) -> Result<(), HostError> {
    const WHERE: &str = "rmem.toml's [provider] base_url";

    let after_scheme = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"));
    let Some(after_scheme) = after_scheme else {
        return Err(HostError::Config(format!(
            "{WHERE} must start with http:// or https://. {NOT_REPEATED}"
        )));
    };

    // The authority ends at the first `/`, `?` or `#` -- whichever comes
    // first marks the end of `host[:port]` and the start of path, query or
    // fragment. `rest` is everything from there on, checked below for the
    // two shapes that swallow whatever this crate appends to it.
    let split = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let (authority, rest) = after_scheme.split_at(split);

    if authority.is_empty() {
        return Err(HostError::Config(format!(
            "{WHERE} must name a host after the scheme. {NOT_REPEATED}"
        )));
    }
    if authority.contains('@') {
        return Err(HostError::Config(format!(
            "{WHERE} must not carry a username or password before the host -- the \"user@\" part of a URL. {NOT_REPEATED}"
        )));
    }
    if rest.contains('?') {
        return Err(HostError::Config(format!(
            "{WHERE} must not carry a query string (\"?...\"); the path this crate appends would land inside the query rather than after it. {NOT_REPEATED}"
        )));
    }
    if rest.contains('#') {
        return Err(HostError::Config(format!(
            "{WHERE} must not carry a fragment (\"#...\"). {NOT_REPEATED}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Config, HostError> {
        toml::from_str(s).map_err(|e| HostError::Config(e.to_string()))
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
        // What it asserts changed once field *names* stopped being quoted
        // back: `assert!(err.contains("api_key"))` now passes for a reason
        // that has nothing to do with this fixture, because the list of legal
        // fields contains `api_key_env`. That is the substring-that-happens-
        // to-match trap this branch has already been caught by twice, so it
        // asserts the shape of the refusal and the location instead -- the
        // parts that actually depend on the paste being refused.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let pasted = TEMPLATE.replace(
            "api_key_env = \"OPENAI_API_KEY\"",
            "api_key_env = \"OPENAI_API_KEY\"
api_key = \"sk-PASTED-FAKE-SECRET-LEAK-CHECK-1234\"",
        );
        std::fs::write(&path, pasted).unwrap();

        let err = Config::load(&path).unwrap_err().to_string();
        assert!(
            err.contains("a field this build does not know"),
            "the paste has to be refused, not dropped: {err}"
        );
        assert!(
            err.contains("line 15"),
            "and located, since the name is no longer quoted: {err}"
        );
        assert!(
            !err.contains("sk-PASTED-FAKE-SECRET"),
            "the value must never be echoed: {err}"
        );
    }

    #[test]
    fn a_field_that_is_not_one_is_refused_rather_than_ignored() {
        use crate::testing::TempDir;

        // The ordinary-mistake half of the same guard. A field written
        // `embedding_model_name` used to fall through and reappear later as
        // "missing field `embedding_model`" -- a message naming the one field
        // the file appears to already have, which sends the reader looking in
        // the wrong place.
        //
        // This used to assert that the refusal named `embedding_model_name`
        // back. It no longer does, and should not: a field name is text out of
        // the file, and `sk-proj-... = "x"` puts a credential in exactly that
        // position. The line and column say where, and
        // `a_parse_error_still_names_the_fields_that_would_have_been_valid`
        // pins the part of the message that is genuinely ours.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(
            &path,
            TEMPLATE.replace("embedding_model =", "embedding_model_name ="),
        )
        .unwrap();

        let err = Config::load(&path).unwrap_err().to_string();
        assert!(err.contains("a field this build does not know"), "{err}");
        assert!(err.contains("line"), "{err}");
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
    fn a_parse_error_never_quotes_the_file_back_in_either_key_or_value_position() {
        use crate::testing::TempDir;

        // The seventh leak, found by sweeping for the rule rather than by
        // waiting for someone to drive it. `9cb29a2` stopped `Display`
        // echoing the source *line* and left `message()` quoting the same
        // text: `toml` names what it did not like, and what it did not like is
        // the file's.
        //
        // Both positions, because they are different mechanisms and the first
        // fix for either would have missed the other -- a key in key position
        // is an unknown field, a key in value position is a type error.
        const FAKE: &str = "sk-proj-FAKE-SECRET-LEAK-CHECK-9999";
        let cases = [
            // Key position: pasted as a field name under [provider].
            TEMPLATE.replace(
                "completion_model =",
                &format!("{FAKE} = \"x\"\ncompletion_model ="),
            ),
            // Value position: pasted where an integer belongs.
            TEMPLATE.replace("dimension = 1536", &format!("dimension = \"{FAKE}\"")),
            // Value position again, on a float, which is a different serde
            // message with the same shape.
            doctored("review_at = 5.24", &format!("review_at = \"{FAKE}\"")),
            // And in a table whose keys are open, where it parses as data
            // rather than failing here at all.
            format!("{TEMPLATE}\"{FAKE}\" = \"nonsense\"\n"),
        ];

        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        for case in cases {
            std::fs::write(&path, &case).unwrap();
            match Config::load(&path) {
                Err(e) => {
                    let text = e.to_string();
                    assert!(!text.contains(FAKE), "the file came back out: {text}");
                    // A parse failure keeps its location, which is what does
                    // the work once the quoted text is gone.
                    assert!(text.contains("line"), "the location has to survive: {text}");
                }
                // The open-map case is valid TOML and valid for the struct, so
                // it parses; the refusal comes later, and the rule holds there
                // too. There is no span to report by then.
                Ok(config) => {
                    // A `match` rather than `expect_err`, which needs
                    // `Policy: Debug` -- the same reason every other refusal
                    // test in this crate destructures instead of unwrapping.
                    let Err(e) = config.policy_for_engine() else {
                        panic!("expected a refusal");
                    };
                    let text = e.to_string();
                    assert!(!text.contains(FAKE), "the file came back out: {text}");
                    assert!(text.contains("[policy.attribute]"), "{text}");
                }
            }
        }
    }

    #[test]
    fn a_parse_error_still_names_the_fields_that_would_have_been_valid() {
        use crate::testing::TempDir;

        // The useful half of what `toml`'s "expected one of ..." used to give,
        // rebuilt from a source that cannot carry a secret: `SCHEMA`, plus the
        // table the fault fell inside, read from this crate's own copy of the
        // file and *matched* against SCHEMA rather than printed.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(
            &path,
            TEMPLATE.replace("embedding_model =", "embedding_model_name ="),
        )
        .unwrap();

        let text = Config::load(&path).unwrap_err().to_string();
        assert!(text.contains("does not know"), "{text}");
        assert!(
            !text.contains("embedding_model_name"),
            "a field name is file text too: {text}"
        );
        assert!(
            text.contains("[provider] takes"),
            "the table has to be named: {text}"
        );
        assert!(
            text.contains("embedding_model") && text.contains("base_url"),
            "and the fields that would have been valid: {text}"
        );
    }

    #[test]
    fn every_toml_message_kind_has_a_sentence_of_our_own() {
        // One input per message kind this version of `toml` can produce,
        // enumerated from its parser's labels, its custom errors and serde's
        // -- not invented. The assertion is that none of them reaches the
        // fallback, because the fallback says only "not valid TOML" and the
        // kinds below are ones where knowing *what* is wrong is the repair:
        // `"\u12"` is four hex digits short, and being told the file is not
        // valid TOML does not say so.
        //
        // Each case also asserts the canary is absent, since several of these
        // messages quote the offending text and this is the guard that they
        // never arrive quoted.
        const FALLBACK: &str = "it is not valid TOML";
        const CANARY: &str = "REALSECRETabc123DEF456";
        let cases: &[(&str, &str)] = &[
            ("table header", "[REALSECRETabc123DEF456 x\nx = 1\n"),
            ("key", "= \"REALSECRETabc123DEF456\"\n"),
            ("integer", "x = 12__3\n"),
            ("hexadecimal integer", "x = 0xZZ\n"),
            ("octal integer", "x = 0o99\n"),
            ("binary integer", "x = 0b22\n"),
            ("floating-point number", "x = 1.2e\n"),
            ("number too large", "x = 999999999999999999999999999\n"),
            ("date-time", "x = 2020-13-45T99:99:99Z\n"),
            ("time offset", "x = 2020-01-01T00:00:00+99:99\n"),
            ("unicode 4-digit", "x = \"\\u12\"\n"),
            ("unicode 8-digit", "x = \"\\U1234\"\n"),
            ("escape sequence", "x = \"REALSECRETabc123DEF456\\q\"\n"),
            ("basic string", "x = \"REALSECRETabc123DEF456\n"),
            ("literal string", "x = 'REALSECRETabc123DEF456\n"),
            (
                "multiline basic string",
                "x = \"\"\"REALSECRETabc123DEF456\n",
            ),
            ("array", "x = [1, 2\n"),
            ("inline table", "x = { a = 1\n"),
            ("string", "x = = 1\n"),
            ("duplicate key", "x = 1\nx = 2\n"),
            ("dotted key", "x = 1\nx.y = 2\n"),
            ("expected", "x = 1 y = 2\n"),
        ];

        for (kind, text) in cases {
            let Err(e) = toml::from_str::<toml::Value>(text) else {
                panic!("{kind}: the fixture stopped being a parse error");
            };
            let ours = our_reason(e.message());
            assert_ne!(
                ours,
                FALLBACK,
                "{kind} degrades to the fallback; toml said {:?}",
                e.message()
            );
            assert!(
                !ours.contains(CANARY),
                "{kind}: our own literal cannot contain the file, so this is impossible"
            );
        }
    }

    #[test]
    fn a_malformed_table_header_says_what_a_header_looks_like() {
        use crate::testing::TempDir;

        // The weakest output before this: `[provider` gave "it is not valid
        // TOML" and `table_hint` had no header to work from either, so the
        // whole message was a location. It is the one case where both halves
        // of the message went missing at once.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, TEMPLATE.replace("[provider]", "[provider")).unwrap();

        let text = Config::load(&path).unwrap_err().to_string();
        assert!(text.contains("table header"), "{text}");
        assert!(text.contains("[[resolution.field]]"), "{text}");
        assert!(text.contains("line"), "{text}");
    }

    #[test]
    fn a_bad_escape_says_which_escapes_toml_actually_allows() {
        use crate::testing::TempDir;

        // Under the filter this replaced, `invalid escape sequence` arrived
        // with its list of legal escapes destroyed -- `n` and `u` survived
        // only because they happen to be config field names, and the trailing
        // ellipsis was the escape handler eating a closing backtick. A user
        // who typed a bad escape learned nothing. Built from our own words,
        // the message can simply say what TOML allows.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, TEMPLATE.replace("memory.json", "memory\\q.json")).unwrap();

        let text = Config::load(&path).unwrap_err().to_string();
        assert!(text.contains("backslash escape"), "{text}");
        assert!(text.contains("\\uXXXX"), "{text}");
        assert!(text.contains("line"), "{text}");
    }

    #[test]
    fn every_key_the_template_uses_is_one_the_schema_lists() {
        // `SCHEMA` is written by hand, so it drifts the moment a field is
        // added. Drift is quiet -- a new field would simply drop out of the
        // hint -- which is exactly why nothing else would catch it.
        let mut table = "";
        for line in TEMPLATE.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                table = line;
                continue;
            }
            // TEMPLATE's comments explain `m` and `u` with prose containing an
            // `=`, so a comment is not a key.
            if line.starts_with('#') {
                continue;
            }
            let Some((key, _)) = line.split_once(" = ") else {
                continue;
            };
            if table == "[policy.attribute]" {
                // The other direction, and the more important one. Keys in
                // this table are attribute names a user invents, so they are
                // data, not vocabulary -- `employer` happens to be in
                // TEMPLATE, and a pasted key could be in someone's. SCHEMA
                // lists no fields for it at all, so no hint is ever offered
                // there.
                assert!(
                    !SCHEMA.iter().any(|(t, _)| *t == table),
                    "[policy.attribute] is an open map and must have no field list"
                );
                continue;
            }
            let fields = SCHEMA
                .iter()
                .find(|(t, _)| *t == table)
                .unwrap_or_else(|| panic!("SCHEMA does not list the table {table:?}"))
                .1;
            assert!(
                fields.contains(&key),
                "TEMPLATE uses {key:?} in {table}, which SCHEMA does not list"
            );
        }
    }

    #[test]
    fn no_parse_refusal_repeats_the_file_however_the_payload_is_shaped() {
        use crate::testing::TempDir;

        // The guard that replaces the scanner, and the shape of it is the
        // point. A review fuzzed 180,000 configs against the filter that used
        // to live here and found 3,455 leaking payloads -- all in key
        // position, all containing a backtick, because `toml` renders a field
        // name inside backticks and does not escape a backtick within one. No
        // scanner can pair those delimiters.
        //
        // So the payloads here are deliberately awkward rather than
        // representative: a backtick in the middle, a backtick leading, quotes,
        // a bare secret. If any of them reaches a message, the approach is
        // wrong again rather than the pattern being one more special case.
        const CANARY: &str = "REALSECRETabc123DEF456";
        let payloads = [
            format!("sk-proj-{CANARY}"),
            format!("sk-proj-`{CANARY}"),
            format!("`{CANARY}"),
            format!("{CANARY}`"),
            format!("`{CANARY}`"),
            format!("sk-proj-\\\"{CANARY}"),
            format!("sk-proj-'{CANARY}'"),
            format!("[{CANARY}]"),
            format!("{CANARY} = {CANARY}"),
        ];

        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let mut refused = 0;
        for payload in &payloads {
            // Key position under a closed table, value position on an integer
            // field, value position on a string field, a duplicate key, and
            // the open map -- five different `toml` error kinds.
            let cases = [
                TEMPLATE.replace(
                    "completion_model =",
                    &format!("\"{payload}\" = \"x\"\ncompletion_model ="),
                ),
                TEMPLATE.replace("dimension = 1536", &format!("dimension = \"{payload}\"")),
                TEMPLATE.replace("metric = \"cosine\"", &format!("metric = \"{payload}\"")),
                TEMPLATE.replace(
                    "path = \"memory.json\"",
                    &format!("path = \"memory.json\"\n\"{payload}\" = \"x\"\npath = \"b\""),
                ),
                format!("{TEMPLATE}\"{payload}\" = \"nonsense\"\n"),
            ];
            for case in cases {
                std::fs::write(&path, &case).unwrap();
                let text = match Config::load(&path) {
                    Err(e) => e.to_string(),
                    Ok(config) => match (config.metric(), config.policy_for_engine()) {
                        (Err(e), _) => e.to_string(),
                        (_, Err(e)) => e.to_string(),
                        _ => continue,
                    },
                };
                refused += 1;
                assert!(
                    !text.contains(CANARY),
                    "the file came back out of a refusal: {text}"
                );
            }
        }
        assert!(
            refused >= payloads.len(),
            "the fixtures stopped being refused, so this stopped testing anything"
        );
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
        // The attribute is not named either, which the first version of this
        // got wrong: it printed the map's key while saying in the same
        // sentence that what was written was not repeated. Both halves of an
        // entry in an open map are text out of the file, so a paste can land
        // in either, and `"sk-proj-..." = "nonsense"` put the key in the half
        // that was being printed.
        assert!(
            err.contains("[policy.attribute]"),
            "the table has to be named so the line can be found: {err}"
        );
        assert!(
            err.contains("most_recent"),
            "and what a strategy may be: {err}"
        );
    }

    #[test]
    fn a_base_url_that_cannot_be_a_base_url_is_refused_before_a_provider_is_ever_built() {
        // `HttpProvider` builds every request as `format!("{base_url}/{path}")`,
        // so `base_url = "https://host/v1?key=X"` silently becomes
        // `https://host/v1?key=X/embeddings` -- the query swallows the path
        // this crate meant to append, and nothing about that fails loudly.
        // Awkward fixtures, deliberately: an uppercase scheme (this hand-rolled
        // check only recognises the literal lowercase prefixes, unlike a real
        // client which treats a scheme case-insensitively -- narrower on
        // purpose, and the fixture pins that narrowing rather than assuming
        // it), userinfo, a query, a fragment, an empty host, and a bare word
        // that never had a scheme at all.
        for bad in [
            "HTTPS://host/v1",
            "ftp://host/v1",
            "not-a-url",
            "https://",
            "https:///v1",
            "https://user:pass@host/v1",
            "https://host/v1?key=X",
            "https://host?key=X",
            "https://host/v1#fragment",
        ] {
            let mut config = parse(TEMPLATE).unwrap();
            config.provider.base_url = bad.to_string();
            match config.provider() {
                Err(HostError::Config(_)) => {}
                Err(other) => panic!("{bad:?} should be a config error, got {other:?}"),
                Ok(_) => panic!("{bad:?} cannot be a base URL to append a path onto"),
            }
        }
    }

    #[test]
    fn a_base_url_with_an_explicit_port_or_a_deep_path_is_accepted() {
        // The awkward fixtures above must not overreach: a `:port` in the
        // authority is not userinfo, and a multi-segment path is not a query
        // or a fragment. Both are ordinary shapes a real provider's base URL
        // takes -- Azure deployments in particular nest several segments deep
        // -- and rejecting them would be exactly the false positive this
        // narrow a check has to avoid.
        for ok in [
            "https://api.openai.com/v1",
            "https://host:443/v1",
            "http://localhost:8080",
            "https://host/openai/deployments/gpt-4o/v1",
        ] {
            let mut config = parse(TEMPLATE).unwrap();
            config.provider.base_url = ok.to_string();
            config.provider.api_key_env = "RMEM_TEST_DEFINITELY_UNSET_FOR_BASE_URL".to_string();
            match config.provider() {
                Err(HostError::MissingKey) => {}
                Err(other) => panic!("{ok:?} is a valid base URL, got {other:?}"),
                Ok(_) => panic!("{ok:?}: RMEM_TEST_DEFINITELY_UNSET_FOR_BASE_URL is not set"),
            }
        }
    }

    #[test]
    fn a_base_url_refusal_never_repeats_the_value_that_was_written() {
        // The categorical rule: an error names a field, a location or a
        // config key, never a value read out of the file. The awkward
        // fixture: a canary containing a backtick, spliced into the query
        // shape -- the one this crate's own doc comment says a fuzz proved
        // unfilterable when it reaches a message at all.
        let canary = "CANARY-`-0123456789abcdef";
        for bad in [
            format!("https://host/v1?key={canary}"),
            format!("https://{canary}@host/v1"),
            format!("not-a-url-{canary}"),
        ] {
            let mut config = parse(TEMPLATE).unwrap();
            config.provider.base_url = bad.clone();
            let err = match config.provider() {
                Err(e) => e,
                Ok(_) => panic!("{bad:?} cannot be a base URL to append a path onto"),
            };
            let text = err.to_string();
            assert!(!text.contains(canary), "{text}");
            assert!(!text.contains('`'), "{text}");
            let debug = format!("{err:?}");
            assert!(!debug.contains(canary), "{debug}");
            assert!(text.contains("base_url"), "{text}");
        }
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
                Err(HostError::Config(_)) => {}
                Err(other) => panic!("{illegal:?} should be a config error, got {other:?}"),
                Ok(_) => panic!("{illegal:?} cannot name an environment variable"),
            }
        }
        for legal in ["_UNDERSCORE_FIRST", "A9", "RMEM_TEST_DEFINITELY_UNSET"] {
            let mut config = parse(TEMPLATE).unwrap();
            config.provider.api_key_env = legal.to_string();
            match config.provider() {
                Err(HostError::MissingKey) => {}
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

    /// `TEMPLATE` with one substitution applied, refusing to return quietly if
    /// it did not apply.
    ///
    /// Several tests here doctor the template by string match to produce a bad
    /// config. A `from` the template no longer contains makes the replacement a
    /// no-op, the config valid, and the test pass by testing nothing -- which
    /// has now happened twice, once when the template's comparator changed and
    /// once when its thresholds did. The assertion is the whole point of the
    /// helper.
    fn doctored(from: &str, to: &str) -> String {
        let out = TEMPLATE.replace(from, to);
        assert_ne!(
            out, TEMPLATE,
            "{from:?} is no longer in the template, so this test doctors nothing"
        );
        out
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

        // review_at = 5.24, match_at = 7.24: read straight off the boundaries
        // decide() draws, since that is the only way to observe them.
        assert_eq!(ruleset.decide(5.2438), Decision::NonMatch);
        assert_eq!(ruleset.decide(5.2439), Decision::Review);
        assert_eq!(ruleset.decide(7.2438), Decision::Review);
        assert_eq!(ruleset.decide(7.2439), Decision::Match);

        // m = 0.9, u = 0.01 on "name": two records that agree completely score
        // exactly the field's agreement weight, log2(m / u). The `kind` rule
        // contributes nothing here because neither record carries the field --
        // silence is not disagreement.
        let a = Record::new().with("name", "Ben Severn");
        let b = Record::new().with("name", "Ben Severn");
        let expected = (0.9_f64 / 0.01).log2();
        let got = ruleset.score(&a, &b);
        assert!(
            (got - expected).abs() < 1e-9,
            "score = {got}, expected {expected}"
        );

        // And with the kind present on both, agreeing, its own weight on top.
        let a = a.with("kind", "person");
        let b = b.with("kind", "person");
        let expected = expected + (0.9_f64 / 0.38).log2();
        let got = ruleset.score(&a, &b);
        assert!(
            (got - expected).abs() < 1e-9,
            "score = {got}, expected {expected}"
        );
    }

    #[test]
    fn the_thresholds_leave_an_agreeing_kind_exactly_where_it_was_and_veto_a_disagreeing_one() {
        // The two properties that justify moving review_at and match_at rather
        // than tuning `u` until the answers looked right.
        use rm_engine::{Decision, Record};
        let ruleset = parse(TEMPLATE).unwrap().ruleset().unwrap();
        let agreement = (0.9_f64 / 0.38).log2();

        // One: both thresholds rose by the kind agreement weight, so a pair
        // whose kinds agree is decided as it was when `name` was the only
        // field and the thresholds were 4.0 and 6.0.
        //
        // Held to a thousandth of a bit rather than exactly: the thresholds
        // are decimal literals in a config file and the weight is irrational,
        // so the two boundaries differ by about 0.00005 bits and a pair inside
        // that sliver really would decide differently. The values below sit
        // clear of it, which is the honest form of this claim.
        let rec = |name, kind| Record::new().with("name", name).with("kind", kind);
        for name_score in [3.9_f64, 4.001, 5.0, 5.999, 6.001, 6.5] {
            let old = if name_score >= 6.0 {
                Decision::Match
            } else if name_score >= 4.0 {
                Decision::Review
            } else {
                Decision::NonMatch
            };
            assert_eq!(
                ruleset.decide(name_score + agreement),
                old,
                "a name scoring {name_score} with an agreeing kind moved band"
            );
        }

        // Two: a kind disagreement is final, not merely expensive. Identical
        // names are the strongest evidence the ruleset can produce, and they
        // are still not enough.
        let identical = ruleset.score(&rec("Paris", "place"), &rec("Paris", "person"));
        assert_eq!(
            ruleset.decide(identical),
            Decision::NonMatch,
            "identical names, different kinds, scored {identical}"
        );
    }

    #[test]
    fn a_field_name_never_reaches_a_refusal_about_its_own_rule() {
        // The leak-through-a-library-error class, and a pointed one: the
        // commit that rewrote `ruleset` to number entries rather than name them
        // said in its own comment that `field = "name"` is text out of the file
        // and quoting it "would be the same mistake in a smaller place" -- and
        // then handed that string to `FieldRule::new`, whose validation
        // interpolates it with `{:?}`.
        //
        // Every condition `FieldRule::validate` can refuse on, driven with a
        // credential in the field name. `m = 0.9, u = 0.95` is the reviewer's
        // repro.
        const CANARY: &str = "sk-proj-REALSECRETabc123DEF456";
        let cases = [
            // m is not greater than u.
            ("m = 0.9", "m = 0.9"),
            // m outside (0, 1).
            ("m = 0.9", "m = 1.0"),
            ("m = 0.9", "m = 0.0"),
            // u outside (0, 1).
            ("u = 0.95", "u = 0.0"),
            ("u = 0.95", "u = 1.0"),
        ];
        for (from, to) in cases {
            let bad = TEMPLATE
                .replace("field = \"name\"", &format!("field = \"{CANARY}\""))
                .replace("u = 0.01", "u = 0.95")
                .replace(from, to);
            let config = parse(&bad).expect("still valid TOML");
            let Err(err) = config.ruleset() else {
                panic!("{from} -> {to} should have been refused");
            };
            let text = err.to_string();
            assert!(
                !text.contains(CANARY),
                "the field name came back out: {text}"
            );
            assert!(
                !format!("{err:?}").contains(CANARY),
                "it came back out through Debug"
            );
            assert!(
                text.contains("[[resolution.field]] entry 1"),
                "the entry has to be located: {text}"
            );
        }
    }

    #[test]
    fn a_config_with_no_field_rules_is_refused_naming_the_table_to_add_one_to() {
        // `Ruleset::new` refuses this too, and its wording is fine -- but it is
        // its wording, and none of those are relayed now. Checking it here also
        // lets the refusal name the table an entry has to be added to, which
        // "no field rules" did not.
        let start = TEMPLATE.find("[[resolution.field]]").unwrap();
        let end = TEMPLATE.find("[[resolution.blocking]]").unwrap();
        let none = format!("{}{}", &TEMPLATE[..start], &TEMPLATE[end..])
            .replace("review_at = 5.24", "field = []\nreview_at = 5.24");

        let config = parse(&none).expect("still valid TOML");
        let Err(err) = config.ruleset() else {
            panic!("a ruleset with no rules cannot resolve anything");
        };
        assert!(err.to_string().contains("[[resolution.field]]"), "{err}");
    }

    #[test]
    fn a_swapped_review_at_and_match_at_is_refused_naming_both() {
        // What actually pins the review_at/match_at swap: not the test above,
        // which cannot reach it (see its comment), but `Ruleset::new`'s own
        // validation, exercised directly here with the file's own numbers
        // swapped -- review_at above match_at leaves no review band at all.
        let swapped = doctored("review_at = 5.24", "review_at = 7.24")
            .replace("match_at = 7.24", "match_at = 5.24");
        let err = parse(&swapped).unwrap().ruleset().unwrap_err();
        assert!(err.to_string().contains("7.24"), "{err}");
        assert!(err.to_string().contains("5.24"), "{err}");
    }

    #[test]
    fn an_unknown_comparator_names_the_entry_and_the_choices_but_not_what_was_written() {
        let bad = TEMPLATE.replace(
            "comparator = \"possessive_aware\"",
            "comparator = \"vibes\"",
        );
        let err = parse(&bad).unwrap().ruleset().unwrap_err().to_string();
        assert!(!err.contains("vibes"), "{err}");
        // The entry by table and 1-based index -- a location, which is what a
        // reader needs. Not `field = "name"`, which is text from the file too.
        assert!(err.contains("[[resolution.field]] entry 1"), "{err}");
        assert!(err.contains("jaro_winkler"), "{err}");
    }

    #[test]
    fn an_unknown_blocking_kind_names_the_entry_and_the_choices_but_not_what_was_written() {
        let bad = TEMPLATE.replace("kind = \"prefix\"", "kind = \"vibes\"");
        let err = parse(&bad).unwrap().ruleset().unwrap_err().to_string();
        assert!(!err.contains("vibes"), "{err}");
        assert!(err.contains("[[resolution.blocking]] entry 1"), "{err}");
        assert!(err.contains("token"), "{err}");
    }

    #[test]
    fn an_unknown_strategy_names_the_field_and_the_choices_but_not_what_was_written() {
        let bad = TEMPLATE.replace("default = \"most_recent\"", "default = \"whatever\"");
        let err = parse(&bad)
            .unwrap()
            .policy_for_engine()
            .unwrap_err()
            .to_string();
        assert!(!err.contains("whatever"), "{err}");
        assert!(err.contains("[policy] default"), "{err}");
        assert!(err.contains("valid_interval"), "{err}");
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
        assert_eq!(err, HostError::MissingKey);
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
        let err = parse(&bad).unwrap().metric().unwrap_err().to_string();
        assert!(!err.contains("euclidean-ish"), "{err}");
        assert!(err.contains("[provider] metric"), "{err}");
        assert!(err.contains("cosine"), "{err}");
    }

    #[test]
    fn no_refusal_about_a_config_value_ever_repeats_the_value() {
        // The categorical guard, and the reason it is one test over every
        // field rather than one test per field. Six leaks on this branch were
        // closed one shape at a time -- verbatim echo, then masked echo, then
        // unknown field, then the open map, then the variable name -- and each
        // fix left the class open, so the next instance turned up in the code
        // the last one added. This asserts the rule instead of the instances:
        // whatever is written in a config value, a refusal about it does not
        // contain it.
        //
        // Every substitution below is a *value* of an existing key, which is
        // the case `deny_unknown_fields` cannot reach: it catches a paste that
        // arrives as a new field, never one that arrives as the value of a
        // field that already exists.
        const FAKE: &str = "sk-proj-FAKE-SECRET-LEAK-CHECK-9999";
        let substitutions = [
            ("metric = \"cosine\"", format!("metric = \"{FAKE}\"")),
            (
                "comparator = \"possessive_aware\"",
                format!("comparator = \"{FAKE}\""),
            ),
            ("kind = \"prefix\"", format!("kind = \"{FAKE}\"")),
            ("default = \"most_recent\"", format!("default = \"{FAKE}\"")),
            // The value half of a `[policy.attribute]` entry ...
            (
                "employer = \"valid_interval\"",
                format!("employer = \"{FAKE}\""),
            ),
            // ... and the key half, which is just as much text from the file.
            (
                "employer = \"valid_interval\"",
                format!("\"{FAKE}\" = \"nonsense\""),
            ),
        ];

        for (from, to) in substitutions {
            let doctored = TEMPLATE.replace(from, &to);
            // A `from` the template no longer contains would make the
            // replacement a no-op, the config valid, and this test pass by
            // testing nothing. That is how this test first went stale: the
            // template's comparator changed and the substitution stopped
            // matching, in silence.
            assert_ne!(doctored, TEMPLATE, "{from:?} is no longer in the template");
            let config = parse(&doctored).expect("still valid TOML");
            // Whichever of the three refuses first; all three are driven so no
            // substitution can silently pass by never being read.
            let errors = [
                config.metric().err(),
                config.ruleset().err(),
                config.policy_for_engine().err(),
            ];
            assert!(
                errors.iter().any(Option::is_some),
                "{to:?} was accepted rather than refused"
            );
            for err in errors.into_iter().flatten() {
                let text = err.to_string();
                assert!(!text.contains(FAKE), "{to:?} came back out: {text}");
                assert!(
                    !format!("{err:?}").contains(FAKE),
                    "{to:?} came back out through Debug"
                );
            }
        }
    }
    /// The hint names the table the error is *in*, not the one before it.
    ///
    /// A missing-field error carries the span of the table header itself, and
    /// `table_hint` used to search `text[..span.start]` -- which excludes that
    /// header and finds the previous one. Every missing-field error in the
    /// second table onward named the wrong table, on exactly the errors where
    /// the hint is what a reader needs. Reported by a session that hand-wrote
    /// a minimal config and was told its `[provider]` problem was about
    /// `[store] takes path`.
    #[test]
    fn a_missing_field_hint_names_its_own_table() {
        let text = "[store]
path = \"m.json\"

[provider]
embedder = \"local\"
";
        let err = Config::parse(Path::new("rmem.toml"), text)
            .expect_err("provider is missing required fields");
        let msg = err.to_string();
        assert!(
            msg.contains("[provider] takes"),
            "hint must name the table the error is in: {msg}"
        );
        assert!(
            !msg.contains("[store] takes"),
            "and must not name the one before it: {msg}"
        );
    }

    /// The first table got no hint at all, which is how this turned out worse
    /// than reported: searching `text[..span.start]` from offset 0 gives an
    /// empty string, so there was no header to find and the message simply
    /// ended after the location.
    #[test]
    fn a_missing_field_in_the_first_table_still_names_it() {
        let text = "[store]
";
        let err = Config::parse(Path::new("rmem.toml"), text).expect_err("store is missing path");
        assert!(err.to_string().contains("[store] takes path"), "{err}");
    }
    /// The named constant and the template's literal are the same number.
    ///
    /// Two copies of a value with nothing checking them is how the
    /// `ValidInterval` sentence and the inert `--valid-at` flag both happened.
    #[test]
    fn the_named_dimension_is_the_one_the_template_carries() {
        let line = TEMPLATE
            .lines()
            .find(|l| l.trim_start().starts_with("dimension ="))
            .expect("the template sets a dimension");
        let written: usize = line
            .split('=')
            .nth(1)
            .and_then(|v| v.trim().parse().ok())
            .expect("and sets it to a number");
        assert_eq!(written, TEMPLATE_DIMENSION);
    }
}
