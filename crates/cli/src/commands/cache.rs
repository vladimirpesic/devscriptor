//! Cache command implementation

use anyhow::Result;

/// Show cache statistics.
///
/// Devscriptor uses SQLite WAL mode and moka in-process caches that are
/// scoped to the lifetime of each command invocation — there is no persistent
/// cross-process cache to inspect.  This command reports the relevant
/// configuration constants so users understand the cache behaviour.
pub fn stats() -> Result<()> {
    println!("\n{}", "═".repeat(60));
    println!(" Cache Statistics");
    println!("{}", "═".repeat(60));

    println!("\nEmbedding cache (SQLite `embedding_cache` table):");
    println!("  Storage:       persistent (lives in the .db file)");
    println!("  Keyed by:      XXH3 of the entity text");
    println!("  Eviction:      none (grows unbounded; prune with `devscriptor clean`)");

    println!("\nIn-process caches (process-lifetime only):");
    println!("  Parser cache:  moka LRU — one Language handle per grammar");
    println!("  Embedding model: one shared tract plan, run concurrently");
    println!("\nNote: in-process cache counters reset on each command invocation.");

    Ok(())
}
