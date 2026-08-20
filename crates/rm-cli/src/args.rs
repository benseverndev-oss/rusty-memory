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

use crate::CliError;

pub const USAGE: &str = "\
rmem — a memory that resolves contradictions deterministically

    rmem init [--force]              write rmem.toml, asking the model its embedding size
    rmem remember \"<turn>\"           extract a turn and record what it said
    rmem recall \"<query>\" [-k N]     find assertions near a query (default 5)
    rmem about <entity> <attribute>  what the store believes an attribute holds
    rmem review                      open questions the resolver could not answer
    rmem review confirm <id>         answer one: the same thing
    rmem review reject <id>          answer one: different things

Entity ids come from `remember` and `recall`. Review ids come from `review`.
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Init { force: bool },
    Remember { text: String },
    Recall { query: String, k: usize },
    About { entity: u64, attribute: String },
    ReviewList,
    ReviewConfirm(u64),
    ReviewReject(u64),
}

/// Parse arguments, excluding the program name.
pub fn parse(args: impl Iterator<Item = String>) -> Result<Command, CliError> {
    let args: Vec<String> = args.collect();
    let usage = || CliError::Usage(USAGE.to_string());

    let Some(first) = args.first() else {
        return Err(usage());
    };

    match first.as_str() {
        "init" => Ok(Command::Init {
            force: args.iter().any(|a| a == "--force"),
        }),

        "remember" => match args.get(1) {
            Some(text) => Ok(Command::Remember { text: text.clone() }),
            None => Err(CliError::Usage(format!(
                "remember needs the turn to remember, in quotes\n\n{USAGE}"
            ))),
        },

        "recall" => {
            let Some(query) = args.get(1) else {
                return Err(CliError::Usage(format!(
                    "recall needs something to search for, in quotes\n\n{USAGE}"
                )));
            };
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
                text: "I moved".into()
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
                attribute: "employer".into()
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
}
