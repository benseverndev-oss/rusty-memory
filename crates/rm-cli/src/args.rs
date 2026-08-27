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

use rm_engine::{StableId, Timestamp};
use rm_host::scope::UNIVERSAL;

use crate::CliError;

pub const USAGE: &str = "\
rmem — a memory that resolves contradictions deterministically

    rmem init [--force] [--local]    write rmem.toml, asking the model its embedding size
                                     (--local uses the offline embedder: no key, no socket)
    rmem remember \"<turn>\" [--speaker <name>]
                                     extract a turn and record what it said
    rmem recall \"<query>\" [-k N] [--scope <s>] [--all]
                                     find assertions near a query (default 5).
                                     --scope asks from a position, --all
                                     searches regardless of reach
    rmem about <entity> <attribute> [--valid-at YYYY-MM-DD] [--as-of YYYY-MM-DD] [--according-to <id>]
                                     what the store believes an attribute holds;
                                     --valid-at asks what was true then, --as-of
                                     what the store knew then
    rmem review                      open questions the resolver could not answer
    rmem review confirm <id>         answer one: the same thing
    rmem review reject <id>          answer one: different things
    rmem ingest <dir> [--dry-run]    read every .md under a directory into a
                                     scratch store; --dry-run calls no model
    rmem note <who> <attr> <value>   record a fact; --absent asserts there is none
    rmem decide \"<title>\" \"<choice>\" --scope <s> [--because <why>]
                                     [--context <what prompted it>]
                                     [--status proposed|accepted|rejected|deprecated]
                                     [--supersedes \"<title>\"] [--at YYYY-MM-DD]
                                     record a decision under a stable, findable title.
                                     --scope says how far it reaches: * for
                                     everywhere, or a path like work/goldenmatch
    rmem decisions [--status <s>] [--scope <s>] [--all]
                   [--valid-at YYYY-MM-DD] [--as-of YYYY-MM-DD]
                                     every decision, and whether it still stands
    rmem rescope \"<title>\" --scope <scope>
                                     correct how far one decision reaches,
                                     without recording a new choice
    rmem reindex                     re-embed every assertion under the current provider
    rmem decision \"<title>\" [--scope <s>] [--all]
                   [--valid-at YYYY-MM-DD] [--as-of YYYY-MM-DD]
                                     one decision in full, and the chain it sits in

Entity ids come from `remember` and `recall`. Review ids come from `review`.
A decision is found again by its title, so write one you would search for.
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Init {
        force: bool,
        /// Write a config for the local embedder: no dimension probe, no key,
        /// no socket. `init` otherwise asks the model its embedding size, so
        /// without a key it writes nothing -- and the keyless path is the one
        /// the documentation recommends.
        local: bool,
    },
    Note {
        who: String,
        /// What sort of thing this is. Defaults to `person`, which is what
        /// the first real dataset is; anything else says so with `--kind`.
        kind: String,
        attribute: String,
        /// `None` when `--absent` was given: an asserted absence, which is
        /// a claim and not a gap.
        value: Option<String>,
        /// Extra mention fields, in the order given. They reach the identity
        /// record the resolver compares, not the attributes.
        fields: Vec<(String, String)>,
        valid_from: Option<Timestamp>,
        scope: Option<String>,
        /// Whose view this is, as an entity id. `None` records the
        /// store's own fact, which is what saying nothing asserts.
        according_to: Option<StableId>,
    },
    Ingest {
        /// The directory to read. Every `.md` under it, recursively.
        path: String,
        /// Chunk and report without calling a model, so the cost of a
        /// run is known before it is paid.
        dry_run: bool,
    },
    Remember {
        text: String,
        /// Who said it, so first-person references resolve to them.
        speaker: Option<String>,
    },
    Recall {
        query: String,
        k: usize,
        /// Ask from this position instead of `RMEM_SCOPE`.
        scope: Option<String>,
        /// Suspend the applicability rule and search everything.
        all: bool,
    },
    About {
        entity: u64,
        attribute: String,
        /// What was true then. `None` is now.
        valid_at: Option<Timestamp>,
        /// What the store knew then. `None` is now.
        as_of: Option<Timestamp>,
        /// Whose view to ask for. `None` asks what the store itself
        /// holds, which never includes anybody's view.
        according_to: Option<StableId>,
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
        /// How far this decision reaches. Required: reach varies per decision,
        /// so no session default can be right.
        scope: String,
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
        /// Ask from this position instead of `RMEM_SCOPE`.
        scope: Option<String>,
        /// Suspend the applicability rule and show everything.
        all: bool,
        /// What held then. `None` is what holds now.
        valid_at: Option<Timestamp>,
        /// What the store knew then. `None` is what it knows now.
        as_of: Option<Timestamp>,
    },
    /// Correct how far one existing decision reaches, and nothing else.
    ///
    /// Separate from `Decide` because re-deciding to attach a scope writes a
    /// second `choice`, and `revisions` counts those -- every backfilled
    /// decision would read as revised when none was.
    Rescope {
        title: String,
        scope: String,
    },
    /// Rebuild every vector in the store under the current provider.
    Reindex,
    /// Read one decision by its exact title.
    Decision {
        title: String,
        /// Ask from this position instead of `RMEM_SCOPE`.
        scope: Option<String>,
        /// Suspend the applicability rule and show everything.
        all: bool,
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
                .filter(|a| *a != "--force" && *a != "--local")
                .collect();
            if !unknown.is_empty() {
                return Err(CliError::Usage(format!(
                    "init does not take {unknown:?} -- the only things it takes are --force and --local

{USAGE}"
                )));
            }
            Ok(Command::Init {
                force: args.iter().any(|a| a == "--force"),
                local: args.iter().any(|a| a == "--local"),
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
                scope: flag(&args, "--scope")?,
                all: args.iter().any(|a| a == "--all"),
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
                // An id, not a name, for the same reason `note` refuses
                // one: resolving here would put a resolution failure in
                // the middle of a read.
                according_to: match flag(&args, "--according-to")? {
                    None => None,
                    Some(v) => Some(v.parse::<StableId>().map_err(|_| {
                        CliError::Usage(format!(
                            "--according-to takes an entity id, not {v:?}\n\n{USAGE}"
                        ))
                    })?),
                },
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

        "ingest" => {
            let Some(path) = args.get(1).filter(|a| !a.starts_with("--")) else {
                return Err(CliError::Usage(format!(
                    "ingest needs a directory to read\n\n{USAGE}"
                )));
            };
            Ok(Command::Ingest {
                path: path.clone(),
                dry_run: args.iter().any(|a| a == "--dry-run"),
            })
        }

        "note" => {
            // Positionals first, exactly as `decide` requires, so a flag
            // cannot be swallowed as a value.
            let absent = args.iter().any(|a| a == "--absent");
            let positional: Vec<&String> = args[1..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .collect();
            let (who, attribute, value) = match (positional.len(), absent) {
                (3, false) => (
                    positional[0].clone(),
                    positional[1].clone(),
                    Some(positional[2].clone()),
                ),
                (2, true) => (positional[0].clone(), positional[1].clone(), None),
                (3, true) => {
                    return Err(CliError::Usage(format!(
                        "a value and --absent contradict each other: --absent says there is no value, so do not also give one

{USAGE}"
                    )))
                }
                _ => {
                    return Err(CliError::Usage(format!(
                        "note takes <who> <attribute> <value>, or <who> <attribute> --absent

{USAGE}"
                    )))
                }
            };

            // `--field` repeats, so `flag` -- which finds the first -- is not
            // enough. A pair with no `=` is refused rather than stored under
            // a field named after the whole argument.
            let mut fields: Vec<(String, String)> = Vec::new();
            for (i, a) in args.iter().enumerate() {
                if a != "--field" {
                    continue;
                }
                let Some(pair) = args.get(i + 1) else {
                    return Err(CliError::Usage(format!(
                        "--field needs name=value after it

{USAGE}"
                    )));
                };
                let Some((k, v)) = pair.split_once('=') else {
                    return Err(CliError::Usage(format!(
                        "--field takes name=value, and {pair:?} has no '=' -- without one there is nothing to compare against

{USAGE}"
                    )));
                };
                fields.push((k.to_string(), v.to_string()));
            }

            Ok(Command::Note {
                who,
                attribute,
                value,
                fields,
                kind: flag(&args, "--kind")?.unwrap_or_else(|| "person".to_string()),
                valid_from: flag(&args, "--valid-from")?
                    .map(|d| rm_host::time::parse_day(&d).map_err(CliError::Usage))
                    .transpose()?,
                scope: flag(&args, "--scope")?,
                // An id, never a name. Resolving a holder's name here
                // would put a resolution failure -- and possibly a
                // review -- in the middle of a write.
                according_to: match flag(&args, "--according-to")? {
                    None => None,
                    Some(v) => Some(v.parse::<StableId>().map_err(|_| {
                        CliError::Usage(format!(
                            "--according-to takes an entity id, not {v:?} -- resolve the name first\n\n{USAGE}"
                        ))
                    })?),
                },
            })
        }

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
            let Some(scope) = flag(&args, "--scope")? else {
                return Err(CliError::Usage(format!(
                    "decide needs --scope: how far this decision reaches. {UNIVERSAL:?} for everywhere, or a path like \"work/goldenmatch\"

{USAGE}"
                )));
            };
            Ok(Command::Decide {
                title: title.clone(),
                choice: choice.clone(),
                scope,
                status: flag(&args, "--status")?,
                decided_at: flag(&args, "--at")?
                    .map(|d| rm_host::time::parse_day(&d).map_err(CliError::Usage))
                    .transpose()?,
                because: flag(&args, "--because")?,
                context: flag(&args, "--context")?,
                supersedes: flag(&args, "--supersedes")?,
            })
        }

        "rescope" => {
            let Some(title) = args.get(1) else {
                return Err(CliError::Usage(format!(
                    "rescope needs the title of the decision to correct

{USAGE}"
                )));
            };
            if title.starts_with("--") {
                return Err(CliError::Usage(format!(
                    "rescope takes the title first, before any flags

{USAGE}"
                )));
            }
            let Some(scope) = flag(&args, "--scope")? else {
                return Err(CliError::Usage(format!(
                    "rescope needs --scope: how far this decision reaches. {UNIVERSAL:?} for everywhere, or a path like \"work/goldenmatch\"

{USAGE}"
                )));
            };
            Ok(Command::Rescope {
                title: title.clone(),
                scope,
            })
        }

        "reindex" => Ok(Command::Reindex),

        "decisions" => Ok(Command::Decisions {
            status: flag(&args, "--status")?,
            scope: flag(&args, "--scope")?,
            all: args.iter().any(|a| a == "--all"),
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
                scope: flag(&args, "--scope")?,
                all: args.iter().any(|a| a == "--all"),
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
            parse_args(&["decide", "T", "C", "--scope", "work", "--at", "2026-03-14"])
        else {
            panic!("a good date should parse")
        };
        // 2026-03-14T00:00:00Z
        assert_eq!(decided_at, Some(1_773_446_400_000));

        let Ok(Command::Decide { decided_at, .. }) =
            parse_args(&["decide", "T", "C", "--scope", "work"])
        else {
            panic!()
        };
        assert_eq!(decided_at, None, "no flag means now, decided downstream");

        for bad in ["14/03/2026", "March", "2026-3-14", "2026-02-30"] {
            let Err(CliError::Usage(why)) =
                parse_args(&["decide", "T", "C", "--scope", "work", "--at", bad])
            else {
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
                according_to: None,
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
                scope: None,
                all: false,
                valid_at: None,
                as_of: None
            }
        );
        assert_eq!(
            parse_args(&["decisions", "--status", "rejected"]).unwrap(),
            Command::Decisions {
                status: Some("rejected".into()),
                scope: None,
                all: false,
                valid_at: None,
                as_of: None
            }
        );
    }

    #[test]
    fn each_command_parses_to_what_it_says() {
        assert_eq!(
            parse_args(&["init"]).unwrap(),
            Command::Init {
                force: false,
                local: false
            }
        );
        assert_eq!(
            parse_args(&["init", "--force"]).unwrap(),
            Command::Init {
                force: true,
                local: false
            }
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
                k: 5,
                scope: None,
                all: false
            }
        );
        assert_eq!(
            parse_args(&["recall", "jobs", "-k", "20"]).unwrap(),
            Command::Recall {
                query: "jobs".into(),
                k: 20,
                scope: None,
                all: false
            }
        );
        assert_eq!(
            parse_args(&["about", "3", "employer"]).unwrap(),
            Command::About {
                entity: 3,
                attribute: "employer".into(),
                valid_at: None,
                as_of: None,
                according_to: None,
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
                k: 20,
                scope: None,
                all: false
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
            Command::Init {
                force: false,
                local: false
            }
        );
        assert_eq!(
            parse_args(&["init", "--force"]).unwrap(),
            Command::Init {
                force: true,
                local: false
            }
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
            ..
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

    #[test]
    fn decide_requires_a_scope_and_the_reads_take_a_position() {
        let Command::Decide { scope, .. } = parse_args(&[
            "decide",
            "Pin the compiler",
            "rust-toolchain.toml names the version",
            "--scope",
            "work/goldenmatch",
        ])
        .unwrap() else {
            panic!("not a decide command")
        };
        assert_eq!(scope, "work/goldenmatch");

        let e = parse_args(&["decide", "A title", "A choice"]).unwrap_err();
        assert!(
            format!("{e}").contains("--scope"),
            "the refusal should name the flag: {e}"
        );

        let Command::Decisions { scope, all, .. } =
            parse_args(&["decisions", "--scope", "personal"]).unwrap()
        else {
            panic!("not a decisions command")
        };
        assert_eq!(scope.as_deref(), Some("personal"));
        assert!(!all);

        let Command::Decision { all, scope, .. } =
            parse_args(&["decision", "A title", "--all"]).unwrap()
        else {
            panic!("not a decision command")
        };
        assert!(all, "--all suspends the rule");
        assert_eq!(scope, None);
    }

    #[test]
    fn recall_takes_a_position_and_a_way_to_ignore_it() {
        let Command::Recall { scope, all, .. } =
            parse_args(&["recall", "a question", "--scope", "personal"]).unwrap()
        else {
            panic!("not a recall command")
        };
        assert_eq!(scope.as_deref(), Some("personal"));
        assert!(!all);

        let Command::Recall { all, scope, .. } =
            parse_args(&["recall", "a question", "--all"]).unwrap()
        else {
            panic!("not a recall command")
        };
        assert!(all, "--all suspends the rule");
        assert_eq!(scope, None);
    }
    /// `--local` is a second flag, and the two compose.
    ///
    /// It exists because `init` probes the model for its embedding dimension,
    /// so without a key it exits 1 and writes nothing -- while the README
    /// recommends `embedder = "local"` as a path needing no key and no socket.
    /// The only documented way to make a config could not make the config the
    /// documentation recommends. Found by a session following those steps
    /// literally.
    #[test]
    fn init_takes_local_and_force_together() {
        assert_eq!(
            parse_args(&["init", "--local"]).unwrap(),
            Command::Init {
                force: false,
                local: true
            }
        );
        assert_eq!(
            parse_args(&["init", "--local", "--force"]).unwrap(),
            Command::Init {
                force: true,
                local: true
            }
        );
        assert_eq!(
            parse_args(&["init"]).unwrap(),
            Command::Init {
                force: false,
                local: false
            }
        );
    }

    /// And a mistyped flag is still refused rather than ignored, with both
    /// names in the message -- the reason the existing check exists.
    #[test]
    fn init_still_refuses_a_flag_it_does_not_know() {
        let err = parse_args(&["init", "--locl"]).unwrap_err().to_string();
        assert!(err.contains("--locl"), "{err}");
        assert!(
            err.contains("--local"),
            "the message must name the real flag: {err}"
        );
        assert!(
            err.contains("--force"),
            "and not drop the one it already named: {err}"
        );
    }
    /// The shape of a note: who, what, and the value.
    #[test]
    fn a_note_parses_who_what_and_value() {
        assert_eq!(
            parse_args(&["note", "Jon Severn", "role", "leads circ"]).unwrap(),
            Command::Note {
                who: "Jon Severn".into(),
                kind: "person".into(),
                attribute: "role".into(),
                value: Some("leads circ".into()),
                fields: vec![],
                valid_from: None,
                scope: None,
                according_to: None,
            }
        );
    }

    /// `--absent` is a claim, so it takes the place of the value rather
    /// than sitting beside one. Given both, the two contradict each other
    /// and neither is guessed at.
    #[test]
    fn absent_replaces_the_value_rather_than_joining_it() {
        assert_eq!(
            parse_args(&["note", "Jon", "reports", "--absent"]).unwrap(),
            Command::Note {
                who: "Jon".into(),
                kind: "person".into(),
                attribute: "reports".into(),
                value: None,
                fields: vec![],
                valid_from: None,
                scope: None,
                according_to: None,
            }
        );
        let err = parse_args(&["note", "Jon", "reports", "none", "--absent"]).unwrap_err();
        assert!(
            format!("{err}").contains("--absent"),
            "a value and --absent contradict each other: {err}"
        );
    }

    /// `--field` repeats, because a person has more than one identifier and
    /// the mention is written once.
    #[test]
    fn field_repeats_and_keeps_its_order() {
        let Command::Note { fields, .. } = parse_args(&[
            "note",
            "Jon",
            "role",
            "x",
            "--field",
            "email=j@example.com",
            "--field",
            "handle=jsev",
        ])
        .unwrap() else {
            panic!("expected Note")
        };
        assert_eq!(
            fields,
            vec![
                ("email".to_string(), "j@example.com".to_string()),
                ("handle".to_string(), "jsev".to_string())
            ]
        );
    }

    /// A `--field` with no `=` is a typo, and typos are refused rather than
    /// stored as a field named after the whole argument.
    #[test]
    fn a_field_without_a_value_is_refused() {
        let err = parse_args(&["note", "Jon", "role", "x", "--field", "email"]).unwrap_err();
        assert!(format!("{err}").contains("--field"), "{err}");
    }
    /// `--according-to` takes an entity id, not a name.
    ///
    /// Resolving a holder's name would put a resolution failure -- and
    /// possibly a review -- in the middle of a write. The host resolves
    /// first; this parses an id or refuses.
    #[test]
    fn according_to_takes_an_id_and_refuses_a_name() {
        let Command::Note { according_to, .. } = parse_args(&[
            "note",
            "Jon",
            "team",
            "Circulation",
            "--according-to",
            "300",
        ])
        .unwrap() else {
            panic!("expected Note")
        };
        assert_eq!(according_to, Some(300));

        let err = parse_args(&[
            "note",
            "Jon",
            "team",
            "Circulation",
            "--according-to",
            "Divya",
        ])
        .unwrap_err();
        assert!(format!("{err}").contains("entity id"), "{err}");
    }

    /// Saying nothing records the store's own fact.
    ///
    /// The default is the risk: two commands differing by one argument
    /// produce records that never meet, so what the absence asserts is
    /// pinned rather than assumed.
    #[test]
    fn a_note_without_the_flag_is_the_stores_own_fact() {
        let Command::Note { according_to, .. } =
            parse_args(&["note", "Jon", "team", "Circulation"]).unwrap()
        else {
            panic!("expected Note")
        };
        assert_eq!(according_to, None);
    }
}
