//! Dispatch: arguments in, an [`Outcome`] or a [`CliError`] out.
//!
//! Here rather than in `main.rs` so it can be tested. It is the only place
//! that knows which ports each command needs, and that knowledge is not
//! visible from inside any single command — `review_list` takes an `&Engine`
//! and nothing else, so nothing a command's own test can see says whether the
//! binary hands it an API key it never asked for. `main.rs` keeps parse-free,
//! decision-free glue: run, print, exit.

use std::path::Path;

use rm_engine::Timestamp;

use crate::args::{parse, Command};
use rm_host::command::{self, Outcome};
use rm_host::config::{Config, InitConfig};
use rm_host::store;
use rm_host::time::At;

use crate::CliError;
use rm_host::HostError;

/// The process exit code a result becomes.
///
/// Zero or one, and the distinction is not cosmetic. `Believed::Unknown` is a
/// real answer — the store was asked and has no opinion — so `about` returning
/// it exits 0. A *refusal* is a failure to answer: survivorship declining
/// under the configured strategy, a review id that does not exist, extraction
/// rejecting a malformed response. Those exit 1.
///
/// A `u8` rather than a `std::process::ExitCode` so a test can assert on it;
/// `ExitCode` implements neither `PartialEq` nor `Debug`, and `main` converts
/// with `ExitCode::from`.
pub fn exit_code(result: &Result<Outcome, CliError>) -> u8 {
    match result {
        Ok(_) => 0,
        Err(_) => 1,
    }
}

/// Parse `args`, do what they say, and report what happened.
///
/// `now` is supplied rather than read here for the same reason `Engine` takes
/// no clock of its own: a caller that cannot control the time cannot test
/// anything that depends on it.
pub fn run(
    args: impl Iterator<Item = String>,
    config_path: &Path,
    now: Timestamp,
    // Where this session stands, from `RMEM_SCOPE`. `None` is no position,
    // which suspends the applicability rule entirely.
    session_scope: Option<String>,
) -> Result<Outcome, CliError> {
    let command = parse(args)?;

    if let Command::Init { force } = command {
        // Loaded here rather than by `init`, because init is what a user runs
        // when there is no config -- so only the provider block has to be
        // readable, and it comes from the template if the file is absent.
        //
        // A file that exists and fails to parse is still surfaced, not
        // treated as if it were absent and silently replaced by the
        // template's defaults -- that silent fallback is the bug this match
        // exists to prevent, and it is still prevented: `--force` is the one
        // way through, and only because a user who passes it has explicitly
        // said "replace this file", which this then says back to them rather
        // than doing quietly. Without `--force` the file wins exactly as it
        // always has.
        let (config, replaced_unparsable) = match Config::read_for_init(config_path)? {
            InitConfig::Absent(config) | InitConfig::Loaded(config) => (config, None),
            InitConfig::Unparsable(e) if force => (Config::from_template(), Some(e.to_string())),
            InitConfig::Unparsable(e) => {
                return Err(CliError::Host(HostError::Config(format!(
                    "{e} -- pass --force to replace it with a fresh template"
                ))));
            }
        };
        // Built *inside* the probe closure, not before it. `command::init`
        // refuses an existing config before it probes -- deliberately, so a
        // user who already has a file does not need a working key and a live
        // model just to be told the file is there -- and constructing the
        // provider up here defeated that from one level out: `rmem init`
        // against an existing file with the key unset blamed the missing key,
        // and setting the key changed the message to the real one. That is the
        // same misdirection the rest of this function exists to remove, one
        // arm short. The closure is only called after the existence check, so
        // the key is only demanded when it is about to be used.
        //
        // `map_err(|e| e.to_string())` collapses the variant into
        // `HostError::Refused`, which is what `command::init`'s probe signature
        // takes -- a closure over a `String`, so it can be tested without a
        // socket. The words survive verbatim, and they are the part that took
        // effort to write.
        // `Ok(..?)` rather than a bare `return`: `?` is what applies
        // `From<HostError>`, and a `return` of the inner result would not
        // compile now that the two error types differ.
        return Ok(command::init(
            config_path,
            force,
            replaced_unparsable,
            &|| {
                let provider = config.provider().map_err(|e| e.to_string())?;
                provider.probe_dimension().map_err(|e| e.to_string())
            },
        )?);
    }

    let config = Config::load(config_path)?;
    let (ruleset, policy, dimension, metric) = (
        config.ruleset()?,
        config.policy_for_engine()?,
        config.provider.dimension,
        config.metric()?,
    );

    // The provider is built inside the two arms that use it, not once above
    // the match. `Config::provider` reads the API key out of the environment
    // and refuses when it is not set, so constructing it unconditionally made
    // `about` and all three `review` subcommands demand a credential none of
    // them ever touches. That is not merely inconvenient: the review band
    // exists so a human answers the question the resolver would not guess at,
    // and answering it is local work over a file on disk.
    // Which of the two brackets the command runs inside, and the only thing
    // that decides it. `store::with_write` holds an exclusive lock across the
    // load, the change and the save; `with_read` takes a shared one and writes
    // nothing. Reading under a lock is not ceremony: without it a `recall` can
    // read the store in the moment another process has replaced it.
    /// A command's model calls, made before the lock and carried into it.
    ///
    /// An enum rather than two `Option`s so the two cannot both be set, and so
    /// the match below pairs each plan with the command it was built from.
    enum Planned {
        Remember(command::RememberPlan),
        Decide(command::DecidePlan),
        Rescope(command::RescopePlan),
    }

    let mutates = matches!(
        command,
        Command::Remember { .. }
            | Command::ReviewConfirm(_)
            | Command::ReviewReject(_)
            | Command::Decide { .. }
            | Command::Rescope { .. }
            | Command::Reindex
    );

    /// Stands in where there is nothing to embed.
    ///
    /// Never called: `plan_reindex` embeds once per text and there are none.
    /// It exists so the empty case does not have to build a provider, which
    /// would demand a credential to do no work.
    struct NoEmbedder;
    impl rm_engine::Embedder for NoEmbedder {
        fn embed(&self, _: &str) -> Result<Vec<f32>, rm_engine::EmbedderError> {
            Err(rm_engine::EmbedderError("nothing to embed".to_string()))
        }
    }

    let path = config.store.path.clone();

    // Its own branch, because it is the one command that reads the store,
    // calls the network, and then writes -- three phases where every other
    // command has two. Folding it into the brackets below would mean either
    // holding a lock across the embeddings or opening the store twice inside
    // one, and both are the thing this file exists to avoid.
    if matches!(command, Command::Reindex) {
        let (r, p2) = (config.ruleset()?, config.policy_for_engine()?);
        let texts = store::with_read(&path, r, p2, dimension, metric, |engine| {
            command::reindex_texts(engine)
        })?;
        // No provider unless there is something to embed. `Config::provider`
        // reads the API key out of the environment and refuses when it is
        // unset, so building it unconditionally made `reindex` demand a
        // credential to do nothing -- the same mistake the two brackets below
        // already carry a comment about avoiding.
        let plan = if texts.is_empty() {
            command::plan_reindex(texts, &NoEmbedder, dimension, metric)?
        } else {
            let embedder = config.embedder()?;
            command::plan_reindex(texts, &embedder, dimension, metric)?
        };
        let (r, p2) = (config.ruleset()?, config.policy_for_engine()?);
        return store::with_write(&path, r, p2, dimension, metric, |engine| {
            command::commit_reindex(engine, plan)
        })
        .map_err(CliError::from);
    }

    // # Every model call happens above the lock, not inside it
    //
    // Both brackets below take a lock that spans a load, a change and a save.
    // What used to happen inside them was an extraction and a set of
    // embeddings -- seconds each, because the model is across a network -- and
    // `Lock::acquire` waits five seconds before refusing. Measured on a live
    // store, that put the ceiling at three concurrent writers: the fourth
    // waited out the bound and was told nothing was written.
    //
    // Nothing about the network calls needed the store. An extraction is a
    // function of the turn and an embedding is a function of its text, so the
    // plan is built here, unlocked, and the closure below is left holding
    // nothing but in-memory work. The one part that genuinely reads the store
    // -- resolving a mention against everything already known -- never leaves
    // the lock, which is why this is a reordering and not a weakening.
    if mutates {
        // Built before the lock is taken, and deliberately not inside a
        // closure that runs under it. `config.provider()` reads the API key
        // out of the environment and refuses when it is unset, which is a
        // failure worth having before a lock is held rather than during.
        let planned = match &command {
            Command::Remember { text, speaker } => {
                let provider = config.provider()?;
                Some(Planned::Remember(command::plan_remember(
                    text,
                    now,
                    "cli",
                    speaker.as_deref(),
                    &provider,
                    &provider,
                    dimension,
                    metric,
                )?))
            }
            Command::Decide {
                title,
                choice,
                scope,
                status,
                decided_at,
                because,
                context,
                supersedes,
            } => {
                // An embedder, not a provider: a decision has a known shape,
                // so it costs embeddings and no completion at all -- and where
                // the embedder is local, no credential and no socket either.
                let embedder = config.embedder()?;
                Some(Planned::Decide(command::plan_decide(
                    title,
                    choice,
                    scope,
                    status.as_deref(),
                    because.as_deref(),
                    context.as_deref(),
                    supersedes.as_deref(),
                    *decided_at,
                    now,
                    "cli",
                    &embedder,
                )?))
            }
            Command::Rescope { title, scope } => {
                // One field, so one embedding. Same bargain as `decide`: the
                // embedder, never a completion provider.
                let embedder = config.embedder()?;
                Some(Planned::Rescope(command::plan_rescope(
                    title, scope, now, "cli", &embedder,
                )?))
            }
            // The review answers write, but they answer a question the
            // resolver already asked. No model is involved either way.
            _ => None,
        };

        store::with_write(&path, ruleset, policy, dimension, metric, |engine| {
            match (command, planned) {
                (Command::Remember { .. }, Some(Planned::Remember(plan))) => {
                    command::commit_remember(engine, plan)
                }
                (Command::Rescope { .. }, Some(Planned::Rescope(plan))) => {
                    command::commit_rescope(engine, plan)
                }
                (Command::Decide { .. }, Some(Planned::Decide(plan))) => {
                    command::commit_decide(engine, plan)
                }
                (Command::ReviewConfirm(id), _) => command::review_confirm(engine, id),
                (Command::ReviewReject(id), _) => command::review_reject(engine, id),
                // Guarded by `mutates` directly above, over the same variants,
                // and the two planned arms are built from those same variants
                // just above -- so a mismatch here is this function having been
                // edited in one place and not the other.
                (other, _) => unreachable!("{other:?} does not write, or was not planned"),
            }
        })
        .map_err(CliError::from)
    } else {
        // Same reordering on the read side. A shared lock lets readers run
        // together, but it still holds a writer off, so a `recall` that
        // embedded its query under the lock made every reader a brake on
        // every writer.
        let weak_below = config.retrieval.weak_below;
        let query_vector = match &command {
            Command::Recall { query, .. } => {
                let embedder = config.embedder()?;
                Some(command::plan_recall(query, &embedder)?)
            }
            _ => None,
        };

        store::with_read(&path, ruleset, policy, dimension, metric, |engine| {
            match (command, query_vector) {
                (Command::Recall { k, .. }, Some(vector)) => {
                    command::commit_recall(engine, vector, k, weak_below)
                }
                (
                    Command::About {
                        entity,
                        attribute,
                        valid_at,
                        as_of,
                    },
                    _,
                ) => command::about(
                    engine,
                    entity,
                    &attribute,
                    valid_at.unwrap_or(now),
                    as_of.unwrap_or(now),
                ),
                (Command::ReviewList, _) => command::review_list(engine),
                // `Timestamp::MAX` rather than `now` for an absent flag, so
                // dropping the flag reads exactly as it did before these
                // commands took a clock. See `At::latest`.
                (
                    Command::Decisions {
                        status,
                        valid_at,
                        as_of,
                        scope,
                        all,
                    },
                    _,
                ) => {
                    // `--all` beats `--scope`, which beats the environment.
                    // `None` is no position, which suspends the rule.
                    // `position` rather than the raw value: an empty
                    // `--scope ""` or `RMEM_SCOPE=` reads as "not configured"
                    // to whoever wrote it and would otherwise be the root
                    // position, where only `*` reaches.
                    let here = rm_host::scope::position(if all {
                        None
                    } else {
                        scope.or_else(|| session_scope.clone())
                    });
                    command::decisions(
                        engine,
                        status.as_deref(),
                        At {
                            valid: valid_at.unwrap_or(Timestamp::MAX),
                            tx: as_of.unwrap_or(Timestamp::MAX),
                        },
                        here.as_deref(),
                    )
                }
                (
                    Command::Decision {
                        title,
                        valid_at,
                        as_of,
                        scope,
                        all,
                    },
                    _,
                ) => {
                    // `position` rather than the raw value: an empty
                    // `--scope ""` or `RMEM_SCOPE=` reads as "not configured"
                    // to whoever wrote it and would otherwise be the root
                    // position, where only `*` reaches.
                    let here = rm_host::scope::position(if all {
                        None
                    } else {
                        scope.or_else(|| session_scope.clone())
                    });
                    command::decision(
                        engine,
                        &title,
                        At {
                            valid: valid_at.unwrap_or(Timestamp::MAX),
                            tx: as_of.unwrap_or(Timestamp::MAX),
                        },
                        here.as_deref(),
                    )
                }
                (Command::Init { .. }, _) => unreachable!("handled above"),
                (other, _) => unreachable!("{other:?} writes, or was not planned"),
            }
        })
        .map_err(CliError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_engine::Believed;
    use rm_host::config::TEMPLATE;
    use rm_host::testing::TempDir;
    use rm_host::HostError;

    /// The name of an environment variable nothing sets.
    ///
    /// The whole point of these tests is what happens when the key is
    /// missing, and the honest way to arrange that is a variable that was
    /// never going to be set rather than `std::env::set_var`, which mutates
    /// process-wide state other tests share.
    const NO_SUCH_VARIABLE: &str = "RMEM_TEST_DEFINITELY_UNSET_API_KEY";

    /// A real `rmem.toml` in `dir`, with its store beside it.
    fn config_in(dir: &TempDir) -> std::path::PathBuf {
        let path = dir.path().join("rmem.toml");
        let store = dir.path().join("memory.json").display().to_string();
        let text = TEMPLATE
            // `{store:?}` rather than the bare path: on Windows it is full of
            // backslashes, and Rust's debug escaping happens to be exactly
            // what a TOML basic string wants.
            .replace("path = \"memory.json\"", &format!("path = {store:?}"))
            .replace(
                "api_key_env = \"OPENAI_API_KEY\"",
                &format!("api_key_env = {NO_SUCH_VARIABLE:?}"),
            );
        std::fs::write(&path, text).unwrap();
        path
    }

    fn go_at(
        config: &std::path::Path,
        args: &[&str],
        session_scope: Option<&str>,
    ) -> Result<Outcome, CliError> {
        run(
            args.iter().map(|s| s.to_string()),
            config,
            1_000,
            session_scope.map(str::to_string),
        )
    }

    fn go(config: &std::path::Path, args: &[&str]) -> Result<Outcome, CliError> {
        run(args.iter().map(|s| s.to_string()), config, 1_000, None)
    }

    #[test]
    fn the_commands_that_never_reach_a_provider_run_without_an_api_key() {
        // Every one of these is local work over a file on disk. `review` in
        // particular is the band where a human answers what the resolver
        // would not guess at, and gating it behind a credential it never
        // uses cuts against the reason the band exists. No single command's
        // test could see this: `review_list` takes an `&Engine` and nothing
        // else, so the coupling only ever existed in dispatch.
        let dir = TempDir::new();
        let config = config_in(&dir);

        for args in [
            vec!["review"],
            vec!["about", "0", "employer"],
            vec!["about", "7", "anything"],
        ] {
            let result = go(&config, &args);
            assert!(
                !matches!(result, Err(CliError::Host(HostError::MissingKey))),
                "{args:?} demanded an API key it never uses: {result:?}"
            );
        }
    }

    #[test]
    fn answering_a_review_that_does_not_exist_refuses_without_an_api_key() {
        // Split from the loop above because it refuses rather than
        // succeeding, and the point is *which* refusal: "no open review with
        // id 3", not "the environment variable ... is not set".
        let dir = TempDir::new();
        let config = config_in(&dir);

        for args in [
            vec!["review", "confirm", "3"],
            vec!["review", "reject", "3"],
        ] {
            let err = go(&config, &args).unwrap_err();
            assert!(
                !matches!(err, CliError::Host(HostError::MissingKey)),
                "{args:?} demanded an API key it never uses: {err}"
            );
            assert!(
                err.to_string().contains('3'),
                "the refusal has to name the id: {err}"
            );
        }
    }

    #[test]
    fn the_commands_that_do_reach_a_provider_still_say_which_variable_is_unset() {
        // The other half: moving construction into the arms must not turn a
        // clear refusal into a mystery further down. `remember` and `recall`
        // are the two that embed, so they are the two that need the key.
        let dir = TempDir::new();
        let config = config_in(&dir);

        for args in [vec!["remember", "I moved"], vec!["recall", "jobs"]] {
            let err = go(&config, &args).unwrap_err();
            assert!(
                matches!(err, CliError::Host(HostError::MissingKey)),
                "{args:?} should have asked for the key: {err}"
            );
            // The field, not the variable it names: the name is a value out
            // of `rmem.toml`, and the likeliest way to get that file wrong is
            // to write the key where the name belongs.
            assert!(err.to_string().contains("api_key_env"), "{err}");
            assert!(!err.to_string().contains(NO_SUCH_VARIABLE), "{err}");
        }
    }

    #[test]
    fn a_refusal_prints_what_was_missing_and_exits_nonzero() {
        // The spec names this test. Exit codes are 0 or 1 and the
        // distinction is not cosmetic: `Believed::Unknown` is the store
        // having been asked and having no opinion, which is an answer, so it
        // exits 0. A refusal is a failure to answer and exits 1 -- and it
        // names what was missing, because the library's own words are the
        // part that took effort to write.
        let dir = TempDir::new();
        let config = config_in(&dir);

        let unknown = go(&config, &["about", "0", "employer"]);
        assert_eq!(unknown, Ok(Outcome::About(Believed::Unknown)));
        assert_eq!(exit_code(&unknown), 0, "a real answer is not a failure");

        let refused = go(&config, &["review", "confirm", "99"]);
        assert_eq!(exit_code(&refused), 1);
        let message = refused.unwrap_err().to_string();
        assert!(message.contains("99"), "it has to name the id: {message}");
        assert!(
            message.len() > 20,
            "a refusal names what was missing: {message}"
        );
    }

    #[test]
    fn a_command_that_changes_nothing_writes_no_store_file() {
        // `mutated` is what decides whether `save` runs, and a read-only
        // command that wrote the store back would turn every `about` into a
        // rewrite of a file it had no business touching.
        let dir = TempDir::new();
        let config = config_in(&dir);
        let store = dir.path().join("memory.json");

        go(&config, &["about", "0", "employer"]).unwrap();
        assert!(!store.exists(), "a read touched the store file");
        go(&config, &["review"]).unwrap();
        assert!(!store.exists(), "a read touched the store file");
    }

    #[test]
    fn init_against_an_existing_config_refuses_the_file_without_asking_for_a_key() {
        // `command::init` refuses an existing config before probing, and
        // `init_refuses_an_existing_config_without_ever_calling_the_probe`
        // pins that -- but it calls `command::init` directly, so it never sees
        // what `run` does on the way in. `run` built the provider first, so
        // `rmem init` on an existing file with the key unset blamed the
        // missing key; setting the key changed the message to the real one.
        // Nothing covered `run(["init"])` at all, which is how one arm was
        // left behind by the fix that removed this everywhere else.
        let dir = TempDir::new();
        let config = config_in(&dir);
        let before = std::fs::read_to_string(&config).unwrap();

        let err = go(&config, &["init"]).unwrap_err();
        assert!(
            !matches!(err, CliError::Host(HostError::MissingKey)),
            "the file is what is in the way, not the key: {err}"
        );
        assert!(err.to_string().contains("--force"), "{err}");
        // And the file it refused to replace is byte-for-byte the file it was.
        assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
    }

    #[test]
    fn init_with_force_does_ask_for_the_key_because_it_is_about_to_probe() {
        // The other side of the ordering: `--force` means the existence check
        // passes, so the probe runs, so the key is genuinely needed. A refusal
        // here is the right one, and it names the field to look at.
        let dir = TempDir::new();
        let config = config_in(&dir);

        let err = go(&config, &["init", "--force"]).unwrap_err();
        assert!(err.to_string().contains("api_key_env"), "{err}");
        assert!(
            !err.to_string().contains("--force"),
            "the file is not what is in the way this time: {err}"
        );
    }

    #[test]
    fn init_without_force_refuses_an_unparsable_config_but_names_force_as_the_way_through() {
        // The regression this guards: before this fix, `run` called
        // `Config::load_or_template(config_path)?` unconditionally, so an
        // unparsable `rmem.toml` blocked `init --force` exactly as hard as
        // plain `init` -- the only escape was deleting the file by hand, and
        // nothing said so. Without `--force` the file still wins here too,
        // but the refusal now names the way through.
        //
        // The awkward fixture: `dimension` wants a `usize` and this hands it
        // a string with a backtick in it, so `toml`'s own message would have
        // quoted it -- the same shape `our_reason` already guards for
        // `Config::parse`'s own refusal. This pins that `run`'s new match arm,
        // which builds a *different* message by appending the `--force`
        // hint, does not reopen that door.
        let dir = TempDir::new();
        let config = dir.path().join("rmem.toml");
        let canary = "CANARY-`-not-a-number";
        let broken = TEMPLATE.replace("dimension = 1536", &format!("dimension = \"{canary}\""));
        std::fs::write(&config, &broken).unwrap();

        let err = go(&config, &["init"]).unwrap_err();
        assert!(
            matches!(err, CliError::Host(HostError::Config(_))),
            "{err:?}"
        );
        assert!(err.to_string().contains("--force"), "{err}");
        // Distinguishes this refusal from `command::init`'s own "already
        // exists, and it may have been edited" refusal, which also mentions
        // `--force` and would otherwise let a regression to the silent
        // fallback hide behind that unrelated check still firing.
        assert!(err.to_string().contains("is not valid"), "{err}");
        assert!(!err.to_string().contains("may have been edited"), "{err}");
        assert!(!err.to_string().contains(canary), "{err}");
        assert!(!err.to_string().contains('`'), "{err}");
        // The file it refused to replace is byte-for-byte the file it was.
        assert_eq!(std::fs::read_to_string(&config).unwrap(), broken);
    }

    #[test]
    fn a_missing_config_says_so_rather_than_asking_for_a_key_first() {
        // Order matters in the message a first-time user sees: they have no
        // `rmem.toml` yet, and being told to set an environment variable
        // first would send them somewhere the problem is not.
        let dir = TempDir::new();
        let err = go(&dir.path().join("rmem.toml"), &["review"]).unwrap_err();
        assert!(
            matches!(err, CliError::Host(HostError::Config(_))),
            "{err:?}"
        );
        assert!(err.to_string().contains("rmem init"), "{err}");
    }

    /// An `RMEM_SCOPE` that is set but empty must read as "not configured",
    /// because that is what it looks like to whoever wrote it.
    ///
    /// It used to be a position, and an empty one splits into a single empty
    /// segment -- the root, where only `*` reaches. On the real 219-decision
    /// store `RMEM_SCOPE=` returned 32 records where unset returned all 219,
    /// and nothing said so. The unit tests on `scope::position` pin the rule;
    /// this pins that dispatch actually calls it.
    #[test]
    fn a_session_scope_that_is_set_but_empty_filters_nothing() {
        let dir = TempDir::new();
        let config = config_in(&dir);
        // Subword hashing, so `decide` opens no socket and needs no key.
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(
            text.contains("embedder = \"http\""),
            "the template's embedder line moved; this rewrite is now silently a no-op"
        );
        std::fs::write(
            &config,
            text.replace("embedder = \"http\"", "embedder = \"local\""),
        )
        .unwrap();

        for (title, scope) in [("Everywhere", "*"), ("Just here", "work/one")] {
            go(&config, &["decide", title, "a choice", "--scope", scope])
                .unwrap_or_else(|e| panic!("recording {title:?}: {e}"));
        }

        let seen = |session: Option<&str>| {
            let Ok(Outcome::Decisions(ds)) = go_at(&config, &["decisions"], session) else {
                panic!("decisions did not return decisions")
            };
            ds.len()
        };

        assert_eq!(seen(None), 2, "no position, no filtering");
        assert_eq!(seen(Some("")), 2, "an empty position is not the root");
        assert_eq!(seen(Some("   ")), 2, "nor is whitespace");
        assert_eq!(seen(Some("work/one")), 2, "both reach work/one");
        // And the rule still bites where a position is genuinely given.
        assert_eq!(seen(Some("elsewhere")), 1, "only the universal one");
    }
}
