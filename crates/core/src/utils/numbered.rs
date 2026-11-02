//! Source text shown with its line numbers.
//!
//! An excerpt of a file names the line it starts on, and a reader that cites a
//! line inside it would have to count down from there, and miscount. So every
//! excerpt Devscriptor answers with is numbered the way agents' file readers
//! number a file: each line as `<number><TAB><text>`.

use std::fmt::Write as _;

use crate::parser::TRUNCATION_MARKER;

/// `text`, whose first line is line `first` (1-based) of its file, with each
/// line shown as `<number><TAB><line>`.
///
/// Lines split as [`str::lines`] splits them: a CR before a line feed is
/// dropped, and a final line feed starts no line. A last line that is only
/// [`TRUNCATION_MARKER`], which Devscriptor puts after a body it cut short, is
/// no line of the file and stays unnumbered. With `first` 0, a line not
/// known, `text` comes back as it is.
#[must_use]
pub fn number_lines(text: &str, first: u32) -> String {
    if first == 0 {
        return text.to_string();
    }
    let mut numbered = String::with_capacity(text.len() + text.len() / 4);
    let mut lines = text.lines().peekable();
    let mut number = u64::from(first);
    while let Some(line) = lines.next() {
        if number > u64::from(first) {
            numbered.push('\n');
        }
        if lines.peek().is_none() && line == TRUNCATION_MARKER {
            numbered.push_str(line);
        } else {
            // Writing to a `String` cannot fail.
            let _ = write!(numbered, "{number}\t{line}");
        }
        number += 1;
    }
    numbered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_line_is_numbered_from_the_first() {
        assert_eq!(
            number_lines("fn a() {\n    b();\n}", 42),
            "42\tfn a() {\n43\t    b();\n44\t}"
        );
        // An empty line keeps its number; a tab in the text stays after the
        // number's own.
        assert_eq!(number_lines("x\n\n\ty", 1), "1\tx\n2\t\n3\t\ty");
    }

    /// Lines split as `str::lines` splits them: no line after a final line
    /// feed, and no CR before one.
    #[test]
    fn line_endings_split_as_str_lines() {
        assert_eq!(number_lines("a\r\nb\r\n", 9), "9\ta\n10\tb");
        assert_eq!(number_lines("", 1), "");
    }

    /// The marker after a body cut short is no line of the file; one at the
    /// end of a real line is part of it.
    #[test]
    fn the_truncation_marker_line_stays_unnumbered() {
        let cut = format!("fn a() {{\n    b();\n{TRUNCATION_MARKER}");
        assert_eq!(
            number_lines(&cut, 7),
            format!("7\tfn a() {{\n8\t    b();\n{TRUNCATION_MARKER}")
        );
        let inline = format!("let x = 1;{TRUNCATION_MARKER}");
        assert_eq!(
            number_lines(&inline, 3),
            format!("3\tlet x = 1;{TRUNCATION_MARKER}")
        );
    }

    #[test]
    fn an_unknown_first_line_leaves_the_text_as_it_is() {
        assert_eq!(number_lines("a\nb", 0), "a\nb");
    }
}
