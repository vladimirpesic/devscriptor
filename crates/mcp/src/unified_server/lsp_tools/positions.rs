//! Where a tool's symbol is: the identifier at a line, a name looked up in the
//! index, and the column the language server counts it in.

use super::*;

/// The error for an `item` that is not the one the preparing call returned.
#[cfg(feature = "lsp")]
pub(super) fn invalid_item(error: &serde_json::Error, prepared_by: &str) -> McpError {
    McpError::invalid_params(
        format!(
            "invalid item: {error}. Pass back one of `{prepared_by}`'s `items` unchanged — \
             {{name, kind, location: {{file_path, range: {{start: {{line, character}}, end: \
             {{line, character}}}}}}}} — or leave `item` out and name the symbol: `name`, or \
             `file_path` with `line` and `character`."
        ),
        None,
    )
}

/// The error for a position the preparing call found no item at.
#[cfg(feature = "lsp")]
pub(super) fn no_item_here() -> McpError {
    McpError::invalid_params(
        "no hierarchy item at that position: name the symbol with `name`, or give the \
         position of its name",
        None,
    )
}

/// A tool result as JSON, or the error the call ended in.
#[cfg(feature = "lsp")]
/// The identifier at the 0-based `line` and `character` of `file`, the
/// character counted in UTF-16 code units, as a language server counts it.
pub(super) fn identifier_at(file: &std::path::Path, line: u32, character: u32) -> Option<String> {
    let content = std::fs::read_to_string(file).ok()?;
    let chars: Vec<char> = content.lines().nth(line as usize)?.chars().collect();
    let mut units = 0;
    let at = chars
        .iter()
        .position(|c| {
            let here = units >= character as usize;
            units += c.len_utf16();
            here
        })
        .unwrap_or(chars.len());
    let in_word = |c: &char| c.is_alphanumeric() || matches!(c, '_' | '$');
    let start = chars[..at]
        .iter()
        .rposition(|c| !in_word(c))
        .map_or(0, |i| i + 1);
    let end = chars[at..]
        .iter()
        .position(|c| !in_word(c))
        .map_or(chars.len(), |i| at + i);
    (start < end).then(|| chars[start..end].iter().collect())
}

pub(super) fn json_result<T: serde::Serialize>(
    result: devscriptor_lsp::error::Result<T>,
) -> Result<CallToolResult, McpError> {
    match result {
        Ok(value) => Ok(CallToolResult::success(vec![
            ContentBlock::json(value).map_err(|e| {
                McpError::internal_error(format!("Failed to serialize result: {e}"), None)
            })?,
        ])),
        Err(e) => Err(McpError::internal_error(e.to_string(), None)),
    }
}

/// Characters that are part of an identifier token.
#[cfg(feature = "lsp")]
pub(super) fn is_identifier_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// The byte offset of `name` as a standalone identifier in `line`, at or after
/// the byte offset `floor`. Rejects matches that are substrings of a larger
/// identifier (so `apply_discount` does not match inside `apply_discount_v2`).
#[cfg(feature = "lsp")]
pub(super) fn identifier_column(line: &str, name: &str, floor: usize) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let mut from = floor.min(line.len());
    while let Some(rel) = line.get(from..).and_then(|rest| rest.find(name)) {
        let at = from + rel;
        let after = at + name.len();
        let before_ok = !line[..at]
            .chars()
            .next_back()
            .is_some_and(is_identifier_char);
        let after_ok = !line[after..].chars().next().is_some_and(is_identifier_char);
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + line[at..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

/// The LSP column of the byte offset `byte` in `line`: LSP positions count
/// UTF-16 code units (the only encoding Devscriptor offers servers), while
/// the code index and [`identifier_column`] count bytes. They differ on any
/// line with non-ASCII text before the column.
#[cfg(feature = "lsp")]
pub(super) fn utf16_column(line: &str, byte: usize) -> usize {
    line.char_indices()
        .take_while(|&(i, _)| i < byte)
        .map(|(_, c)| c.len_utf16())
        .sum()
}

/// A source file's text, read as given or relative to the working directory.
#[cfg(feature = "lsp")]
pub(super) fn read_source(file_path: &str) -> Option<String> {
    std::fs::read_to_string(file_path)
        .ok()
        .or_else(|| std::fs::read_to_string(std::env::current_dir().ok()?.join(file_path)).ok())
}

/// How many lines past a definition's start to scan for its identifier, covering
/// decorators and multi-line signatures.
#[cfg(feature = "lsp")]
pub(super) const IDENTIFIER_SCAN_LINES: usize = 4;

/// Locate the identifier `name` near a definition's start, returning its
/// 0-indexed LSP `(line, character)`.
///
/// The Context Graph records a definition's *start* — the `def`/`fn`/`class`
/// keyword — not its identifier, so raw coordinates land off the symbol. Scan
/// the start line (from the keyword column, a byte offset, onward) and a few
/// following lines for the identifier itself, which is where LSP servers
/// answer.
#[cfg(feature = "lsp")]
pub(super) fn locate_identifier(
    file_path: &str,
    start_line: u32,
    start_column: u32,
    name: &str,
) -> Option<(u32, u32)> {
    let content = read_source(file_path)?;
    let lines: Vec<&str> = content.lines().collect();
    let start = start_line as usize;
    for offset in 0..IDENTIFIER_SCAN_LINES {
        let Some(line) = lines.get(start + offset) else {
            break;
        };
        let floor = if offset == 0 {
            start_column as usize
        } else {
            0
        };
        if let Some(column) = identifier_column(line, name, floor) {
            return Some(((start + offset) as u32, utf16_column(line, column) as u32));
        }
    }
    None
}

/// The definition start itself, as an LSP position, for when its identifier
/// is not found nearby. The stored column is a byte offset, converted against
/// the line when the file can be read.
#[cfg(feature = "lsp")]
pub(super) fn definition_start(file_path: &str, start_line: u32, start_column: u32) -> (u32, u32) {
    let column = read_source(file_path)
        .and_then(|content| {
            let line = content.lines().nth(start_line as usize)?;
            u32::try_from(utf16_column(line, start_column as usize)).ok()
        })
        .unwrap_or(start_column);
    (start_line, column)
}

/// Whether a stored file path satisfies a caller's file hint — exact, a path
/// suffix, or the same file name.
#[cfg(feature = "lsp")]
pub(super) fn path_hint_matches(stored: &str, hint: &str) -> bool {
    fn base(p: &str) -> &str {
        p.rsplit(['/', '\\']).next().unwrap_or(p)
    }
    stored == hint || stored.ends_with(hint) || base(stored) == base(hint)
}

/// Choose the best indexed entity for a symbol name: a file-path hint wins, then
/// a type match, then the first candidate.
#[cfg(feature = "lsp")]
pub(super) fn pick_symbol<'a>(
    candidates: &'a [devscriptor_core::entities::Entity],
    entity_type: Option<&str>,
    file_hint: Option<&str>,
) -> Option<&'a devscriptor_core::entities::Entity> {
    let type_ok = |e: &devscriptor_core::entities::Entity| {
        entity_type.is_none_or(|t| e.entity_type().eq_ignore_ascii_case(t))
    };
    if let Some(hint) = file_hint
        && let Some(entity) = candidates
            .iter()
            .find(|e| type_ok(e) && path_hint_matches(e.file_path(), hint))
    {
        return Some(entity);
    }
    candidates
        .iter()
        .find(|e| type_ok(e))
        .or_else(|| candidates.first())
}

/// The LSP position of the indexed symbol `name`: its file, and the 0-indexed
/// line and UTF-16 column of its identifier. `entity_type` and `file_hint`
/// choose between same-named symbols ([`pick_symbol`]).
///
/// The candidates are read from an index current for their files
/// ([`crate::freshness::fresh_answer`]): the language server reads the file
/// on disk, and a position the index took from an older version of it points
/// at the wrong line.
#[cfg(feature = "lsp")]
pub(super) async fn symbol_position_by_name(
    state: &crate::server::DevscriptorServerState,
    name: &str,
    entity_type: Option<&str>,
    file_hint: Option<&str>,
) -> Result<(String, u32, u32), McpError> {
    let (candidates, _) = crate::freshness::fresh_answer(state, || async move {
        let candidates = state
            .database()
            .get_entities_by_exact_name(name, 50)
            .await
            .map_err(|e| {
                McpError::internal_error(format!("symbol lookup failed for '{name}': {e}"), None)
            })?;
        let files = candidates
            .iter()
            .map(|candidate| candidate.file_path().to_string())
            .collect();
        Ok((candidates, files))
    })
    .await?;

    let chosen = pick_symbol(&candidates, entity_type, file_hint).ok_or_else(|| {
        let of_type = entity_type
            .map(|t| format!(" of type '{t}'"))
            .unwrap_or_default();
        McpError::internal_error(
            format!(
                "no indexed symbol named '{name}'{of_type}. Run scan_codebase first, or pass explicit line/character."
            ),
            None,
        )
    })?;

    let file_path = chosen.file_path().to_string();
    let location = chosen.location();
    // The Context Graph stores 1-indexed lines (`start.row + 1` in the
    // parser); LSP positions are 0-indexed. `start_column` is already
    // 0-indexed, but counts bytes; both helpers return UTF-16 columns.
    let zero_indexed_line = location.start_line.saturating_sub(1);
    let (line, character) =
        locate_identifier(&file_path, zero_indexed_line, location.start_column, name)
            .unwrap_or_else(|| {
                definition_start(&file_path, zero_indexed_line, location.start_column)
            });
    Ok((file_path, line, character))
}
