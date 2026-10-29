//! Tests for `devscriptor clean`.
//!
//! `clean` renames the database into a temporary directory before deleting it, so
//! that lingering connections see the file vanish atomically rather than racing a
//! "check exists, then remove". That rename only works within a single filesystem:
//! across devices it fails with `EXDEV` ("Invalid cross-device link"). So the
//! trash directory is made beside the database, never in `$TMPDIR`, and a
//! database on another filesystem than `$TMPDIR` — such as the MCP server's
//! default under `~/.local/share/devscriptor/` — can be cleaned too.

use std::path::Path;

use devscriptor_cli::commands::database::clean;

/// Create a database file with its SQLite `-wal` / `-shm` sidecars.
fn make_db(dir: &Path) -> std::path::PathBuf {
    let db = dir.join("test.db");
    std::fs::write(&db, b"not really sqlite, clean() never opens it").unwrap();
    std::fs::write(dir.join("test.db-wal"), b"wal").unwrap();
    std::fs::write(dir.join("test.db-shm"), b"shm").unwrap();
    db
}

fn assert_all_gone(dir: &Path, db: &Path) {
    assert!(!db.exists(), "database survived clean");
    assert!(!dir.join("test.db-wal").exists(), "-wal survived clean");
    assert!(!dir.join("test.db-shm").exists(), "-shm survived clean");
    assert!(
        !dir.join("test.db.lock").exists(),
        "advisory lock file was left behind"
    );
}

#[tokio::test]
async fn clean_removes_database_and_its_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path());

    clean(db.clone()).await.expect("clean succeeds");

    assert_all_gone(dir.path(), &db);
}

/// `clean` must not depend on `$TMPDIR` being usable, because the trash directory
/// has to live on the database's filesystem anyway.
///
/// With `$TMPDIR` pointing at a directory that does not exist, a trash directory
/// made with `tempfile::tempdir()` would fail outright; `tempdir_in(parent)` is
/// unaffected. This checks the rule on every platform, without two filesystems.
#[tokio::test]
async fn clean_ignores_tmpdir() {
    let dir = tempfile::tempdir().unwrap();
    let db = make_db(dir.path());

    let bogus = dir.path().join("no-such-tmpdir");
    let previous = std::env::var_os("TMPDIR");

    // SAFETY: single-threaded within this test, and no other test in this binary
    // reads `TMPDIR` — nor does `clean`, which is what this asserts.
    unsafe { std::env::set_var("TMPDIR", &bogus) };
    let result = clean(db.clone()).await;
    match previous {
        Some(v) => unsafe { std::env::set_var("TMPDIR", v) },
        None => unsafe { std::env::remove_var("TMPDIR") },
    }

    result.expect("clean must not require a usable $TMPDIR");
    assert_all_gone(dir.path(), &db);
}

/// A true cross-device reproduction: put the database on `$HOME`'s filesystem
/// while `$TMPDIR` stays on `/tmp`. Skipped when both share a device, which is the
/// case on many CI images.
#[tokio::test]
async fn clean_works_across_filesystems() {
    let Some(home) = dirs::home_dir() else {
        eprintln!("skipping: no home directory");
        return;
    };
    let Ok(home_dir) = tempfile::tempdir_in(&home) else {
        eprintln!(
            "skipping: cannot create a temp dir under {}",
            home.display()
        );
        return;
    };
    let Ok(tmp_dir) = tempfile::TempDir::new() else {
        eprintln!("skipping: cannot create a temp dir in $TMPDIR");
        return;
    };

    if device_of(home_dir.path()) == device_of(tmp_dir.path()) {
        eprintln!("skipping: $HOME and $TMPDIR share a filesystem");
        return;
    }

    let db = make_db(home_dir.path());
    clean(db.clone())
        .await
        .expect("clean must work when the database is not on the $TMPDIR filesystem");
    assert_all_gone(home_dir.path(), &db);
}

#[cfg(unix)]
fn device_of(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map(|m| m.dev()).unwrap_or(0)
}

#[cfg(not(unix))]
fn device_of(_path: &Path) -> u64 {
    0
}

/// Cleaning a database that was never created must succeed: the sidecars and the
/// lock file still get tidied up, and there is nothing to rename.
#[tokio::test]
async fn clean_is_idempotent_on_a_missing_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("test.db");

    clean(db.clone())
        .await
        .expect("clean on a missing database");
    assert!(!db.exists());
}
