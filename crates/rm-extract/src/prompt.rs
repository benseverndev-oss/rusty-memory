//! The question this crate asks, and the shape of answer it will accept.
//!
//! Kept beside the parser deliberately. The two have to agree, and nothing
//! outside this crate checks that they do — a prompt that has drifted from its
//! schema yields a thin extraction rather than an error.

use crate::Turn;

/// The prompt for one turn.
///
/// Public so a host can read it, log it, diff it across versions, or build a
/// few-shot variant on top of it. The crate owning the contract does not mean
/// the contract has to be a secret.
pub fn prompt(turn: &Turn) -> String {
    let speaker = match &turn.speaker {
        Some(name) => format!("The speaker is {name}. Resolve \"I\", \"me\" and \"my\" to them."),
        // Stated rather than omitted: a prompt with a blank where the speaker
        // should be reads as a template that failed to render, and a model will
        // fill it. Saying the speaker is unknown is an instruction.
        None => "The speaker is not known. Do not invent a name for them; leave first-person references unattributed.".to_string(),
    };

    format!(
        r#"Extract what this turn of dialogue says about people, organisations and places, as JSON.

{speaker}

Turn:
{text}

Reply with only a JSON object of this shape, and nothing else:

{{
  "mentions": [
    {{"kind": "person", "name": "Alex Chen", "text": "Alex"}},
    {{"kind": "organisation", "name": "Globex", "text": "Globex"}}
  ],
  "facts": [
    {{"subject": 0, "attribute": "employer", "value": "Globex",
      "text": "Alex works at Globex", "days_ago": null}}
  ],
  "relations": [
    {{"subject": 0, "predicate": "employed_by", "object": 1, "days_ago": null}}
  ],
  "closures": [
    {{"subject": 0, "predicate": "employed_by", "days_ago": null,
      "because": "starting a new job ends the previous one"}}
  ]
}}

Rules:

- "mentions" lists every distinct thing the turn refers to. "subject" and
  "object" everywhere else are indices into it, starting at 0.
- "kind" is what sort of thing it is: person, organisation, place, or another
  word that fits.
- "name" is what to call it, used to recognise it again in later turns. Use the
  fullest form the turn gives.
- "text" on a mention is the phrasing the turn used. "text" on a fact is a short
  sentence stating that fact on its own, because it is searched for separately.
- "value" may be null to say an attribute has no value — "he is between jobs"
  is a fact with a null value, not a missing fact.
- "days_ago" is how long before now the thing began or ended, as a whole number
  of days, or null if it is happening now. It is never negative: nothing here
  is in the future. Do not output dates or timestamps.
- "closures" is for relationships that have ended. If the turn says someone
  started a new job, their previous employment ended: emit a closure naming the
  subject and the predicate, and say why in "because". Do not name what it
  ended — that is not known here. Emit nothing if nothing ended.
- Every list may be empty. A turn that says nothing worth remembering is a valid
  answer with four empty lists.
"#,
        text = turn.text
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(text: &str, speaker: Option<&str>) -> Turn {
        Turn {
            text: text.to_string(),
            speaker: speaker.map(str::to_string),
            observed_at: 1_720_000_000_000,
            session: "session-1".to_string(),
        }
    }

    #[test]
    fn the_prompt_carries_the_turn_it_is_about() {
        let p = prompt(&turn("I started at Globex", None));
        assert!(p.contains("I started at Globex"));
    }

    #[test]
    fn the_prompt_names_the_speaker_so_first_person_resolves() {
        // Without it the model has no way to turn "I" into a mention anything
        // can be resolved against, and every turn invents a new person.
        let p = prompt(&turn("I started at Globex", Some("Ben Severn")));
        assert!(p.contains("Ben Severn"));
    }

    #[test]
    fn an_unattributed_turn_says_so_rather_than_leaving_a_gap() {
        // A prompt with an empty slot where the speaker should be reads as a
        // template that failed to render, and a model will fill it with
        // something. Saying the speaker is unknown is an instruction; leaving
        // a blank is an invitation.
        let p = prompt(&turn("Alice started at Globex", None));
        assert!(!p.contains("Ben Severn"));
        assert!(
            p.to_lowercase().contains("speaker is not known"),
            "the prompt must state the absence rather than omit the line"
        );
    }

    #[test]
    fn the_prompt_describes_every_field_the_parser_reads() {
        // The prompt and the schema have to agree, and nothing else checks
        // that they do. If a field is added to the wire format without being
        // described here, the model never emits it and the extraction is
        // silently thin.
        let p = prompt(&turn("anything", None));
        for field in [
            "mentions",
            "facts",
            "relations",
            "closures",
            "kind",
            "name",
            "text",
            "subject",
            "attribute",
            "value",
            "predicate",
            "object",
            "days_ago",
            "because",
        ] {
            assert!(p.contains(field), "the prompt never mentions {field:?}");
        }
    }

    /// Find the JSON example embedded in the prompt's instructions.
    ///
    /// Panics rather than returning an empty string when the marker or a
    /// balanced brace is missing, because a locator that fails quietly would
    /// make the round-trip test below vacuous -- it would "pass" by parsing
    /// nothing.
    fn example_json(prompt: &str) -> &str {
        let after_marker = prompt
            .find("nothing else:")
            .map(|i| &prompt[i..])
            .expect("the prompt should tell the model to reply with only JSON");
        let start = after_marker
            .find('{')
            .expect("the prompt should show a JSON example after that instruction");
        let body = &after_marker[start..];
        let mut depth = 0usize;
        for (i, ch) in body.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &body[..=i];
                    }
                }
                _ => {}
            }
        }
        panic!("the prompt's JSON example never closes its opening brace");
    }

    #[test]
    fn the_prompt_s_example_round_trips_through_the_wire_schema() {
        // The substring test above catches a field added to the types and
        // never described, but a textual check cannot catch a field dropped
        // from the example while its prose sentence lingers, or one renamed to
        // a word already present elsewhere. Parsing the example itself as the
        // schema the parser actually reads catches both: a removed or renamed
        // field fails to parse, or parses into the wrong shape.
        let p = prompt(&turn("anything", None));
        let json = example_json(&p);
        let wire: crate::WireExtraction = serde_json::from_str(json)
            .expect("the prompt's own example must parse as the wire schema it teaches");

        assert_eq!(wire.mentions.len(), 2);
        assert_eq!(wire.mentions[0].name, "Alex Chen");
        assert_eq!(wire.mentions[1].name, "Globex");
        assert_eq!(wire.facts.len(), 1);
        assert_eq!(wire.facts[0].attribute, "employer");
        assert_eq!(wire.facts[0].value.as_deref(), Some("Globex"));
        assert_eq!(wire.relations.len(), 1);
        assert_eq!(wire.relations[0].predicate, "employed_by");
        assert_eq!(wire.closures.len(), 1);
        assert_eq!(
            wire.closures[0].because,
            "starting a new job ends the previous one"
        );
    }

    /// A completer that answers with the prompt's own example, which is what a
    /// model copying the shape it was shown would send back.
    struct Echo;

    impl crate::Completer for Echo {
        fn complete(&self, prompt: &str) -> Result<String, crate::CompleterError> {
            Ok(example_json(prompt).to_string())
        }
    }

    #[test]
    fn the_prompt_s_example_survives_every_refusal_extract_applies() {
        // Parsing the example as `WireExtraction` proves only that serde can
        // read it. `extract` then applies checks serde cannot express -- every
        // index in range, no relation to itself, no nameless mention -- and the
        // example shipped for a while with `"object": 1` against a
        // single-entry `mentions` array, which parsed cleanly and would have
        // been refused. A prompt teaching an extraction its own crate rejects
        // is the drift the crate owning both exists to prevent, so the guard
        // has to be the same validation, not a weaker one that happens to
        // stand next to it.
        let out = crate::extract(&turn("anything", None), &Echo)
            .unwrap_or_else(|e| panic!("the prompt's own example must survive `extract`: {e}"));

        assert_eq!(out.mentions.len(), 2);
        assert_eq!(out.mentions[1].name, "Globex");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.relations.len(), 1);
        assert_eq!(out.closures.len(), 1);
        // The index the old example got wrong: the relation's object is the
        // second mention, so the example needs a second mention to name.
        assert_eq!(out.relations[0].object, 1);
    }

    #[test]
    fn the_prompt_tells_the_model_when_to_close_a_relationship() {
        // The whole closure mechanism depends on the model volunteering one.
        let p = prompt(&turn("I started at Globex", None));
        let lower = p.to_lowercase();
        assert!(lower.contains("closure") || lower.contains("closures"));
        assert!(
            lower.contains("ended") || lower.contains("no longer"),
            "the prompt has to say what a closure is for, not just name the field"
        );
    }
}
