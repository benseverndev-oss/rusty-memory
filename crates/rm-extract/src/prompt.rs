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
    {{"kind": "person", "name": "Alex Chen", "text": "Alex"}}
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
  of days, or null if it is happening now. Do not output dates or timestamps.
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
