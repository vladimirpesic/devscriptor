//! Which files this process re-indexed, and when, so that an answer read from
//! the index can tell whether one of its files was re-indexed while it was
//! being computed.
//!
//! An answer that read a file's entities just before a re-index of the file
//! committed, and checked the file against the disk just after, finds the file
//! current and has nothing to refresh, yet holds its old code. Several agents
//! asking one server at once hit that window easily: a semantic search takes
//! hundreds of milliseconds, and another answer's freshness check or the
//! watcher re-indexes meanwhile.

use std::collections::HashMap;

/// How many paths the log keeps before it forgets the older runs.
const LIMIT: usize = 65_536;

/// The record behind [`Database::index_epoch`](super::Database::index_epoch)
/// and [`Database::reindexed_since`](super::Database::reindexed_since).
#[derive(Debug, Default)]
pub(crate) struct ReindexLog {
    /// How many re-index runs have finished.
    epoch: u64,
    /// Runs up to this epoch are forgotten: the log was cleared to stay
    /// bounded.
    floor: u64,
    /// The epoch of the run that last re-indexed each path.
    by_path: HashMap<String, u64>,
}

impl ReindexLog {
    /// How many re-index runs have finished.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Record a finished run over `paths`.
    pub(crate) fn record(&mut self, paths: impl IntoIterator<Item = String>) {
        self.epoch += 1;
        if self.by_path.len() >= LIMIT {
            self.by_path.clear();
            self.floor = self.epoch - 1;
        }
        for path in paths {
            self.by_path.insert(path, self.epoch);
        }
    }

    /// Of `paths`, those that a run finished after `epoch` re-indexed; all
    /// of them when the log no longer reaches back to `epoch`.
    pub(crate) fn since(&self, epoch: u64, paths: &[String]) -> Vec<String> {
        if epoch < self.floor {
            return paths.to_vec();
        }
        paths
            .iter()
            .filter(|path| self.by_path.get(*path).is_some_and(|&run| run > epoch))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_path_counts_from_the_run_after_the_epoch_taken() {
        let mut log = ReindexLog::default();
        log.record(paths(&["/p/a.py"]));
        let epoch = log.epoch();
        assert!(log.since(epoch, &paths(&["/p/a.py"])).is_empty());

        log.record(paths(&["/p/b.py"]));
        assert_eq!(
            log.since(epoch, &paths(&["/p/a.py", "/p/b.py"])),
            ["/p/b.py"]
        );
        assert_eq!(log.since(0, &paths(&["/p/a.py", "/p/c.py"])), ["/p/a.py"]);
    }

    #[test]
    fn past_its_limit_the_log_answers_conservatively() {
        let mut log = ReindexLog::default();
        log.record((0..LIMIT).map(|i| format!("/p/{i}.py")));
        let early = log.epoch() - 1;
        let late = log.epoch();
        log.record(paths(&["/p/new.py"]));
        // The first run is forgotten: anything may have changed since before it.
        assert_eq!(log.since(early, &paths(&["/p/0.py"])), ["/p/0.py"]);
        // From the last remembered epoch on, it still knows.
        assert!(log.since(late, &paths(&["/p/0.py"])).is_empty());
        assert_eq!(log.since(late, &paths(&["/p/new.py"])), ["/p/new.py"]);
    }
}
