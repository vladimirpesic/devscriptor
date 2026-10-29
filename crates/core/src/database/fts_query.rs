//! Turning a search as people write it into an FTS5 query.
//!
//! FTS5 reads whitespace as AND and gives its punctuation meaning. Passed
//! through raw, "where are shipping costs computed?" would match only an
//! entity containing every one of those words — "where" and "are" included —
//! and a stray quote, parenthesis or hyphen would make the query invalid. So a
//! question in words would find nothing without embeddings, and a keyword
//! search would break on a `-`.
//!
//! Here every term is quoted, so nothing in the input is FTS5 syntax; the
//! terms are ORed, so BM25 ranks an entity by how many of them it has and how
//! rare they are; words that carry no meaning in code are dropped; an
//! identifier also contributes its parts, as `name_tokens` indexes them; and a
//! word with a common English ending also matches as a prefix of its stem, so
//! "costs" finds `cost` and "computed" finds `compute`.

use super::embeddings::split_identifier_tokens;

/// Words that say nothing about which code is meant.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "been", "but", "by", "can", "could", "did", "do",
    "does", "done", "for", "from", "had", "has", "have", "how", "i", "if", "in", "into", "is",
    "it", "its", "me", "my", "of", "on", "or", "our", "should", "so", "some", "than", "that",
    "the", "their", "them", "then", "there", "these", "they", "this", "those", "to", "us", "was",
    "we", "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will", "with",
    "would", "you", "your",
];

/// Endings stripped to find a stem that also matches as a prefix, longest
/// first so "-ing" is not read as "-s".
const SUFFIXES: &[&str] = &["ing", "ies", "ed", "es", "s"];

/// The shortest stem worth a prefix match: shorter prefixes match too much.
const MIN_STEM: usize = 3;

/// The FTS5 query for `raw`, or `None` when it holds no searchable term.
#[must_use]
pub fn match_query(raw: &str) -> Option<String> {
    let mut clauses: Vec<String> = Vec::new();
    let mut add = |clause: String| {
        if !clauses.contains(&clause) {
            clauses.push(clause);
        }
    };

    for word in raw
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
    {
        let lower = word.to_lowercase();
        let parts: Vec<String> = split_identifier_tokens(word)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        if parts.len() > 1 {
            // An identifier: as written (a camelCase body token), as its
            // parts in order (`name_tokens`, a snake_case body), and part by
            // part.
            add(quoted(&lower));
            add(quoted(&parts.join(" ")));
            for part in parts.iter().filter(|part| part.chars().count() > 1) {
                add(quoted(part));
            }
            continue;
        }
        if lower.chars().count() < 2 || STOP_WORDS.contains(&lower.as_str()) {
            continue;
        }
        add(quoted(&lower));
        if let Some(stem) = stem(&lower) {
            add(format!("{}*", quoted(stem)));
        }
    }

    (!clauses.is_empty()).then(|| clauses.join(" OR "))
}

/// `term` as an FTS5 string: taken literally, whatever it contains.
fn quoted(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// `word` without a common English ending, when enough is left to match on.
fn stem(word: &str) -> Option<&str> {
    let letters = word.chars().all(char::is_alphabetic);
    if !letters {
        return None;
    }
    SUFFIXES.iter().find_map(|suffix| {
        let stem = word.strip_suffix(suffix)?;
        (stem.chars().count() >= MIN_STEM).then_some(stem)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_becomes_its_meaningful_words_ored() {
        assert_eq!(
            match_query("where are shipping costs computed?").as_deref(),
            Some(r#""shipping" OR "shipp"* OR "costs" OR "cost"* OR "computed" OR "comput"*"#)
        );
    }

    #[test]
    fn identifiers_match_whole_and_in_parts() {
        assert_eq!(
            match_query("applyBulkDiscount").as_deref(),
            Some(
                r#""applybulkdiscount" OR "apply bulk discount" OR "apply" OR "bulk" OR "discount""#
            )
        );
        assert_eq!(
            match_query("FREE_SHIPPING_THRESHOLD").as_deref(),
            Some(
                r#""free_shipping_threshold" OR "free shipping threshold" OR "free" OR "shipping" OR "threshold""#
            )
        );
    }

    /// Nothing in the input is FTS5 syntax: operators, quotes and column
    /// filters are words or separators, never commands.
    #[test]
    fn fts5_syntax_in_the_input_is_inert() {
        let query = match_query(r#"name:"x" AND (y OR -z) NEAR/2 w*"#).unwrap();
        assert!(!query.contains("NEAR/"), "{query}");
        assert!(!query.contains('('), "{query}");
        assert!(
            query.split(" OR ").all(|clause| clause.starts_with('"')),
            "{query}"
        );
    }

    #[test]
    fn nothing_searchable_is_none() {
        assert_eq!(match_query("   "), None);
        assert_eq!(match_query("where is it?"), None);
        assert_eq!(match_query("?!"), None);
    }

    /// The built query runs against a real FTS5 table and finds what the raw
    /// one could not.
    #[test]
    fn the_query_finds_what_the_raw_text_could_not() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(name, body, tokenize='unicode61 separators ''_.$/\\-@#''');
             INSERT INTO t VALUES ('compute_shipping_cost', 'return flat shipping below the threshold');
             INSERT INTO t VALUES ('render_banner', 'draw the banner');",
        )
        .unwrap();
        let hits = |query: &str| -> rusqlite::Result<Vec<String>> {
            let mut stmt = conn.prepare("SELECT name FROM t WHERE t MATCH ?1 ORDER BY rank")?;
            stmt.query_map([query], |row| row.get(0))?.collect()
        };
        let question = "where are shipping costs computed?";
        assert!(hits(question).is_err(), "raw, the `?` is a syntax error");
        assert_eq!(
            hits("where are shipping costs computed").unwrap(),
            Vec::<String>::new(),
            "raw, every word is required"
        );
        assert_eq!(
            hits(&match_query(question).unwrap()).unwrap(),
            ["compute_shipping_cost"]
        );
    }
}
