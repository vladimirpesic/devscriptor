//! Client for the Devscriptor test fixture server.
//!
//! When the fixture server is running, tests can fetch cached analysis
//! results instead of re-running the full pipeline.  This eliminates
//! duplicate model loading and analysis across test binaries.
//!
//! To start the server:
//!   cargo test --test fixture_server -- --nocapture
//!
//! The server is opt-in; if it is not running, tests silently fall back
//! to local analysis.

use std::path::PathBuf;
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tokio::time::{Duration, timeout};

#[cfg(unix)]
const SOCKET_PATH: &str = "/tmp/devscriptor_test_fixture.sock";
#[cfg(unix)]
const CONNECT_TIMEOUT_MS: u64 = 500;

/// Request types supported by the fixture server.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum FixtureRequest {
    Multilang,
    Semantic,
}

/// Response from the fixture server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FixtureResponse {
    pub success: bool,
    pub status: String,
    pub files_processed: usize,
    pub total_entities: usize,
    pub total_relationships: usize,
    pub embeddings_generated: usize,
    pub analysis_time_secs: f64,
    pub db_path: String,
    pub stats: super::DatabaseStats,
    pub errors: Vec<String>,
}

impl FixtureResponse {
    /// Convert into the full [`AnalysisResult`] used by tests.
    /// `embedding_service` is always `None` because it cannot be shared
    /// across processes.
    pub fn into_analysis_result(self) -> super::AnalysisResult {
        super::AnalysisResult {
            status: self.status,
            files_processed: self.files_processed,
            total_entities: self.total_entities,
            total_relationships: self.total_relationships,
            embeddings_generated: self.embeddings_generated,
            analysis_time_secs: self.analysis_time_secs,
            db_path: PathBuf::from(self.db_path),
            stats: self.stats,
            errors: self.errors,
            embedding_service: None,
        }
    }
}

/// Try to fetch a multilang analysis result from the fixture server.
/// Returns `None` if the server is not running or the request fails.
pub async fn try_fetch_multilang() -> Option<super::AnalysisResult> {
    #[cfg(unix)]
    {
        let response = send_request(FixtureRequest::Multilang).await.ok()?;
        if !response.success {
            return None;
        }
        Some(response.into_analysis_result())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Try to fetch a semantic-search analysis result from the fixture server.
/// Returns `None` if the server is not running or the request fails.
pub async fn try_fetch_semantic() -> Option<super::AnalysisResult> {
    #[cfg(unix)]
    {
        let response = send_request(FixtureRequest::Semantic).await.ok()?;
        if !response.success {
            return None;
        }
        Some(response.into_analysis_result())
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
async fn send_request(req: FixtureRequest) -> Result<FixtureResponse, Box<dyn std::error::Error>> {
    let stream = timeout(
        Duration::from_millis(CONNECT_TIMEOUT_MS),
        UnixStream::connect(SOCKET_PATH),
    )
    .await??;

    let mut stream = stream;
    let req_json = serde_json::to_vec(&req)?;
    stream.write_all(&req_json).await?;
    stream.shutdown().await?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;

    let resp: FixtureResponse = serde_json::from_slice(&buf)?;
    Ok(resp)
}
