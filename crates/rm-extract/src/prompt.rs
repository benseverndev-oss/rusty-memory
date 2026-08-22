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
///
/// # What 1,165 real responses said
///
/// Reading the cached responses from a corpus run, rather than guessing from
/// the counts they produced: **45% of responses listed no mentions at all**,
/// 40% listed exactly one, and only 15% listed two or more.
///
/// That single number explains the two failures that had resisted every
/// instruction. A relation names two mention indices, so only 15% of turns
/// *could* carry one — and when the model did list two things it related them
/// 26% of the time, which is not the problem. And 258 responses carried facts
/// with no mentions at all, every sampled one of them about the speaker:
///
/// ```text
/// subject=0  text="Melanie enjoys family camping trips"
/// subject=0  text="Caroline went to a pride parade a few weeks ago"
/// ```
///
/// The model was treating the speaker as an implicit mention 0 — writing
/// `subject: 0` while listing nobody. Those were never the model ignoring an
/// instruction; they were it assuming something the schema never granted.
///
/// So the speaker line now asks for the speaker *as a mention*. It had always
/// said "resolve I, me and my to them", which tells the model who the pronouns
/// mean and not that the person is a thing to list.
///
/// # A rule that was tried and removed
///
/// The commonest thing this prompt gets back is a fact whose `subject` names a
/// mention the same response did not list — 178 of them in 419 turns. The
/// obvious rule was added: *every fact\'s subject must be an index into
/// mentions; if the turn has nothing worth listing, emit no facts either.*
///
/// It did nothing to the count (178 to 170) and cost 0.047 of overall recall,
/// with facts stored falling from 574 to 498. Removing it recovered both. The
/// reading that survives is that a model told to withhold facts when it is
/// unsure has an easy way to comply, and it complies with the good ones too.
///
/// So the shape is still there and is handled where it can be handled without
/// side effects: `extract` drops the unanchored fact and keeps the turn. Do not
/// re-add the rule without measuring it — it has been measured once and it lost.
///
/// # A second rule that was tried and removed
///
/// [`rm_core::Supersession`] needs someone to say whether a later fact under
/// one attribute replaces the earlier ones or joins them, and the model reading
/// the turn is the only party that ever knows. So this prompt asked, per fact,
/// for a `"replaces"` boolean — phrased as arity, "can someone have only one of
/// these at a time", because that is answerable from a single turn and "does
/// this contradict the store" is not.
///
/// It answered well. Over conversation 0 it classified all 134 assertions that
/// had something later in their slot, leaving none unstated: 89 additions and
/// 45 corrections, which is the two-thirds-were-never-replaced result the type
/// exists for.
///
/// And it cost more than it bought. Three runs of conversation 0, one variable:
///
/// ```text
///                        facts   entities   recall@10
///   old prompt, day 1      735        125       0.617
///   old prompt, day 2      763        131       0.617
///   with "replaces"        616        147       0.604
/// ```
///
/// Two samplings of the unchanged prompt bracket the noise at about ±4%. The
/// rule cost 19% of the facts, far outside it, and the shape of the loss is
/// legible: mentions went *up* while facts went down, so a model given one more
/// question per fact answers it by emitting fewer of them. 147 facts that no
/// longer exist is a worse trade than 134 that read `Unstated` and go on
/// standing.
///
/// The measurement points somewhere better than a reworded rule. Arity is a
/// property of the *attribute name*, not of the fact — `employer` admits one at
/// a time whatever turn it came from — so it can be asked once per distinct
/// name, away from extraction, and cached. Conversation 0 has 418 distinct
/// names against 616 facts. That is the next thing to try, and it cannot cost
/// an extraction anything, because it does not touch one.
pub fn prompt(turn: &Turn) -> String {
    let speaker = match &turn.speaker {
        Some(name) => format!(
            "The speaker is {name}. Resolve \"I\", \"me\" and \"my\" to them, and list \
             {name} in \"mentions\" whenever the turn says anything about them. A fact \
             about the speaker needs a mention to point at, exactly like a fact about \
             anyone else."
        ),
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
    fn the_prompt_asks_for_the_speaker_as_a_mention_not_just_as_a_referent() {
        // Measured over 1,165 real responses: 45% listed no mentions at all,
        // and 258 carried facts about the speaker with nobody listed to attach
        // them to -- `subject: 0` against an empty list. The old line said
        // "resolve I, me and my to them", which says who the pronouns mean and
        // not that the person is a thing to list.
        let p = prompt(&turn("I started at Globex", Some("Ben Severn")));
        assert!(
            p.contains("list Ben Severn in \"mentions\""),
            "the speaker has to be asked for as a mention: {p}"
        );
        assert!(
            p.contains("needs a mention to point at"),
            "and the reason has to be given, or it reads as an aside"
        );
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
    fn the_prompt_does_not_tell_the_model_to_withhold_facts() {
        // The rule that used to sit here told the model to emit no facts when
        // it had no mentions. It did not reduce the unanchored facts it was
        // aimed at (178 to 170) and it cost 0.047 of overall recall, because a
        // model told to withhold when unsure withholds the good ones too. It is
        // gone, and this is here so that adding it back is a deliberate act.
        let p = prompt(&turn("anything", None));
        assert!(
            !p.contains("emit no facts either"),
            "the withholding rule was measured and lost; see `prompt`'s documentation"
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
