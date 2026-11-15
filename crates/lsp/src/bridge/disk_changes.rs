//! The changes on disk each language server has yet to hear of.
//!
//! A language server keeps its own view of the files it has not been sent: it
//! read them once, and many servers read them again only when the client says
//! they changed (`workspace/didChangeWatchedFiles`). The files a re-index of the
//! code index finds created, changed or deleted are noted here
//! ([`Translator::note_disk_changes`](super::Translator::note_disk_changes)),
//! and each running server is told of those under its folders before its next
//! request.

use std::collections::HashMap;

/// What happened to a file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskChange {
    /// A file that did not exist.
    Created,
    /// A file whose content changed.
    Changed,
    /// A file that is gone.
    Deleted,
}

impl DiskChange {
    /// What a server that heard of neither must hear of `self` followed by
    /// `next`: one that heard of `self` already hears of `next` alone, so a
    /// deleted file that comes back is created for both.
    fn then(self, next: Self) -> Self {
        match (self, next) {
            (Self::Created, Self::Changed) | (Self::Deleted, Self::Created | Self::Changed) => {
                Self::Created
            }
            (_, next) => next,
        }
    }

    /// The LSP kind of the change.
    pub(crate) fn file_change_type(self) -> lsp_types::FileChangeType {
        match self {
            Self::Created => lsp_types::FileChangeType::CREATED,
            Self::Changed => lsp_types::FileChangeType::CHANGED,
            Self::Deleted => lsp_types::FileChangeType::DELETED,
        }
    }
}

/// Each file's latest change, numbered in the order they were noted, and how
/// far each running server has heard.
#[derive(Debug, Default)]
pub(crate) struct DiskChangeLog {
    /// The number of the last change noted.
    last: u64,
    /// By path: the number of its latest change, and that change merged with
    /// the earlier ones ([`DiskChange::then`]).
    changes: HashMap<String, (u64, DiskChange)>,
    /// By language: the number of the last change its running server heard
    /// of.
    heard: HashMap<String, u64>,
}

impl DiskChangeLog {
    /// Note that `path` was `change`d, for the servers running now.
    pub(crate) fn note(&mut self, path: String, change: DiskChange) {
        if self.heard.is_empty() {
            return;
        }
        self.last += 1;
        let merged = self
            .changes
            .get(&path)
            .map_or(change, |(_, before)| before.then(change));
        self.changes.insert(path, (self.last, merged));
    }

    /// `language`'s server started: it reads the files as they are now.
    pub(crate) fn started(&mut self, language: &str) {
        self.heard.insert(language.to_string(), self.last);
    }

    /// `language`'s server is gone.
    pub(crate) fn stopped(&mut self, language: &str) {
        self.heard.remove(language);
        self.prune();
    }

    /// No server runs.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The changes `language`'s server has not heard of whose paths `under`
    /// takes, sorted by path; the server has heard of every change after.
    /// Nothing when it is not running.
    pub(crate) fn take(
        &mut self,
        language: &str,
        under: impl Fn(&str) -> bool,
    ) -> Vec<(String, DiskChange)> {
        let Some(heard) = self.heard.get_mut(language) else {
            return Vec::new();
        };
        let since = std::mem::replace(heard, self.last);
        let mut changes: Vec<(String, DiskChange)> = self
            .changes
            .iter()
            .filter(|(path, (number, _))| *number > since && under(path))
            .map(|(path, (_, change))| (path.clone(), *change))
            .collect();
        changes.sort_by(|(a, _), (b, _)| a.cmp(b));
        self.prune();
        changes
    }

    /// Forget the changes every running server has heard of.
    fn prune(&mut self) {
        let oldest = self.heard.values().min().copied().unwrap_or(self.last);
        self.changes.retain(|_, (number, _)| *number > oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn everywhere(_: &str) -> bool {
        true
    }

    #[test]
    fn a_server_hears_each_change_once() {
        let mut log = DiskChangeLog::default();
        log.started("python");
        log.note("/p/a.py".into(), DiskChange::Changed);
        assert_eq!(
            log.take("python", everywhere),
            [("/p/a.py".to_string(), DiskChange::Changed)]
        );
        assert!(log.take("python", everywhere).is_empty());
        assert!(log.changes.is_empty(), "heard by every server: forgotten");
    }

    #[test]
    fn a_server_started_later_hears_only_what_followed() {
        let mut log = DiskChangeLog::default();
        log.started("python");
        log.note("/p/a.py".into(), DiskChange::Changed);
        log.started("go");
        log.note("/p/b.go".into(), DiskChange::Created);
        assert_eq!(
            log.take("go", everywhere),
            [("/p/b.go".to_string(), DiskChange::Created)]
        );
        assert_eq!(log.take("python", everywhere).len(), 2);
    }

    #[test]
    fn changes_outside_a_servers_folders_are_not_sent_to_it() {
        let mut log = DiskChangeLog::default();
        log.started("python");
        log.note("/p/a.py".into(), DiskChange::Changed);
        log.note("/q/b.py".into(), DiskChange::Changed);
        assert_eq!(
            log.take("python", |path| path.starts_with("/p/")),
            [("/p/a.py".to_string(), DiskChange::Changed)]
        );
    }

    #[test]
    fn nothing_is_kept_while_no_server_runs() {
        let mut log = DiskChangeLog::default();
        log.note("/p/a.py".into(), DiskChange::Changed);
        assert!(log.changes.is_empty());
        log.started("python");
        log.stopped("python");
        log.note("/p/a.py".into(), DiskChange::Changed);
        assert!(log.changes.is_empty());
        assert!(log.take("python", everywhere).is_empty());
    }

    #[test]
    fn successive_changes_of_a_file_merge() {
        use DiskChange::{Changed, Created, Deleted};
        assert_eq!(Created.then(Changed), Created);
        assert_eq!(Created.then(Deleted), Deleted);
        assert_eq!(Deleted.then(Created), Created);
        assert_eq!(Changed.then(Deleted), Deleted);
        assert_eq!(Changed.then(Changed), Changed);

        let mut log = DiskChangeLog::default();
        log.started("python");
        log.note("/p/a.py".into(), Deleted);
        log.note("/p/a.py".into(), Created);
        log.note("/p/a.py".into(), Changed);
        assert_eq!(
            log.take("python", everywhere),
            [("/p/a.py".to_string(), Created)]
        );
    }
}
