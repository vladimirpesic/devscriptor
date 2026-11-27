//! What a change to the code touches: the entities a diff changes, the
//! entities that depend on them, and the tests among both.
//!
//! The change is what git reports between a revision and the working tree,
//! untracked files included, or between two revisions: each file's changed
//! lines, read from a diff without context lines. A changed line belongs to
//! the innermost entity whose lines hold it — the method, not its class — and
//! lines removed with nothing in their place belong to the entity holding the
//! lines on both sides of them.
//!
//! From the changed entities the index's relationships are followed backwards,
//! level by level, to what depends on them: their callers, the entities that
//! use, reference, extend, implement or override them, then what depends on
//! those. Modules, namespaces and documentation sections are left out of that
//! walk: a module's imports would make every file that imports it a
//! dependent, where the code that calls what changed is found directly. The tests among the changed entities and their dependents are
//! listed apart ([`crate::analysis::test_code`]): the tests to run for the
//! change, as far as the index connects them to it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::analysis::test_code;
use crate::database::{Database, EntityHeader};
use crate::entities::{Entity, EntityId, Relationship, RelationshipType};
use crate::error::{Error, Result};

/// The longest a git command may run.
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// The most of git's error output an error quotes.
const GIT_ERROR_CHARS: usize = 2_000;
/// The most entities the walk to the dependents visits before it stops.
const MAX_VISITED: usize = 5_000;
/// The most lines of one file the changed lines are looked up for.
const MAX_LINES_PER_FILE: u32 = 200_000;
/// The most entities one query for relationships names.
const RELATIONSHIP_QUERY_IDS: usize = 900;
/// The kinds of entity the walk to the dependents passes over: they hold
/// code rather than being called, and their edges are a file's imports.
const NOT_WALKED: &[&str] = &["module", "namespace", "section"];

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    /// Added to the repository.
    Added,
    /// Changed in place.
    Modified,
    /// Removed.
    Deleted,
    /// Moved, perhaps changed too.
    Renamed,
    /// In the working tree, not yet known to git.
    Untracked,
}

/// One file's change, as git tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Its path in the repository, with `/` between folders: after the
    /// change, or before it for a deleted file.
    pub path: String,
    /// How it changed.
    pub status: FileStatus,
    /// The lines the change added or rewrote, as ranges counted from 1 in the
    /// file after the change; a new file's whole text is one range.
    pub changed: Vec<(u32, u32)>,
    /// The lines removed with nothing in their place: after which line of the
    /// file as it is now (0 for its top), how many, and the text git shows
    /// with the hunk, the declaration the lines were in.
    pub removed: Vec<Removal>,
}

/// Lines a change removed with nothing in their place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Removal {
    /// The line of the file as it is now that the removed lines followed.
    pub after_line: u32,
    /// How many lines were removed.
    pub lines: u32,
    /// The declaration git names with the hunk, when it names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub near: Option<String>,
}

/// Reads git's unified diff, without context lines (`--unified=0`), one line
/// at a time, keeping only what [`FileChange`] holds: the text of the changed
/// lines is passed over, so a diff of any size is read in little memory.
#[derive(Debug, Default)]
pub struct DiffReader {
    files: Vec<FileChange>,
    current: Option<FileChange>,
    /// Whether the current file's hunks have begun: from then on, a line is
    /// a hunk's header or text, never a file header.
    in_hunks: bool,
}

impl DiffReader {
    /// Read one line of the diff, without its line break.
    pub fn line(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.end_file();
            self.current = Some(FileChange {
                path: git_header_path(rest),
                status: FileStatus::Modified,
                changed: Vec::new(),
                removed: Vec::new(),
            });
            return;
        }
        let Some(file) = self.current.as_mut() else {
            return;
        };
        if let Some(header) = line.strip_prefix("@@ ") {
            self.in_hunks = true;
            if let Some(hunk) = Hunk::parse(header) {
                hunk.record(file);
            }
            return;
        }
        if self.in_hunks {
            return;
        }
        if line.starts_with("new file mode") {
            file.status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            file.status = FileStatus::Deleted;
        } else if let Some(to) = line.strip_prefix("rename to ") {
            file.path = unquote(to);
            file.status = FileStatus::Renamed;
        } else if let Some(path) = line.strip_prefix("+++ ") {
            if path != "/dev/null" {
                file.path = without_prefix(&unquote(path), "b/");
            }
        } else if let Some(path) = line.strip_prefix("--- ")
            && file.status == FileStatus::Deleted
            && path != "/dev/null"
        {
            file.path = without_prefix(&unquote(path), "a/");
        }
    }

    /// The files read, in the diff's order.
    #[must_use]
    pub fn finish(mut self) -> Vec<FileChange> {
        self.end_file();
        self.files
    }

    fn end_file(&mut self) {
        self.in_hunks = false;
        if let Some(file) = self.current.take() {
            self.files.push(file);
        }
    }
}

/// A hunk's header: `-a[,b] +c[,d] @@ near`.
struct Hunk {
    removed: u32,
    start: u32,
    added: u32,
    near: Option<String>,
}

impl Hunk {
    fn parse(header: &str) -> Option<Self> {
        let (ranges, near) = header.split_once(" @@").unwrap_or((header, ""));
        let mut parts = ranges.split_whitespace();
        let (_, removed) = Self::range(parts.next()?.strip_prefix('-')?)?;
        let (start, added) = Self::range(parts.next()?.strip_prefix('+')?)?;
        let near = near.trim();
        Some(Self {
            removed,
            start,
            added,
            near: (!near.is_empty()).then(|| near.to_string()),
        })
    }

    /// `n[,count]`, the count 1 when left out.
    fn range(text: &str) -> Option<(u32, u32)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    }

    fn record(self, file: &mut FileChange) {
        if self.added > 0 {
            file.changed.push((self.start, self.start + self.added - 1));
        } else if self.removed > 0 {
            file.removed.push(Removal {
                after_line: self.start,
                lines: self.removed,
                near: self.near,
            });
        }
    }
}

/// The path of `diff --git a/<path> b/<path>`, for a file whose diff names it
/// nowhere else (a binary one): the part after the last ` b/`.
fn git_header_path(rest: &str) -> String {
    let rest = rest.trim();
    if rest.starts_with('"') {
        // `"a/…" "b/…"`: the second quoted path.
        let second = rest.rfind(" \"").map_or(rest, |at| &rest[at + 1..]);
        return without_prefix(&unquote(second), "b/");
    }
    rest.rfind(" b/")
        .map_or_else(|| rest.to_string(), |at| rest[at + 3..].to_string())
}

fn without_prefix(path: &str, prefix: &str) -> String {
    path.strip_prefix(prefix).unwrap_or(path).to_string()
}

/// A path as git writes it: as it is, or, holding a quote, a backslash or a
/// control character, quoted in C's way with octal escapes for each byte.
fn unquote(text: &str) -> String {
    let Some(inner) = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return text.to_string();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buffer = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => bytes.push(b'\n'),
            Some('t') => bytes.push(b'\t'),
            Some('r') => bytes.push(b'\r'),
            Some('a') => bytes.push(0x07),
            Some('b') => bytes.push(0x08),
            Some('f') => bytes.push(0x0c),
            Some('v') => bytes.push(0x0b),
            Some(digit @ '0'..='7') => {
                let mut value = digit.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().and_then(|c| c.to_digit(8)) {
                        Some(next) => {
                            value = value * 8 + next;
                            chars.next();
                        }
                        None => break,
                    }
                }
                bytes.push(u8::try_from(value).unwrap_or(b'?'));
            }
            Some(other) => {
                let mut buffer = [0u8; 4];
                bytes.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
            None => bytes.push(b'\\'),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Whether `base` can name a revision or a range to git, and nothing else:
/// not empty, not an option, without blanks or control characters.
#[must_use]
pub fn is_revision(base: &str) -> bool {
    !base.is_empty()
        && !base.starts_with('-')
        && !base.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Run git in `dir` with `args`, handing each line of what it prints to
/// `on_line`; its error output is the error when it fails.
async fn git(dir: &Path, args: &[&str], mut on_line: impl FnMut(&str)) -> Result<()> {
    let mut child = tokio::process::Command::new("git")
        .current_dir(dir)
        // A repository's config could otherwise run a program while git reads.
        .args(["-c", "core.fsmonitor=false", "-c", "core.quotePath=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| Error::Analysis(format!("cannot run git: {e}")))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let run = async {
        let errors = tokio::spawn(async move {
            let mut text = String::new();
            if let Some(stderr) = stderr {
                let _ = stderr
                    .take(GIT_ERROR_CHARS as u64)
                    .read_to_string(&mut text)
                    .await;
            }
            text
        });
        if let Some(stdout) = stdout {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line).await? == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&line);
                on_line(text.trim_end_matches(['\n', '\r']));
            }
        }
        let status = child.wait().await?;
        let errors = errors.await.unwrap_or_default();
        Ok::<_, std::io::Error>((status, errors))
    };
    let (status, errors) = tokio::time::timeout(GIT_TIMEOUT, run).await.map_err(|_| {
        Error::Analysis(format!("git {} took longer than {GIT_TIMEOUT:?}", args[0]))
    })??;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Analysis(format!(
            "git {} failed: {}",
            args[0],
            errors.trim()
        )))
    }
}

/// The top of the git repository `dir` is in, spelled as a scan stores paths.
///
/// # Errors
/// `dir` is not in a git repository, or git cannot be run.
pub async fn repository_top(dir: &Path) -> Result<PathBuf> {
    let mut top = String::new();
    git(dir, &["rev-parse", "--show-toplevel"], |line| {
        if top.is_empty() {
            top = line.to_string();
        }
    })
    .await?;
    Ok(crate::utils::paths::canonical(Path::new(&top)))
}

/// The files changed between `base` and the working tree of the repository
/// at `top`, untracked ones included, or, for a range `a..b`, between those
/// revisions.
///
/// # Errors
/// `base` is no revision ([`is_revision`]) or git refuses it.
pub async fn changed_files(top: &Path, base: &str) -> Result<Vec<FileChange>> {
    if !is_revision(base) {
        return Err(Error::Analysis(format!(
            "`{base}` is not a revision or a range of revisions"
        )));
    }
    let mut reader = DiffReader::default();
    git(
        top,
        &[
            "diff",
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            base,
            "--",
        ],
        |line| reader.line(line),
    )
    .await?;
    let mut files = reader.finish();
    if !base.contains("..") {
        let mut untracked = Vec::new();
        git(
            top,
            &["ls-files", "--others", "--exclude-standard"],
            |line| {
                if !line.is_empty() {
                    untracked.push(unquote(line));
                }
            },
        )
        .await?;
        files.extend(untracked.into_iter().map(|path| FileChange {
            path,
            status: FileStatus::Untracked,
            changed: vec![(1, u32::MAX)],
            removed: Vec::new(),
        }));
    }
    Ok(files)
}

/// What a change touches, as [`impact`] finds it.
#[derive(Debug, Clone, Serialize)]
pub struct ChangeImpact {
    /// The files the change touches.
    pub files: Vec<ChangedFile>,
    /// The entities it changes, file by file.
    pub changed: Vec<ChangedEntity>,
    /// The entities that depend on them, nearest first; tests apart.
    pub dependents: Vec<Dependent>,
    /// The tests among the changed entities (`depth` 0) and their
    /// dependents: what to run for the change.
    pub tests: Vec<Dependent>,
    /// Changed files the index holds no entity of: not code it parses, or
    /// not scanned.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_indexed: Vec<String>,
    /// Whether a list was cut at its limit, or the walk stopped before its
    /// depth.
    pub truncated: bool,
}

/// One changed file.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    /// Its path, as the index stores paths.
    pub file_path: String,
    /// How it changed.
    pub status: FileStatus,
    /// The lines the change added or rewrote; `None` for an untracked file,
    /// whose lines are all new.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_lines: Option<u64>,
    /// The lines removed with nothing in their place that no entity of the
    /// file as it is now holds: the code of entities removed whole.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<Removal>,
}

/// One entity a change changes.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedEntity {
    /// Its name.
    pub name: String,
    /// Its kind: `function`, `class`, …
    pub entity_type: String,
    /// Its file, as the index stores paths.
    pub file_path: String,
    /// Its first line, counted from 1.
    pub start_line: u32,
    /// Its last line.
    pub end_line: u32,
    /// The lines of it the change touches, as `12-14, 20`.
    pub lines: String,
}

/// One entity that depends on what a change changes.
#[derive(Debug, Clone, Serialize)]
pub struct Dependent {
    /// Its name.
    pub name: String,
    /// Its kind: `function`, `class`, …
    pub entity_type: String,
    /// Its file, as the index stores paths.
    pub file_path: String,
    /// Its first line, counted from 1.
    pub start_line: u32,
    /// How many relationships away from a changed entity: 1 for a direct
    /// caller or user, 0 for a changed test itself.
    pub depth: usize,
    /// The changed entities it depends on, by name, at most five.
    pub reaches: Vec<String>,
}

/// How far [`impact`] looks and how much it lists.
#[derive(Debug, Clone, Copy)]
pub struct ImpactLimits {
    /// The most relationships followed from a changed entity.
    pub depth: usize,
    /// The most entries of each list.
    pub limit: usize,
}

/// What the `changes` to the repository at `top` touch in the index.
///
/// # Errors
/// The index cannot be read.
pub async fn impact(
    db: &Database,
    top: &Path,
    changes: &[FileChange],
    limits: ImpactLimits,
) -> Result<ChangeImpact> {
    let mut truncated = false;
    let mut files = Vec::new();
    let mut changed: Vec<(Entity, Vec<u32>)> = Vec::new();
    let mut not_indexed = Vec::new();
    for change in changes {
        let path = stored_path(top, &change.path);
        let file_path = path.display().to_string();
        let entities = if change.status == FileStatus::Deleted {
            Vec::new()
        } else {
            db.get_entities_by_file(&file_path).await?
        };
        if entities.is_empty() && change.status != FileStatus::Deleted {
            not_indexed.push(file_path.clone());
        }
        let (touched, unheld) = touched_entities(&entities, change);
        changed.extend(touched);
        files.push(ChangedFile {
            file_path,
            status: change.status,
            changed_lines: (change.status != FileStatus::Untracked).then(|| {
                change
                    .changed
                    .iter()
                    .map(|(start, end)| u64::from(end - start) + 1)
                    .sum()
            }),
            removed: unheld,
        });
    }

    let origins: Vec<(EntityId, bool)> = changed
        .iter()
        .map(|(entity, _)| (entity.id(), !NOT_WALKED.contains(&entity.entity_type())))
        .collect();
    let names: Vec<String> = changed
        .iter()
        .map(|(entity, _)| entity.name().to_string())
        .collect();
    let walk = dependents_of(db, &origins, limits.depth).await?;
    truncated |= walk.stopped;

    let mut tests = Vec::new();
    for (entity, _) in &changed {
        if is_test_entity(top, entity) {
            tests.push(Dependent {
                name: entity.name().to_string(),
                entity_type: entity.entity_type().to_string(),
                file_path: entity.file_path().to_string(),
                start_line: entity.location().start_line,
                depth: 0,
                reaches: vec![entity.name().to_string()],
            });
        }
    }

    let dependent_ids: Vec<EntityId> = walk.found.keys().copied().collect();
    let marks = db.get_entity_marks_by_ids(&dependent_ids).await?;
    let mut dependents = Vec::new();
    let mut headers = walk.headers;
    for (id, (depth, origins)) in walk.found {
        let Some(header) = headers.remove(&id) else {
            continue;
        };
        let (language, decorators) = marks.get(&header.id).cloned().unwrap_or_default();
        let is_test =
            test_code::is_test_code(&within(top, &header.file_path), &header.name, &language)
                || decorators.iter().any(|d| test_code::is_test_decorator(d));
        let dependent = Dependent {
            name: header.name,
            entity_type: header.entity_type,
            file_path: header.file_path,
            start_line: header.start_line,
            depth,
            reaches: origins.iter().take(5).map(|&i| names[i].clone()).collect(),
        };
        if is_test {
            tests.push(dependent);
        } else {
            dependents.push(dependent);
        }
    }
    let order = |a: &Dependent, b: &Dependent| {
        (a.depth, &a.file_path, a.start_line).cmp(&(b.depth, &b.file_path, b.start_line))
    };
    dependents.sort_by(order);
    tests.sort_by(order);

    let mut changed: Vec<ChangedEntity> = changed
        .into_iter()
        .map(|(entity, lines)| {
            let location = entity.location();
            ChangedEntity {
                name: entity.name().to_string(),
                entity_type: entity.entity_type().to_string(),
                file_path: entity.file_path().to_string(),
                start_line: location.start_line,
                end_line: location.end_line,
                lines: line_ranges(&lines),
            }
        })
        .collect();
    for list_len in [changed.len(), dependents.len(), tests.len()] {
        truncated |= list_len > limits.limit;
    }
    changed.truncate(limits.limit);
    dependents.truncate(limits.limit);
    tests.truncate(limits.limit);
    Ok(ChangeImpact {
        files,
        changed,
        dependents,
        tests,
        not_indexed,
        truncated,
    })
}

/// The path a scan stores for the file at `path` (with `/` between folders)
/// in the repository at `top`.
fn stored_path(top: &Path, path: &str) -> PathBuf {
    let joined = path
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(top.to_path_buf(), |joined, part| joined.join(part));
    crate::pipeline::as_stored(&joined)
}

/// `file_path` within the repository at `top`, which the test rules read.
fn within(top: &Path, file_path: &str) -> PathBuf {
    let path = Path::new(file_path);
    path.strip_prefix(top).unwrap_or(path).to_path_buf()
}

/// Whether `entity`, of the repository at `top`, is test code: by its file,
/// its name, or a decorator that marks a test.
fn is_test_entity(top: &Path, entity: &Entity) -> bool {
    let decorators: &[String] = match entity {
        Entity::Function(function) => &function.decorators,
        Entity::Class(class) => &class.decorators,
        _ => &[],
    };
    test_code::is_test_code(
        &within(top, entity.file_path()),
        entity.name(),
        entity.language(),
    ) || decorators.iter().any(|d| test_code::is_test_decorator(d))
}

/// The entities of a file the `change` touches, each with the lines of it
/// touched, in the order of their first line; and the removals no entity
/// holds, which removed entities whole.
fn touched_entities(
    entities: &[Entity],
    change: &FileChange,
) -> (Vec<(Entity, Vec<u32>)>, Vec<Removal>) {
    // Innermost first: of the entities holding a line, the smallest.
    let mut by_size: Vec<&Entity> = entities.iter().collect();
    by_size.sort_by_key(|entity| {
        let location = entity.location();
        (
            location.end_line.saturating_sub(location.start_line),
            std::cmp::Reverse(location.start_line),
        )
    });
    let innermost = |line: u32| {
        by_size.iter().position(|entity| {
            let location = entity.location();
            location.start_line <= line && line <= location.end_line
        })
    };
    let last_line = entities
        .iter()
        .map(|entity| entity.location().end_line)
        .max()
        .unwrap_or(0)
        .min(MAX_LINES_PER_FILE);
    let mut lines: HashMap<usize, Vec<u32>> = HashMap::new();
    for &(start, end) in &change.changed {
        for line in start..=end.min(last_line) {
            if let Some(at) = innermost(line) {
                lines.entry(at).or_default().push(line);
            }
        }
    }
    let mut unheld = Vec::new();
    for removal in &change.removed {
        // Within an entity when the lines on both sides are in it.
        let held = innermost(removal.after_line.max(1))
            .filter(|&at| Some(at) == innermost(removal.after_line + 1));
        match held {
            Some(at) => lines.entry(at).or_default().push(removal.after_line),
            None => unheld.push(removal.clone()),
        }
    }
    let mut touched: Vec<(Entity, Vec<u32>)> = lines
        .into_iter()
        .map(|(at, mut lines)| {
            lines.sort_unstable();
            lines.dedup();
            (by_size[at].clone(), lines)
        })
        .collect();
    touched.sort_by_key(|(entity, _)| entity.location().start_line);
    (touched, unheld)
}

/// `lines`, sorted, as ranges: `12-14, 20`.
fn line_ranges(lines: &[u32]) -> String {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for &line in lines {
        match ranges.last_mut() {
            Some((_, end)) if line == *end + 1 => *end = line,
            Some((_, end)) if line == *end => {}
            _ => ranges.push((line, line)),
        }
    }
    ranges
        .iter()
        .map(|&(start, end)| {
            if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The dependent and its dependency in `relationship`, where it is an edge
/// of dependence — a call, a use or reference, a supertype, an override —
/// whichever way it is stored: the forward edge's source depends on its
/// target, and a reverse row's target on its source.
fn dependence(relationship: &Relationship) -> Option<(EntityId, EntityId)> {
    use RelationshipType::{
        CalledBy, Calls, HasMethod, ImplementedBy, Implements, InheritedBy, Inherits, MethodOf,
        OverriddenBy, Overrides, ReferencedBy, References, RelatedTo, UsedBy, Uses,
    };
    match relationship.relationship_type {
        Calls | Uses | References | Inherits | Implements | Overrides => {
            Some((relationship.source_id, relationship.target_id))
        }
        CalledBy | UsedBy | ReferencedBy | InheritedBy | ImplementedBy | OverriddenBy => {
            Some((relationship.target_id, relationship.source_id))
        }
        HasMethod | MethodOf | RelatedTo => None,
    }
}

/// What [`dependents_of`] found.
struct Walk {
    /// Each dependent, with its depth and the changed entities (by their place
    /// among the origins) it depends on.
    found: HashMap<EntityId, (usize, BTreeSet<usize>)>,
    /// The name, kind and place of each dependent.
    headers: HashMap<EntityId, EntityHeader>,
    /// Whether the walk stopped at [`MAX_VISITED`] before its depth.
    stopped: bool,
}

/// What depends on the changed entities `origins` — each with whether the
/// walk starts from it ([`NOT_WALKED`]) — up to `depth` relationships away.
async fn dependents_of(db: &Database, origins: &[(EntityId, bool)], depth: usize) -> Result<Walk> {
    let mut reaching: HashMap<EntityId, BTreeSet<usize>> = HashMap::new();
    for (at, (id, _)) in origins.iter().enumerate() {
        reaching.entry(*id).or_default().insert(at);
    }
    let changed_set: HashSet<EntityId> = origins.iter().map(|(id, _)| *id).collect();
    let mut walk = Walk {
        found: HashMap::new(),
        headers: HashMap::new(),
        stopped: false,
    };
    let mut frontier: Vec<EntityId> = origins
        .iter()
        .filter(|(_, walked)| *walked)
        .map(|(id, _)| *id)
        .collect();
    for level in 1..=depth {
        if frontier.is_empty() {
            break;
        }
        if walk.found.len() >= MAX_VISITED {
            walk.stopped = true;
            break;
        }
        let in_frontier: HashSet<EntityId> = frontier.iter().copied().collect();
        let mut relationships = Vec::new();
        // Within SQLite's bound on a statement's parameters.
        for ids in frontier.chunks(RELATIONSHIP_QUERY_IDS) {
            relationships.extend(db.get_relationships_for_ids(ids).await?);
        }
        let candidates: Vec<(EntityId, EntityId)> = relationships
            .iter()
            .filter_map(dependence)
            .filter(|(dependent, dependency)| {
                dependent != dependency
                    && in_frontier.contains(dependency)
                    && !changed_set.contains(dependent)
            })
            .collect();
        let unknown: Vec<EntityId> = candidates
            .iter()
            .map(|(dependent, _)| *dependent)
            .filter(|dependent| !walk.headers.contains_key(dependent))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        for ids in unknown.chunks(RELATIONSHIP_QUERY_IDS) {
            for header in db.get_entity_names_by_ids(ids).await? {
                walk.headers.insert(header.id, header);
            }
        }
        let mut next = Vec::new();
        for (dependent, dependency) in candidates {
            let walked = walk
                .headers
                .get(&dependent)
                .is_some_and(|header| !NOT_WALKED.contains(&header.entity_type.as_str()));
            if !walked {
                continue;
            }
            let reached = reaching.get(&dependency).cloned().unwrap_or_default();
            let entry = walk.found.entry(dependent).or_insert_with(|| {
                next.push(dependent);
                (level, BTreeSet::new())
            });
            // A dependent met again at its own level depends on more of the
            // change; met later, it was already found nearer.
            if entry.0 == level {
                entry.1.extend(reached.iter().copied());
                reaching.entry(dependent).or_default().extend(reached);
            }
        }
        frontier = next;
    }
    Ok(walk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(diff: &str) -> Vec<FileChange> {
        let mut reader = DiffReader::default();
        for line in diff.lines() {
            reader.line(line);
        }
        reader.finish()
    }

    #[test]
    fn a_diff_is_read_into_changed_and_removed_lines() {
        let files = read(
            "diff --git a/src/cart.rs b/src/cart.rs\n\
             index 1111111..2222222 100644\n\
             --- a/src/cart.rs\n\
             +++ b/src/cart.rs\n\
             @@ -12 +12 @@ fn total(items: &[Item]) -> u32 {\n\
             -    0\n\
             +    1\n\
             @@ -20,3 +19,0 @@ fn old() {\n\
             -a\n\
             -b\n\
             -+++ b/not/a/header\n\
             @@ -30,0 +28,2 @@\n\
             +x\n\
             +y\n\
             diff --git a/new.py b/new.py\n\
             new file mode 100644\n\
             --- /dev/null\n\
             +++ b/new.py\n\
             @@ -0,0 +1,3 @@\n\
             +def f():\n\
             +    pass\n\
             +\n\
             diff --git a/gone.go b/gone.go\n\
             deleted file mode 100644\n\
             --- a/gone.go\n\
             +++ /dev/null\n\
             @@ -1,2 +0,0 @@\n\
             -package x\n\
             -\n\
             diff --git a/old name.js b/new name.js\n\
             similarity index 90%\n\
             rename from old name.js\n\
             rename to new name.js\n\
             --- a/old name.js\n\
             +++ b/new name.js\n\
             @@ -4 +4 @@\n\
             -a\n\
             +b\n\
             diff --git a/logo.png b/logo.png\n\
             Binary files a/logo.png and b/logo.png differ\n",
        );
        assert_eq!(
            files,
            vec![
                FileChange {
                    path: "src/cart.rs".into(),
                    status: FileStatus::Modified,
                    changed: vec![(12, 12), (28, 29)],
                    removed: vec![Removal {
                        after_line: 19,
                        lines: 3,
                        near: Some("fn old() {".into()),
                    }],
                },
                FileChange {
                    path: "new.py".into(),
                    status: FileStatus::Added,
                    changed: vec![(1, 3)],
                    removed: vec![],
                },
                FileChange {
                    path: "gone.go".into(),
                    status: FileStatus::Deleted,
                    changed: vec![],
                    removed: vec![Removal {
                        after_line: 0,
                        lines: 2,
                        near: None,
                    }],
                },
                FileChange {
                    path: "new name.js".into(),
                    status: FileStatus::Renamed,
                    changed: vec![(4, 4)],
                    removed: vec![],
                },
                FileChange {
                    path: "logo.png".into(),
                    status: FileStatus::Modified,
                    changed: vec![],
                    removed: vec![],
                },
            ]
        );
    }

    #[test]
    fn quoted_paths_are_unquoted() {
        assert_eq!(unquote("\"tab\\there\""), "tab\there");
        assert_eq!(unquote("\"caf\\303\\251.rs\""), "café.rs");
        assert_eq!(unquote("\"say \\\"hi\\\".rs\""), "say \"hi\".rs");
        assert_eq!(unquote("plain.rs"), "plain.rs");
        assert_eq!(
            git_header_path("\"a/caf\\303\\251.rs\" \"b/caf\\303\\251.rs\""),
            "café.rs"
        );
        assert_eq!(git_header_path("a/x y.rs b/x y.rs"), "x y.rs");
    }

    #[test]
    fn only_a_revision_or_a_range_is_taken() {
        for base in [
            "HEAD",
            "HEAD~2",
            "main",
            "origin/main",
            "a1b2c3d",
            "v1.0..HEAD",
        ] {
            assert!(is_revision(base), "{base}");
        }
        for base in ["", "--output=/tmp/x", "-p", "HEAD main", "a\nb"] {
            assert!(!is_revision(base), "{base:?}");
        }
    }

    #[test]
    fn lines_are_written_as_ranges() {
        assert_eq!(line_ranges(&[12, 13, 14, 20, 22, 23]), "12-14, 20, 22-23");
        assert_eq!(line_ranges(&[5]), "5");
        assert_eq!(line_ranges(&[]), "");
    }
}
