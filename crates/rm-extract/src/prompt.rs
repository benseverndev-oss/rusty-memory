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
        r#"Extract what this turn of dialogue says about the people, organisations, places and other things it names, as JSON.

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

- "mentions" lists the things the turn refers to that could be recognised again
  in a later turn. "subject" and "object" everywhere else are indices into it,
  starting at 0.
- "kind" must be exactly one of: person, organisation, place, event, work,
  animal, thing. Use "thing" when none of the others fit. Do not use any other
  word.
- "name" is what to call it, used to recognise it again in later turns. Use the
  fullest form the turn gives.
- A name must be able to identify something on its own. Do not emit a mention
  whose name is built out of another mention's name: "Melanie's son" and "my
  daughter" describe a relationship, not a name. If the turn also gives that
  person's name, emit them as their own mention and a relation from Melanie to
  them. If it does not, emit neither — there is nothing to recognise later.
- Do not emit a mention for an unnamed group: "the kids", "my family",
  "friends", "people at work". Say what the turn says about them as a fact
  about someone who is named.
- Do not emit a mention for an activity, a feeling or an idea: "camping",
  "pottery", "self-care", "happiness". Those are facts about a person, not
  things in their own right. "the pottery studio on Vine Street" is a place and
  may be a mention; "pottery" is not.
- "text" on a mention is the phrasing the turn used. "text" on a fact is a short
  sentence stating that fact on its own, because it is searched for separately.
- Every fact\'s "subject" must be an index into "mentions". If the turn has
  nothing worth listing as a mention, emit no facts either: a fact with nothing
  to attach to cannot be stored, and it is dropped rather than guessed at.
- "value" is a string, or null. Never a number and never true or false — write
  "2" and "true" if those are the values. Null means the attribute has no
  value: "he is between jobs" is a fact with a null value, not a missing fact.
- "days_ago" is how long before now the thing began or ended, as a whole number
  of days, or null if it is happening now. It is never negative: nothing here
  is in the future. Do not output dates or timestamps.
- "relations" is how two mentions stand to each other: employment, family,
  membership, ownership. A possessive in the turn is usually a relation --
  prefer one over inventing a mention that spells out the possession.
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

    /// The kinds the prompt allows, in the order it lists them.
    const KINDS: [&str; 7] = [
        "person",
        "organisation",
        "place",
        "event",
        "work",
        "animal",
        "thing",
    ];

    #[test]
    fn the_kind_vocabulary_is_closed_and_the_example_stays_inside_it() {
        // A real consistency check rather than a substring assertion: the
        // example is what a model copies, so an example using a kind the rules
        // forbid teaches the opposite of what the rules say. This is the same
        // failure the round-trip test below guards against, one level up.
        let p = prompt(&turn("anything", None));
        let example = crate::extract(&turn("anything", None), &Echo).expect("the example extracts");
        for mention in &example.mentions {
            assert!(
                KINDS.contains(&mention.kind.as_str()),
                "the example uses kind {:?}, which its own rules forbid",
                mention.kind
            );
        }
        for kind in KINDS {
            assert!(p.contains(kind), "the prompt never lists the kind {kind:?}");
        }
    }

    #[test]
    fn the_prompt_refuses_names_built_out_of_other_names() {
        // The failure this rule exists for, measured on real dialogue: an
        // extractor with no such rule emitted "Melanie", "Melanie's family",
        // "Melanie's kids", "Melanie's children" and "Melanie's son" as five
        // separate entities, which then generated review-band questions
        // against each other. Their true relationship is possession, so the
        // resolver was being asked the wrong question and correctly could not
        // answer it. The same turns produced 16 relations across 419 turns,
        // because the relationships were being spent on entity names.
        let p = prompt(&turn("anything", None));
        assert!(
            p.contains("built out of another mention's name"),
            "the prompt must forbid relationship-shaped names outright"
        );
        assert!(
            p.contains("relations") && p.contains("possessive"),
            "and must say where a possessive belongs instead"
        );
    }

    #[test]
    fn the_prompt_excludes_what_cannot_be_recognised_again() {
        // Unnamed groups and bare activities were the other half of the 138
        // entities one conversation produced: "the kids", "friends",
        // "camping", "pottery". Nothing can resolve them to anything later,
        // so each new turn makes another one.
        let p = prompt(&turn("anything", None));
        for forbidden in ["the kids", "camping", "pottery"] {
            assert!(
                p.contains(forbidden),
                "the prompt should name {forbidden:?} as an example of what not to emit"
            );
        }
        assert!(p.contains("recognised again"));
    }

    #[test]
    fn the_prompt_says_a_fact_needs_a_subject_that_exists() {
        // Measured: 178 facts in 419 turns named mention 0 of a list with none
        // in it, after the rules above told the model to stop emitting mentions
        // for unnamed groups. It complied and kept writing the facts that
        // referred to them. Removing the mentions without saying what becomes
        // of their facts was half a rule.
        let p = prompt(&turn("anything", None));
        assert!(
            p.contains("must be an index into \"mentions\""),
            "the prompt must constrain a fact's subject, not just describe it"
        );
        assert!(
            p.contains("emit no facts either"),
            "and must say what to do when there is nothing to attach one to"
        );
    }

    #[test]
    fn the_prompt_says_a_value_is_a_string_or_null() {
        // Measured: 49 facts answered a yes/no-flavoured attribute with the
        // JSON literal `true`, and 4 more with a number. The schema has always
        // wanted a string; the prompt never said so, and "may be null" reads as
        // the only constraint on the field.
        let p = prompt(&turn("anything", None));
        assert!(p.contains("\"value\" is a string, or null"), "{p}");
        assert!(
            p.contains("never true or false"),
            "the shape the model actually produced has to be named to be excluded"
        );
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
    fn the_prompt_s_example_round_trips_through_the_schema_that_reads_it() {
        // The substring test above catches a field added to the types and
        // never described, but a textual check cannot catch a field dropped
        // from the example while its prose sentence lingers, or one renamed to
        // a word already present elsewhere. Running the example through
        // `extract` catches both, and catches more than parsing it as the wire
        // types did: since the wire lists became opaque values parsed one at a
        // time, a field of the wrong type no longer fails the document -- it
        // drops that item instead. So a renamed field would have shown up as a
        // silently empty extraction under the old assertion, and shows up here.
        let out = crate::extract(&turn("anything", None), &Echo)
            .expect("the prompt's own example must survive the parser it teaches");

        assert!(
            out.dropped.is_empty(),
            "the prompt's example must not teach a shape this crate discards: {:?}",
            out.dropped
        );
        assert_eq!(out.mentions.len(), 2);
        assert_eq!(out.mentions[0].name, "Alex Chen");
        assert_eq!(out.mentions[1].name, "Globex");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].attribute, "employer");
        assert_eq!(out.facts[0].value.as_deref(), Some("Globex"));
        assert_eq!(out.relations.len(), 1);
        assert_eq!(out.relations[0].predicate, "employed_by");
        assert_eq!(out.closures.len(), 1);
        assert_eq!(
            out.closures[0].because,
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
