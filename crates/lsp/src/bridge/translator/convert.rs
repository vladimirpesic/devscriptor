//! Conversions between the MCP tools' types and LSP's, and the paths, URIs,
//! offsets and edits they carry.

use super::*;

/// The LSP language identifier of the document at `path`, as `didOpen` gives
/// it: the language of the server that handles it, except where one server
/// takes several. typescript-language-server parses a document as the language
/// it is opened as, so a `.tsx` file opened as `typescript` would have its JSX
/// read as syntax errors — "'>' expected" at every tag.
pub(super) fn document_language_id(path: &Path, server_language: &str) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("tsx") => "typescriptreact",
        Some("jsx") => "javascriptreact",
        Some("js" | "mjs" | "cjs") => "javascript",
        Some("ts" | "mts" | "cts") => "typescript",
        _ => server_language,
    }
    .to_string()
}

/// Whether `dir` holds a source file `config`'s server reads, other than a
/// project marker of its own: a Gradle build script is a `.kts` file, a
/// Swift package's manifest a `.swift` one.
pub(super) fn holds_source_of(dir: &Path, config: &LspServerConfig) -> bool {
    /// Deep enough for `src/main/kotlin/com/example/app/Main.kt`.
    const MAX_DEPTH: usize = 12;
    let markers: &[String] = config
        .heuristics
        .as_ref()
        .map_or(&[], |heuristics| heuristics.markers.as_slice());
    ignore::WalkBuilder::new(dir)
        .max_depth(Some(MAX_DEPTH))
        .build()
        .flatten()
        .any(|entry| {
            entry.file_type().is_some_and(|kind| kind.is_file())
                && entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| config.extensions.iter().any(|e| e == extension))
                && !entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| markers.iter().any(|marker| marker == name))
        })
}

/// `path` made absolute, symlinks resolved, so that workspace folders and
/// projects compare by where they are rather than by how they are spelled:
/// the workspace root is `.` by default, a file an agent asks about absolute.
/// Compared as spelled, a server started for `./services/api` would not cover
/// `/repo/services/api`, and would be restarted to add it a second time.
pub(super) fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Why no server answers for files of `extension`: the registry has none for
/// it, or has one that does not run here ([`crate::manager::Unavailable`]).
pub(super) fn no_server_for(extension: &str) -> LspError {
    let registered = crate::generated_registry::SERVER_IDS.iter().find(|id| {
        crate::generated_registry::extensions(id).is_some_and(|exts| exts.contains(&extension))
    });
    match registered {
        Some(id) => LspError::ServerUnavailable(format!(
            "the {} language server ({id}) for .{extension} files {}",
            crate::generated_registry::language_id(id).unwrap_or(id),
            crate::manager::why_unavailable(id).map_or_else(
                || "does not run here".to_string(),
                |unavailable| unavailable.reason
            )
        )),
        None => LspError::NoServerForFile(format!("No LSP server for extension: .{extension}")),
    }
}

/// Workspace folders as a message shows them.
pub(super) fn show_folders(folders: &[PathBuf]) -> String {
    folders
        .iter()
        .map(|folder| folder.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A `workspace/symbol` answer's symbols, flat or nested; a nested one's
/// symbols without a range (a `WorkspaceLocation`) are left out.
pub(super) fn workspace_symbols_from_lsp(
    response: Option<lsp_types::WorkspaceSymbolResponse>,
) -> Vec<WorkspaceSymbol> {
    match response {
        Some(lsp_types::WorkspaceSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .map(|s| WorkspaceSymbol {
                name: s.name,
                kind: format!("{:?}", s.kind),
                location: location_from_lsp(s.location),
                container_name: s.container_name,
            })
            .collect(),
        Some(lsp_types::WorkspaceSymbolResponse::Nested(symbols)) => symbols
            .into_iter()
            .filter_map(|s| match &s.location {
                lsp_types::OneOf::Left(loc) => Some(WorkspaceSymbol {
                    name: s.name,
                    kind: format!("{:?}", s.kind),
                    location: Location::new(uri_to_path(&loc.uri), range_from_lsp(loc.range)),
                    container_name: s.container_name,
                }),
                lsp_types::OneOf::Right(_) => None,
            })
            .collect(),
        None => vec![],
    }
}

/// `error`, a formatting request's, with what to do where the server has no
/// formatter to run: FsAutoComplete formats with Fantomas, and one installed
/// without it says it found none.
pub(super) fn with_formatter_hint(error: LspError) -> LspError {
    match error {
        LspError::LspServerError { code, message }
            if message.contains("Fantomas install not found") =>
        {
            LspError::LspServerError {
                code,
                message: format!(
                    "{message} `devscriptor lsp setup fsautocomplete` installs FsAutoComplete \
                     with Fantomas beside it."
                ),
            }
        }
        other => other,
    }
}

/// The capability a server declares for the LSP `method`, and the feature it
/// is named as in an [`LspError::Unsupported`].
pub(super) fn declaring_capability(method: &str) -> Option<(&'static str, &'static str)> {
    Some(match method {
        "textDocument/hover" => ("hoverProvider", "hover"),
        "textDocument/definition" => ("definitionProvider", "go to definition"),
        "textDocument/references" => ("referencesProvider", "references"),
        "textDocument/completion" | "completionItem/resolve" => {
            ("completionProvider", "completion")
        }
        "textDocument/formatting" => ("documentFormattingProvider", "formatting"),
        "textDocument/rangeFormatting" => ("documentRangeFormattingProvider", "range formatting"),
        "textDocument/codeAction" | "codeAction/resolve" => ("codeActionProvider", "code actions"),
        "textDocument/rename" => ("renameProvider", "rename"),
        "textDocument/signatureHelp" => ("signatureHelpProvider", "signature help"),
        "textDocument/typeDefinition" => ("typeDefinitionProvider", "type definitions"),
        "textDocument/implementation" => ("implementationProvider", "implementations"),
        "textDocument/prepareCallHierarchy"
        | "callHierarchy/incomingCalls"
        | "callHierarchy/outgoingCalls" => ("callHierarchyProvider", "call hierarchy"),
        "textDocument/prepareTypeHierarchy"
        | "typeHierarchy/subtypes"
        | "typeHierarchy/supertypes" => ("typeHierarchyProvider", "type hierarchy"),
        "textDocument/documentHighlight" => ("documentHighlightProvider", "document highlights"),
        "textDocument/documentColor" => ("colorProvider", "document colors"),
        "textDocument/foldingRange" => ("foldingRangeProvider", "folding ranges"),
        "textDocument/selectionRange" => ("selectionRangeProvider", "selection ranges"),
        "textDocument/linkedEditingRange" => {
            ("linkedEditingRangeProvider", "linked editing ranges")
        }
        "textDocument/inlayHint" => ("inlayHintProvider", "inlay hints"),
        "textDocument/documentLink" => ("documentLinkProvider", "document links"),
        "textDocument/semanticTokens/full" => ("semanticTokensProvider", "semantic tokens"),
        _ => return None,
    })
}

/// Whether an `initialize` capabilities object advertises `provider`.
///
/// Per the LSP spec a supported provider is `true` or an options object; it is
/// unsupported when the field is `false`, `null`, or absent. A request is
/// rejected up-front by it, rather than left to fail with an opaque `-32601`.
pub(super) fn capability_supported(capabilities: &serde_json::Value, provider: &str) -> bool {
    !matches!(
        capabilities.get(provider),
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(false))
    )
}

/// Convert a file path to a `file://` URI, percent-encoding each component
/// (RFC 3986) so paths with spaces, `#`, `?` or non-ASCII characters give a
/// valid URI. On Windows the path is written with `/` and after a `/`, as a
/// `file:` URI has it: `C:\a` is `file:///C:/a`, and the UNC path
/// `\\srv\share\a` is `file://srv/share/a`.
///
/// # Errors
///
/// Returns an error if the path cannot be converted to a URI.
pub fn path_to_uri(path: &Path) -> Result<lsp_types::Uri> {
    let canonical = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    // percent-encode each path component separately so separators stay intact.
    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
    // Chars that must be encoded in a URI path segment (RFC 3986 §3.3).
    const PATH_SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'[')
        .add(b']')
        .add(b'^')
        .add(b'`')
        .add(b'{')
        .add(b'|')
        .add(b'}');

    let path = canonical.to_string_lossy();
    let path = if cfg!(windows) {
        windows_uri_path(&path)
    } else {
        path.into_owned()
    };
    let encoded = utf8_percent_encode(&path, PATH_SEGMENT).to_string();
    let uri_str = format!("file://{encoded}");

    uri_str
        .parse::<lsp_types::Uri>()
        .map_err(|e| LspError::InvalidPath(format!("Failed to create URI: {e}")))
}

/// Convert a URI to a file path string, correctly decoding percent-encoded bytes.
///
/// Each `%XX` is decoded to a byte, and the bytes then as UTF-8, so multi-byte
/// characters survive.
#[must_use]
pub fn uri_to_path(uri: &lsp_types::Uri) -> String {
    let s = uri.as_str();
    match s.strip_prefix("file://") {
        Some(path) if cfg!(windows) => windows_path(&percent_decode_utf8(path)),
        Some(path) => percent_decode_utf8(path),
        // Another scheme names no file: it is passed on as it is.
        None => percent_decode_utf8(s),
    }
}

/// The part of a `file:` URI after `file://` that names the Windows path
/// `path`: `/C:/a` for `C:\a`, and `srv/share/a`, with the server as its host,
/// for the UNC path `\\srv\share\a`.
pub(super) fn windows_uri_path(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    match slashed.strip_prefix("//") {
        Some(unc) => unc.to_string(),
        None => format!("/{slashed}"),
    }
}

/// The Windows path a decoded `file:` URI path names: `/C:/a`, or `/c:/a` as
/// some servers write it, is `C:\a`, with the drive's letter in capitals so
/// the two spellings compare equal; `srv/share/a`, a URI with a host, is the
/// UNC path `\\srv\share\a`.
pub(crate) fn windows_path(path: &str) -> String {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        let drive = char::from(bytes[1]).to_ascii_uppercase();
        let rest = path[3..].replace('/', "\\");
        if rest.is_empty() {
            format!("{drive}:\\")
        } else {
            format!("{drive}:{rest}")
        }
    } else if path.starts_with('/') {
        path.replace('/', "\\")
    } else {
        format!("\\\\{}", path.replace('/', "\\"))
    }
}

/// Convert a 0-indexed LSP position (line, UTF-16 character offset) into a byte
/// offset into `content`. Clamps to the end of the target line / of `content`.
pub(super) fn position_to_byte_offset(content: &str, line: u32, character: u32) -> usize {
    let bytes = content.as_bytes();
    let mut idx = 0usize;
    let mut cur_line = 0u32;
    while cur_line < line && idx < bytes.len() {
        if bytes[idx] == b'\n' {
            cur_line += 1;
        }
        idx += 1;
    }
    // `idx` is now at the first byte of the target line (or end of content).
    let mut utf16 = 0u32;
    for ch in content[idx..].chars() {
        if utf16 >= character || ch == '\n' {
            break;
        }
        utf16 += ch.len_utf16() as u32;
        idx += ch.len_utf8();
    }
    idx
}

/// Apply a set of LSP text edits to `content`, returning the new text. Edits are
/// applied last-position-first so that byte offsets of earlier edits remain
/// valid (the LSP spec guarantees edits do not overlap).
pub(super) fn apply_text_edits(content: &str, edits: &[TextEdit]) -> String {
    let mut ordered: Vec<&TextEdit> = edits.iter().collect();
    ordered.sort_by(|a, b| {
        (b.range.start.line, b.range.start.character)
            .cmp(&(a.range.start.line, a.range.start.character))
    });
    let mut result = content.to_string();
    for edit in ordered {
        let start =
            position_to_byte_offset(&result, edit.range.start.line, edit.range.start.character);
        let end = position_to_byte_offset(&result, edit.range.end.line, edit.range.end.character);
        if start <= end && end <= result.len() {
            result.replace_range(start..end, &edit.new_text);
        }
    }
    result
}

/// Convert an LSP text edit into the crate's `TextEdit`.
pub(super) fn text_edit_from_lsp(e: lsp_types::TextEdit) -> TextEdit {
    TextEdit {
        range: range_from_lsp(e.range),
        new_text: e.new_text,
    }
}

/// Apply text edits to a file on disk. Returns the number of edits written (0
/// when the edit list is empty or the content is unchanged).
pub(super) fn apply_edits_to_file(file_path: &str, edits: &[TextEdit]) -> Result<usize> {
    if edits.is_empty() {
        return Ok(0);
    }
    let content = std::fs::read_to_string(file_path)?;
    let updated = apply_text_edits(&content, edits);
    if updated == content {
        return Ok(0);
    }
    std::fs::write(file_path, updated)?;
    Ok(edits.len())
}

/// Decode a percent-encoded string by collecting bytes then UTF-8 decoding.
pub(super) fn percent_decode_utf8(s: &str) -> String {
    let mut bytes: Vec<u8> = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let high = chars.next().and_then(|c| c.to_digit(16));
            let low = chars.next().and_then(|c| c.to_digit(16));
            if let (Some(h), Some(l)) = (high, low) {
                bytes.push(((h << 4) | l) as u8);
            } else {
                bytes.push(b'%');
            }
        } else {
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| s.to_string())
}

/// The symbols of `file` as `response` gives them, a tree flattened parent
/// first. Each symbol is located at its name in `text`, the file's (see
/// [`at_name`]), and keeps its own name, its parent's in `container_name`, as
/// a flat answer gives them.
pub(super) fn symbols_from_lsp(
    response: Option<lsp_types::DocumentSymbolResponse>,
    file: &str,
    text: Option<&str>,
) -> Vec<Symbol> {
    fn flatten(
        symbols: Vec<lsp_types::DocumentSymbol>,
        parent: Option<&str>,
        file: &str,
        text: Option<&str>,
        out: &mut Vec<Symbol>,
    ) {
        for s in symbols {
            let range = text.map_or(s.selection_range, |t| {
                at_name(t, &s.name, s.selection_range)
            });
            out.push(Symbol {
                name: s.name.clone(),
                kind: format!("{:?}", s.kind),
                location: Location::new(file.to_string(), range_from_lsp(range)),
                container_name: parent.map(str::to_string),
            });
            if let Some(children) = s.children {
                flatten(children, Some(&s.name), file, text, out);
            }
        }
    }

    match response {
        Some(lsp_types::DocumentSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .map(|s| {
                let mut location = s.location;
                if let Some(t) = text {
                    location.range = at_name(t, &s.name, location.range);
                }
                Symbol {
                    name: s.name,
                    kind: format!("{:?}", s.kind),
                    location: location_from_lsp(location),
                    container_name: s.container_name,
                }
            })
            .collect(),
        Some(lsp_types::DocumentSymbolResponse::Nested(symbols)) => {
            let mut out = Vec::new();
            flatten(symbols, None, file, text, &mut out);
            out
        }
        None => Vec::new(),
    }
}

/// `range` within the document `file` holds ([`within_text`]), or as it is
/// where the file cannot be read.
pub(super) fn within_document(file: &Path, range: lsp_types::Range) -> lsp_types::Range {
    match std::fs::read_to_string(file) {
        Ok(text) => within_text(&text, range),
        Err(_) => range,
    }
}

/// `range` within `text`: a position past its last line is its end, and one
/// past the end of its line is that line's end, as LSP has a server take it.
/// Servers refuse a range that reaches past the document (clangd's "Line
/// value is out of range", rust-analyzer's "Invalid offset"), which a caller
/// asking about a whole file may well give. Columns count UTF-16 code units.
pub(super) fn within_text(text: &str, range: lsp_types::Range) -> lsp_types::Range {
    // A final newline ends the document on an empty line.
    let lines: Vec<&str> = text.split('\n').collect();
    let last = u32::try_from(lines.len() - 1).unwrap_or(u32::MAX);
    let within = |position: lsp_types::Position| {
        let (line, character) = if position.line > last {
            (last, u32::MAX)
        } else {
            (position.line, position.character)
        };
        let text = lines[line as usize];
        let text = text.strip_suffix('\r').unwrap_or(text);
        let length = u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX);
        lsp_types::Position {
            line,
            character: character.min(length),
        }
    };
    lsp_types::Range {
        start: within(range.start),
        end: within(range.end),
    }
}

/// The range of `name` in `text`, for a symbol the server put at `range`:
/// `range` where the text there starts with the name, else the name's first
/// whole-word occurrence inside `range` on a line that is no comment, and
/// `range` itself where the name is not in it. FSAutoComplete starts a
/// symbol's selection range at its doc comment, and a flat answer gives a
/// declaration's whole range (`package com.tidewater.berths;` for the
/// package): a position there is not on the symbol, and asked about it, a
/// server finds nothing. The name is also looked for without its signature
/// (`overlaps(BerthWindow)`) and its qualifier (`Revenue.total`). Columns
/// count UTF-16 code units, as the server's do.
pub(super) fn at_name(text: &str, name: &str, range: lsp_types::Range) -> lsp_types::Range {
    fn utf16(s: &str) -> u32 {
        u32::try_from(s.encode_utf16().count()).unwrap_or(u32::MAX)
    }
    /// The byte offset of the UTF-16 column `character` in `line`.
    fn byte_at(line: &str, character: u32) -> Option<usize> {
        let mut units = 0;
        for (byte, c) in line.char_indices() {
            if units >= character {
                return Some(byte);
            }
            units += u32::try_from(c.len_utf16()).unwrap_or(2);
        }
        (units >= character).then_some(line.len())
    }
    fn identifier(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }
    const COMMENTS: &[&str] = &["//", "#", "--", "/*", "*", "(*", ";"];

    let lines: Vec<&str> = text.lines().collect();
    let mut names = vec![name];
    for part in [
        name.split(['(', '<', ' ', ':']).next(),
        name.rsplit(['.', ':']).next(),
    ]
    .into_iter()
    .flatten()
    {
        if !part.is_empty() && !names.contains(&part) {
            names.push(part);
        }
    }
    let at = |line: u32, character: u32, n: &str| {
        lines
            .get(line as usize)
            .and_then(|l| byte_at(l, character).map(|b| l[b..].starts_with(n)))
            .unwrap_or(false)
    };
    if names
        .iter()
        .any(|n| at(range.start.line, range.start.character, n))
    {
        return range;
    }
    let last = range.end.line.min(
        u32::try_from(lines.len())
            .unwrap_or(u32::MAX)
            .saturating_sub(1),
    );
    for n in &names {
        for line_no in range.start.line..=last {
            let Some(line) = lines.get(line_no as usize) else {
                break;
            };
            if COMMENTS.iter().any(|c| line.trim_start().starts_with(c)) {
                continue;
            }
            for (byte, _) in line.match_indices(n) {
                let column = utf16(&line[..byte]);
                let end = column + utf16(n);
                let whole_word = !line[..byte].chars().next_back().is_some_and(identifier)
                    && !line[byte + n.len()..]
                        .chars()
                        .next()
                        .is_some_and(identifier);
                let inside = (line_no > range.start.line || column >= range.start.character)
                    && (line_no < range.end.line || end <= range.end.character);
                if whole_word && inside {
                    return lsp_types::Range {
                        start: lsp_types::Position {
                            line: line_no,
                            character: column,
                        },
                        end: lsp_types::Position {
                            line: line_no,
                            character: end,
                        },
                    };
                }
            }
        }
    }
    range
}

/// Convert an LSP location to our Location type.
pub(super) fn location_from_lsp(loc: lsp_types::Location) -> Location {
    Location::new(uri_to_path(&loc.uri), range_from_lsp(loc.range))
}

/// Convert an LSP range to our Range type.
///
/// Both LSP positions and our `Position2D`/`Range` types are 0-indexed,
/// so this is a direct passthrough.
pub(super) fn range_from_lsp(range: lsp_types::Range) -> Range {
    Range {
        start: Position2D {
            line: range.start.line,
            character: range.start.character,
        },
        end: Position2D {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

/// Convert a `MarkedString` to a String.
///
/// lsp-types 0.97 `MarkedString` doesn't implement Display.
pub(super) fn marked_string_to_string(ms: lsp_types::MarkedString) -> String {
    match ms {
        lsp_types::MarkedString::String(s) => s,
        lsp_types::MarkedString::LanguageString(ls) => {
            format!(
                "```{language}\n{value}\n```",
                language = ls.language,
                value = ls.value
            )
        }
    }
}

/// Convert our Range type to LSP Range.
pub(super) fn range_to_lsp(range: Range) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position {
            line: range.start.line,
            character: range.start.character,
        },
        end: lsp_types::Position {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

/// Parse a symbol kind string to `lsp_types::SymbolKind`.
pub(super) fn parse_symbol_kind(kind: &str) -> lsp_types::SymbolKind {
    match kind {
        "file" => lsp_types::SymbolKind::FILE,
        "module" => lsp_types::SymbolKind::MODULE,
        "namespace" => lsp_types::SymbolKind::NAMESPACE,
        "package" => lsp_types::SymbolKind::PACKAGE,
        "class" => lsp_types::SymbolKind::CLASS,
        "method" => lsp_types::SymbolKind::METHOD,
        "property" => lsp_types::SymbolKind::PROPERTY,
        "field" => lsp_types::SymbolKind::FIELD,
        "constructor" => lsp_types::SymbolKind::CONSTRUCTOR,
        "enum" => lsp_types::SymbolKind::ENUM,
        "interface" => lsp_types::SymbolKind::INTERFACE,
        "function" => lsp_types::SymbolKind::FUNCTION,
        "variable" => lsp_types::SymbolKind::VARIABLE,
        "constant" => lsp_types::SymbolKind::CONSTANT,
        "string" => lsp_types::SymbolKind::STRING,
        "number" => lsp_types::SymbolKind::NUMBER,
        "boolean" => lsp_types::SymbolKind::BOOLEAN,
        "array" => lsp_types::SymbolKind::ARRAY,
        "object" => lsp_types::SymbolKind::OBJECT,
        "key" => lsp_types::SymbolKind::KEY,
        "null" => lsp_types::SymbolKind::NULL,
        "enumMember" => lsp_types::SymbolKind::ENUM_MEMBER,
        "struct" => lsp_types::SymbolKind::STRUCT,
        "event" => lsp_types::SymbolKind::EVENT,
        "operator" => lsp_types::SymbolKind::OPERATOR,
        "typeParameter" => lsp_types::SymbolKind::TYPE_PARAMETER,
        _ => lsp_types::SymbolKind::VARIABLE,
    }
}

/// Convert LSP `CallHierarchyItem` to our type.
pub(super) fn call_hierarchy_item_from_lsp(
    item: lsp_types::CallHierarchyItem,
) -> CallHierarchyItem {
    let file_path = uri_to_path(&item.uri);
    let name = range_from_lsp(item.selection_range);
    CallHierarchyItem {
        name: item.name,
        kind: format!("{:?}", item.kind),
        cite: cite(&file_path, name.start.line, name.start.character),
        location: Location::new(file_path, range_from_lsp(item.range)),
        selection_range: Some(name),
        container_name: None,
        data: item.data.unwrap_or_default(),
    }
}

/// Our `CallHierarchyItem` as the LSP item it came from, for a follow-up
/// request. An item without a `selection_range` — built by a caller rather
/// than returned by `get_call_hierarchy` — is selected by its whole range.
pub(super) fn call_hierarchy_item_to_lsp(
    item: &CallHierarchyItem,
) -> Result<lsp_types::CallHierarchyItem> {
    let selection = item
        .selection_range
        .clone()
        .unwrap_or_else(|| item.location.range.clone());
    Ok(lsp_types::CallHierarchyItem {
        name: item.name.clone(),
        kind: parse_symbol_kind(&item.kind),
        tags: None,
        detail: None,
        uri: path_to_uri(Path::new(&item.location.file_path))?,
        range: range_to_lsp(item.location.range.clone()),
        selection_range: range_to_lsp(selection),
        data: Some(item.data.clone()),
    })
}

/// Convert LSP `TypeHierarchyItem` to our type.
pub(super) fn type_hierarchy_item_from_lsp(
    item: lsp_types::TypeHierarchyItem,
) -> TypeHierarchyItem {
    let file_path = uri_to_path(&item.uri);
    let name = range_from_lsp(item.selection_range);
    TypeHierarchyItem {
        name: item.name,
        kind: format!("{:?}", item.kind),
        cite: cite(&file_path, name.start.line, name.start.character),
        location: Location::new(file_path, range_from_lsp(item.range)),
        selection_range: Some(name),
        data: item.data.unwrap_or_default(),
    }
}

/// Our `TypeHierarchyItem` as the LSP item it came from; see
/// [`call_hierarchy_item_to_lsp`].
pub(super) fn type_hierarchy_item_to_lsp(
    item: &TypeHierarchyItem,
) -> Result<lsp_types::TypeHierarchyItem> {
    let selection = item
        .selection_range
        .clone()
        .unwrap_or_else(|| item.location.range.clone());
    Ok(lsp_types::TypeHierarchyItem {
        name: item.name.clone(),
        kind: parse_symbol_kind(&item.kind),
        tags: None,
        detail: None,
        uri: path_to_uri(Path::new(&item.location.file_path))?,
        range: range_to_lsp(item.location.range.clone()),
        selection_range: range_to_lsp(selection),
        data: Some(item.data.clone()),
    })
}

/// Parse completion kind string.
pub(super) fn parse_completion_kind(kind: &str) -> Option<lsp_types::CompletionItemKind> {
    match kind {
        "text" => Some(lsp_types::CompletionItemKind::TEXT),
        "method" => Some(lsp_types::CompletionItemKind::METHOD),
        "function" => Some(lsp_types::CompletionItemKind::FUNCTION),
        "constructor" => Some(lsp_types::CompletionItemKind::CONSTRUCTOR),
        "field" => Some(lsp_types::CompletionItemKind::FIELD),
        "variable" => Some(lsp_types::CompletionItemKind::VARIABLE),
        "class" => Some(lsp_types::CompletionItemKind::CLASS),
        "interface" => Some(lsp_types::CompletionItemKind::INTERFACE),
        "module" => Some(lsp_types::CompletionItemKind::MODULE),
        "property" => Some(lsp_types::CompletionItemKind::PROPERTY),
        "unit" => Some(lsp_types::CompletionItemKind::UNIT),
        "value" => Some(lsp_types::CompletionItemKind::VALUE),
        "enum" => Some(lsp_types::CompletionItemKind::ENUM),
        "keyword" => Some(lsp_types::CompletionItemKind::KEYWORD),
        "snippet" => Some(lsp_types::CompletionItemKind::SNIPPET),
        "color" => Some(lsp_types::CompletionItemKind::COLOR),
        "file" => Some(lsp_types::CompletionItemKind::FILE),
        "reference" => Some(lsp_types::CompletionItemKind::REFERENCE),
        "folder" => Some(lsp_types::CompletionItemKind::FOLDER),
        "enumMember" => Some(lsp_types::CompletionItemKind::ENUM_MEMBER),
        "constant" => Some(lsp_types::CompletionItemKind::CONSTANT),
        "struct" => Some(lsp_types::CompletionItemKind::STRUCT),
        "event" => Some(lsp_types::CompletionItemKind::EVENT),
        "operator" => Some(lsp_types::CompletionItemKind::OPERATOR),
        "typeParameter" => Some(lsp_types::CompletionItemKind::TYPE_PARAMETER),
        _ => None,
    }
}
