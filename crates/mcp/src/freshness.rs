//! Read-your-writes for answers read from the index.
//!
//! The watcher keeps the index current, but an answer can still be older than
//! a file: within the watcher's debounce, before its re-index finishes, while
//! it polls or after it failed, and always with `--no-watch`. So before a query
//! tool answers, the files its answer names are compared with the scan
//! manifest, and those that changed on disk are re-indexed, within a budget.
//! What the budget does not cover is named in the answer as stale rather than
//! passed off as current. An answer computed while one of its files was
//! re-indexed, by another answer's check or by the watcher, is computed again
//! ([`fresh_answer`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{Value, json};
use tracing::{debug, warn};

use crate::server::DevscriptorServerState;

/// The environment variable that sets how long, in milliseconds, an answer
/// waits for its files to be re-indexed.
pub const BUDGET_ENV_VAR: &str = "DEVSCRIPTOR_FRESHNESS_BUDGET_MS";

/// How long an answer waits for its files to be re-indexed when
/// [`BUDGET_ENV_VAR`] is not set.
pub const DEFAULT_BUDGET: Duration = Duration::from_millis(250);

/// The most files one answer re-indexes. Changed files past it are reported
/// stale.
pub const MAX_FILES: usize = 20;

/// How many times [`fresh_answer`] computes an answer again before it reports
/// the files that keep changing under it as stale.
pub const MAX_RERUNS: usize = 2;

/// The budget [`BUDGET_ENV_VAR`] sets, or [`DEFAULT_BUDGET`] when it is unset
/// or not a number of milliseconds.
#[must_use]
pub fn budget_from_env() -> Duration {
    match std::env::var(BUDGET_ENV_VAR) {
        Ok(value) => value.trim().parse().map_or_else(
            |_| {
                warn!(
                    "{BUDGET_ENV_VAR}={value:?} is not a number of milliseconds; using {} ms",
                    DEFAULT_BUDGET.as_millis()
                );
                DEFAULT_BUDGET
            },
            Duration::from_millis,
        ),
        Err(_) => DEFAULT_BUDGET,
    }
}

/// What [`ensure_fresh`] did with the files an answer named.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FreshnessReport {
    /// Files that changed on disk, or were deleted, and were re-indexed: the
    /// index now holds what is on disk.
    pub refreshed: Vec<String>,
    /// Files that changed on disk but were not re-indexed: over the budget,
    /// outside every scanned root, excluded by the scan rules, or not
    /// parsable. An answer may be older than these files.
    pub stale: Vec<String>,
}

impl FreshnessReport {
    /// Whether the index held every file as it is on disk.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.refreshed.is_empty() && self.stale.is_empty()
    }

    /// Add the files `refreshed` names to [`Self::refreshed`], once each.
    fn add_refreshed(&mut self, refreshed: Vec<String>) {
        for path in refreshed {
            if !self.refreshed.contains(&path) {
                self.refreshed.push(path);
            }
        }
    }

    /// Add `refreshed_files` and `stale_files` to an answer, each only when
    /// it names a file. An answer that is not a JSON object is left alone.
    pub fn annotate(&self, answer: &mut Value) {
        let Some(answer) = answer.as_object_mut() else {
            return;
        };
        if !self.refreshed.is_empty() {
            answer.insert("refreshed_files".into(), json!(self.refreshed));
        }
        if !self.stale.is_empty() {
            answer.insert("stale_files".into(), json!(self.stale));
        }
    }
}

/// The distinct values of every `file_path` field in `answer`, at any depth,
/// in the order they appear: the files a query tool's answer was read from.
#[must_use]
pub fn named_files(answer: &Value) -> Vec<String> {
    fn collect<'a>(value: &'a Value, seen: &mut HashSet<&'a str>, files: &mut Vec<String>) {
        match value {
            Value::Object(fields) => {
                for (key, field) in fields {
                    match field {
                        Value::String(path) if key == "file_path" => {
                            if seen.insert(path) {
                                files.push(path.clone());
                            }
                        }
                        _ => collect(field, seen, files),
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect(item, seen, files);
                }
            }
            _ => {}
        }
    }
    let mut files = Vec::new();
    collect(answer, &mut HashSet::new(), &mut files);
    files
}

/// Compute an answer from the index that is current for the files it names.
///
/// `query` reads the answer and names the files it was read from. Those are
/// checked against the disk ([`ensure_fresh`]); the answer is computed again
/// when any of them was re-indexed after it began to be read, whether by that
/// check or by a concurrent one (another answer's, the watcher's), and the
/// files of the new answer are checked in turn. A check finds a file current
/// once the concurrent re-index is done, though the answer read it before:
/// several agents asking one server at once would be shown code they had just
/// replaced. Once the answer has been computed again
/// [`MAX_RERUNS`] times, the files still re-indexed under it are reported
/// stale. Re-indexes in other processes are seen only by the check.
///
/// The report names every file re-indexed for the answer and the files the
/// final answer may be older than.
///
/// # Errors
/// Returns the error of `query`.
pub async fn fresh_answer<T, E, Fut>(
    state: &DevscriptorServerState,
    mut query: impl FnMut() -> Fut,
) -> Result<(T, FreshnessReport), E>
where
    Fut: Future<Output = Result<(T, Vec<String>), E>>,
{
    let database = state.database();
    let mut began = database.index_epoch();
    let (mut answer, mut files) = query().await?;
    let mut report = FreshnessReport::default();
    for run in 0..=MAX_RERUNS {
        let checked = ensure_fresh(state, &files).await;
        report.add_refreshed(checked.refreshed);
        report.stale = checked.stale;
        let raced = database.reindexed_since(began, &files);
        if raced.is_empty() {
            break;
        }
        if run == MAX_RERUNS {
            for path in raced {
                if !report.stale.contains(&path) {
                    report.stale.push(path);
                }
            }
            break;
        }
        began = database.index_epoch();
        (answer, files) = query().await?;
    }
    Ok((answer, report))
}

/// Make the index hold `paths` as they are on disk, as far as the budget
/// allows, before an answer that names them is given.
///
/// Each file is compared with its scan manifest row by size and modification
/// time. A changed or deleted file is re-indexed, and so is a file the index
/// has never seen that a scan would index; each under the deepest scanned root
/// that contains it, the root the watcher uses. At most [`MAX_FILES`] are
/// re-indexed, through the server's structural pipeline
/// ([`DevscriptorServerState::structural_pipeline`]), and the answer waits for
/// them at most [`DevscriptorServerState::freshness_budget`]. A re-index that
/// takes longer goes on in the background, and its files are reported stale.
///
/// A file outside every scanned root, or that the scan rules exclude, is
/// never re-indexed: it is reported stale when the index holds an older
/// version of it, and not at all when the index never held it. A failure to
/// check is logged and reports nothing; the answer stands either way.
pub async fn ensure_fresh(state: &DevscriptorServerState, paths: &[String]) -> FreshnessReport {
    let mut seen = HashSet::new();
    let paths: Vec<String> = paths
        .iter()
        .filter(|path| Path::new(path.as_str()).is_absolute() && seen.insert(path.as_str()))
        .cloned()
        .collect();
    if paths.is_empty() {
        return FreshnessReport::default();
    }
    match check(state, &paths).await {
        Ok(mut report) => {
            // Stale files come up at several steps: list them in the order
            // the answer names them, as the refreshed ones are.
            let order: HashMap<&str, usize> = paths
                .iter()
                .enumerate()
                .map(|(index, path)| (path.as_str(), index))
                .collect();
            report
                .stale
                .sort_by_key(|path| order.get(path.as_str()).copied());
            report
        }
        Err(e) => {
            warn!("Could not check the files of an answer against the disk: {e}");
            FreshnessReport::default()
        }
    }
}

/// A file's size and modification time (whole seconds), as the scan records
/// them; `None` when it is not a file on disk.
type DiskState = Option<(u64, u64)>;

/// A scan manifest row: content hash, size and modification time.
type ManifestRow = (String, u64, u64);

async fn check(
    state: &DevscriptorServerState,
    paths: &[String],
) -> anyhow::Result<FreshnessReport> {
    let database = state.database();
    let before = manifest_rows(state, paths).await?;
    let disk = disk_states(paths.to_vec()).await?;
    let changed: Vec<&String> = paths
        .iter()
        .filter(|path| {
            let on_disk = disk.get(*path).copied().flatten();
            match before.get(*path) {
                Some((_, size, mtime)) => on_disk != Some((*size, *mtime)),
                // Never indexed: worth a look only if it is a file.
                None => on_disk.is_some(),
            }
        })
        .collect();
    if changed.is_empty() {
        return Ok(FreshnessReport::default());
    }

    let mut report = FreshnessReport::default();
    let roots = database.indexed_roots().await?;
    let pipeline = state.structural_pipeline().await?;
    // The files to re-index, by root in the order the roots come up.
    let mut groups: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    let mut attempted: Vec<String> = Vec::new();
    for path in changed {
        let known = before.contains_key(path);
        let Some(root) = deepest_root(&roots, Path::new(path)) else {
            // Outside every scanned root: never re-indexed.
            if known {
                report.stale.push(path.clone());
            }
            continue;
        };
        if !known && !scan_would_index(&pipeline, root, Path::new(path)).await? {
            continue;
        }
        if attempted.len() == MAX_FILES {
            report.stale.push(path.clone());
            continue;
        }
        attempted.push(path.clone());
        match groups.iter_mut().find(|(group_root, _)| group_root == root) {
            Some((_, files)) => files.push(PathBuf::from(path)),
            None => groups.push((root.clone(), vec![PathBuf::from(path)])),
        }
    }
    if attempted.is_empty() {
        return Ok(report);
    }

    debug!(
        "Re-indexing {} changed files for an answer",
        attempted.len()
    );
    let task = tokio::spawn(async move {
        for (root, files) in groups {
            if let Err(e) = pipeline.reindex_paths(&root, &files).await {
                warn!(
                    "Re-indexing {} changed files under {} failed: {e}",
                    files.len(),
                    root.display()
                );
            }
        }
    });
    match tokio::time::timeout(state.freshness_budget(), task).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            warn!("Re-indexing the changed files of an answer stopped: {e}");
            report.stale.extend(attempted);
            return Ok(report);
        }
        Err(_) => {
            // The re-index goes on in the background; the next answer finds
            // its files current.
            report.stale.extend(attempted);
            return Ok(report);
        }
    }

    // What the index holds now, against what is on disk now.
    let after = manifest_rows(state, &attempted).await?;
    let disk = disk_states(attempted.clone()).await?;
    for path in attempted {
        let row = after.get(&path);
        let current = match (row, disk.get(&path).copied().flatten()) {
            (None, None) => true,
            (Some((_, size, mtime)), Some(on_disk)) => (*size, *mtime) == on_disk,
            _ => false,
        };
        let hash_before = before.get(&path).map(|(hash, ..)| hash);
        if !current {
            report.stale.push(path);
        } else if row.map(|(hash, ..)| hash) != hash_before {
            report.refreshed.push(path);
        }
    }
    Ok(report)
}

/// The scan manifest rows of `paths`, by path.
async fn manifest_rows(
    state: &DevscriptorServerState,
    paths: &[String],
) -> anyhow::Result<HashMap<String, ManifestRow>> {
    Ok(state
        .database()
        .get_scan_manifest_entries(paths)
        .await?
        .into_iter()
        .map(|entry| {
            (
                entry.path,
                (entry.content_hash, entry.size_bytes, entry.mtime_secs),
            )
        })
        .collect())
}

/// The state of each of `paths` on disk, by path.
async fn disk_states(paths: Vec<String>) -> anyhow::Result<HashMap<String, DiskState>> {
    Ok(tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|path| {
                let state = std::fs::metadata(&path)
                    .ok()
                    .filter(std::fs::Metadata::is_file)
                    .map(|metadata| {
                        let mtime = metadata
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .map_or(0, |since| since.as_secs());
                        (metadata.len(), mtime)
                    });
                (path, state)
            })
            .collect()
    })
    .await?)
}

/// The deepest of `roots` that contains `path`.
fn deepest_root<'a>(roots: &'a [PathBuf], path: &Path) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root) && path != root.as_path())
        .max_by_key(|root| root.components().count())
}

/// Whether a scan of `root` would index `path`.
async fn scan_would_index(
    pipeline: &devscriptor_core::pipeline::CodebasePipeline,
    root: &Path,
    path: &Path,
) -> anyhow::Result<bool> {
    let scanner = pipeline.scanner();
    let (root, path) = (root.to_path_buf(), path.to_path_buf());
    Ok(
        tokio::task::spawn_blocking(move || !scanner.entries_for(&root, &[path]).is_empty())
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_files_are_distinct_and_in_answer_order() {
        let answer = json!({
            "functions": [
                {"name": "a", "file_path": "/p/b.py"},
                {"name": "b", "file_path": "/p/a.py"},
                {"name": "c", "file_path": "/p/b.py"}
            ],
            "file_path": "/p/c.py",
            "nested": {"code_pointers": [{"file_path": "/p/d.py"}]},
            "not_a_path": {"file_path": 3}
        });
        let files = named_files(&answer);
        let mut sorted = files.clone();
        sorted.sort();
        assert_eq!(sorted, ["/p/a.py", "/p/b.py", "/p/c.py", "/p/d.py"]);
        let position = |file: &str| files.iter().position(|f| f == file);
        assert!(position("/p/b.py") < position("/p/a.py"), "{files:?}");
    }

    #[test]
    fn a_report_adds_only_what_it_names() {
        let mut answer = json!({"results": []});
        FreshnessReport::default().annotate(&mut answer);
        assert_eq!(answer, json!({"results": []}));

        let report = FreshnessReport {
            refreshed: vec!["/p/a.py".into()],
            stale: Vec::new(),
        };
        report.annotate(&mut answer);
        assert_eq!(
            answer,
            json!({"results": [], "refreshed_files": ["/p/a.py"]})
        );
    }

    #[test]
    fn the_deepest_containing_root_is_chosen() {
        let roots = [PathBuf::from("/r"), PathBuf::from("/r/service")];
        assert_eq!(
            deepest_root(&roots, Path::new("/r/service/app.py")),
            Some(&roots[1])
        );
        assert_eq!(
            deepest_root(&roots, Path::new("/r/lib.py")),
            Some(&roots[0])
        );
        assert_eq!(deepest_root(&roots, Path::new("/elsewhere/x.py")), None);
        assert_eq!(deepest_root(&roots, Path::new("/r")), None);
    }
}
