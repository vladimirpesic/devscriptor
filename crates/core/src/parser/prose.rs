//! Section extraction for prose documents.
//!
//! Markdown carries the part of a codebase that code cannot state: why a value
//! is what it is, which approach was rejected, what a threshold means. Indexed,
//! it answers "why is the limit 85?" — a question no code entity answers.
//!
//! The unit is a **section**: one heading and the prose beneath it, up to the
//! next heading of the same or higher level. That is the granularity a reader
//! uses and the granularity that embeds well — a whole document is too coarse to
//! retrieve precisely, and a paragraph is too small to stand on its own.
//!
//! Parsed by scanning lines rather than with a grammar. Headings are the only
//! structure needed, ATX (`## Title`) and Setext (`Title` over `---`) alike, and
//! a grammar dependency would buy nothing for it.

use time::OffsetDateTime;
use uuid::Uuid;

use crate::entities::{Documentation, Entity, SourceLocation};

/// A fenced code block's delimiter, so headings inside examples are ignored.
const FENCES: [&str; 2] = ["```", "~~~"];

/// Extract one entity per section of a prose document.
///
/// Text before the first heading becomes a section named for the file, so a
/// document with no headings at all still indexes as one unit rather than
/// vanishing.
pub fn extract_sections(content: &str, file_path: &str, language: &str) -> Vec<Entity> {
    let now = OffsetDateTime::now_utc();
    let lines: Vec<&str> = content.lines().collect();
    let headings = find_headings(&lines);

    let mut sections = Vec::new();

    // The preamble: everything before the first heading.
    let first_heading_line = headings.first().map_or(lines.len(), |h| h.line);
    if first_heading_line > 0 && has_prose(&lines[..first_heading_line]) {
        sections.push(section_entity(
            &file_stem(file_path),
            &lines[..first_heading_line],
            0,
            first_heading_line.saturating_sub(1),
            file_path,
            language,
            now,
        ));
    }

    for (index, heading) in headings.iter().enumerate() {
        // A section runs until the next heading of the same or higher level;
        // deeper headings are part of it, which is what makes a `##` section
        // read as a complete thought rather than being cut off by its own
        // subsections.
        let end = headings[index + 1..]
            .iter()
            .find(|next| next.level <= heading.level)
            .map_or(lines.len(), |next| next.line);

        sections.push(section_entity(
            &heading.title,
            &lines[heading.body_start..end],
            heading.line,
            end.saturating_sub(1),
            file_path,
            language,
            now,
        ));
    }

    sections
}

/// One heading and where its content begins.
struct Heading {
    title: String,
    level: usize,
    /// Line the heading itself is on.
    line: usize,
    /// First line of the section's text, heading included.
    body_start: usize,
}

fn find_headings(lines: &[&str]) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut fenced = false;

    for (index, raw) in lines.iter().enumerate() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        // A heading inside a fenced block is example text, not structure.
        if FENCES.iter().any(|fence| trimmed.starts_with(fence)) {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }

        if let Some(level) = atx_level(trimmed) {
            let title = trimmed[level..].trim().trim_end_matches('#').trim();
            if !title.is_empty() {
                headings.push(Heading {
                    title: title.to_string(),
                    level,
                    line: index,
                    body_start: index,
                });
            }
            continue;
        }

        // Setext: a line of text underlined by `===` (level 1) or `---`
        // (level 2). The underline must follow non-empty text.
        if let Some(level) = setext_level(line)
            && index > 0
        {
            let title = lines[index - 1].trim();
            if !title.is_empty() && atx_level(title).is_none() {
                headings.push(Heading {
                    title: title.to_string(),
                    level,
                    line: index - 1,
                    body_start: index - 1,
                });
            }
        }
    }

    headings
}

/// The level of an ATX heading (`#` … `######`), if this line is one.
fn atx_level(trimmed: &str) -> Option<usize> {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    // `#hashtag` is not a heading: ATX requires a space after the run.
    (1..=6).contains(&hashes).then_some(hashes).filter(|_| {
        trimmed[hashes..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace())
    })
}

/// The level implied by a Setext underline, if this line is one.
fn setext_level(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if trimmed.len() < 2 {
        return None;
    }
    if trimmed.chars().all(|c| c == '=') {
        return Some(1);
    }
    if trimmed.chars().all(|c| c == '-') {
        return Some(2);
    }
    None
}

/// Whether these lines hold anything worth indexing.
fn has_prose(lines: &[&str]) -> bool {
    lines.iter().any(|line| !line.trim().is_empty())
}

fn file_stem(file_path: &str) -> String {
    std::path::Path::new(file_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_path.to_string())
}

#[allow(clippy::too_many_arguments)]
fn section_entity(
    title: &str,
    lines: &[&str],
    start_line: usize,
    end_line: usize,
    file_path: &str,
    language: &str,
    now: OffsetDateTime,
) -> Entity {
    Entity::Section(Documentation {
        id: Uuid::new_v4(),
        name: title.to_string(),
        file_path: file_path.to_string(),
        body: lines.join("\n").trim().to_string(),
        location: SourceLocation {
            // `start_line` and `end_line` index `lines`, from 0; the index
            // counts lines from 1, as for every entity a grammar extracts.
            start_line: u32::try_from(start_line + 1).unwrap_or(u32::MAX),
            end_line: u32::try_from(end_line + 1).unwrap_or(u32::MAX),
            start_column: 0,
            end_column: 0,
        },
        language: language.to_string(),
        doc_type: Some("section".to_string()),
        associated_entity: None,
        associated_entity_type: None,
        created_at: now,
        updated_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(md: &str) -> Vec<(String, String)> {
        extract_sections(md, "docs/THRESHOLDS.md", "markdown")
            .into_iter()
            .map(|e| match e {
                Entity::Section(d) => (d.name, d.body),
                other => panic!("expected a section, got {}", other.entity_type()),
            })
            .collect()
    }

    #[test]
    fn each_heading_becomes_one_section() {
        let out = sections("# Title\n\nintro\n\n## First\n\nalpha\n\n## Second\n\nbeta\n");
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Title", "First", "Second"]);
    }

    /// A `##` section must carry its own `###` subsections, or every section
    /// ends at its first subheading and reads as a fragment.
    #[test]
    fn a_section_contains_its_deeper_subsections() {
        let out = sections("## Parent\n\nlead\n\n### Child\n\ndetail\n\n## Sibling\n\nother\n");
        let parent = &out.iter().find(|(n, _)| n == "Parent").expect("parent").1;
        assert!(parent.contains("lead"), "{parent}");
        assert!(
            parent.contains("Child") && parent.contains("detail"),
            "{parent}"
        );
        assert!(
            !parent.contains("other"),
            "must stop at the next `##`: {parent}"
        );
    }

    /// The canonical value in a design doc is usually stated before any
    /// subheading — losing the preamble loses the point of the document.
    #[test]
    fn text_before_the_first_heading_is_kept() {
        let out = sections("A note with no heading yet.\n\n# Later\n\nbody\n");
        assert_eq!(out[0].0, "THRESHOLDS", "named for the file");
        assert!(out[0].1.contains("no heading yet"), "{:?}", out[0].1);
    }

    #[test]
    fn a_document_without_headings_still_indexes() {
        let out = sections("Just prose, no structure at all.\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "THRESHOLDS");
    }

    /// Comments in a fenced example are not document structure.
    #[test]
    fn headings_inside_code_fences_are_ignored() {
        let out = sections("# Real\n\n```bash\n# not a heading\necho hi\n```\n\n## Also real\n");
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Real", "Also real"]);
    }

    #[test]
    fn setext_headings_are_recognised() {
        let out = sections("Title\n=====\n\nbody\n\nSub\n---\n\nmore\n");
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Title", "Sub"]);
    }

    /// `#tag` is not a heading — ATX requires whitespace after the hashes.
    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        let out = sections("#hashtag stays prose\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "THRESHOLDS");
    }

    /// Sections count lines from 1, like every other entity: a heading on the
    /// first line starts at line 1, and a section ends on its last line.
    #[test]
    fn section_lines_count_from_one() {
        let lines = |md: &str| -> Vec<(String, u32, u32)> {
            extract_sections(md, "README.md", "markdown")
                .into_iter()
                .map(|e| {
                    let location = e.location();
                    (e.name().to_string(), location.start_line, location.end_line)
                })
                .collect()
        };
        assert_eq!(
            lines("# Title\n\nintro\n\n## Rules\n\n1. one\n2. two\n"),
            [("Title".to_string(), 1, 8), ("Rules".to_string(), 5, 8),]
        );
        assert_eq!(
            lines("Preamble.\n\n# Later\n"),
            [("README".to_string(), 1, 2), ("Later".to_string(), 3, 3)]
        );
    }

    #[test]
    fn sections_are_embedded_unlike_docstrings() {
        use crate::embeddings::selection::{EmbeddingSelection, should_embed};
        let section = extract_sections("# T\n\nbody\n", "a.md", "markdown")
            .into_iter()
            .next()
            .expect("one section");
        assert!(
            should_embed(&section, EmbeddingSelection::Standard),
            "a prose section is exactly what semantic search should reach"
        );
    }
}
