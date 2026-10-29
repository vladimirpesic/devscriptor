//! Test fixture server — shared analysis cache across test binaries.
//!
//! This binary runs the multilanguage and semantic-search analyses once,
//! then serves the results to any test binary that connects via a Unix
//! domain socket.  This eliminates duplicate model loading (~760 MB) and
//! re-analysis across test processes.
//!
//! # Usage
//!
//! Start the server (run once, leave running):
//! ```bash
//! cargo test --test fixture_server -- --nocapture
//! ```
//!
//! Then run the integration tests (they will connect to the server):
//! ```bash
//! cargo test --features slow-tests,model-download
//! ```
//!
//! # Why this helps
//!
//! `cargo test` compiles each `#[test]` binary into a separate executable
//! and runs them sequentially when `CARGO_TEST_JOBS=1`.  Each binary that
//! needs embeddings loads its own copy of the model.  The fixture server
//! holds one model in memory and shares results, cutting peak memory by
//! ~760 MB per additional test binary.

mod common;

#[cfg(unix)]
use common::{AnalysisResult, get_multilang_analysis_result, get_semantic_analysis_result};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
#[cfg(unix)]
use tokio::signal;
#[cfg(unix)]
use tokio::sync::RwLock;

#[cfg(unix)]
const SOCKET_PATH: &str = "/tmp/devscriptor_test_fixture.sock";

/// Shared cached analysis results.
#[cfg(unix)]
struct CachedResults {
    multilang: Option<AnalysisResult>,
    semantic: Option<AnalysisResult>,
}

#[cfg(unix)]
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Clean up stale socket
    let _ = tokio::fs::remove_file(SOCKET_PATH).await;

    let cache = Arc::new(RwLock::new(CachedResults {
        multilang: None,
        semantic: None,
    }));

    let listener = UnixListener::bind(SOCKET_PATH).expect("Failed to bind fixture server socket");
    tracing::info!("Fixture server listening on {}", SOCKET_PATH);

    // Spawn the background worker that populates caches on first request
    let cache_worker = Arc::clone(&cache);
    tokio::spawn(async move {
        populate_caches(cache_worker).await;
    });

    // Accept connections until interrupted
    loop {
        tokio::select! {
            Ok((stream, _)) = listener.accept() => {
                let cache = Arc::clone(&cache);
                tokio::spawn(handle_client(stream, cache));
            }
            _ = signal::ctrl_c() => {
                tracing::info!("Fixture server shutting down...");
                break;
            }
        }
    }

    let _ = tokio::fs::remove_file(SOCKET_PATH).await;
}

#[cfg(unix)]
async fn populate_caches(cache: Arc<RwLock<CachedResults>>) {
    tracing::info!("Pre-populating multilang analysis cache...");
    match get_multilang_analysis_result().await {
        Ok(result) => {
            cache.write().await.multilang = Some(result);
            tracing::info!("Multilang cache populated");
        }
        Err(e) => {
            tracing::error!("Failed to populate multilang cache: {}", e);
        }
    }

    tracing::info!("Pre-populating semantic analysis cache...");
    match get_semantic_analysis_result().await {
        Ok(result) => {
            cache.write().await.semantic = Some(result);
            tracing::info!("Semantic cache populated");
        }
        Err(e) => {
            tracing::error!("Failed to populate semantic cache: {}", e);
        }
    }
}

#[cfg(unix)]
async fn handle_client(mut stream: UnixStream, cache: Arc<RwLock<CachedResults>>) {
    let mut buf = Vec::new();
    if let Err(e) = stream.read_to_end(&mut buf).await {
        tracing::warn!("Fixture client read error: {}", e);
        return;
    }

    let req: common::fixture_client::FixtureRequest = match serde_json::from_slice(&buf) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Fixture client deserialization error: {}", e);
            return;
        }
    };

    let response = match req {
        common::fixture_client::FixtureRequest::Multilang => {
            cache.read().await.multilang.clone().map(into_response)
        }
        common::fixture_client::FixtureRequest::Semantic => {
            cache.read().await.semantic.clone().map(into_response)
        }
    }
    .unwrap_or_else(|| common::fixture_client::FixtureResponse {
        success: false,
        status: "not_ready".to_string(),
        files_processed: 0,
        total_entities: 0,
        total_relationships: 0,
        embeddings_generated: 0,
        analysis_time_secs: 0.0,
        db_path: String::new(),
        stats: common::DatabaseStats::default(),
        errors: vec!["Cache not populated yet".to_string()],
    });

    let resp_json = match serde_json::to_vec(&response) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("Fixture response serialization error: {}", e);
            return;
        }
    };

    if let Err(e) = stream.write_all(&resp_json).await {
        tracing::warn!("Fixture client write error: {}", e);
    }
}

#[cfg(unix)]
#[cfg(not(unix))]
fn main() {
    eprintln!("Fixture server is only supported on Unix platforms.");
}

#[cfg(unix)]
fn into_response(result: AnalysisResult) -> common::fixture_client::FixtureResponse {
    common::fixture_client::FixtureResponse {
        success: true,
        status: result.status,
        files_processed: result.files_processed,
        total_entities: result.total_entities,
        total_relationships: result.total_relationships,
        embeddings_generated: result.embeddings_generated,
        analysis_time_secs: result.analysis_time_secs,
        db_path: result.db_path.to_string_lossy().to_string(),
        stats: result.stats,
        errors: result.errors,
    }
}
