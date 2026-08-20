//! Two real processes over one store.
//!
//! The unit tests in `store` take their locks from separate file descriptors,
//! which on Unix is the same mechanism as two processes — `flock` binds to the
//! open file description, not the process. That is an argument, though, and the
//! claim this change actually makes is about a `rmem` invocation meeting a
//! running `rmem-mcp`. So this spawns a second process and checks it.
//!
//! The child is this same test binary, re-invoked to run an `#[ignore]`d test
//! that holds the lock and waits. No fixture binary to build, and the child
//! takes its lock through the standard library rather than through `rm_host`,
//! so nothing here can pass by both sides sharing a mistake.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use rm_host::store;

/// Set by the parent to tell the child which store to lock.
const PATH_VAR: &str = "RM_HOST_LOCK_TEST_PATH";

/// The child. Ignored so a normal run never executes it.
///
/// Takes the lock the way any other program would — `lock_path` and the
/// standard library — announces that it has it, and then holds it until it is
/// killed.
#[test]
#[ignore = "helper process, spawned by the test below"]
fn lock_holder_child() {
    let path = PathBuf::from(std::env::var(PATH_VAR).expect("parent sets the path"));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(store::lock_path(&path))
        .expect("open the sidecar");
    lock.lock().expect("take the lock");
    println!("HELD");
    // Long enough for the parent to finish; the parent kills this first.
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[test]
fn a_second_process_holding_the_lock_is_refused_rather_than_ignored() {
    let dir = tempdir();
    let path = dir.join("memory.json");

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_holder_child", "--ignored", "--nocapture"])
        .env(PATH_VAR, &path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the holder");

    // Wait for the child to actually hold it, rather than racing it.
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            out.read_line(&mut line).expect("read the child") > 0,
            "the holder exited before taking the lock"
        );
        if line.trim() == "HELD" {
            break;
        }
    }

    let refusal = store::with_write(
        &path,
        rm_host::config::Config::from_template().ruleset().unwrap(),
        rm_host::config::Config::from_template()
            .policy_for_engine()
            .unwrap(),
        3,
        rm_engine::Metric::Cosine,
        |_| Ok(()),
    );

    let _ = child.kill();
    let _ = child.wait();

    let err = refusal.expect_err("wrote to a store another process was holding");
    let text = err.to_string();
    assert!(text.contains("another process is using"), "{text}");
    assert!(text.contains("Nothing was written"), "{text}");
    assert!(
        !path.exists(),
        "a refused write must leave the store untouched"
    );

    // And once the holder is gone the lock is free again -- no stale file to
    // clean up, which is the whole reason this is an OS lock and not a
    // create-and-delete lock file.
    let mut freed = false;
    for _ in 0..50 {
        if store::with_write(
            &path,
            rm_host::config::Config::from_template().ruleset().unwrap(),
            rm_host::config::Config::from_template()
                .policy_for_engine()
                .unwrap(),
            3,
            rm_engine::Metric::Cosine,
            |_| Ok(()),
        )
        .is_ok()
        {
            freed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(freed, "the lock was not released when the holder died");
}

/// A directory of our own, without a dependency for it.
fn tempdir() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "rm-host-lock-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}
