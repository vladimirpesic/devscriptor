//! A relationship's `created_at` round-trips through the database.
//!
//! `insert_relationships` stores it through rusqlite's `time` `ToSql`, an RFC
//! 3339 form, which `row_to_relationship` reads; the `time` crate's `Display`
//! form (`to_string()`, e.g. a single-digit hour) is not RFC 3339, and stored,
//! it would fail every relationship read, `call-graph`'s among them. This locks
//! in the store/read format contract.

use devscriptor_core::database::{Database, DatabaseConfig};
use devscriptor_core::parser::parse_content_sync;
use std::path::Path;

#[tokio::test]
async fn relationship_created_at_roundtrips_through_db() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::new(DatabaseConfig {
        path: tmp.path().join("rel_ts.db").to_string_lossy().to_string(),
        ..DatabaseConfig::default()
    })
    .expect("db");

    // An intra-file call yields entities + `Calls`/`CalledBy` relationships whose
    // `created_at` is the real (recent) parse time.
    let pf = parse_content_sync(
        "fn callee() -> u8 { 1 }\nfn caller() -> u8 { callee() }\n",
        "rust",
        Path::new("lib.rs"),
    )
    .expect("parse");
    assert!(
        !pf.relationships.is_empty(),
        "expected parsed relationships"
    );

    db.batch_insert_parsed_files(&[pf.to_record()], None)
        .await
        .expect("insert");

    // Reading must succeed (pre-fix it errored on the malformed timestamp) and
    // preserve the real timestamp — a store/read format mismatch would surface
    // as the epoch (1970) fallback.
    let rels = db
        .get_all_relationships()
        .await
        .expect("get_all_relationships must not fail on a freshly stored timestamp");
    assert!(!rels.is_empty(), "no relationships read back");
    for r in &rels {
        assert!(
            r.created_at.year() >= 2020,
            "created_at did not round-trip (got {:?}); store/read timestamp formats are out of sync",
            r.created_at
        );
    }
}
