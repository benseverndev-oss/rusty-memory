# Config-Relative Store Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve a relative `[store] path` against the directory holding the config file, not against the caller's working directory.

**Architecture:** `Config::parse` already receives the config's own path and currently uses it only for error text. Resolution happens there, once, so `rm-cli`, `rm-mcp` and every later consumer inherit it without changing. `rmem init` additionally writes an absolute path so the question does not arise for new configs.

**Tech Stack:** Rust, `toml`, `serde`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-26-config-relative-store-design.md`

## Global Constraints

- Absolute paths are used unchanged. Only relative paths are re-anchored.
- The not-found error must name **both** the resolved store path and the config that named it. A moved store and an empty store are otherwise indistinguishable, which is the failure this plan exists to prevent.
- `Config::from_template()` has no file to anchor against and keeps today's behaviour. It is used only by `load_or_template` before `init` has written anything, at which point no store is opened.
- Error messages name fields and locations, never values — the existing rule in `config.rs`, which exists because `field = "sk-proj-SECRET"` once printed a credential.

---

### Task 1: Resolve the path in `parse`

**Files:**
- Modify: `crates/rm-host/src/config.rs` (`Config::parse`, around line 440)

**Interfaces:**
- Consumes: `Config::parse(path: &Path, text: &str) -> Result<Config, HostError>`, `Store { path: PathBuf }`
- Produces: no signature change. `config.store.path` is absolute whenever the config came from a file with a parent directory.

- [ ] **Step 1: Write the failing tests**

Add to `config.rs`'s test module, beside the other `Config::load` tests:

```rust
/// A relative store path belongs to the config that names it.
///
/// `RMEM_CONFIG` exists so one config can be shared between projects. A path
/// resolved against the caller's cwd makes that config name a different file
/// for every caller, which is the opposite of sharing it. This silently
/// pointed two stores at one file on 2026-08-26 and the tell was implausibly
/// clean data, not an error.
#[test]
fn a_relative_store_path_resolves_against_the_config_not_the_caller() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("rmem.toml");
    std::fs::write(&config_path, TEMPLATE).unwrap();

    let config = Config::load(&config_path).unwrap();
    assert_eq!(
        config.store.path,
        dir.path().join("memory.json"),
        "the store belongs beside its config, wherever the caller stood"
    );
}

/// Two configs naming the same relative path are two stores, not one.
///
/// This is the property the failed experiment needed: a comparison between
/// two configurations is meaningless if both write to the same file.
#[test]
fn two_configs_naming_one_relative_path_are_two_stores() {
    let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let mut paths = Vec::new();
    for dir in [&a, &b] {
        let p = dir.path().join("rmem.toml");
        std::fs::write(&p, TEMPLATE).unwrap();
        paths.push(Config::load(&p).unwrap().store.path);
    }
    assert_ne!(paths[0], paths[1]);
}

/// An absolute path is already unambiguous and is left alone.
#[test]
fn an_absolute_store_path_is_not_re_anchored() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("rmem.toml");
    let absolute = if cfg!(windows) { "C:/elsewhere/memory.json" } else { "/elsewhere/memory.json" };
    std::fs::write(
        &config_path,
        TEMPLATE.replace(r#"path = "memory.json""#, &format!(r#"path = "{absolute}""#)),
    )
    .unwrap();

    assert_eq!(
        Config::load(&config_path).unwrap().store.path,
        PathBuf::from(absolute)
    );
}
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p rm-host --lib relative_store_path`
Expected: `a_relative_store_path_resolves_against_the_config_not_the_caller` fails, asserting the raw `memory.json` against the anchored path.

Run the other two by name too. `an_absolute_store_path_is_not_re_anchored` should already PASS — it is the guard that the change does not over-reach, not a red test.

- [ ] **Step 3: Resolve in `parse`**

In `Config::parse`, after the `toml::from_str` succeeds:

```rust
fn parse(path: &Path, text: &str) -> Result<Config, HostError> {
    let mut config: Config = toml::from_str(text).map_err(|e| {
        // ... unchanged error construction ...
    })?;

    // A relative store path is relative to the config that names it, not to
    // whoever happened to start the process. `RMEM_CONFIG` exists so one
    // config can be shared between projects; resolving against the caller's
    // cwd makes that config name a different file per caller, which is the
    // opposite of what sharing it was for.
    //
    // `parent()` is `None` only for a path with no directory component at
    // all, which `load` cannot produce because it read a file from it.
    if config.store.path.is_relative() {
        if let Some(dir) = path.parent() {
            config.store.path = dir.join(&config.store.path);
        }
    }

    Ok(config)
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rm-host --lib`
Expected: PASS, including `the_template_this_crate_writes_is_one_it_can_read_back`.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-host/src/config.rs
git commit -m "A relative store path belongs to its config, not to the caller"
```

---

### Task 2: Say so when the store is not where it was

**Files:**
- Modify: `crates/rm-host/src/config.rs` (or wherever the store-open error is raised — locate with `rg 'could not read|no such file' crates/rm-store crates/rm-host`)

**Interfaces:**
- Consumes: `config.store.path` (now absolute), the config path from Task 1.
- Produces: an error naming both paths.

- [ ] **Step 1: Write the failing test**

```rust
/// A moved store and an empty store look identical, so the error has to
/// distinguish them. A user whose data appears to have vanished after an
/// upgrade must be told the resolution rule changed, or they will conclude
/// it is gone.
#[test]
fn a_missing_store_names_both_the_path_and_the_config_that_chose_it() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("rmem.toml");
    std::fs::write(&config_path, TEMPLATE).unwrap();

    let err = open_store_for(&config_path).unwrap_err().to_string();
    assert!(err.contains("memory.json"), "{err}");
    assert!(err.contains("rmem.toml"), "the config that named it: {err}");
}
```

Replace `open_store_for` with the real entry point found above; if opening a
missing store currently creates one rather than erroring, this task narrows to
the README note in Task 3 and the test is dropped — say so in the commit rather
than leaving a test asserting behaviour that does not exist.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rm-host --lib names_both_the_path`
Expected: FAIL — the message names one path.

- [ ] **Step 3: Include the config path in the message**

- [ ] **Step 4: Run the test**

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rm-host/src/config.rs
git commit -m "A store that is not there names the config that chose the path"
```

---

### Task 3: `init` writes an absolute path, and the README says the rule

**Files:**
- Modify: `crates/rm-host/src/config.rs` (`TEMPLATE` and the `init` write path)
- Modify: `README.md` (under "Where a store lives")

- [ ] **Step 1: Write the failing test**

```rust
/// `init` writes a path that needs no rule to interpret.
///
/// Task 1's anchoring is what makes hand-written and older configs behave
/// sensibly. Writing an absolute path is what stops the question arising for
/// configs this crate produces itself.
#[test]
fn init_writes_a_store_path_that_is_absolute() {
    let dir = TempDir::new().unwrap();
    let written = init_config_text(dir.path());
    let config = Config::parse(&dir.path().join("rmem.toml"), &written).unwrap();
    assert!(config.store.path.is_absolute(), "{:?}", config.store.path);
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p rm-host --lib init_writes_a_store_path`
Expected: FAIL — `TEMPLATE` says `memory.json`.

- [ ] **Step 3: Make `init` substitute the absolute path**

`TEMPLATE` keeps `path = "memory.json"` so the committed template stays
readable and portable; `init` substitutes the absolute path as it writes.
Note in the code why the two differ, or a later reader will "fix" one of them.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p rm-host`
Expected: PASS.

- [ ] **Step 5: Document the rule**

Add to `README.md` under "Where a store lives":

```markdown
A relative `[store] path` is resolved against the directory holding the config
file, not against wherever the command was run. This is what lets one
`RMEM_CONFIG` be shared: a path resolved against the caller would name a
different store for every caller, and two stores are not an error, so nothing
would report it. `rmem init` writes an absolute path, so the rule only matters
for configs written by hand or before this was true.
```

- [ ] **Step 6: Run the whole gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
git add crates/rm-host/src/config.rs README.md
git commit -m "init writes an absolute store path, and the README states the rule"
```

---

## Finishing

Use `superpowers:finishing-a-development-branch`. Base branch is `main`.

The pull request states plainly that **existing configs with a relative store path, run from anywhere other than the config's own directory, will point at a different file after this change** — and that such a setup was already resolving to a different store per caller, which is the bug being fixed rather than a regression introduced by it.
