//! Turning an [`Outcome`] into something to read.
//!
//! Separate from the commands so they can be tested without scraping text, and
//! so changing how something reads never risks changing what it does.

use rm_engine::{Believed, Standing};

use rm_host::command::{Found, MentionLanding, Outcome, DEFAULT_STATUS, SUPERSEDED};
use rm_host::time::format_day;

pub fn render(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Initialised {
            path,
            dimension,
            local,
            replaced_unparsable,
        } => {
            let notice = match replaced_unparsable {
                Some(why) => format!(
                    "the existing config could not be parsed, and was replaced because --force was passed: {why}\n\n"
                ),
                None => String::new(),
            };
            format!(
                // Where the dimension came from, said accurately. `--local`
                // asks no model anything, so reporting "taken from the model"
                // there was a claim about a call that never happened.
                "{notice}wrote {}\nembedding dimension {dimension}, {}\n\nnext: rmem remember \"something you want to remember\"",
                path.display(),
                if *local {
                    "the offline embedder's own -- no model was asked"
                } else {
                    "taken from the model"
                }
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

        // Not the same as "no decision by that title", and the difference is
        // the point: the title is real, so the reader must not be sent looking
        // for a spelling mistake. Both days, because either clock can be the
        // one that excluded it.
        Outcome::Decision(Found::NotYetRecorded {
            title,
            first_recorded,
            first_held,
        }) => format!(
            "{title:?} is on record, but nothing of it stood at the time you asked.\n\n  \
             first recorded  {}\n  holds from      {}\n\n\
             Ask on or after both of those, or drop the flags for what stands now.\n",
            format_day(*first_recorded),
            format_day(*first_held),
        ),
        // Not "no decision by that title": the title is real, so sending the
        // reader after a spelling mistake would be a lie. Both places named,
        // because knowing where it does apply is the actionable half.
        Outcome::Decision(Found::NotHere {
            title,
            scope,
            asked_from,
        }) => format!(
            "{title:?} is on record, but it does not apply here.\n\n  \
             it reaches     {scope}\n  you asked from {asked_from}\n\n\
             Use --scope {scope} to ask from there, or --all to ignore reach.\n"
        ),
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
                // Present tense only for a present-tense question. Under a past
                // clock this sentence is the exact failure the feature exists
                // to expose.
                match d.answered_at {
                    None => out.push_str("\nthis is what stands.\n"),
                    Some(at) => out.push_str(&format!(
                        "\nthis is what stood as of {}.\n",
                        format_day(at.tx)
                    )),
                }
            } else if d.status != SUPERSEDED && d.superseded_by.is_empty() {
                // Not replaced, and never in force. The status is the whole
                // reason, and the sentence here used to say "replaced, but
                // nothing records by what" -- which is false about a rejected
                // option: nothing replaced it because it never stood. It sent
                // a reader looking for a supersession that does not exist, on
                // 11 of the decisions in this project's own seeded log.
                //
                // `rm-mcp`'s renderer has said the right thing since statuses
                // arrived; this one was not brought along.
                out.push_str(&format!(
                    "\nthis never stood: its status is {:?}, not {DEFAULT_STATUS:?}.\n",
                    d.status
                ));
            } else if d.superseded_by.is_empty() {
                // Marked replaced with no edge recording by what. `decide`
                // cannot produce this -- `--supersedes` writes both ends -- so
                // it means the edge was lost rather than never written.
                out.push_str(
                    "\nmarked replaced, but nothing records by what.\n",
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

        Outcome::Rescoped {
            title,
            scope,
            previous,
            ..
        } => match previous {
            // Named rather than folded into one sentence: a decision that had
            // no reach and one whose reach was wrong are different mistakes,
            // and across a backfill the second is the one worth noticing.
            None => format!("{title:?} now reaches {scope}. It had no scope before."),
            Some(was) if was == scope => {
                format!("{title:?} already reached {scope}. Nothing changed.")
            }
            Some(was) => format!("{title:?} now reaches {scope}, where it reached {was}."),
        },

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
        Outcome::Noted {
            entity,
            attribute,
            absent,
            merged,
            reviews,
        } => {
            let what = if *absent {
                format!("{attribute} recorded as absent")
            } else {
                format!("{attribute} recorded")
            };
            let who = if *merged {
                format!("on entity {entity}, which the store already knew")
            } else {
                format!("on entity {entity}, new")
            };
            // The open questions are reported rather than swallowed. One
            // nobody is told about is one nobody settles, and the engine's
            // own position is that the fact is kept either way -- what is
            // uncertain is only whose it is.
            if reviews.is_empty() {
                return format!("{what} {who}");
            }
            let open: Vec<String> = reviews
                .iter()
                .map(|r| {
                    let other = if r.a == *entity { r.b } else { r.a };
                    format!(
                        "  scored {:.2} against entity {other}, inside the review band. `rmem review confirm {}` says they are the same; `rmem review reject {}` says they are not.",
                        r.score, r.id, r.id
                    )
                })
                .collect();
            format!(
                "{what} {who}\n\nopen question{}: the fact above is recorded either way -- what is open is only whose it is.\n{}",
                if reviews.len() == 1 { "" } else { "s" },
                open.join("\n")
            )
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
            local: false,
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
            local: false,
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

    fn a_standing_decision() -> rm_host::command::DecisionDetail {
        rm_host::command::DecisionDetail {
            entity: 1,
            title: "Pin the compiler".into(),
            choice: "a choice".into(),
            because: None,
            context: None,
            still_stands: true,
            status: "accepted".into(),
            supersedes: vec![],
            superseded_by: vec![],
            history: vec![],
            answered_at: None,
        }
    }

    /// The tense has to follow the clock. "this is what stands" under an
    /// `--as-of` in the past is the present tense about the past, which is the
    /// failure this feature exists to expose rather than commit.
    #[test]
    fn a_past_clock_is_not_described_in_the_present_tense() {
        const AUGUST: rm_engine::Timestamp = 1_787_529_600_000;
        let now = render(&Outcome::Decision(Found::Decision(Box::new(
            a_standing_decision(),
        ))));
        assert!(now.contains("this is what stands."), "{now}");

        let then = render(&Outcome::Decision(Found::Decision(Box::new(
            rm_host::command::DecisionDetail {
                answered_at: Some(rm_host::time::At {
                    valid: AUGUST,
                    tx: AUGUST,
                }),
                ..a_standing_decision()
            },
        ))));
        assert!(then.contains("stood as of 2026-08-24"), "{then}");
        assert!(
            !then.contains("this is what stands."),
            "present tense under a past clock: {then}"
        );
    }

    #[test]
    fn a_decision_the_store_had_not_heard_of_says_when_it_arrived() {
        const AUGUST: rm_engine::Timestamp = 1_787_529_600_000;
        const MARCH: rm_engine::Timestamp = 1_772_236_800_000;
        let out = render(&Outcome::Decision(Found::NotYetRecorded {
            title: "Pin the compiler".into(),
            first_recorded: AUGUST,
            first_held: MARCH,
        }));
        assert!(out.contains("2026-08-24"), "the day it arrived: {out}");
        assert!(out.contains("2026-02-28"), "the day it holds from: {out}");
        assert!(
            !out.contains("no decision by that title"),
            "must not read as a typo: {out}"
        );
    }

    #[test]
    fn a_decision_out_of_reach_names_both_places() {
        let out = render(&Outcome::Decision(Found::NotHere {
            title: "A sibling".into(),
            scope: "work/other".into(),
            asked_from: "work/goldenmatch".into(),
        }));
        assert!(out.contains("work/other"), "{out}");
        assert!(out.contains("work/goldenmatch"), "{out}");
        assert!(
            !out.contains("no decision by that title"),
            "must not read as a typo: {out}"
        );
    }

    /// A rejected option was never in force, so nothing replaced it. Saying
    /// "replaced, but nothing records by what" sent a reader looking for a
    /// supersession that does not exist -- on 11 decisions in this project's
    /// own seeded log, including the one recording why this was not fixed.
    #[test]
    fn a_decision_that_never_stood_says_so_rather_than_claiming_it_was_replaced() {
        for status in ["rejected", "proposed", "deprecated"] {
            let out = render(&Outcome::Decision(Found::Decision(Box::new(
                rm_host::command::DecisionDetail {
                    still_stands: false,
                    status: status.to_string(),
                    superseded_by: vec![],
                    ..a_standing_decision()
                },
            ))));
            assert!(out.contains("this never stood"), "{status}: {out}");
            assert!(out.contains(status), "the status is the reason: {out}");
            assert!(
                !out.contains("replaced, but nothing records by what"),
                "{status} was never replaced: {out}"
            );
        }
    }

    /// The one case the old sentence was actually about: marked replaced with
    /// no edge saying by what. `decide` cannot produce it, so it means a lost
    /// edge rather than a never-written one.
    #[test]
    fn a_superseded_decision_with_no_edge_still_says_something_true() {
        let out = render(&Outcome::Decision(Found::Decision(Box::new(
            rm_host::command::DecisionDetail {
                still_stands: false,
                status: "superseded".to_string(),
                superseded_by: vec![],
                ..a_standing_decision()
            },
        ))));
        assert!(out.contains("marked replaced"), "{out}");
        assert!(!out.contains("this never stood"), "{out}");
    }
    /// Every command this crate tells a user to run must parse.
    ///
    /// The `Noted` hint shipped saying `rmem review --confirm <id>`, which is
    /// a usage error -- `review` takes a subcommand, not a flag, and the
    /// correct spelling was already a few hundred lines up in this same file.
    /// A string-equality test against the right text would have missed it
    /// just as easily, so this runs what the message says through the real
    /// parser: the only thing that can tell a command from a plausible one.
    #[test]
    fn every_command_the_output_suggests_actually_parses() {
        let rendered = render(&Outcome::Noted {
            entity: 0,
            attribute: "team".into(),
            absent: false,
            merged: false,
            reviews: vec![rm_engine::PendingReview {
                id: 3,
                a: 0,
                b: 27,
                score: 6.17,
            }],
        });

        // Every `rmem ...` span the message offers, taken from the text
        // rather than restated -- restating it is how the two drift.
        let suggested: Vec<Vec<String>> = rendered
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|c| c.starts_with("rmem "))
            .map(|c| c.split_whitespace().skip(1).map(str::to_string).collect())
            .collect();
        assert!(
            suggested.len() >= 2,
            "the hint stopped offering commands: {rendered}"
        );
        for argv in suggested {
            crate::args::parse(argv.iter().cloned()).unwrap_or_else(|e| {
                panic!(
                    "the output suggests `rmem {}`, which does not parse: {e}",
                    argv.join(" ")
                )
            });
        }
    }
}
