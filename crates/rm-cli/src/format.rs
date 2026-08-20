//! Turning an [`Outcome`] into something to read.
//!
//! Separate from the commands so they can be tested without scraping text, and
//! so changing how something reads never risks changing what it does.

use rm_engine::Believed;

use crate::command::{MentionLanding, Outcome};

pub fn render(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Initialised { path, dimension } => format!(
            "wrote {}\nembedding dimension {dimension}, taken from the model\n\nnext: rmem remember \"something you want to remember\"",
            path.display()
        ),

        Outcome::Remembered(ingested, landings) => {
            let mut out = format!(
                "remembered {} mention(s), {} assertion(s)\n",
                landings.len(),
                ingested.assertions.len()
            );
            for MentionLanding { name, entity, was_new } in landings {
                let how = if *was_new { "new" } else { "recognised" };
                out.push_str(&format!("  {name}  → entity {entity} ({how})\n"));
            }
            // Kept apart from the facts on purpose: a closure is provenanced as
            // an agent's inference precisely so nobody reads it as testimony,
            // and listing it beside what was actually said would undo that.
            if !ingested.closed.is_empty() {
                out.push_str("inferred, not stated:\n");
                for c in &ingested.closed {
                    out.push_str(&format!(
                        "  ended: entity {} {} entity {} — \"{}\"\n",
                        c.subject, c.predicate, c.object, c.because
                    ));
                }
            }
            if !ingested.reviews.is_empty() {
                out.push_str("open questions (nothing was merged):\n");
                for id in &ingested.reviews {
                    out.push_str(&format!(
                        "  review {id} — `rmem review confirm {id}` or `rmem review reject {id}`\n"
                    ));
                }
            }
            out
        }

        Outcome::Recalled(hits) if hits.is_empty() => {
            "nothing recalled — the store has nothing near that yet".to_string()
        }
        Outcome::Recalled(hits) => {
            let mut out = String::new();
            for h in hits {
                let value = h.value.as_deref().unwrap_or("(no value)");
                let stale = if h.superseded { "  [superseded]" } else { "" };
                out.push_str(&format!(
                    "entity {}  {} = {value}  ({:.3}){stale}\n",
                    h.entity, h.attribute, h.score
                ));
            }
            out
        }

        Outcome::About(Believed::Value(v)) => v.clone(),
        Outcome::About(Believed::Absent) => "no value — asserted to have none".to_string(),
        Outcome::About(Believed::Unknown) => {
            "nothing known — this was never discussed".to_string()
        }

        Outcome::Reviews(lines) if lines.is_empty() => "no open questions".to_string(),
        Outcome::Reviews(lines) => {
            let mut out = String::new();
            for l in lines {
                out.push_str(&format!(
                    "review {}  entity {} vs entity {}  ({:.2} bits)\n",
                    l.id, l.a, l.b, l.score
                ));
            }
            out
        }

        Outcome::Confirmed { survivor } => {
            format!("merged — entity {survivor} survives")
        }
        Outcome::Rejected => "kept apart, and not asked again".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_engine::{Believed, Closed, Ingested};

    /// The one rendered line mentioning `name`.
    ///
    /// Panics rather than returning an `Option` so a fixture that stops
    /// producing the line fails here, naming it, rather than further down
    /// inside an assertion about its contents.
    fn line_for<'a>(name: &str, text: &'a str) -> &'a str {
        let mut found = text.lines().filter(|l| l.contains(name));
        let line = found.next().unwrap_or_else(|| {
            panic!(
                "no line names {name}:
{text}"
            )
        });
        assert!(found.next().is_none(), "more than one line names {name}");
        line
    }

    #[test]
    fn remembering_shows_what_was_inferred_apart_from_what_was_said() {
        // The library provenances a closure as AgentInference precisely so
        // nobody mistakes it for testimony. Printing it in the same list as the
        // facts would undo that at the last step.
        let ingested = Ingested {
            entities: vec![0, 7],
            assertions: vec![0, 1],
            reviews: vec![],
            closed: vec![Closed {
                subject: 0,
                predicate: "employed_by".to_string(),
                object: 3,
                because: "starting a new job ends the previous one".to_string(),
            }],
        };
        let landings = vec![
            MentionLanding {
                name: "Ben Severn".into(),
                entity: 0,
                was_new: false,
            },
            MentionLanding {
                name: "Globex".into(),
                entity: 7,
                was_new: true,
            },
        ];
        let text = render(&Outcome::Remembered(ingested, landings));

        // Tied to the entity, not merely present anywhere in the output.
        // `assert!(text.contains("new"))` passed with the mapping inverted:
        // the fixture carries one landing of each kind so both words appear
        // whatever `was_new` maps to, and the closure's own `because` string
        // says "starting a new job ends the previous one", which satisfies a
        // bare substring check for "new" for reasons that have nothing to do
        // with labelling. What shipped under that mutation called every newly
        // created entity "recognised" and every recognised one "new" --
        // inverting the thing the spec, `MentionLanding`'s doc comment and
        // this test's own comment all call the most useful thing on the
        // screen.
        assert_eq!(
            line_for("Ben Severn", &text),
            "  Ben Severn  → entity 0 (recognised)",
            "Ben was already known"
        );
        assert_eq!(
            line_for("Globex", &text),
            "  Globex  → entity 7 (new)",
            "Globex had never been seen"
        );

        assert!(text.to_lowercase().contains("inferred"), "{text}");
        assert!(
            text.contains("starting a new job ends the previous one"),
            "{text}"
        );
    }

    #[test]
    fn a_review_raised_while_remembering_is_shown_with_its_id() {
        // A review nobody sees is the same as no review, which is the argument
        // the engine makes for returning them rather than logging them.
        let ingested = Ingested {
            entities: vec![0],
            assertions: vec![0],
            reviews: vec![4],
            closed: vec![],
        };
        let landings = vec![MentionLanding {
            name: "Ben".into(),
            entity: 0,
            was_new: true,
        }];
        let text = render(&Outcome::Remembered(ingested, landings));
        assert!(text.contains('4'), "the id has to be actionable: {text}");
        assert!(text.to_lowercase().contains("review"), "{text}");
    }

    #[test]
    fn an_unknown_belief_reads_as_nothing_known_rather_than_as_an_empty_value() {
        let text = render(&Outcome::About(Believed::Unknown));
        assert!(text.to_lowercase().contains("nothing"), "{text}");
        let absent = render(&Outcome::About(Believed::Absent));
        assert!(absent.to_lowercase().contains("no value"), "{absent}");
        assert_ne!(text, absent, "unknown and absent are different answers");
    }

    #[test]
    fn an_empty_recall_says_so_rather_than_printing_nothing() {
        let text = render(&Outcome::Recalled(vec![]));
        assert!(!text.trim().is_empty(), "silence is not an answer");
    }
}
