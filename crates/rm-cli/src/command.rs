//! What each command does. Data out; rendering lives in `format`.

use std::path::{Path, PathBuf};

use crate::config::TEMPLATE;
use crate::CliError;

/// What a command did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Initialised { path: PathBuf, dimension: usize },
}

/// Write a config, with the embedding dimension taken from the model.
///
/// `probe` is a closure rather than a provider so this is testable without a
/// socket; the binary passes one that calls `HttpProvider::probe_dimension`.
///
/// Probing before writing is deliberate. Half a config is worse than none: the
/// next command would read it and fail somewhere further from the cause.
pub fn init(
    config_path: &Path,
    force: bool,
    probe: &dyn Fn() -> Result<usize, String>,
) -> Result<Outcome, CliError> {
    if config_path.exists() && !force {
        return Err(CliError::Config(format!(
            "{} already exists, and it may have been edited -- pass --force to replace it",
            config_path.display()
        )));
    }

    let dimension = probe().map_err(CliError::Refused)?;

    // The template's own value is an example. Substituting rather than
    // formatting keeps the file one literal, so the test that parses it is
    // testing the bytes a user receives.
    let contents = TEMPLATE.replace("dimension = 1536", &format!("dimension = {dimension}"));

    std::fs::write(config_path, contents)
        .map_err(|e| CliError::Config(format!("could not write {}: {e}", config_path.display())))?;

    Ok(Outcome::Initialised {
        path: config_path.to_path_buf(),
        dimension,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn init_writes_a_config_whose_dimension_came_from_the_model() {
        // Not from a default and not from the user. A dimension that disagrees
        // with the embedding model makes every distance meaningless, and
        // nothing reports it.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let out = init(&path, false, &|| Ok(1536)).unwrap();

        assert_eq!(
            out,
            Outcome::Initialised {
                path: path.clone(),
                dimension: 1536
            }
        );
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("dimension = 1536"), "{written}");
    }

    #[test]
    fn init_writes_the_dimension_the_probe_reported_not_the_one_in_the_template() {
        // The template carries 1536 as an example. If init copied it verbatim
        // the whole probe would be theatre, and a 3072-dimension model would
        // silently produce a broken store.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        init(&path, false, &|| Ok(3072)).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("dimension = 3072"), "{written}");
        assert!(!written.contains("dimension = 1536"), "{written}");
    }

    #[test]
    fn init_refuses_to_overwrite_an_existing_config() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "# hand-edited, do not lose").unwrap();

        let err = init(&path, false, &|| Ok(1536)).unwrap_err();
        assert!(err.to_string().contains("--force"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# hand-edited, do not lose",
            "the existing file must be untouched"
        );
    }

    #[test]
    fn init_force_overwrites_and_says_it_did() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        std::fs::write(&path, "# old").unwrap();
        init(&path, true, &|| Ok(768)).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("dimension = 768"));
    }

    #[test]
    fn init_writes_nothing_when_the_probe_fails() {
        // Half a config is worse than none: the next command would read it and
        // fail somewhere less obvious.
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        let err = init(&path, false, &|| Err("quota exceeded".to_string())).unwrap_err();
        assert!(err.to_string().contains("quota exceeded"), "{err}");
        assert!(!path.exists(), "no config may be left behind");
    }

    #[test]
    fn what_init_writes_is_what_the_config_loader_reads() {
        let dir = TempDir::new();
        let path = dir.path().join("rmem.toml");
        init(&path, false, &|| Ok(1536)).unwrap();
        let config = crate::config::Config::load(&path).unwrap();
        assert_eq!(config.provider.dimension, 1536);
        config.ruleset().unwrap();
        config.policy_for_engine().unwrap();
    }
}
