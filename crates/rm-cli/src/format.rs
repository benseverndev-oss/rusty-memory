//! Turning an [`Outcome`] into something to read.
//!
//! Separate from the commands so they can be tested without scraping text, and
//! so changing how something reads never risks changing what it does.

use rm_engine::{Believed, Standing};

use rm_host::command::{Found, MentionLanding, Outcome};

pub fn render(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Initialised {
            path,
            dimension,
            replaced_unparsable,
        } => {
            let notice = match replaced_unparsable {
                Some(why) => format!(
                    "the existing config could not be parsed, and was replaced because --force was passed: {why}\n\n"
                ),
                None => String::new(),
            };
            format!(
                "{notice}wrote {}\nembedding dimension {dimension}, taken from the model\n\nnext: rmem remember \"something you want to remember\"",
                path.display()
            )
        }

        Outcome::Remembered {
            ingested,
            landings,
            relations,
            dropped,
        } => {
            // The counts the spec's worked example shows, and the three it
            // shows: mentions, facts, relationships. `assertions` is not one
            // of them -- it is mentions plus facts, since `Ingested` documents
            // one `kind` assertion per mention followed by one per fact -- so
            // printing it named a number nothing on screen explained.
            let facts = ingested.assertions.len().saturating_sub(landings.len());
            let mut out = format!(
                "remembered {}, {}, {}\n",
                plural(landings.len(), "mention"),
                plural(facts, "fact"),
                plural(*relations, "relationship"),
            );
            for MentionLanding {
                name,
                entity,
                was_new,
            } in landings
            {
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
                        "  ended: {} {} {} — \"{}\"\n",
                        named(c.subject, landings),
                        c.predicate,
                        named(c.object, landings),
                        c.because
                    ));
                }
            }
            // Its own heading, for the same reason closures get one: this is
            // the turn saying less than the model described, and a reader who
            // cannot see it has no way to tell that from a turn that said
            // less. `extract` salvages instead of refusing precisely because
            // this line exists.
            if !dropped.is_empty() {
                out.push_str("not remembered from this turn:\n");
                for d in dropped {
                    out.push_str(&format!("  {} {} — {}\n", d.what, d.index, d.why));
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

        Outcome::Recalled { hits, .. } if hits.is_empty() => {
            "nothing recalled — the store has nothing near that yet".to_string()
        }
        Outcome::Recalled { hits, weak_below } => {
            let mut out = String::new();
            // Said once, at the top, before any of it is read. Not per hit and
            // not a filter: measured against LoCoMo's adversarial questions,
            // dropping enough of them to matter costs between a tenth and a
            // third of real answers, so everything is returned and the caller
            // is told how near the nearest thing actually was.
            if *weak_below > 0.0 && hits.first().is_some_and(|h| h.score < *weak_below) {
                out.push_str(&format!(
                    "nothing here is a close match -- the nearest is {:.3}, under the {weak_below:.2} bar.\nWhat follows may be about something else.\n\n",
                    hits[0].score
                ));
            }
            for h in hits {
                let value = h.value.as_deref().unwrap_or("(no value)");
                // Four states, not two, because "something later exists" and
                // "something later replaced this" are different things and the
                // second was being printed for both. `Joined` is worth its own
                // line: a reader who sees one pet wants to know there are two.
                let stale = match h.standing {
                    Standing::Latest => "",
                    Standing::Joined => "  [one of several]",
                    Standing::Corrected => "  [corrected by a later assertion]",
                    Standing::Unsettled => {
                        "  [a later assertion exists; neither said which replaces which]"
                    }
                };
                // The name leads, because it is what the line is about. An id
                // alone made every hit a pointer to chase: `entity 14
                // because = the k-curve is still 0.926 at k=200` is the answer
                // with the question missing. The id stays beside it -- it is
                // what `rmem about` takes.
                let who = match &h.name {
                    Some(n) => format!("{n} (entity {})", h.entity),
                    None => format!("entity {}", h.entity),
                };
                out.push_str(&format!(
                    "{who}  {} = {value}  ({:.3}){stale}\n",
                    h.attribute, h.score
                ));
            }
            out
        }

        Outcome::Decided {
            entity,
            superseded,
            supersedes_unknown,
        } => {
            let mut out = format!("decision recorded as entity {entity}\n");
            if let Some((old, title)) = superseded {
                out.push_str(&format!(
                    "  supersedes {title:?} (entity {old}), now retired\n"
                ));
            }
            if let Some(missing) = supersedes_unknown {
                // Loud, because the decision they meant to retire is still
                // standing and a plain success would never say so.
                out.push_str(&format!(
                    "  NOTHING SUPERSEDED: no decision is titled {missing:?}, so whatever it \n                     was meant to replace is still standing. Check `rmem decisions` for the \n                     exact title.\n"
                ));
            }
            out
        }

        Outcome::Decisions(lines) if lines.is_empty() => {
            "no decisions recorded yet — `rmem decide \"<title>\" \"<choice>\"`".to_string()
        }
        Outcome::Decisions(lines) => {
            let mut out = String::new();
            for d in lines {
                // The mark is about the choice, not the status field: a
                // decision re-decided under the same title is retired whether
                // or not anybody wrote a status.
                let mark = if d.still_stands { " " } else { "~" };
                out.push_str(&format!(
                    "{mark} entity {:<4} {} [{}]\n    {}\n",
                    d.entity, d.title, d.status, d.choice
                ));
                if let Some(why) = &d.because {
                    out.push_str(&format!("    because {why}\n"));
                }
                if let Some((id, title)) = &d.superseded_by {
                    out.push_str(&format!("    replaced by entity {id}, {title:?}\n"));
                }
                // Not a staleness mark: the choice above is the latest one and
                // it stands. This says the title changed its mind on the way
                // here, which `rmem decision` will show in full.
                if d.revisions > 1 {
                    out.push_str(&format!("    revised {} times\n", d.revisions));
                }
            }
            // The mark used to mean "replaced", which was the only way to
            // stop standing when `accepted` and `superseded` were the whole
            // vocabulary. With `proposed`, `rejected` and `deprecated` it has
            // to mean the more useful thing -- do not act on this -- and the
            // status beside each one says which reason applies.
            out.push_str("\n~ marks a decision that is not in force: see its status.\n");
            out
        }

        // Replaced in Task 4 with a real message; here only so the crate
        // compiles while `decision` grows its third answer.
        Outcome::Decision(Found::NotYetRecorded { .. }) => String::new(),
        Outcome::Decision(Found::Unknown) => {
            "no decision by that title — `rmem decisions` lists them, and the title has to match exactly".to_string()
        }
        Outcome::Decision(Found::Decision(d)) => {
            let mut out = format!("{}  [{}]\n", d.title, d.status);
            out.push_str(&format!("  entity {}\n\n", d.entity));
            out.push_str(&format!("  choice   {}\n", d.choice));
            if let Some(why) = &d.because {
                out.push_str(&format!("  because  {why}\n"));
            }
            if let Some(ctx) = &d.context {
                out.push_str(&format!("  context  {ctx}\n"));
            }

            // The successor first, and named. A reader who arrives here from a
            // search is holding an answer that may be retired, and the next
            // line either confirms it stands or carries them to the one that
            // does.
            if d.still_stands {
                out.push_str("\nthis is what stands.\n");
            } else if d.superseded_by.is_empty() {
                out.push_str(
                    "\nreplaced, but nothing records by what — it was re-decided under this same\ntitle, so the history below is the whole story.\n",
                );
            } else {
                out.push_str("\nreplaced by:\n");
                for (i, (id, title)) in d.superseded_by.iter().enumerate() {
                    let arrow = if i + 1 == d.superseded_by.len() {
                        "  → "
                    } else {
                        "    "
                    };
                    out.push_str(&format!("{arrow}entity {id:<4} {title}\n"));
                }
                out.push_str("  → the last of these is what stands now.\n");
            }

            if !d.supersedes.is_empty() {
                out.push_str("\nit replaced:\n");
                for (id, title) in &d.supersedes {
                    out.push_str(&format!("    entity {id:<4} {title}\n"));
                }
            }

            // More than one entry means this title was re-decided, which is the
            // other way a decision stops standing and the one no status field
            // records.
            if d.history.len() > 1 {
                out.push_str(&format!(
                    "\ndecided {} times under this title, oldest first:\n",
                    d.history.len()
                ));
                for (at, choice) in &d.history {
                    out.push_str(&format!("    {}  {choice}\n", rm_host::time::format_day(*at)));
                }
            }
            out
        }

        Outcome::Reindexed {
            assertions,
            dimension,
        } => format!(
            "re-embedded {} under the current provider, at {dimension} dimensions.
Every vector in this store now comes from one model.",
            plural(*assertions, "assertion")
        ),

        Outcome::About(Believed::Value(v)) => v.clone(),
        Outcome::About(Believed::Absent) => "no value — asserted to have none".to_string(),
        Outcome::About(Believed::Unknown) => "nothing known — this was never discussed".to_string(),

        Outcome::Reviews(lines) if lines.is_empty() => "no open questions".to_string(),
        Outcome::Reviews(lines) => {
            let mut out = String::new();
            for l in lines {
                // Name and kind on the line itself. The pair is a question, and
                // a question whose subjects are two integers cannot be answered
                // without two more commands per pair.
                let side = |name: &Option<String>, id, kind: &str| match name {
                    Some(n) => format!("{n:?} [{kind}] (entity {id})"),
                    None => format!("entity {id} [{kind}]"),
                };
                out.push_str(&format!(
                    "review {}  {}  vs  {}  ({:.2} bits)\n",
                    l.id,
                    side(&l.a_name, l.a, &l.a_kind),
                    side(&l.b_name, l.b, &l.b_kind),
                    l.score
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

/// `1 fact`, `2 facts`. Every word this renders pluralises with a bare `s`.
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// What to call `entity` on screen.
///
/// The name if this turn mentioned it, and the id otherwise. A closed edge's
/// subject is always a mention of the turn that closed it -- `Closure.subject`
/// is an index into the extraction's own mentions -- so it always has a name.
/// Its *object* usually does not: the whole point of "I started at Globex" is
/// that it ends an edge to a previous employer the turn never named, and this
/// crate has no way to ask the store for an entity's name. Printing the id
/// there is honest; printing it for the subject too, which the code has in
/// hand, threw away the tie to the mention lines two rows above.
fn named(entity: rm_engine::StableId, landings: &[MentionLanding]) -> String {
    landings
        .iter()
        .find(|l| l.entity == entity)
        .map_or_else(|| format!("entity {entity}"), |l| l.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_engine::{Believed, Closed, Ingested};

    // Moved here from `rm_host::command` when the host concerns left this
    // crate. It was always a test of the *join* -- extraction, ingest, closure
    // and rendering -- and rendering is the half that stayed, so the seam it
    // guards now runs between two crates rather than two modules. That makes
    // it more worth keeping, not less: nothing on the `rm-host` side can see
    // this text at all.
    use rm_engine::{Engine, Metric, VectorIndex};
    use rm_host::command::Dropped;
    use rm_host::command::Outcome as HostOutcome;
    use rm_host::testing::StubProvider;

    fn engine() -> Engine {
        let config: rm_host::config::Config = toml::from_str(rm_host::config::TEMPLATE).unwrap();
        Engine::new(
            VectorIndex::new(3, Metric::Cosine),
            config.ruleset().unwrap(),
            config.policy_for_engine().unwrap(),
        )
    }

    #[test]
    fn a_new_job_closes_the_old_one_all_the_way_through_to_what_is_printed() {
        // The one test that drives extraction -> ingest -> closure -> render.
        // Every other `remember` fixture in this file carries `"closures":[]`,
        // and closure rendering was covered only by a hand-constructed
        // `Ingested` in `format`. So both ends were tested and the join was
        // not -- and the join is where the inference-versus-testimony
        // distinction the library works hardest to preserve either survives
        // to the screen or does not.
        let mut e = engine();

        let first = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"Ben"},
                            {"kind":"organisation","name":"Acme","text":"Acme"}],
                "facts":[],
                "relations":[{"subject":0,"predicate":"employed_by","object":1,"days_ago":null}],
                "closures":[]}"#,
        ]);
        rm_host::command::remember(&mut e, "I work at Acme", 100, "cli", None, &first, &first)
            .unwrap();

        // "I started at Globex last month": a new employment, and the model
        // volunteering that the previous one ended. `spared` keeps the
        // closure from closing Globex as fast as it opened it.
        let second = StubProvider::new(vec![
            r#"{"mentions":[{"kind":"person","name":"Ben Severn","text":"Ben"},
                            {"kind":"organisation","name":"Globex","text":"Globex"}],
                "facts":[{"subject":0,"attribute":"employer","value":"Globex",
                          "text":"Ben works at Globex","days_ago":null}],
                "relations":[{"subject":0,"predicate":"employed_by","object":1,"days_ago":null}],
                "closures":[{"subject":0,"predicate":"employed_by",
                             "because":"starting a new job ends the previous one",
                             "days_ago":null}]}"#,
        ]);
        let out = rm_host::command::remember(
            &mut e,
            "I started at Globex",
            200,
            "cli",
            None,
            &second,
            &second,
        )
        .unwrap();

        let HostOutcome::Remembered { ref ingested, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(
            ingested.closed.len(),
            1,
            "the Acme edge is what the closure ends, and Globex is spared"
        );
        assert_eq!(ingested.closed[0].predicate, "employed_by");
        assert_eq!(
            ingested.closed[0].because,
            "starting a new job ends the previous one"
        );

        let text = render(&out);
        assert_eq!(
            text.lines().next().unwrap(),
            "remembered 2 mentions, 1 fact, 1 relationship"
        );
        // Under its own heading, not among the facts. A closure is
        // provenanced `AgentInference` precisely so nobody reads it as
        // testimony; printing it in the same list would undo that at the last
        // possible step.
        let heading = text
            .lines()
            .position(|l| l.contains("inferred, not stated:"))
            .expect("the inference has to be marked as one");
        let ended = text
            .lines()
            .position(|l| l.contains("ended:"))
            .expect("the closed edge has to be shown");
        assert!(heading < ended, "the heading has to come first:\n{text}");
        assert!(
            text.contains("ended: Ben Severn employed_by entity 1"),
            "the subject is a mention of this turn, so it is named:\n{text}"
        );
        assert!(
            text.contains("\"starting a new job ends the previous one\""),
            "the model's reason is the whole point of showing it:\n{text}"
        );
    }

    /// The one mention-landing line for `name`.
    ///
    /// Narrowed to the landing block — the lines carrying an arrow — because
    /// the closure below now names its subject too, so "the line mentioning
    /// Ben Severn" is no longer one line. Panics rather than returning an
    /// `Option` so a fixture that stops producing the line fails here, naming
    /// it, rather than further down inside an assertion about its contents.
    fn landing_line_for<'a>(name: &str, text: &'a str) -> &'a str {
        let mut found = text
            .lines()
            .filter(|l| l.contains("→ entity") && l.contains(name));
        let line = found
            .next()
            .unwrap_or_else(|| panic!("no landing line names {name} in:\n{text}"));
        assert!(
            found.next().is_none(),
            "more than one landing line names {name}"
        );
        line
    }

    #[test]
    fn initialising_over_an_unparsable_config_says_so_plainly() {
        let text = render(&Outcome::Initialised {
            path: std::path::PathBuf::from("rmem.toml"),
            dimension: 1536,
            replaced_unparsable: Some(
                "rmem.toml is not valid: that is not valid TOML (line 1, column 1)".to_string(),
            ),
        });
        assert!(
            text.contains("could not be parsed") && text.contains("--force"),
            "{text}"
        );
        assert!(text.contains("wrote rmem.toml"), "{text}");
    }

    #[test]
    fn initialising_a_fresh_config_carries_no_notice_about_a_replaced_one() {
        let text = render(&Outcome::Initialised {
            path: std::path::PathBuf::from("rmem.toml"),
            dimension: 1536,
            replaced_unparsable: None,
        });
        assert!(!text.contains("could not be parsed"), "{text}");
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
        let text = render(&Outcome::Remembered {
            ingested,
            landings,
            relations: 1,
            dropped: Vec::new(),
        });

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
            landing_line_for("Ben Severn", &text),
            "  Ben Severn  → entity 0 (recognised)",
            "Ben was already known"
        );
        assert_eq!(
            landing_line_for("Globex", &text),
            "  Globex  → entity 7 (new)",
            "Globex had never been seen"
        );

        // The spec's worked example, line for line. It counts mentions, facts
        // and relationships; "assertion" was a number of the library's own
        // that nothing else on screen explained, and relationships went
        // unmentioned altogether.
        assert_eq!(
            text.lines().next().unwrap(),
            "remembered 2 mentions, 0 facts, 1 relationship"
        );

        assert!(text.to_lowercase().contains("inferred"), "{text}");
        // Named, not numbered. The subject of a closed edge is always a
        // mention of this same turn, so its name is two lines above and the
        // code has it in hand; printing `entity 0` there broke the tie for no
        // reason. Entity 3 stays an id because this turn never mentioned it,
        // which is the ordinary case for a previous employer.
        assert_eq!(
            text.lines().find(|l| l.contains("ended:")).unwrap(),
            "  ended: Ben Severn employed_by entity 3 — \"starting a new job ends the previous one\""
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
        let text = render(&Outcome::Remembered {
            ingested,
            landings,
            relations: 0,
            dropped: Vec::new(),
        });
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
        let text = render(&Outcome::Recalled {
            hits: vec![],
            weak_below: 0.62,
        });
        assert!(!text.trim().is_empty(), "silence is not an answer");
    }

    #[test]
    fn what_was_not_remembered_is_printed_under_its_own_heading() {
        // Kept apart from the facts for the same reason closures are: this is
        // the turn having said less than the model described, and a reader who
        // cannot see it has no way to tell that from a turn that said less.
        let text = render(&Outcome::Remembered {
            ingested: Ingested {
                entities: vec![0],
                assertions: vec![0, 1],
                reviews: vec![],
                closed: vec![],
            },
            landings: vec![MentionLanding {
                name: "Ben Severn".into(),
                entity: 0,
                was_new: true,
            }],
            relations: 0,
            dropped: vec![Dropped {
                what: "fact",
                index: 1,
                why: "it names mention 9, but the response listed 1".to_string(),
            }],
        });
        assert!(text.contains("not remembered from this turn:"), "{text}");
        assert!(text.contains("fact 1 — it names mention 9"), "{text}");
        // And the turn's own content is still reported: the point is that both
        // are true at once.
        assert!(text.contains("Ben Severn"), "{text}");
    }

    #[test]
    fn a_clean_turn_prints_no_such_heading() {
        let text = render(&Outcome::Remembered {
            ingested: Ingested {
                entities: vec![0],
                assertions: vec![0],
                reviews: vec![],
                closed: vec![],
            },
            landings: vec![MentionLanding {
                name: "Ben Severn".into(),
                entity: 0,
                was_new: true,
            }],
            relations: 0,
            dropped: Vec::new(),
        });
        assert!(
            !text.contains("not remembered"),
            "an empty list must say nothing at all: {text}"
        );
    }
}
