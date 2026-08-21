//! Extraction against responses a real model actually returned.
//!
//! Every other test in this crate asserts what the author expected. These
//! assert what happened: each case is a response *shape* observed while running
//! `benches/locomo` over LoCoMo conversation 0 — 419 turns of real dialogue
//! through `gpt-4o-mini` — with the count it occurred at. The turn text is real
//! too, taken from the conversation, so a reader can see what the model was
//! looking at when it produced the shape.
//!
//! They exist because the benchmark is the wrong loop for this work. A run
//! costs thirteen minutes and real money, moves five numbers at once, and its
//! per-category figures do not replicate between runs. Editing a prompt against
//! that is guessing with a delay. These run in milliseconds, are deterministic,
//! and fail on exactly the thing being edited.
//!
//! # These characterised a cost, and then it was paid down
//!
//! When written, several of these asserted that a whole turn was discarded over
//! one bad field — what the crate did at the time — and said so in terms of what
//! was thrown away rather than merely that something was. A test that only
//! checked `is_err()` would have let that cost stay invisible, which is how it
//! stayed invisible until a real corpus went through it.
//!
//! `extract` now drops the offending item and keeps the rest, so those
//! assertions have changed. They are kept in that changed form deliberately:
//! each one still names the shape, still carries the count it occurred at, and
//! now states what survives it. Read together they are the measurement of the
//! change — the same 135 responses, and what is left of them.

use rm_extract::{extract, CompleterError, ExtractError, Turn};

/// A completer that returns one fixed response, standing in for the model.
struct Canned(&'static str);

impl rm_extract::Completer for Canned {
    fn complete(&self, _prompt: &str) -> Result<String, CompleterError> {
        Ok(self.0.to_string())
    }
}

/// A real turn from LoCoMo conversation 0.
fn turn(text: &str, speaker: &str) -> Turn {
    Turn {
        text: text.to_string(),
        speaker: Some(speaker.to_string()),
        observed_at: 1_683_554_160_000, // 1:56 pm on 8 May 2023, session 1's date
        session: "D1:1".to_string(),
    }
}

fn unparsable(e: ExtractError) -> String {
    match e {
        ExtractError::Unparsable(why) => why,
        other => panic!("expected Unparsable, got {other:?}"),
    }
}

// ---- the dominant failure -------------------------------------------------

/// Observed **76 times in 419 turns** (18%) after the prompt was told to stop
/// emitting mentions for unnamed groups, and 14 times before it.
///
/// The rule worked — "the kids" is no longer a mention — but the model still
/// wrote the fact that referred to them, and a fact must index a mention that
/// exists.
///
/// This response carries nothing else, so nothing survives it. What changed is
/// that it is no longer an *error*: the turn produced an empty extraction and
/// said why, which a caller can distinguish from a turn that said nothing.
#[test]
fn a_fact_naming_a_mention_that_was_not_listed_is_dropped() {
    let response = r#"{
      "mentions": [],
      "facts": [
        {"subject": 0, "attribute": "occupation", "value": "busy with kids and work",
         "text": "Melanie is swamped with the kids and work", "days_ago": null}
      ],
      "relations": [],
      "closures": []
    }"#;
    let turn = turn(
        "Hey Caroline! Good to see you! I'm swamped with the kids & work.",
        "Melanie",
    );
    let out = extract(&turn, &Canned(response)).expect("no longer an error");
    assert!(out.mentions.is_empty());
    assert!(out.facts.is_empty());
    assert_eq!(out.dropped.len(), 1);
    assert!(
        out.dropped[0]
            .why
            .contains("names mention 0, but the response listed 0"),
        "{}",
        out.dropped[0]
    );
}

/// The same failure with something worth keeping beside it.
///
/// This is the case that made the cost legible, and the one that measures the
/// change. The model named Melanie correctly and wrote a good fact about her;
/// under the old behaviour a *second* fact with a bad index threw both away.
/// Across the run that was 76 turns of 419 reduced to nothing, most with a
/// usable mention in them.
#[test]
fn one_unanchored_fact_no_longer_discards_the_mentions_that_parsed_cleanly() {
    let response = r#"{
      "mentions": [
        {"kind": "person", "name": "Melanie", "text": "I"}
      ],
      "facts": [
        {"subject": 0, "attribute": "activity", "value": "charity race",
         "text": "Melanie ran a charity race", "days_ago": 3},
        {"subject": 4, "attribute": "cause", "value": "mental health",
         "text": "the race raised awareness for mental health", "days_ago": null}
      ],
      "relations": [],
      "closures": []
    }"#;
    let turn = turn(
        "Hey Caroline, since we last chatted, I've had a lot of things happening to me. I ran a charity race.",
        "Melanie",
    );
    let out = extract(&turn, &Canned(response)).expect("one bad fact is not a bad turn");

    // What used to be lost, now kept.
    assert_eq!(out.mentions.len(), 1);
    assert_eq!(out.mentions[0].name, "Melanie");
    assert_eq!(out.facts.len(), 1);
    assert_eq!(out.facts[0].attribute, "activity");
    assert_eq!(out.facts[0].value.as_deref(), Some("charity race"));

    // And what was actually wrong is still reported rather than swallowed.
    assert_eq!(out.dropped.len(), 1);
    assert_eq!(out.dropped[0].what, "fact");
    assert_eq!(out.dropped[0].index, 1);
}

// ---- responses that are not JSON at all -----------------------------------

/// Observed **26 times in 419 turns**. The prompt says "reply with only a JSON
/// object ... and nothing else"; the model prefaces it anyway.
///
/// The one shape salvage cannot help: there is no parsed half to keep. This
/// remains the crate's only whole-response refusal, and these 26 are the
/// residue the change does not touch.
#[test]
fn prose_before_the_json_is_still_refused_whole() {
    let response = r#"Sure! Here's the extraction:

{"mentions": [], "facts": [], "relations": [], "closures": []}"#;
    let why = unparsable(extract(&turn("Hey Mel!", "Caroline"), &Canned(response)).unwrap_err());
    assert!(why.contains("expected value at line 1 column 1"), "{why}");
}

/// The same shape wearing a markdown fence, which is how a chat-tuned model
/// most often volunteers JSON.
#[test]
fn a_markdown_fence_is_still_refused_whole_too() {
    let response =
        "```json\n{\"mentions\": [], \"facts\": [], \"relations\": [], \"closures\": []}\n```";
    let why = unparsable(extract(&turn("Hey Mel!", "Caroline"), &Canned(response)).unwrap_err());
    assert!(why.contains("expected value"), "{why}");
}

// ---- fields of the wrong type ---------------------------------------------

/// Observed **24 times in 419 turns**, at many different columns — the model
/// answers a yes/no-shaped attribute with a JSON boolean rather than the string
/// the schema asks for.
#[test]
fn a_boolean_where_a_string_belongs_is_unparsable() {
    let response = r#"{
      "mentions": [{"kind": "person", "name": "Caroline", "text": "I"}],
      "facts": [
        {"subject": 0, "attribute": "attended_support_group", "value": true,
         "text": "Caroline attended an LGBTQ support group", "days_ago": 1}
      ],
      "relations": [],
      "closures": []
    }"#;
    let turn = turn(
        "I went to a LGBTQ support group yesterday and it was so powerful.",
        "Caroline",
    );
    let out = extract(&turn, &Canned(response)).expect("one bad field is not a bad turn");
    assert_eq!(
        out.mentions.len(),
        1,
        "Caroline used to go with the bad fact"
    );
    assert_eq!(out.mentions[0].name, "Caroline");
    assert!(out.facts.is_empty());
    assert!(
        out.dropped[0].why.contains("invalid type: boolean"),
        "{}",
        out.dropped[0]
    );
}

/// Observed 4 times: a count answered as a number.
#[test]
fn an_integer_where_a_string_belongs_drops_only_that_fact() {
    let response = r#"{
      "mentions": [{"kind": "person", "name": "Melanie", "text": "I"}],
      "facts": [
        {"subject": 0, "attribute": "children", "value": 2,
         "text": "Melanie has two children", "days_ago": null}
      ],
      "relations": [],
      "closures": []
    }"#;
    let out = extract(
        &turn("I'm swamped with the kids", "Melanie"),
        &Canned(response),
    )
    .unwrap();
    assert_eq!(out.mentions.len(), 1, "Melanie survives the bad fact");
    assert!(out.facts.is_empty());
    assert!(
        out.dropped[0].why.contains("invalid type: integer"),
        "{}",
        out.dropped[0]
    );
}

/// Observed twice: a null value for a *field*, which is different from the
/// null the schema does allow for `value`.
#[test]
fn a_null_name_drops_the_mention_where_a_null_value_is_fine() {
    let nameless = r#"{
      "mentions": [{"kind": "person", "name": null, "text": "someone"}],
      "facts": [], "relations": [], "closures": []
    }"#;
    let out = extract(&turn("someone said so", "Caroline"), &Canned(nameless)).unwrap();
    assert!(out.mentions.is_empty());
    assert!(
        out.dropped[0].why.contains("invalid type: null"),
        "{}",
        out.dropped[0]
    );

    // The contrast, so the rule is visible rather than inferred: `value` is
    // nullable on purpose -- it is how "he is between jobs" is said.
    let null_value = r#"{
      "mentions": [{"kind": "person", "name": "Melanie", "text": "I"}],
      "facts": [
        {"subject": 0, "attribute": "employer", "value": null,
         "text": "Melanie is between jobs", "days_ago": null}
      ],
      "relations": [], "closures": []
    }"#;
    let out = extract(&turn("I'm between jobs", "Melanie"), &Canned(null_value))
        .expect("a null value is a fact, not a failure");
    assert_eq!(out.facts.len(), 1);
    assert_eq!(out.facts[0].value, None);
}

// ---- impossible relations --------------------------------------------------

/// Observed 5 times. The model relates a mention to itself, usually on a turn
/// where someone talks about their own family.
#[test]
fn a_relation_from_a_mention_to_itself_is_dropped() {
    let response = r#"{
      "mentions": [{"kind": "person", "name": "Melanie", "text": "I"}],
      "facts": [],
      "relations": [
        {"subject": 0, "predicate": "parent_of", "object": 0, "days_ago": null}
      ],
      "closures": []
    }"#;
    let out = extract(
        &turn("I'm swamped with the kids", "Melanie"),
        &Canned(response),
    )
    .unwrap();
    assert_eq!(
        out.mentions.len(),
        1,
        "Melanie survives her own bad relation"
    );
    assert!(out.relations.is_empty());
    assert!(
        out.dropped[0].why.contains("to itself"),
        "{}",
        out.dropped[0]
    );
}

// ---- the control -----------------------------------------------------------

/// A response of the shape the prompt asks for, on a real turn, so the cases
/// above are read against something that works rather than against nothing.
#[test]
fn a_well_formed_response_extracts_everything_it_carries() {
    let response = r#"{
      "mentions": [
        {"kind": "person", "name": "Caroline", "text": "I"},
        {"kind": "organisation", "name": "LGBTQ support group", "text": "a LGBTQ support group"}
      ],
      "facts": [
        {"subject": 0, "attribute": "attended", "value": "LGBTQ support group",
         "text": "Caroline attended an LGBTQ support group", "days_ago": 1}
      ],
      "relations": [
        {"subject": 0, "predicate": "member_of", "object": 1, "days_ago": 1}
      ],
      "closures": []
    }"#;
    let turn = turn(
        "I went to a LGBTQ support group yesterday and it was so powerful.",
        "Caroline",
    );
    let out = extract(&turn, &Canned(response)).expect("the shape the prompt asks for");
    assert!(
        out.dropped.is_empty(),
        "a clean response must drop nothing -- otherwise `dropped` says nothing when it is empty"
    );
    assert_eq!(out.mentions.len(), 2);
    assert_eq!(out.facts.len(), 1);
    assert_eq!(out.relations.len(), 1);
    // "yesterday" is one day before the turn, not the turn's own timestamp.
    assert!(out.facts[0].valid_from < turn.observed_at);
}
