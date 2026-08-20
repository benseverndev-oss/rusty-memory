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
    fn parse(path: &Path, text: &str) -> Result<Config, CliError> {
        toml::from_str(text).map_err(|e| {
            let span = e.span();
            CliError::Config(format!(
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
    pub fn ruleset(&self) -> Result<Ruleset, CliError> {
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
                    return Err(CliError::Config(format!(
                        "{where_} in rmem.toml gives {name} = {p}, but it is a probability and must be strictly between 0 and 1 -- 0 or 1 would make this one field's agreement decide every comparison on its own, whatever the other evidence says"
                    )));
                }
            }
            if f.m <= f.u {
                return Err(CliError::Config(format!(
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
            return Err(CliError::Config(
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
            return Err(CliError::Config(
                "rmem.toml's [resolution] review_at and match_at must both be finite numbers of bits".to_string(),
            ));
        }
        if review_at > match_at {
            return Err(CliError::Config(format!(
                "rmem.toml's [resolution] gives review_at = {review_at}, which is above match_at = {match_at}. That leaves no review band and makes some matches unreachable; review_at must be at or below match_at."
            )));
        }

        Ruleset::new(rules, blocking, review_at, match_at).map_err(|_| {
            CliError::Config(
                "rmem.toml's [resolution] section is not one this build can turn into a resolver, for a reason the checks above did not anticipate. Its own words are not repeated here: they name the fields they were given, and a field name comes out of the file.".to_string(),
            )
        })
    }

    /// The survivorship policy this file describes.
    pub fn policy_for_engine(&self) -> Result<Policy, CliError> {
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
    pub fn metric(&self) -> Result<Metric, CliError> {
        match self.provider.metric.as_str() {
            "cosine" => Ok(Metric::Cosine),
            "l2" => Ok(Metric::L2),
            _ => Err(CliError::Config(format!(
                "rmem.toml's [provider] metric is not one this build knows; use \"cosine\" or \"l2\". It is not defaulted because choosing wrong is a silent quality bug -- results stay plausible and get subtly worse. {NOT_REPEATED}"
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
/// The cost is real and worth stating: a kind not listed here degrades to the
/// last line, so a reader gets the location and "not valid TOML" rather than
/// the parser's own words. The location, and the field list `table_hint` adds,
/// are what carry the repair.
fn our_reason(message: &str) -> &'static str {
    const KINDS: &[(&str, &str)] = &[
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
        (
            "invalid escape sequence",
            "a backslash escape inside that string is not one TOML defines; TOML allows \\b, \\t, \\n, \\f, \\r, \\\", \\\\, \\uXXXX and \\UXXXXXXXX",
        ),
        ("invalid string", "that string is not well formed"),
        ("unterminated", "that value is never closed"),
        ("expected", "the syntax there is not valid TOML"),
        ("unexpected", "the syntax there is not valid TOML"),
        ("trailing", "there is more text after the end of that value"),
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
    let Some(header) = text[..start]
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
fn comparator(name: &str, where_: &str) -> Result<Comparator, CliError> {
    match name {
        "exact" => Ok(Comparator::Exact),
        "normalized" => Ok(Comparator::Normalized),
        "jaro_winkler" => Ok(Comparator::JaroWinkler),
        "token_jaccard" => Ok(Comparator::TokenJaccard),
        _ => Err(CliError::Config(format!(
            "{where_} in rmem.toml names a comparator this build does not know; use \"exact\", \"normalized\", \"jaro_winkler\" or \"token_jaccard\". {NOT_REPEATED}"
        ))),
    }
}

fn blocking_key(b: &BlockingConfig, where_: &str) -> Result<BlockingKey, CliError> {
    match b.kind.as_str() {
        "exact" => Ok(BlockingKey::Exact(b.field.clone())),
        "token" => Ok(BlockingKey::Token(b.field.clone())),
        "prefix" if b.n > 0 => Ok(BlockingKey::Prefix(b.field.clone(), b.n)),
        "prefix" => Err(CliError::Config(format!(
            "{where_} in rmem.toml is a prefix key and needs n greater than 0; n = 0 puts every record in one block, which compares everything to everything"
        ))),
        _ => Err(CliError::Config(format!(
            "{where_} in rmem.toml names a blocking kind this build does not know; use \"exact\", \"prefix\" or \"token\". {NOT_REPEATED}"
        ))),
    }
}

fn strategy(name: &str, where_: &str) -> Result<Strategy, CliError> {
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
        "source_priority" => Err(CliError::Config(format!(
            "{where_} in rmem.toml asks for source_priority, which needs an order of sources to rank, and this config format has no way to say one yet -- choose another strategy, or rank them in code"
        ))),
        _ => Err(CliError::Config(format!(
            "{where_} in rmem.toml names a strategy this build does not know; use one of most_complete, longest_value, majority_vote, confidence_majority, first_non_null, unanimous_or_null, most_recent, valid_interval. {NOT_REPEATED}"
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
            TEMPLATE.replace("review_at = 4.0", &format!("review_at = \"{FAKE}\"")),
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
            .replace("review_at = 4.0", "field = []\nreview_at = 4.0");

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
        let swapped = TEMPLATE
            .replace("review_at = 4.0", "review_at = 6.0")
            .replace("match_at = 6.0", "match_at = 4.0");
        let err = parse(&swapped).unwrap().ruleset().unwrap_err();
        assert!(err.to_string().contains('6'), "{err}");
        assert!(err.to_string().contains('4'), "{err}");
    }

    #[test]
    fn an_unknown_comparator_names_the_entry_and_the_choices_but_not_what_was_written() {
        let bad = TEMPLATE.replace("comparator = \"jaro_winkler\"", "comparator = \"vibes\"");
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
                "comparator = \"jaro_winkler\"",
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
            let config = parse(&TEMPLATE.replace(from, &to)).expect("still valid TOML");
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
}
