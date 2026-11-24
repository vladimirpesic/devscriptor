//! Long runs of comment lines in Python, kept out of tree-sitter's way.
//!
//! tree-sitter-python's external scanner decides indentation at every
//! newline after a statement by skipping ahead over the blank and comment
//! lines that follow, to the next line of code. The comments are then lexed
//! one at a time, and at the next newline the scanner skips ahead again: a
//! run of N comment lines costs on the order of N² — a file that is mostly a
//! 20,000-line comment block took minutes to parse.
//!
//! Comments hold no entities, so such a run is simply not given to the parser:
//! [`included_ranges`] names the parts of the file it reads, leaving out the
//! body of every long run of comment and blank lines. tree-sitter keeps every
//! node's byte and line position in the whole file. The last
//! [`KEPT_LINES`] lines of a run stay in, so a comment above a definition is
//! still beside it.
//!
//! Only lines that really are comments count: a line starting with `#` inside
//! a triple-quoted string is part of the string. The scan follows Python's
//! strings — single- and triple-quoted, with escapes — and its comments; a
//! `#` inside an f-string's replacement field, the one place it does not
//! follow, cannot start a line on its own.

/// A run of comment and blank lines this long or longer is left out.
pub const MIN_RUN_LINES: usize = 256;
/// The lines at the end of a left-out run that are parsed after all.
pub const KEPT_LINES: usize = 32;

/// The parts of `content` to parse, when it has a run of comment and blank
/// lines long enough to leave out; `None` when the whole file is parsed.
#[must_use]
pub fn included_ranges(content: &str) -> Option<Vec<tree_sitter::Range>> {
    let excluded = excluded_spans(content);
    if excluded.is_empty() {
        return None;
    }
    // Every span starts and ends at the start of a line, whose row it names.
    let line_start = |(byte, row): (usize, usize)| (byte, tree_sitter::Point { row, column: 0 });
    let mut ranges = Vec::with_capacity(excluded.len() + 1);
    let mut start = line_start((0, 0));
    for (from, to) in excluded {
        let from = line_start(from);
        if from.0 > start.0 {
            ranges.push(range(start, from));
        }
        start = line_start(to);
    }
    if start.0 < content.len() {
        ranges.push(range(start, (content.len(), end_point(content))));
    }
    Some(ranges)
}

/// The spans left out of the parse, each a whole number of lines, from the
/// first line of a run to its last [`KEPT_LINES`]: `((byte, row), (byte, row))`.
fn excluded_spans(content: &str) -> Vec<((usize, usize), (usize, usize))> {
    let mut spans = Vec::new();
    // The current run as `(byte, row)` of each line, and whether it has a
    // comment: a run of blank lines alone costs nothing.
    let mut run: Vec<(usize, usize)> = Vec::new();
    let mut has_comment = false;
    let mut close = |run: &mut Vec<(usize, usize)>, has_comment: &mut bool| {
        if *has_comment && run.len() >= MIN_RUN_LINES {
            spans.push((run[0], run[run.len() - KEPT_LINES]));
        }
        run.clear();
        *has_comment = false;
    };
    for (row, line) in classify_lines(content).into_iter().enumerate() {
        match line.kind {
            LineKind::Comment => {
                run.push((line.start, row));
                has_comment = true;
            }
            LineKind::Blank => run.push((line.start, row)),
            LineKind::Code => close(&mut run, &mut has_comment),
        }
    }
    close(&mut run, &mut has_comment);
    spans
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    /// Nothing but a comment, outside any string.
    Comment,
    /// Nothing but whitespace, outside any string.
    Blank,
    /// Anything else, a line inside a string included.
    Code,
}

struct Line {
    start: usize,
    kind: LineKind,
}

/// Each line of `content`, from its first byte, as a comment line, a blank
/// line or code.
fn classify_lines(content: &str) -> Vec<Line> {
    let bytes = content.as_bytes();
    let mut lines = Vec::new();
    // Inside a string: its quote and whether it is triple-quoted.
    let mut string: Option<(u8, bool)> = None;
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let in_string_at_start = string.is_some();
        // Leading whitespace of a line outside a string.
        if !in_string_at_start {
            while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\x0c' | b'\r') {
                i += 1;
            }
            let kind = match bytes.get(i) {
                None | Some(b'\n') => Some(LineKind::Blank),
                Some(b'#') => Some(LineKind::Comment),
                _ => None,
            };
            if let Some(kind) = kind {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                i += 1;
                lines.push(Line { start, kind });
                continue;
            }
        }
        // A line of code, or one that starts inside a string: follow strings
        // and comments to its end.
        while i < bytes.len() && bytes[i] != b'\n' {
            match string {
                Some((quote, triple)) => {
                    if bytes[i] == b'\\' {
                        // An escape: the next byte is the string's, unless it
                        // ends the line, which stays a line of its own.
                        i += if bytes.get(i + 1) == Some(&b'\n') {
                            1
                        } else {
                            2
                        };
                        continue;
                    }
                    if bytes[i] == quote
                        && (!triple || bytes[i..].starts_with(&[quote, quote, quote]))
                    {
                        i += if triple { 3 } else { 1 };
                        string = None;
                        continue;
                    }
                    i += 1;
                }
                None => match bytes[i] {
                    b'#' => {
                        while i < bytes.len() && bytes[i] != b'\n' {
                            i += 1;
                        }
                    }
                    quote @ (b'"' | b'\'') => {
                        let triple = bytes[i..].starts_with(&[quote, quote, quote]);
                        i += if triple { 3 } else { 1 };
                        string = Some((quote, triple));
                    }
                    _ => i += 1,
                },
            }
        }
        // A single-quoted string ends with its line, closed or not; an escaped
        // newline continues it.
        if let Some((_, false)) = string
            && !(i > 0 && bytes.get(i - 1) == Some(&b'\\'))
        {
            string = None;
        }
        i += 1;
        lines.push(Line {
            start,
            kind: LineKind::Code,
        });
    }
    lines
}

/// The range between two positions, each a byte and its point.
fn range(
    (start_byte, start_point): (usize, tree_sitter::Point),
    (end_byte, end_point): (usize, tree_sitter::Point),
) -> tree_sitter::Range {
    tree_sitter::Range {
        start_byte,
        end_byte,
        start_point,
        end_point,
    }
}

/// The point at the end of `content`.
fn end_point(content: &str) -> tree_sitter::Point {
    let bytes = content.as_bytes();
    let row = bytes.iter().filter(|&&b| b == b'\n').count();
    let column = bytes.len() - bytes.iter().rposition(|&b| b == b'\n').map_or(0, |n| n + 1);
    tree_sitter::Point { row, column }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(content: &str) -> Vec<LineKind> {
        classify_lines(content)
            .into_iter()
            .map(|line| line.kind)
            .collect()
    }

    #[test]
    fn comment_lines_are_told_from_strings() {
        use LineKind::{Blank, Code, Comment};
        assert_eq!(
            kinds("# c\n\nx = 1  # trailing\n    # indented\n"),
            [Comment, Blank, Code, Comment]
        );
        // `#` lines inside triple-quoted strings belong to the string.
        assert_eq!(
            kinds("s = \"\"\"\n# not a comment\n\"\"\"\n# a comment\n"),
            [Code, Code, Code, Comment]
        );
        assert_eq!(kinds("t = '''a\n# x\n'''\n"), [Code, Code, Code]);
        // An escaped quote does not end a string, and a single-quoted string
        // ends with its line.
        assert_eq!(kinds("u = 'it\\'s # not'\n# c\n"), [Code, Comment]);
        assert_eq!(kinds("v = 'open\n# c\n"), [Code, Comment]);
        // An escaped newline continues a string on the next line, and every
        // physical line is still a line of its own.
        assert_eq!(
            kinds("w = 'one \\\n# still the string'\n# c\n"),
            [Code, Code, Comment]
        );
    }

    #[test]
    fn only_a_long_run_is_left_out_and_its_last_lines_stay() {
        let block = "# comment line\n".repeat(MIN_RUN_LINES);
        let content = format!("def f():\n    return 1\n{block}def g():\n    return 2\n");
        let ranges = included_ranges(&content).expect("a long run");
        assert_eq!(ranges.len(), 2);
        let kept = &content[ranges[1].start_byte..ranges[1].end_byte];
        assert_eq!(kept.matches("# comment line").count(), KEPT_LINES);
        assert!(kept.ends_with("def g():\n    return 2\n"));
        assert_eq!(ranges[1].start_point.row, 2 + MIN_RUN_LINES - KEPT_LINES);
        assert_eq!(ranges[1].start_point.column, 0);

        let short = "# c\n".repeat(MIN_RUN_LINES - 1);
        assert!(included_ranges(&format!("x = 1\n{short}y = 2\n")).is_none());
        let blank = "\n".repeat(MIN_RUN_LINES * 2);
        assert!(included_ranges(&format!("x = 1\n{blank}y = 2\n")).is_none());
        let in_string = format!("s = \"\"\"\n{}\"\"\"\n", "# c\n".repeat(MIN_RUN_LINES * 2));
        assert!(included_ranges(&in_string).is_none());
    }
}
