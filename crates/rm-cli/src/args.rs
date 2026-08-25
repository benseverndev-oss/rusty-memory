//! Parsing, by hand.
//!
//! Five subcommands is about sixty lines against a dependency tree that pulls
//! in `syn`, `quote` and `proc-macro2`. This workspace has twice chosen to
//! write the small thing instead — exact search rather than an approximate
//! index, ports rather than an HTTP client — and both held up.
//!
//! The cost is real: [`USAGE`] is written by hand and can drift from this
//! parser. `the_usage_text_names_every_command_the_parser_accepts` is the guard
//! against that, and it is the only one there is.
//!
//! # Why these messages quote what the user typed
//!
//! Everywhere else in this crate a refusal names a field or a location and
//! never a value, because eight credential leaks came out of error messages
//! echoing `rmem.toml`. Every message in this module does the opposite: it
//! quotes the argument it did not understand.
//!
//! That is deliberate, and the difference is where the value has already been.
//! A config file is committed, shared, and read by a program while nobody is
//! watching, so a secret in one reaches places its author never chose. A value
//! typed on a command line is already in the shell's history, in the process
//! table, and on the screen it was typed on — echoing it back exposes nothing
//! that was not exposed by typing it. Against that, refusing to show which
//! argument was wrong would make every usage error useless: "that is not an
//! rmem command" without saying which word is a worse message than no message.
//!
//! Checked rather than assumed: every interpolation below draws from the
//! `args` iterator and none of them from any file. Anything that begins
//! reading a file in here needs this paragraph revisited.

use rm_engine::Timestamp;

use crate::CliError;

pub const USAGE: &str = "\
rmem — a memory that resolves contradictions deterministically

    rmem init [--force]              write rmem.toml, asking the model its embedding size
    rmem remember \"<turn>\" [--speaker <name>]
                                     extract a turn and record what it said
    rmem recall \"<query>\" [-k N]     find assertions near a query (default 5)
    rmem about <entity> <attribute> [--valid-at YYYY-MM-DD] [--as-of YYYY-MM-DD]
                                     what the store believes an attribute holds;
                                     --valid-at asks what was true then, --as-of
                                     what the store knew then
    rmem review                      open questions the resolver could not answer
    rmem review confirm <id>         answer one: the same thing
    rmem review reject <id>          answer one: different things
    rmem decide \"<title>\" \"<choice>\" [--because <why>] [--context <what prompted it>]
                                     [--status proposed|accepted|rejected|deprecated]
                                     [--supersedes \"<title>\"] [--at YYYY-MM-DD]
                                     record a decision under a stable, findable title
    rmem decisions [--status <s>] [--valid-at YYYY-MM-DD] [--as-of YYYY-MM-DD]
                                     every decision, and whether it still stands
    rmem reindex                     re-embed every assertion under the current provider
    rmem decision \"<title>\" [--valid-at YYYY-MM-DD] [--as-of YYYY-MM-DD]
                                     one decision in full, and the chain it sits in

Entity ids come from `remember` and `recall`. Review ids come from `review`.
A decision is found again by its title, so write one you would search for.
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Init {
        force: bool,
    },
    Remember {
        text: String,
        /// Who said it, so first-person references resolve to them.
        speaker: Option<String>,
    },
    Recall {
        query: String,
        k: usize,
    },
    About {
        entity: u64,
        attribute: String,
        /// What was true then. `None` is now.
        valid_at: Option<Timestamp>,
        /// What the store knew then. `None` is now.
        as_of: Option<Timestamp>,
    },
    ReviewList,
    ReviewConfirm(u64),
    ReviewReject(u64),
    /// Record a decision. Unlike [`Command::Remember`] this never reaches a
    /// completion model: the shape is known, so the fields are written
    /// directly under names that stay findable.
    Decide {
        title: String,
        choice: String,
        /// One of `DECISION_STATUSES`. `None` means `accepted`.
        status: Option<String>,
        /// When the decision was made, as milliseconds. `None` means now.
        ///
        /// Parsed from `--at YYYY-MM-DD` here rather than carried as a string,
        /// so a date nobody can read is refused while the user is still at the
        /// prompt to fix it.
        decided_at: Option<Timestamp>,
        because: Option<String>,
        context: Option<String>,
        supersedes: Option<String>,
    },
    Decisions {
        /// Show only decisions with this status. `None` shows every one.
        status: Option<String>,
        /// What held then. `None` is what holds now.
        valid_at: Option<Timestamp>,
        /// What the store knew then. `None` is what it knows now.
        as_of: Option<Timestamp>,
    },
    /// Rebuild every vector in the store under the current provider.
    Reindex,
    /// Read one decision by its exact title.
    Decision {
        title: String,
        /// What held then. `None` is what holds now.
        valid_at: Option<Timestamp>,
        /// What the store knew then. `None` is what it knows now.
        as_of: Option<Timestamp>,
    },
}

/// A `YYYY-MM-DD` flag, as the *end* of the day it names.
///
/// Both axes read this way, so a query naming today sees what was recorded this
/// morning. See `rm_host::time::parse_day_end`.
///
/// A free function rather than a closure per command: `about`, `decisions` and
/// `decision` all take these two flags, and three copies is three places for
/// the end-of-day reading to stop being true in one of them.
fn day(args: &[String], name: &str) -> Result<Option<Timestamp>, CliError> {
    match flag(args, name)? {
        None => Ok(None),
        Some(d) => rm_host::time::parse_day_end(&d)
            .map(Some)
            .map_err(CliError::Usage),
    }
}

/// The value after a named flag, if the flag is there.
///
/// A flag present with nothing after it is an error rather than `None`: someone
/// who typed `--because` and then forgot the reason has said something
/// different from someone who never typed it, and silently dropping it would
/// record a decision with no stated reason and report success.
fn flag(args: &[String], name: &str) -> Result<Option<String>, CliError> {
    match args.iter().position(|a| a == name) {
        None => Ok(None),
        Some(i) => match args.get(i + 1) {
            Some(v) if !v.starts_with("--") => Ok(Some(v.clone())),
            _ => Err(CliError::Usage(format!(
                "{name} needs a value after it\n\n{USAGE}"
            ))),
        },
    }
}

/// Parse arguments, excluding the program name.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let args: Vec<String> = args.collect();
    let usage = || CliError::Usage(USAGE.to_string());

    let Some(first) = args.first() else {
        return Err(usage());
    };

    match first.as_str() {
        "init" => {
            // Anything that is not `--force` is refused rather than ignored.
            // `rmem init --frce` used to parse as `Init { force: false }` and
            // then refuse with "pass --force to replace it" -- which reads as
            // the flag being broken, and sends the reader to look at
            // everything except the four characters they mistyped.
            let unknown: Vec<&str> = args[1..]
                .iter()
                .map(String::as_str)
                .filter(|a| *a != "--force")
                .collect();
            if !unknown.is_empty() {
                return Err(CliError::Usage(format!(
                    "init does not take {unknown:?} -- the only thing it takes is --force

{USAGE}"
                )));
            }
            Ok(Command::Init {
                force: args.iter().any(|a| a == "--force"),
            })
        }

        "remember" => {
            let Some(text) = args.get(1) else {
                return Err(CliError::Usage(format!(
                    "remember needs the turn to remember, in quotes\n\n{USAGE}"
                )));
            };
            // The same trap `-k` set for `recall`: a flag sitting where the
            // positional belongs parses as the positional, and the command
            // succeeds having remembered the word "--speaker".
            if text == "--speaker" {
                return Err(CliError::Usage(format!(
                    "remember needs the turn before --speaker, in quotes: `rmem remember \"<turn>\" --speaker \"<name>\"`\n\n{USAGE}"
                )));
            }
            let speaker = match args.iter().position(|a| a == "--speaker") {
                None => None,
                Some(i) => Some(
                    args.get(i + 1)
                        .filter(|n| !n.trim().is_empty())
                        .ok_or_else(|| {
                            CliError::Usage(format!("--speaker needs a name\n\n{USAGE}"))
                        })?
                        .clone(),
                ),
            };
            Ok(Command::Remember {
                text: text.clone(),
                speaker,
            })
        }

        "recall" => {
            let Some(query) = args.get(1) else {
                return Err(CliError::Usage(format!(
                    "recall needs something to search for, in quotes\n\n{USAGE}"
                )));
            };
            // `-k` in the query position is a missing query, not a search
            // for the literal string "-k". The flag scan below would find it
            // at index 1 and read the number after it, so `rmem recall -k 20`
            // quietly searched for "-k" with k = 20 and reported nothing near
            // it -- a wrong answer that looks exactly like a right one.
            if query == "-k" {
                return Err(CliError::Usage(format!(
                    "recall needs something to search for before -k, in quotes: `rmem recall \"<query>\" -k 20`

{USAGE}"
                )));
            }
            let k = match args.iter().position(|a| a == "-k") {
                None => 5,
                Some(i) => args
                    .get(i + 1)
                    .and_then(|n| n.parse().ok())
                    .ok_or_else(|| CliError::Usage(format!("-k needs a number\n\n{USAGE}")))?,
            };
            Ok(Command::Recall {
                query: query.clone(),
                k,
            })
        }

        "about" => {
            let (Some(entity), Some(attribute)) = (args.get(1), args.get(2)) else {
                return Err(CliError::Usage(format!(
                    "about needs an entity id and an attribute\n\n{USAGE}"
                )));
            };
            let entity = entity.parse().map_err(|_| {
                CliError::Usage(format!(
                    "{entity:?} is not an entity id -- they are numbers, printed by `remember` and `recall`\n\n{USAGE}"
                ))
            })?;
            Ok(Command::About {
                entity,
                attribute: attribute.clone(),
                valid_at: day(&args, "--valid-at")?,
                as_of: day(&args, "--as-of")?,
            })
        }

        "review" => match (args.get(1).map(String::as_str), args.get(2)) {
            (None, _) => Ok(Command::ReviewList),
            (Some("confirm"), Some(id)) => id
                .parse()
                .map(Command::ReviewConfirm)
                .map_err(|_| CliError::Usage(format!("{id:?} is not a review id\n\n{USAGE}"))),
            (Some("reject"), Some(id)) => id
                .parse()
                .map(Command::ReviewReject)
                .map_err(|_| CliError::Usage(format!("{id:?} is not a review id\n\n{USAGE}"))),
            _ => Err(CliError::Usage(format!(
                "review takes nothing, or `confirm <id>`, or `reject <id>`\n\n{USAGE}"
            ))),
        },

        "decide" => {
            let (Some(title), Some(choice)) = (args.get(1), args.get(2)) else {
                return Err(CliError::Usage(format!(
                    "decide needs a title and a choice -- the title is how it is found again\n\n{USAGE}"
                )));
            };
            if title.starts_with("--") || choice.starts_with("--") {
                return Err(CliError::Usage(format!(
                    "decide takes the title and the choice first, before any flags\n\n{USAGE}"
                )));
            }
            Ok(Command::Decide {
                title: title.clone(),
                choice: choice.clone(),
                status: flag(&args, "--status")?,
                decided_at: flag(&args, "--at")?
                    .map(|d| rm_host::time::parse_day(&d).map_err(CliError::Usage))
                    .transpose()?,
                because: flag(&args, "--because")?,
                context: flag(&args, "--context")?,
                supersedes: flag(&args, "--supersedes")?,
            })
        }

        "reindex" => Ok(Command::Reindex),

        "decisions" => Ok(Command::Decisions {
            status: flag(&args, "--status")?,
            valid_at: day(&args, "--valid-at")?,
            as_of: day(&args, "--as-of")?,
        }),

        // Singular, and a different command: `decisions` is the index and this
        // is the entry. The two names differ by one character on purpose --
        // they are the same noun -- so the usage error below quotes what was
        // typed rather than guessing which was meant.
        "decision" => {
            let Some(title) = args.get(1) else {
                return Err(CliError::Usage(format!(
                    "decision needs the title to read, exactly as it was recorded -- `rmem decisions` lists them\n\n{USAGE}"
                )));
            };
            Ok(Command::Decision {
                title: title.clone(),
                valid_at: day(&args, "--valid-at")?,
                as_of: day(&args, "--as-of")?,
            })
        }

        other => Err(CliError::Usage(format!(
            "{other:?} is not an rmem command\n\n{USAGE}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().map(|s| s.to_string()))
    }

    /// `--at` is a day, and a day this cannot read is refused at the prompt.
    ///
    /// Parsed here rather than carried through as a string so the refusal
    /// arrives while the user is still standing in front of it, before a
    /// provider is built or a lock is taken. The alternative -- falling back to
    /// the clock -- would file the decision under today, which is the one thing
    /// passing a date was meant to avoid.
    #[test]
    fn a_decision_can_be_dated_and_a_bad_date_is_refused() {
        let Ok(Command::Decide { decided_at, .. }) =
            parse_args(&["decide", "T", "C", "--at", "2026-03-14"])
        else {
            panic!("a good date should parse")
        };
        // 2026-03-14T00:00:00Z
        assert_eq!(decided_at, Some(1_773_446_400_000));

        let Ok(Command::Decide { decided_at, .. }) = parse_args(&["decide", "T", "C"]) else {
            panic!()
        };
        assert_eq!(decided_at, None, "no flag means now, decided downstream");

        for bad in ["14/03/2026", "March", "2026-3-14", "2026-02-30"] {
            let Err(CliError::Usage(why)) = parse_args(&["decide", "T", "C", "--at", bad]) else {
                panic!("{bad:?} should be refused")
            };
            assert!(!why.is_empty(), "for {bad:?}");
        }
    }

    /// `about` can ask along both axes, and a date names the whole day.
    ///
    /// The pair is the point of a bi-temporal store, and until now only the
    /// MCP door could ask it: this one always passed `now, now`, so a store
    /// that could record when a decision held had no way to be asked.
    #[test]
    fn about_can_ask_along_both_axes() {
        assert_eq!(
            parse_args(&["about", "3", "choice"]).unwrap(),
            Command::About {
                entity: 3,
                attribute: "choice".into(),
                valid_at: None,
                as_of: None,
            },
            "no flags means now on both, decided downstream"
        );

        let Ok(Command::About {
            valid_at, as_of, ..
        }) = parse_args(&[
            "about",
            "3",
            "choice",
            "--valid-at",
            "2026-03-14",
            "--as-of",
            "2026-08-24",
        ])
        else {
            panic!("both flags should parse")
        };
        // The end of each day, not the start -- otherwise a query naming the
        // day something was recorded cannot see it.
        assert_eq!(valid_at, Some(1_773_446_400_000 + 86_399_999));
        assert_eq!(as_of, Some(1_787_529_600_000 + 86_399_999));

        for bad in ["2026-13-01", "yesterday", "03/14/2026"] {
            assert!(
                parse_args(&["about", "3", "choice", "--valid-at", bad]).is_err(),
                "{bad:?} should be refused"
            );
        }
    }

    /// `decisions` takes a status to filter by, and takes none to mean all.
    #[test]
    fn decisions_parses_its_filter() {
        assert_eq!(
            parse_args(&["decisions"]).unwrap(),
            Command::Decisions {
                status: None,
                valid_at: None,
                as_of: None
            }
        );
        assert_eq!(
            parse_args(&["decisions", "--status", "rejected"]).unwrap(),
            Command::Decisions {
                status: Some("rejected".into()),
                valid_at: None,
                as_of: None
            }
        );
    }

    #[test]
    fn each_command_parses_to_what_it_says() {
        assert_eq!(
            parse_args(&["init"]).unwrap(),
            Command::Init { force: false }
        );
        assert_eq!(
            parse_args(&["init", "--force"]).unwrap(),
            Command::Init { force: true }
        );
        assert_eq!(
            parse_args(&["remember", "I moved"]).unwrap(),
            Command::Remember {
                text: "I moved".into(),
                speaker: None,
            }
        );
        assert_eq!(
            parse_args(&["recall", "jobs"]).unwrap(),
            Command::Recall {
                query: "jobs".into(),
                k: 5
            }
        );
        assert_eq!(
            parse_args(&["recall", "jobs", "-k", "20"]).unwrap(),
            Command::Recall {
                query: "jobs".into(),
                k: 20
            }
        );
        assert_eq!(
            parse_args(&["about", "3", "employer"]).unwrap(),
            Command::About {
                entity: 3,
                attribute: "employer".into(),
                valid_at: None,
                as_of: None,
            }
        );
        assert_eq!(parse_args(&["review"]).unwrap(), Command::ReviewList);
        assert_eq!(
            parse_args(&["review", "confirm", "1"]).unwrap(),
            Command::ReviewConfirm(1)
        );
        assert_eq!(
            parse_args(&["review", "reject", "1"]).unwrap(),
            Command::ReviewReject(1)
        );
    }

    #[test]
    fn no_arguments_prints_the_usage_rather_than_guessing() {
        let err = parse_args(&[]).unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
        assert!(err.to_string().contains("remember"), "{err}");
    }

    #[test]
    fn an_unknown_command_says_so_and_shows_what_there_is() {
        let err = parse_args(&["recollect", "x"]).unwrap_err();
        assert!(err.to_string().contains("recollect"), "{err}");
        assert!(err.to_string().contains("recall"), "{err}");
    }

    #[test]
    fn a_command_missing_its_argument_names_what_is_missing() {
        for args in [vec!["remember"], vec!["recall"], vec!["about", "3"]] {
            let err = parse_args(&args).unwrap_err();
            assert!(err.to_string().len() > 20, "{args:?}: {err}");
        }
    }

    #[test]
    fn recall_with_no_query_before_the_flag_is_refused_rather_than_searched_for() {
        // `rmem recall -k 20` took "-k" as the query, then found the same
        // "-k" at index 1 in the flag scan and read 20 off the end of it. So
        // it searched the store for the literal string "-k" and reported
        // nothing near it -- a wrong answer indistinguishable from a right
        // one, which is the worst shape a parser bug can take.
        let err = parse_args(&["recall", "-k", "20"]).unwrap_err();
        assert!(matches!(err, CliError::Usage(_)), "{err:?}");
        assert!(err.to_string().contains("-k"), "{err}");
        assert!(
            err.to_string().contains("search for"),
            "it has to say what was expected: {err}"
        );
        // A real query with the flag after it is still the ordinary case.
        assert_eq!(
            parse_args(&["recall", "jobs", "-k", "20"]).unwrap(),
            Command::Recall {
                query: "jobs".into(),
                k: 20
            }
        );
    }

    #[test]
    fn init_refuses_an_argument_it_does_not_recognise_and_names_it() {
        // `rmem init --frce` parsed as `Init { force: false }` and was then
        // refused by `command::init` with "pass --force to replace it" --
        // which reads as the flag being broken rather than mistyped, and
        // sends the reader everywhere except the four characters at fault.
        let err = parse_args(&["init", "--frce"]).unwrap_err();
        assert!(err.to_string().contains("--frce"), "{err}");
        assert!(err.to_string().contains("--force"), "{err}");
        // And the two spellings it does accept still parse.
        assert_eq!(
            parse_args(&["init"]).unwrap(),
            Command::Init { force: false }
        );
        assert_eq!(
            parse_args(&["init", "--force"]).unwrap(),
            Command::Init { force: true }
        );
    }

    #[test]
    fn a_non_numeric_entity_says_it_wanted_a_number() {
        let err = parse_args(&["about", "Ben", "employer"]).unwrap_err();
        assert!(err.to_string().contains("Ben"), "{err}");
    }

    /// The commands [`USAGE`]'s table names, one entry per line.
    ///
    /// A line is `    rmem <invocation>` then two or more spaces then the
    /// description, so the invocation is everything before the first double
    /// space. Placeholders (`<id>`, `[--force]`, `"<turn>"`) are dropped,
    /// leaving the literal words a user types: `init`, `review confirm`.
    ///
    /// Parsed out rather than checked with `USAGE.contains(name)`, which was
    /// the bug: the prose line "Entity ids come from `remember` and `recall`.
    /// Review ids come from `review`." satisfied a bare substring check for
    /// three of the five commands, so deleting their table lines outright
    /// left every test passing.
    fn commands_in_usage() -> Vec<String> {
        USAGE
            .lines()
            // Indented: the unindented banner at the top of USAGE is a
            // title, not a table entry, and it starts with "rmem " too.
            .filter(|l| l.starts_with("    rmem "))
            .map(|l| {
                l.trim()
                    .split("  ")
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .skip(1)
                    .take_while(|w| w.starts_with(char::is_alphabetic))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }

    #[test]
    fn the_usage_text_names_every_command_the_parser_accepts() {
        // The one guard against a hand-written usage drifting from a
        // hand-written parser, which is the cost this crate accepted by not
        // taking clap. Every form the `match` in `parse` accepts needs its
        // own line -- `review` and `review confirm` included, since a table
        // that lists only the subcommands never tells a reader that bare
        // `rmem review` is a thing.
        let table = commands_in_usage();
        for invocation in [
            "init",
            "remember",
            "recall",
            "about",
            "review",
            "review confirm",
            "review reject",
        ] {
            assert!(
                table.iter().any(|entry| entry == invocation),
                "the command table has no line for `rmem {invocation}`; it has {table:?}"
            );
        }
    }

    #[test]
    fn every_command_the_usage_names_is_one_the_parser_recognises() {
        // Drift has two directions. The test above catches a command the
        // parser accepts and the usage forgot; this one catches a command
        // the usage promises and the parser dropped, which is the worse of
        // the two -- a user reads the line, types it, and is told it is not
        // an rmem command.
        for entry in commands_in_usage() {
            let name = entry.split(' ').next().unwrap().to_string();
            if let Err(e) = parse(std::iter::once(name.clone())) {
                assert!(
                    !e.to_string().contains("is not an rmem command"),
                    "the usage names `rmem {name}`, which the parser refuses outright"
                );
            }
        }
    }

    #[test]
    fn remember_takes_a_speaker() {
        assert_eq!(
            parse(
                ["remember", "I moved to Chicago", "--speaker", "Melanie"]
                    .map(String::from)
                    .into_iter()
            )
            .unwrap(),
            Command::Remember {
                text: "I moved to Chicago".into(),
                speaker: Some("Melanie".into()),
            }
        );
    }

    #[test]
    fn remember_without_a_speaker_says_so_rather_than_guessing() {
        let Command::Remember { speaker, .. } =
            parse(["remember", "someone moved"].map(String::from).into_iter()).unwrap()
        else {
            panic!("expected Remember")
        };
        assert_eq!(speaker, None);
    }

    #[test]
    fn a_speaker_flag_in_the_turn_position_is_refused() {
        // The trap `-k` set for `recall`: a flag where the positional belongs
        // parses as the positional, and the command succeeds having
        // remembered the word "--speaker".
        let err = parse(
            ["remember", "--speaker", "Melanie"]
                .map(String::from)
                .into_iter(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("needs the turn before --speaker"),
            "{err}"
        );
    }

    #[test]
    fn a_speaker_flag_with_no_name_is_refused() {
        let err = parse(
            ["remember", "a turn", "--speaker"]
                .map(String::from)
                .into_iter(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("--speaker needs a name"), "{err}");
    }

    #[test]
    fn the_decision_reads_take_both_clocks() {
        let Command::Decision {
            valid_at,
            as_of,
            title,
        } = parse_args(&[
            "decision",
            "Pin the compiler",
            "--valid-at",
            "2026-03-01",
            "--as-of",
            "2026-08-24",
        ])
        .unwrap()
        else {
            panic!("not a decision command")
        };
        assert_eq!(title, "Pin the compiler");
        // End of the named day, same as `about`.
        assert_eq!(valid_at, Some(1_772_323_200_000 + 86_399_999));
        assert_eq!(as_of, Some(1_787_529_600_000 + 86_399_999));

        let Command::Decisions {
            valid_at, as_of, ..
        } = parse_args(&["decisions", "--as-of", "2026-08-24"]).unwrap()
        else {
            panic!("not a decisions command")
        };
        assert_eq!(valid_at, None, "an absent flag stays absent");
        assert_eq!(as_of, Some(1_787_529_600_000 + 86_399_999));

        assert!(
            parse_args(&["decision", "X", "--as-of", "not-a-date"]).is_err(),
            "a date that is not one must be refused"
        );
    }
}
