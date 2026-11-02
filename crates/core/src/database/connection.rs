//! Connection pool for SQLite

use crate::error::Result;
use parking_lot::Mutex;
use r2d2::{Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use sqlite_vec::sqlite3_vec_init;
use std::collections::HashMap;
use std::sync::{LazyLock, Once};

/// Global cache mapping database paths to their connection pools.
/// Prevents multiple `Database::new()` calls for the same file from
/// creating competing r2d2 pools.
static POOL_CACHE: LazyLock<Mutex<HashMap<String, ConnectionPool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static SQLITE_VEC_INIT: Once = Once::new();

/// Safe C-ABI wrapper for sqlite-vec initialization that matches the
/// `sqlite3_auto_extension` callback signature without transmute.
unsafe extern "C" fn sqlite_vec_auto_extension(
    _db: *mut rusqlite::ffi::sqlite3,
    _pz_err_msg: *mut *mut std::ffi::c_char,
    _p_api: *const rusqlite::ffi::sqlite3_api_routines,
) -> std::ffi::c_int {
    // sqlite-vec's init function takes no arguments in its Rust binding.
    // We invoke it directly and report success.
    unsafe { sqlite3_vec_init() };
    rusqlite::ffi::SQLITE_OK
}

/// Thread-safe connection pool for SQLite database access
///
/// This pool manages a set of database connections that can be reused
/// across multiple threads, reducing connection overhead.
#[derive(Debug)]
pub struct ConnectionPool {
    /// The underlying r2d2 connection pool
    pool: Pool<SqliteConnectionManager>,
}

impl ConnectionPool {
    /// Create a new connection pool
    ///
    /// # Arguments
    ///
    /// * `database_path` - Path to the SQLite database file
    /// * `max_size` - Maximum number of connections in the pool
    ///
    /// # Returns
    ///
    /// A new `ConnectionPool` instance
    ///
    /// # Errors
    ///
    /// Returns an error if the pool cannot be created
    pub fn new(database_path: &str, max_size: u32) -> Result<Self> {
        // A cached pool per path, so concurrent or repeated `Database::new()`
        // calls share one r2d2 pool instead of making several that compete for the
        // same SQLite file. The key is the canonical path, so "devscriptor.db" and
        // "./devscriptor.db" share one.
        let canonical_key = dunce::canonicalize(database_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| database_path.to_string());
        {
            let cache = POOL_CACHE.lock();
            if let Some(pool) = cache.get(&canonical_key) {
                return Ok(pool.clone());
            }
        }

        // Register sqlite-vec extension BEFORE opening any connections.
        // Uses a proper C-ABI wrapper to avoid unsafe transmute between
        // incompatible function-pointer types.
        SQLITE_VEC_INIT.call_once(|| unsafe {
            let result = rusqlite::ffi::sqlite3_auto_extension(Some(sqlite_vec_auto_extension));
            if result != rusqlite::ffi::SQLITE_OK {
                tracing::error!(
                    "sqlite3_auto_extension failed with error code {result}; sqlite-vec extension will not be available"
                );
            } else {
                tracing::info!("sqlite-vec extension auto-extension registered successfully");
            }
        });

        // WAL is a property of the database file, so it is set once, here, on
        // a single connection. Set in `with_init`, every pooled connection
        // tried to switch the mode as r2d2 opened them side by side; on a new
        // database two of them would each hold a shared lock and want an
        // exclusive one, SQLite refused one of them at once ("database is
        // locked", which no busy timeout waits out), and r2d2 logged an error
        // and retried.
        {
            let conn = rusqlite::Connection::open(database_path)?;
            conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL;")?;
        }

        // Every performance-relevant PRAGMA is set in `with_init`, so every pooled
        // connection has it: synchronous, cache_size and temp_store are
        // per-connection settings, and set once they would leave the other
        // connections with FULL sync and the default 2 MB cache.
        let cache_size = super::adaptive_cache_size();
        let manager = SqliteConnectionManager::file(database_path).with_init(move |conn| {
            conn.execute_batch(&format!(
                "PRAGMA busy_timeout=5000;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA cache_size={cache_size};
                 PRAGMA temp_store=FILE;
                 PRAGMA foreign_keys=ON;"
            ))?;

            // Register the REGEXP scalar function so `name REGEXP ?` in
            // search_by_pattern works. The pattern is pre-validated by
            // CodebaseSearcher before it reaches SQLite, so we compile here
            // without extra guards.
            conn.create_scalar_function(
                "regexp",
                2,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8
                    | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
                |ctx| {
                    // Cache the compiled regex as aux data on arg 0 (the pattern).
                    // SQLite reuses aux data across rows whenever the argument is
                    // constant for the statement (a literal or a bound parameter),
                    // so the pattern is compiled ONCE per query instead of once per
                    // row — important when matching across a large `entities` table.
                    let re = if let Some(re) = ctx.get_aux::<regex::Regex>(0)? {
                        re
                    } else {
                        let pattern: String = ctx.get(0)?;
                        let compiled = regex::Regex::new(&pattern).map_err(|e| {
                            rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                                format!("invalid regexp: {e}"),
                            )))
                        })?;
                        ctx.set_aux::<regex::Regex>(0, compiled)?
                    };
                    let text: String = ctx.get(1)?;
                    Ok(re.is_match(&text))
                },
            )
        });
        let pool = Pool::builder().max_size(max_size).build(manager)?;

        // Verify the extension loaded correctly on the first connection.
        // Vector search is a documented capability, so failure to load is a
        // hard error rather than a silent degradation.
        let conn = pool.get()?;
        if let Err(e) = conn.execute_batch("SELECT vec_version();") {
            return Err(crate::error::Error::Database(
                rusqlite::Error::InvalidParameterName(format!(
                    "sqlite-vec extension failed to load; vector search is unavailable: {e}"
                )),
            ));
        }

        let pool = Self { pool };

        // Cache the pool so later `Database::new()` calls for the same path reuse
        // it, under the canonical key.
        POOL_CACHE
            .lock()
            .insert(canonical_key.clone(), pool.clone());

        Ok(pool)
    }

    /// Get a connection from the pool
    ///
    /// # Returns
    ///
    /// A pooled database connection
    ///
    /// # Errors
    ///
    /// Returns an error if no connection is available
    pub fn get(&self) -> Result<PooledConnection<SqliteConnectionManager>> {
        Ok(self.pool.get()?)
    }

    /// Verify that the pool can serve a connection and execute a query.
    ///
    /// Returns `true` if a connection can be acquired and `SELECT 1` succeeds.
    pub fn check_connection(&self) -> bool {
        match self.get() {
            Ok(conn) => conn.execute_batch("SELECT 1;").is_ok(),
            Err(_) => false,
        }
    }
}

impl ConnectionPool {
    /// Remove a database path, canonicalized, from the global pool cache.
    ///
    /// Later `Database::new()` calls for this path then make a new pool: the CLI
    /// `clean` command removes it so that no new connection is opened from the
    /// cache while the database file is being removed.
    pub fn remove_from_cache(database_path: &str) {
        let canonical_key = dunce::canonicalize(database_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| database_path.to_string());
        let mut cache = POOL_CACHE.lock();
        cache.remove(&canonical_key);
        // Also try raw key for DBs that weren't yet created on disk when cached.
        cache.remove(database_path);
    }
}

impl Clone for ConnectionPool {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
        }
    }
}
