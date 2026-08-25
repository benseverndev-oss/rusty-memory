//! Turning an [`Outcome`] into what a tool call returns.
//!
//! Two renderings of the same thing, and both are needed. `structuredContent`
//! is the machine-readable half, and is what a client validating against an
//! `outputSchema` would check. The `content` text block is what most clients
//! actually put in front of a model, and older revisions have nothing else — so
//! anything true only of the structured half is, for those clients, not said at
//! all.
//!
//! That is why the text here is not `rm_cli::format`'s text. The CLI writes for
//! someone who asked the question and is holding the context; this writes for a
//! model that will read the line back later with none of it.

use serde_json::{json, Value};

use rm_engine::{Believed, Provenance, Recalled, Source, Standing};
use rm_host::command::{Found, MentionLanding, Outcome};
use rm_host::time::format_day;

/// What one tool call returns.
pub struct Rendered {
    pub text: String,
    pub structured: Value,
}

pub fn render(outcome: &Outcome) -> Rendered {
    match outcome {
        Outcome::Remembered {
            ingested,
            landings,
            relations,
            dropped,
        } => {
            // Mentions first, then facts: `Ingested::assertions` documents one
            // assertion per mention followed by one per fact, which is the
            // only way back to the count -- an `AssertionId` says nothing
            // about what produced it.
            let facts = ingested.assertions.len().saturating_sub(landings.len());
            let mut text = format!(
                "Remembered {} mention(s), {facts} fact(s), {relations} relationship(s).\n",
                landings.len()
            );
            for MentionLanding {
                name,
                entity,
                was_new,
            } in landings
            {
                let how = if *was_new { "new" } else { "recognised" };
                text.push_str(&format!("  {name} -> entity {entity} ({how})\n"));
            }
            if !ingested.closed.is_empty() {
                // Under its own heading, exactly as the CLI does it. A closure
                // is provenanced as an agent's inference precisely so nobody
                // reads it as testimony, and listing it beside what was said
                // would undo that at the last possible step -- here, in front
                // of the model most likely to repeat it as fact.
                text.push_str("Inferred, not stated:\n");
                for c in &ingested.closed {
                    text.push_str(&format!(
                        "  ended: entity {} {} entity {} -- \"{}\"\n",
                        c.subject, c.predicate, c.object, c.because
                    ));
                }
            }
            // Under its own heading, as the closures are, and for a sharper
            // reason: the model on the other end of this is the one that wrote
            // the turn. Told what was not kept and why, it can say the same
            // thing again in a shape that is -- which is the whole point of
            // returning the library's own words rather than a code.
            if !dropped.is_empty() {
                text.push_str("Not remembered from this turn:\n");
                for d in dropped {
                    text.push_str(&format!("  {} {} -- {}\n", d.what, d.index, d.why));
                }
            }
            if !ingested.reviews.is_empty() {
                text.push_str(&format!(
                    "Open questions (nothing was merged): {}. Call reviews to see them.\n",
                    ingested
                        .reviews
                        .iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Rendered {
                text,
                structured: json!({
                    "mentions": landings.iter().map(|l| json!({
                        "name": l.name,
                        "entity": l.entity,
                        "was_new": l.was_new,
                    })).collect::<Vec<_>>(),
                    "facts": facts,
                    "relations": relations,
                    "inferred_closures": ingested.closed.iter().map(|c| json!({
                        "subject": c.subject,
                        "predicate": c.predicate,
                        "object": c.object,
                        "because": c.because,
                    })).collect::<Vec<_>>(),
                    "reviews": ingested.reviews,
                    "dropped": dropped.iter().map(|d| json!({
                        "what": d.what,
                        "index": d.index,
                        "why": d.why,
                    })).collect::<Vec<_>>(),
                }),
            }
        }

        Outcome::Recalled { hits, .. } if hits.is_empty() => Rendered {
            text: "Nothing recalled: memory has nothing near that yet.".to_string(),
            structured: json!({"hits": []}),
        },
        Outcome::Recalled { hits, weak_below } => {
            // Before the hits, because the model reads top to bottom and this
            // changes what the rest of it means. Not a filter: measured against
            // LoCoMo's adversarial questions, dropping enough of them to matter
            // costs a tenth to a third of real answers, so everything is
            // returned and the caller is told how near the nearest one was.
            let weak = *weak_below > 0.0 && hits.first().is_some_and(|h| h.score < *weak_below);
            let mut text = if weak {
                format!(
                    "NOTHING HERE IS A CLOSE MATCH. The nearest is {:.3}, under the {weak_below:.2} bar, so the store may hold nothing on this. Treat what follows as possibly about something else, and say so rather than answering from it.\n\n{} hit(s), nearest first:\n",
                    hits[0].score,
                    hits.len()
                )
            } else {
                format!("{} hit(s), nearest first:\n", hits.len())
            };
            for h in hits {
                // `None` is a tombstone, not an absence of data: an assertion
                // that says nothing is never stored, so this is a claim that
                // the attribute had no value. Printing it as blank would make
                // it indistinguishable from a value nobody bothered to fill
                // in, which is the flattening this project exists to refuse.
                let value = match &h.value {
                    Some(v) => v.clone(),
                    None => "(asserted to have no value)".to_string(),
                };
                // The model reads this line and decides whether to say the
                // fact out loud, so it has to distinguish the case where a
                // later assertion replaced this one from the case where a
                // later assertion merely exists. Collapsing them told an agent
                // that a second pet had replaced the first.
                let stale = match h.standing {
                    Standing::Latest => "",
                    Standing::Joined => "  [one of several under this attribute; still true]",
                    Standing::Corrected => "  [corrected by a later assertion]",
                    Standing::Unsettled => {
                        "  [a later assertion exists under this attribute and did not say \
                         whether it replaces this one -- both may be true]"
                    }
                };
                // The name first, because it is what the line is *about*.
                // Without it a hit reads `entity 14  because = the k-curve is
                // still 0.926 at k=200` -- the right answer with the question
                // missing, and no way to tell which decision it belongs to
                // short of another call per hit. The id stays: it is what
                // `about` takes.
                let who = match &h.name {
                    Some(n) => format!("{n} (entity {})", h.entity),
                    None => format!("entity {}", h.entity),
                };
                text.push_str(&format!(
                    "  {who}  {} = {value}  (score {:.3}, {}){stale}\n",
                    h.attribute,
                    h.score,
                    source_name(&h.provenance.source),
                ));
            }
            Rendered {
                text,
                structured: json!({
                    "hits": hits.iter().map(hit).collect::<Vec<_>>(),
                    // The field to branch on. True means the nearest hit is
                    // under the configured bar and the store may hold nothing
                    // on the subject.
                    "weak_match": weak,
                }),
            }
        }

        // Three states, and the whole thesis is that they stay three. `null`
        // cannot tell "they have no employer, and someone said so" from
        // "nobody has ever mentioned an employer", and neither can an empty
        // string, so the structured half is tagged and the text says which one
        // it is in words.
        Outcome::Decided {
            entity,
            superseded,
            supersedes_unknown,
        } => {
            let mut text = format!("Decision recorded as entity {entity}.");
            if let Some((old, title)) = superseded {
                text.push_str(&format!(
                    " It supersedes {title:?} (entity {old}), which is now marked retired."
                ));
            }
            if let Some(missing) = supersedes_unknown {
                // The model asked to retire something and nothing was retired.
                // Saying so is the whole point: it can look up the real title
                // and try again, which it will not do if this reads as success.
                text.push_str(&format!(
                    " NOTHING WAS SUPERSEDED: no decision is titled {missing:?}, so whatever                      this was meant to replace is still standing. Call decisions to see the                      exact titles."
                ));
            }
            Rendered {
                text,
                structured: json!({
                    "entity": entity,
                    "superseded": superseded.as_ref().map(|(e, t)| json!({"entity": e, "title": t})),
                    "supersedes_unknown": supersedes_unknown,
                }),
            }
        }

        Outcome::Decision(Found::Unknown) => Rendered {
            text: "No decision has that title. Titles are matched exactly -- call `decisions` to see them as recorded.".to_string(),
            structured: json!({"found": false}),
        },
        // Not the same as the above, and the difference is the whole reason
        // this variant exists: the title is real, so a model told "no such
        // decision" would go looking for a spelling mistake instead of
        // widening its clock.
        Outcome::Decision(Found::NotYetRecorded {
            title,
            first_recorded,
            first_held,
        }) => Rendered {
            text: format!(
                "{title:?} is on record, but nothing of it stood at the time you asked.
                 It was first recorded {} and holds from {}. Ask on or after both                  of those, or drop as_of and valid_at for what stands now.",
                format_day(*first_recorded),
                format_day(*first_held),
            ),
            structured: json!({
                "found": true,
                "stood_then": false,
                "first_recorded": format_day(*first_recorded),
                "first_held": format_day(*first_held),
            }),
        },
        Outcome::Decision(Found::Decision(d)) => {
            let mut text = format!("{} [{}]\n  {}\n", d.title, d.status, d.choice);
            if let Some(why) = &d.because {
                text.push_str(&format!("  because {why}\n"));
            }
            if let Some(ctx) = &d.context {
                text.push_str(&format!("  context {ctx}\n"));
            }
            // Loud, and first after the fields, because the model reading this
            // is about to act on the choice above. "It stands" and "it does
            // not, here is what does" are the two things it needs before it
            // uses any of it.
            if d.still_stands {
                text.push_str("\nTHIS IS THE DECISION THAT STANDS.\n");
            } else if d.status != "superseded" && d.superseded_by.is_empty() {
                // Not replaced, but not in force either. The status is the
                // whole reason, and a model that read only "does not stand"
                // would have no idea whether this was never adopted or on its
                // way out.
                text.push_str(&format!(
                    "\nDO NOT ACT ON THIS: its status is {:?}, not accepted.\n",
                    d.status
                ));
            } else if let Some((id, t)) = d.superseded_by.last() {
                text.push_str(&format!(
                    "\nDO NOT ACT ON THE CHOICE ABOVE -- IT WAS REPLACED.\nWhat stands now is entity {id}, {t:?}. Read that one.\n"
                ));
            } else {
                text.push_str(
                    "\nThis title was re-decided; the choice above is the latest under it.\n",
                );
            }
            if !d.supersedes.is_empty() {
                text.push_str("\nIt replaced, most recent first:\n");
                for (id, t) in &d.supersedes {
                    text.push_str(&format!("  entity {id}  {t}\n"));
                }
            }
            if d.history.len() > 1 {
                text.push_str(&format!(
                    "\nDecided {} times under this title, oldest first:\n",
                    d.history.len()
                ));
                for (at, choice) in &d.history {
                    text.push_str(&format!("  {at}  {choice}\n"));
                }
            }
            Rendered {
                text,
                structured: json!({
                    "found": true,
                    "entity": d.entity,
                    "title": d.title,
                    "status": d.status,
                    "choice": d.choice,
                    "because": d.because,
                    "context": d.context,
                    // The field to branch on. False means the choice above is
                    // out of date and `superseded_by` names what is not.
                    "still_stands": d.still_stands,
                    "supersedes": d.supersedes.iter()
                        .map(|(id, t)| json!({"entity": id, "title": t}))
                        .collect::<Vec<_>>(),
                    "superseded_by": d.superseded_by.iter()
                        .map(|(id, t)| json!({"entity": id, "title": t}))
                        .collect::<Vec<_>>(),
                    "history": d.history.iter()
                        .map(|(at, c)| json!({"recorded_at": at, "choice": c}))
                        .collect::<Vec<_>>(),
                }),
            }
        }

        Outcome::Reindexed {
            assertions,
            dimension,
        } => Rendered {
            text: format!(
                "Re-embedded {assertions} assertion(s) at {dimension} dimensions. Every vector in this store now comes from one model."
            ),
            structured: json!({"assertions": assertions, "dimension": dimension}),
        },

        Outcome::Decisions(lines) if lines.is_empty() => Rendered {
            text: "No decisions have been recorded.".to_string(),
            structured: json!({"decisions": []}),
        },
        Outcome::Decisions(lines) => {
            let mut text = format!("{} decision(s), newest first:\n", lines.len());
            for d in lines {
                text.push_str(&format!(
                    "  entity {}  {} [{}]{}\n    {}\n",
                    d.entity,
                    d.title,
                    d.status,
                    match &d.superseded_by {
                        Some((_, t)) => format!("  (replaced by {t:?})"),
                        None if d.revisions > 1 => format!("  (revised {} times)", d.revisions),
                        None => String::new(),
                    },
                    d.choice
                ));
                if let Some(why) = &d.because {
                    text.push_str(&format!("    because {why}\n"));
                }
            }
            Rendered {
                text,
                structured: json!({
                    "decisions": lines.iter().map(|d| json!({
                        "entity": d.entity,
                        "title": d.title,
                        "status": d.status,
                        "choice": d.choice,
                        "because": d.because,
                        // The one a caller should branch on: a decision is
                        // current when nothing later replaced its choice,
                        // whatever the status field happens to say.
                        "still_stands": d.still_stands,
                    })).collect::<Vec<_>>()
                }),
            }
        }

        Outcome::About(Believed::Value(v)) => Rendered {
            text: v.clone(),
            structured: json!({"believed": "value", "value": v}),
        },
        Outcome::About(Believed::Absent) => Rendered {
            text: "Absent: this was asserted to have no value. That is an answer, not a gap."
                .to_string(),
            structured: json!({"believed": "absent"}),
        },
        Outcome::About(Believed::Unknown) => Rendered {
            text: "Unknown: nothing has ever been said about this. It is not that there is no value -- it has never come up.".to_string(),
            structured: json!({"believed": "unknown"}),
        },

        Outcome::Reviews(lines) if lines.is_empty() => Rendered {
            text: "No open questions.".to_string(),
            structured: json!({"reviews": []}),
        },
        Outcome::Reviews(lines) => {
            let mut text = format!("{} open question(s), nothing merged:\n", lines.len());
            for l in lines {
                // A model deciding this needs the same thing a person does:
                // what the two are called and what they are. Two ids and a
                // score are not enough to answer on, and a caller that cannot
                // answer will either guess or ignore the queue.
                let side = |name: &Option<String>, id, kind: &str| match name {
                    Some(n) => format!("{n:?} [{kind}] (entity {id})"),
                    None => format!("entity {id} [{kind}]"),
                };
                text.push_str(&format!(
                    "  review {}: {} against {} ({:.2} bits of evidence)\n",
                    l.id,
                    side(&l.a_name, l.a, &l.a_kind),
                    side(&l.b_name, l.b, &l.b_kind),
                    l.score
                ));
            }
            Rendered {
                text,
                structured: json!({"reviews": lines.iter().map(|l| json!({
                    "id": l.id,
                    "a": l.a,
                    "a_name": l.a_name,
                    "a_kind": l.a_kind,
                    "b": l.b,
                    "b_name": l.b_name,
                    "b_kind": l.b_kind,
                    "score": l.score,
                })).collect::<Vec<_>>()}),
            }
        }

        Outcome::Confirmed { survivor } => Rendered {
            text: format!("Merged. Entity {survivor} survives, and carries both histories."),
            structured: json!({"merged": true, "survivor": survivor}),
        },
        Outcome::Rejected => Rendered {
            text: "Kept apart, and this pair will not be asked about again.".to_string(),
            structured: json!({"merged": false}),
        },

        // Not reachable from this crate's tool table -- `init` writes a config
        // file, which is `rmem`'s job and not a thing to do down a socket. It
        // is rendered rather than panicked on because a server that aborts on
        // an unexpected value takes the whole conversation with it, and this
        // one is trivially renderable.
        Outcome::Initialised {
            path,
            dimension,
            replaced_unparsable,
        } => Rendered {
            // The notice leads, for the same reason it leads in `rmem`'s own
            // rendering: a file the user wrote is gone, and that is the part
            // they need first. It is `command::init`'s words verbatim, which
            // name a location in the old file and never a value out of it.
            text: match replaced_unparsable {
                Some(why) => format!(
                    "The existing configuration could not be parsed, and was replaced because --force was passed: {why}

Wrote a configuration at {} with embedding dimension {dimension}.",
                    path.display()
                ),
                None => format!(
                    "Wrote a configuration at {} with embedding dimension {dimension}.",
                    path.display()
                ),
            },
            // Present as `null` rather than omitted when nothing was replaced,
            // so a client reading `structuredContent` can tell "nothing was
            // overwritten" from "this server is too old to say".
            structured: json!({
                "dimension": dimension,
                "replaced_unparsable": replaced_unparsable,
            }),
        },
    }
}

fn hit(h: &Recalled) -> Value {
    let Provenance {
        source,
        observed_at,
        source_ref,
    } = &h.provenance;
    json!({
        "entity": h.entity,
        // What the entity is called. Null when it has none -- an entity exists
        // as soon as something is asserted about it, and nothing requires the
        // mention that created it to have carried a name.
        "name": h.name,
        "attribute": h.attribute,
        // Null here is unambiguous because it sits beside `asserted_absent`:
        // the pair says "this assertion claimed there is no value", which a
        // bare null could not.
        "value": h.value,
        "asserted_absent": h.value.is_none(),
        "score": h.score,
        // "latest" | "joined" | "corrected" | "unsettled". A string rather
        // than the boolean this used to be, because the boolean could only say
        // "something later exists" and was read as "this was replaced".
        "standing": standing_name(h.standing),
        // Kept as the one thing a caller usually wants to branch on: whether
        // this may still be stated as current. True for everything but a
        // correction -- an unanswered question is not a correction.
        "still_stands": h.standing.still_stands(),
        "valid_from": h.valid.from,
        "valid_to": h.valid.to,
        "source": source_name(source),
        "observed_at": observed_at,
        "session": source_ref,
    })
}

/// What to call a [`Source`] on the wire.
///
/// Written out rather than derived through `serde`, because the name is part of
/// this server's contract with a model and `Source` is a library type that is
/// free to gain variants. `External` carries the host's own label, which is the
/// one case where the value is the informative part.
/// The wire name for a [`Standing`].
///
/// Spelled out rather than derived from `Debug`, so a rename in `rm_engine`
/// cannot silently change a field every client is parsing.
fn standing_name(standing: Standing) -> &'static str {
    match standing {
        Standing::Latest => "latest",
        Standing::Joined => "joined",
        Standing::Corrected => "corrected",
        Standing::Unsettled => "unsettled",
    }
}

fn source_name(source: &Source) -> String {
    match source {
        Source::UserAssertion => "user_assertion".to_string(),
        Source::ToolOutput => "tool_output".to_string(),
        Source::AgentInference => "agent_inference".to_string(),
        Source::External(who) => format!("external:{who}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_engine::{Closed, Ingested, Interval, Provenance, Source};
    use rm_host::command::ReviewLine;

    fn landing(name: &str, entity: rm_engine::StableId, was_new: bool) -> MentionLanding {
        MentionLanding {
            name: name.to_string(),
            entity,
            was_new,
        }
    }

    #[test]
    fn the_three_states_of_believed_stay_three_on_the_wire() {
        // The thesis at its last possible step. `Absent` -- someone said there
        // is none -- and `Unknown` -- nobody has ever mentioned it -- are
        // different answers, and every system that flattens them to null or to
        // "" has thrown away the distinction this project exists to keep.
        let value = render(&Outcome::About(Believed::Value("Globex".into())));
        let absent = render(&Outcome::About(Believed::Absent));
        let unknown = render(&Outcome::About(Believed::Unknown));

        assert_eq!(
            value.structured,
            json!({"believed": "value", "value": "Globex"})
        );
        assert_eq!(absent.structured, json!({"believed": "absent"}));
        assert_eq!(unknown.structured, json!({"believed": "unknown"}));
        assert_ne!(absent.structured, unknown.structured);
    }

    #[test]
    fn the_text_block_alone_distinguishes_absent_from_unknown() {
        // The structured half is not enough. Most clients put the text block
        // in front of the model and nothing else, and revisions before
        // 2025-06-18 have no structured half at all -- so a distinction made
        // only there is, for those readers, not made.
        let absent = render(&Outcome::About(Believed::Absent)).text;
        let unknown = render(&Outcome::About(Believed::Unknown)).text;
        assert_ne!(absent, unknown);
        assert!(absent.to_lowercase().contains("asserted"), "{absent}");
        assert!(unknown.to_lowercase().contains("never"), "{unknown}");
        // And neither may be empty or a bare value, which is what a model
        // reading back a transcript would take for "no answer".
        assert!(absent.len() > 20 && unknown.len() > 20);
    }

    /// A query the store has nothing near is said to be a weak match, and the
    /// hits still come back.
    ///
    /// Found by seeding a real decision log: asking it about database
    /// migrations, a subject the project has never discussed, returned three
    /// unrelated decisions at 0.27-0.31 with nothing to say they were
    /// unrelated. Naming the entities made that worse rather than better --
    /// anonymous ids read like pointers, named decisions read like an answer.
    ///
    /// It labels rather than filters, and that is a measured choice. Against
    /// LoCoMo's adversarial questions over 382 answerable and 112 unanswerable,
    /// a cutoff that refuses a third of the unanswerable also throws away a
    /// tenth of the real answers; refusing 87% costs 37% of them.
    #[test]
    fn a_query_with_nothing_near_it_is_marked_weak_and_still_answered() {
        let far = Recalled {
            entity: 0,
            name: Some("Store bi-temporally".into()),
            assertion: 0,
            attribute: "context".into(),
            value: Some("choosing the storage model".into()),
            valid: Interval::since(100),
            provenance: Provenance::new(Source::UserAssertion, 100, "s"),
            score: 0.308,
            standing: Standing::Latest,
        };
        let out = render(&Outcome::Recalled {
            hits: vec![far.clone()],
            weak_below: 0.62,
        });
        assert!(
            out.text.contains("NOTHING HERE IS A CLOSE MATCH"),
            "{}",
            out.text
        );
        assert_eq!(out.structured["weak_match"], json!(true));
        assert_eq!(
            out.structured["hits"].as_array().map(Vec::len),
            Some(1),
            "the hit is still returned -- this labels, it does not filter"
        );

        // A near hit says nothing extra, and the same hit says nothing when the
        // bar is off.
        let near = Recalled {
            score: 0.75,
            ..far.clone()
        };
        let out = render(&Outcome::Recalled {
            hits: vec![near],
            weak_below: 0.62,
        });
        assert!(!out.text.contains("NOTHING HERE"), "{}", out.text);
        assert_eq!(out.structured["weak_match"], json!(false));

        let out = render(&Outcome::Recalled {
            hits: vec![far],
            weak_below: 0.0,
        });
        assert!(
            !out.text.contains("NOTHING HERE"),
            "zero turns the notice off: {}",
            out.text
        );
    }

    /// A hit says what it is about, not only what it says.
    ///
    /// Found by seeding a real decision log and reading it: every hit came back
    /// as `entity 14  because = the k-curve is still 0.926 at k=200` -- the
    /// right answer with the question missing, and no way to tell which
    /// decision it belonged to without another call per hit.
    #[test]
    fn a_recalled_hit_names_the_entity_it_is_about() {
        let named = Recalled {
            entity: 14,
            name: Some("Rerank the recall results".into()),
            assertion: 0,
            attribute: "because".into(),
            value: Some("the k-curve is still 0.926 at k=200".into()),
            valid: Interval::since(100),
            provenance: Provenance::new(Source::UserAssertion, 100, "s"),
            score: 0.53,
            standing: Standing::Latest,
        };
        let out = render(&recalled(vec![named.clone()]));
        assert!(
            out.text.contains("Rerank the recall results"),
            "the name has to be in the text a model reads: {}",
            out.text
        );
        assert!(
            out.text.contains("entity 14"),
            "and the id stays -- it is what `about` takes: {}",
            out.text
        );
        assert_eq!(
            out.structured["hits"][0]["name"],
            json!("Rerank the recall results")
        );

        // An entity with no name still renders, and says null rather than
        // inventing one. Nothing requires a mention to have carried a name.
        let anonymous = Recalled {
            name: None,
            ..named
        };
        let out = render(&recalled(vec![anonymous]));
        assert!(out.text.contains("entity 14"), "{}", out.text);
        assert_eq!(out.structured["hits"][0]["name"], Value::Null);
    }

    #[test]
    fn a_recalled_tombstone_is_not_a_blank() {
        // `Recalled::value` of `None` is a claim that the attribute had no
        // value, not a missing field: an assertion that says nothing is never
        // stored and cannot be recalled. A blank in the text would be read as
        // a value nobody filled in.
        let hit = Recalled {
            entity: 3,
            name: Some("Ben".into()),
            assertion: 0,
            attribute: "employer".into(),
            value: None,
            valid: Interval::since(100),
            provenance: Provenance::new(Source::UserAssertion, 100, "session-a"),
            score: 0.5,
            standing: Standing::Latest,
        };
        let out = render(&recalled(vec![hit]));
        assert!(out.text.contains("no value"), "{}", out.text);
        assert_eq!(out.structured["hits"][0]["value"], Value::Null);
        assert_eq!(out.structured["hits"][0]["asserted_absent"], json!(true));
    }

    /// Hits with the notice off, so a test about anything else is unaffected
    /// by where the weak-match bar happens to sit.
    fn recalled(hits: Vec<Recalled>) -> Outcome {
        Outcome::Recalled {
            hits,
            weak_below: 0.0,
        }
    }

    fn stood(standing: Standing) -> Recalled {
        Recalled {
            entity: 1,
            name: Some("Ben".into()),
            assertion: 0,
            attribute: "employer".into(),
            value: Some("Acme".into()),
            valid: Interval::between(100, 200),
            provenance: Provenance::new(Source::AgentInference, 150, "s"),
            score: 0.9,
            standing,
        }
    }

    #[test]
    fn a_corrected_hit_says_so_in_both_halves() {
        // It is returned rather than hidden -- what was believed is part of
        // the record -- which only works if the reader is told it is stale.
        let out = render(&recalled(vec![stood(Standing::Corrected)]));
        assert!(out.text.contains("corrected"), "{}", out.text);
        assert_eq!(out.structured["hits"][0]["standing"], json!("corrected"));
        assert_eq!(out.structured["hits"][0]["still_stands"], json!(false));
        assert_eq!(out.structured["hits"][0]["valid_to"], json!(200));
        assert_eq!(
            out.structured["hits"][0]["source"],
            json!("agent_inference")
        );
    }

    #[test]
    fn a_hit_with_something_later_beside_it_is_not_rendered_as_replaced() {
        // The distinction the boolean could not draw. Both of these have a
        // later assertion under the same attribute; neither has been replaced,
        // and a model told otherwise drops a fact that is still true.
        for standing in [Standing::Joined, Standing::Unsettled] {
            let out = render(&recalled(vec![stood(standing)]));
            assert!(
                !out.text.contains("corrected"),
                "{standing:?} must not read as a correction: {}",
                out.text
            );
            assert_eq!(
                out.structured["hits"][0]["still_stands"],
                json!(true),
                "{standing:?}"
            );
        }
        assert_eq!(
            render(&recalled(vec![stood(Standing::Joined)])).structured["hits"][0]["standing"],
            json!("joined")
        );
        assert_eq!(
            render(&recalled(vec![stood(Standing::Unsettled)])).structured["hits"][0]["standing"],
            json!("unsettled")
        );
    }

    #[test]
    fn an_empty_recall_is_an_answer_and_not_a_shrug() {
        let out = render(&recalled(vec![]));
        assert_eq!(out.structured, json!({"hits": []}));
        assert!(!out.text.is_empty());
    }

    #[test]
    fn an_inferred_closure_is_kept_out_of_the_facts() {
        // The same separation `rm-cli` makes, and for a sharper reason here:
        // the reader is a model, and a closure listed among the facts is one
        // an agent will repeat as something the user said.
        let ingested = Ingested {
            entities: vec![0, 1],
            assertions: vec![0, 1, 2],
            reviews: vec![],
            closed: vec![Closed {
                subject: 0,
                predicate: "employed_by".into(),
                object: 1,
                because: "starting a new job ends the previous one".into(),
            }],
        };
        let out = render(&Outcome::Remembered {
            ingested,
            landings: vec![landing("Ben Severn", 0, false), landing("Globex", 1, true)],
            relations: 1,
            dropped: Vec::new(),
        });

        let heading = out
            .text
            .find("Inferred, not stated:")
            .expect("the inference has to be marked as one");
        let ended = out.text.find("ended:").expect("the closure has to show");
        assert!(heading < ended, "the heading comes first:\n{}", out.text);
        assert!(out.text.contains("starting a new job"), "{}", out.text);
        // And it is its own field, not an entry in the facts count.
        assert_eq!(out.structured["facts"], json!(1));
        assert_eq!(
            out.structured["inferred_closures"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn remembering_says_which_mentions_were_recognised_and_which_were_new() {
        // The most useful thing an agent can be told here: whether memory
        // learned about someone or recognised them.
        let out = render(&Outcome::Remembered {
            ingested: Ingested {
                entities: vec![0, 1],
                assertions: vec![0, 1],
                reviews: vec![7],
                closed: vec![],
            },
            landings: vec![landing("Ben Severn", 0, false), landing("Globex", 1, true)],
            relations: 0,
            dropped: Vec::new(),
        });
        assert!(
            out.text.contains("Ben Severn -> entity 0 (recognised)"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("Globex -> entity 1 (new)"),
            "{}",
            out.text
        );
        assert_eq!(out.structured["mentions"][0]["was_new"], json!(false));
        // And an open question is surfaced with the tool that answers it,
        // since nothing was merged and something has to say so.
        assert!(out.text.contains("nothing was merged"), "{}", out.text);
        assert!(out.text.contains("reviews"), "{}", out.text);
        assert_eq!(out.structured["reviews"], json!([7]));
    }

    #[test]
    fn a_review_line_carries_its_evidence() {
        let out = render(&Outcome::Reviews(vec![ReviewLine {
            id: 4,
            a: 1,
            b: 2,
            a_name: Some("Mel".to_string()),
            b_name: Some("Melanie".to_string()),
            a_kind: "person".to_string(),
            b_kind: "person".to_string(),
            score: 4.98,
        }]));
        assert!(out.text.contains("review 4"), "{}", out.text);
        assert!(out.text.contains("4.98"), "{}", out.text);
        assert_eq!(out.structured["reviews"][0]["id"], json!(4));
        // The evidence is the pair, not the ids: a caller must be able to see
        // that this asks whether "Mel" and "Melanie" are one person.
        assert!(out.text.contains("Mel"), "{}", out.text);
        assert!(out.text.contains("Melanie"), "{}", out.text);
        assert!(out.text.contains("person"), "{}", out.text);
        assert_eq!(out.structured["reviews"][0]["a_name"], json!("Mel"));
        assert_eq!(out.structured["reviews"][0]["b_kind"], json!("person"));
    }

    #[test]
    fn a_merge_names_the_survivor_because_the_other_id_is_now_gone() {
        let out = render(&Outcome::Confirmed { survivor: 2 });
        assert!(out.text.contains("Entity 2"), "{}", out.text);
        assert_eq!(out.structured, json!({"merged": true, "survivor": 2}));
        let out = render(&Outcome::Rejected);
        assert_eq!(out.structured, json!({"merged": false}));
    }

    #[test]
    fn a_host_named_source_keeps_its_label() {
        // `External` is the one variant where the value is the informative
        // part: "crm" and "calendar" are different provenance, and collapsing
        // both to "external" would lose the only thing they carry.
        assert_eq!(source_name(&Source::External("crm".into())), "external:crm");
    }

    #[test]
    fn what_was_not_remembered_reaches_both_halves_of_the_result() {
        // Both, deliberately. This module's own rule is that anything true only
        // of the structured half is, for a client that cannot read it, not
        // said -- and the client here is a model that wrote the turn and could
        // say it again in a shape that survives.
        let out = render(&Outcome::Remembered {
            ingested: Ingested {
                entities: vec![0],
                assertions: vec![0, 1],
                reviews: vec![],
                closed: vec![],
            },
            landings: vec![landing("Ben Severn", 0, true)],
            relations: 0,
            dropped: vec![rm_host::command::Dropped {
                what: "fact",
                index: 1,
                why: "it names mention 9, but the response listed 1".to_string(),
            }],
        });
        assert!(
            out.text.contains("Not remembered from this turn:"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("fact 1 -- it names mention 9"),
            "{}",
            out.text
        );
        assert_eq!(out.structured["dropped"][0]["what"], json!("fact"));
        assert_eq!(out.structured["dropped"][0]["index"], json!(1));

        // What was kept is still reported: both are true at once.
        assert!(out.text.contains("Ben Severn"), "{}", out.text);
    }

    #[test]
    fn a_clean_turn_reports_nothing_dropped() {
        let out = render(&Outcome::Remembered {
            ingested: Ingested {
                entities: vec![0],
                assertions: vec![0],
                reviews: vec![],
                closed: vec![],
            },
            landings: vec![landing("Ben Severn", 0, true)],
            relations: 0,
            dropped: Vec::new(),
        });
        assert!(!out.text.contains("Not remembered"), "{}", out.text);
        assert_eq!(out.structured["dropped"], json!([]));
    }
}
