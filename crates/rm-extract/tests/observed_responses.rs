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
//! # These characterise, they do not endorse
//!
//! Several cases below assert that a whole turn is discarded over one bad
//! field. That is what the crate does today, and writing it down is how the
//! cost of it becomes visible: the assertions say what was thrown away, not
//! merely that something was. A test that only checked `is_err()` would let
//! that cost stay invisible, which is how it stayed invisible until a real
//! corpus was run through it.

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

fn malformed(e: ExtractError) -> String {
    match e {
        ExtractError::Malformed(why) => why,
        other => panic!("expected Malformed, got {other:?}"),
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
/// exists. The turn is refused whole.
#[test]
fn a_fact_naming_a_mention_that_was_not_listed_discards_the_whole_turn() {
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
    let why = malformed(extract(&turn, &Canned(response)).unwrap_err());
    assert_eq!(why, "a fact names mention 0, but the response listed 0");
}

/// The same failure with something worth keeping beside it.
///
/// This is the case that makes the cost legible: the model named Melanie
/// correctly, and one unanchored fact throws her away too. Across the run that
/// is 76 turns of a 419-turn conversation reduced to nothing, most of which had
/// a usable mention in them.
#[test]
fn one_unanchored_fact_discards_the_mentions_that_parsed_cleanly() {
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
    let why = malformed(extract(&turn, &Canned(response)).unwrap_err());
    assert_eq!(why, "a fact names mention 4, but the response listed 1");

    // What that costs, stated rather than implied: a named person and a
    // well-formed fact about her, both discarded over a second fact.
    // If this crate ever learns to drop the offending fact instead, this
    // assertion is the one that should change, and it should change loudly.
}

// ---- responses that are not JSON at all -----------------------------------

/// Observed **26 times in 419 turns**. The prompt says "reply with only a JSON
/// object ... and nothing else"; the model prefaces it anyway.
#[test]
fn prose_before_the_json_is_unparsable() {
    let response = r#"Sure! Here's the extraction:

{"mentions": [], "facts": [], "relations": [], "closures": []}"#;
    let why = unparsable(extract(&turn("Hey Mel!", "Caroline"), &Canned(response)).unwrap_err());
    assert!(why.contains("expected value at line 1 column 1"), "{why}");
}

/// The same shape wearing a markdown fence, which is how a chat-tuned model
/// most often volunteers JSON.
#[test]
fn a_markdown_fence_is_unparsable_too() {
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
    let why = unparsable(extract(&turn, &Canned(response)).unwrap_err());
    assert!(why.contains("invalid type: boolean"), "{why}");
}

/// Observed 4 times: a count answered as a number.
#[test]
fn an_integer_where_a_string_belongs_is_unparsable() {
    let response = r#"{
      "mentions": [{"kind": "person", "name": "Melanie", "text": "I"}],
      "facts": [
        {"subject": 0, "attribute": "children", "value": 2,
         "text": "Melanie has two children", "days_ago": null}
      ],
      "relations": [],
      "closures": []
    }"#;
    let why = unparsable(
        extract(
            &turn("I'm swamped with the kids", "Melanie"),
            &Canned(response),
        )
        .unwrap_err(),
    );
    assert!(why.contains("invalid type: integer"), "{why}");
}

/// Observed twice: a null value for a *field*, which is different from the
/// null the schema does allow for `value`.
#[test]
fn a_null_name_is_unparsable_where_a_null_value_is_fine() {
    let nameless = r#"{
      "mentions": [{"kind": "person", "name": null, "text": "someone"}],
      "facts": [], "relations": [], "closures": []
    }"#;
    let why =
        unparsable(extract(&turn("someone said so", "Caroline"), &Canned(nameless)).unwrap_err());
    assert!(why.contains("invalid type: null"), "{why}");

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
fn a_relation_from_a_mention_to_itself_discards_the_whole_turn() {
    let response = r#"{
      "mentions": [{"kind": "person", "name": "Melanie", "text": "I"}],
      "facts": [],
      "relations": [
        {"subject": 0, "predicate": "parent_of", "object": 0, "days_ago": null}
      ],
      "closures": []
    }"#;
    let why = malformed(
        extract(
            &turn("I'm swamped with the kids", "Melanie"),
            &Canned(response),
        )
        .unwrap_err(),
    );
    assert!(why.contains("runs from mention 0 to itself"), "{why}");
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
    assert_eq!(out.mentions.len(), 2);
    assert_eq!(out.facts.len(), 1);
    assert_eq!(out.relations.len(), 1);
    // "yesterday" is one day before the turn, not the turn's own timestamp.
    assert!(out.facts[0].valid_from < turn.observed_at);
}
