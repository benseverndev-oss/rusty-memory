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
use crate::command::{self, Outcome};
use crate::config::Config;
use crate::{store, CliError};

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
) -> Result<Outcome, CliError> {
    let command = parse(args)?;

    if let Command::Init { force } = command {
        // Loaded here rather than by `init`, because init is what a user runs
        // when there is no config -- so only the provider block has to be
        // readable, and it comes from the template if the file is absent.
        // `?` here matters: a file that exists and fails to parse must
        // surface that, not be treated as if it were absent and silently
        // replaced by the template's defaults.
        let config = Config::load_or_template(config_path)?;
        let provider = config.provider()?;
        return command::init(config_path, force, &|| {
            provider.probe_dimension().map_err(|e| e.to_string())
        });
    }

    let config = Config::load(config_path)?;
    let mut engine = store::load(
        &config.store.path,
        config.ruleset()?,
        config.policy_for_engine()?,
        config.provider.dimension,
        config.metric()?,
    )?;

    // The provider is built inside the two arms that use it, not once above
    // the match. `Config::provider` reads the API key out of the environment
    // and refuses when it is not set, so constructing it unconditionally made
    // `about` and all three `review` subcommands demand a credential none of
    // them ever touches. That is not merely inconvenient: the review band
    // exists so a human answers the question the resolver would not guess at,
    // and answering it is local work over a file on disk.
    let (outcome, mutated) = match command {
        Command::Init { .. } => unreachable!("handled above"),
        Command::Remember { text } => {
            let provider = config.provider()?;
            (
                command::remember(&mut engine, &text, now, "cli", &provider, &provider)?,
                true,
            )
        }
        Command::Recall { query, k } => {
            let provider = config.provider()?;
            (command::recall(&engine, &query, k, &provider)?, false)
        }
        Command::About { entity, attribute } => (
            command::about(&engine, entity, &attribute, now, now)?,
            false,
        ),
        Command::ReviewList => (command::review_list(&engine)?, false),
        Command::ReviewConfirm(id) => (command::review_confirm(&mut engine, id)?, true),
        Command::ReviewReject(id) => (command::review_reject(&mut engine, id)?, true),
    };

    if mutated {
        store::save(&config.store.path, &engine)?;
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TEMPLATE;
    use crate::testing::TempDir;
    use rm_engine::Believed;

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

    fn go(config: &std::path::Path, args: &[&str]) -> Result<Outcome, CliError> {
        run(args.iter().map(|s| s.to_string()), config, 1_000)
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
                !matches!(result, Err(CliError::MissingKey)),
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
                !matches!(err, CliError::MissingKey),
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
                matches!(err, CliError::MissingKey),
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
    fn a_missing_config_says_so_rather_than_asking_for_a_key_first() {
        // Order matters in the message a first-time user sees: they have no
        // `rmem.toml` yet, and being told to set an environment variable
        // first would send them somewhere the problem is not.
        let dir = TempDir::new();
        let err = go(&dir.path().join("rmem.toml"), &["review"]).unwrap_err();
        assert!(matches!(err, CliError::Config(_)), "{err:?}");
        assert!(err.to_string().contains("rmem init"), "{err}");
    }
}
