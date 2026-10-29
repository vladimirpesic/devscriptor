//! Concurrent writers must wait for the write lock, not fail.
//!
//! Writes begin with `BEGIN IMMEDIATE`. `BEGIN DEFERRED` (what
//! `Connection::unchecked_transaction` emits) takes a *read* snapshot and only
//! upgrades to a write lock at the first write statement. In WAL mode, if
//! another connection commits in between, that upgrade fails at once with
//! `SQLITE_BUSY_SNAPSHOT` — and SQLite does **not** call the busy handler for
//! it, so `PRAGMA busy_timeout` never applies, and several MCP tool calls
//! writing at once would abort with `database is locked`.
//!
//! `BEGIN IMMEDIATE` takes the write lock up front, where `busy_timeout` does
//! apply. These tests pin that behaviour at the SQLite level, independent of the
//! pipeline, so they stay meaningful even if the calling code moves.

use rusqlite::{Connection, TransactionBehavior};

/// Open a WAL connection with the same pragmas the pool sets.
fn open(path: &std::path::Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA busy_timeout=5000;
         PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;",
    )
    .unwrap();
    conn
}

fn seed(path: &std::path::Path) {
    let conn = open(path);
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);")
        .unwrap();
    conn.execute("INSERT INTO t (v) VALUES ('seed')", [])
        .unwrap();
}

/// The interleaving that ends in `database is locked`:
///
/// 1. writer A opens a deferred transaction and **reads** (taking a snapshot),
/// 2. writer B commits a write,
/// 3. writer A tries to write — its snapshot is stale.
///
/// With `BEGIN DEFERRED` step 3 fails instantly. With `BEGIN IMMEDIATE` the
/// transaction never holds a stale snapshot, so there is nothing to fail on.
#[test]
fn deferred_transaction_fails_on_a_stale_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.db");
    seed(&db);

    let a = open(&db);
    let b = open(&db);

    // A: deferred begin, then read -> snapshot taken here.
    let tx_a = rusqlite::Transaction::new_unchecked(&a, TransactionBehavior::Deferred).unwrap();
    let _: i64 = tx_a
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();

    // B: commit a write, invalidating A's snapshot.
    b.execute("INSERT INTO t (v) VALUES ('b')", []).unwrap();

    // A: upgrade to a write. This is the failure the pipeline hit.
    let err = tx_a
        .execute("INSERT INTO t (v) VALUES ('a')", [])
        .expect_err("deferred upgrade must fail against a stale snapshot");

    assert!(
        matches!(
            err.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        ),
        "expected SQLITE_BUSY(_SNAPSHOT), got {err:?}"
    );
}

/// An immediate transaction takes the write lock at `BEGIN`, so the same
/// interleaving cannot strand it on a stale snapshot.
#[test]
fn immediate_transaction_survives_a_concurrent_commit() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.db");
    seed(&db);

    let b = open(&db);
    // B writes *before* A begins, exactly as in the deferred case.
    b.execute("INSERT INTO t (v) VALUES ('b')", []).unwrap();

    let a = open(&db);
    let tx_a = rusqlite::Transaction::new_unchecked(&a, TransactionBehavior::Immediate).unwrap();
    let _: i64 = tx_a
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();
    tx_a.execute("INSERT INTO t (v) VALUES ('a')", [])
        .expect("immediate transaction must be able to write");
    tx_a.commit().expect("commit succeeds");

    let n: i64 = open(&db)
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3, "seed + b + a");
}

/// `busy_timeout` applies to `BEGIN IMMEDIATE`, so a writer blocked by another
/// writer waits rather than failing. Holding the lock briefly must not error.
#[test]
fn immediate_transaction_waits_for_a_held_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("t.db");
    seed(&db);

    let holder = open(&db);
    let tx_hold =
        rusqlite::Transaction::new_unchecked(&holder, TransactionBehavior::Immediate).unwrap();
    tx_hold
        .execute("INSERT INTO t (v) VALUES ('hold')", [])
        .unwrap();

    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_writer = std::sync::Arc::clone(&done);
    let db_path = db.clone();
    let waiter = std::thread::spawn(move || {
        let c = open(&db_path);
        // Blocks on busy_timeout until the holder commits, then succeeds.
        let tx = rusqlite::Transaction::new_unchecked(&c, TransactionBehavior::Immediate)
            .expect("must wait for the lock, not fail");
        tx.execute("INSERT INTO t (v) VALUES ('waiter')", [])
            .unwrap();
        tx.commit().unwrap();
        done_writer.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        !done.load(std::sync::atomic::Ordering::SeqCst),
        "waiter should still be blocked while the lock is held"
    );

    tx_hold.commit().unwrap();
    waiter.join().unwrap();

    let n: i64 = open(&db)
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 3, "seed + hold + waiter");
}
