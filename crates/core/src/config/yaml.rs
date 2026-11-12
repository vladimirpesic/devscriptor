//! YAML for `devscriptor.yaml` and configuration templates, read and written
//! with `serde-saphyr`.
//!
//! The rules are YAML 1.2's: only `true` and `false` (in any case) are
//! booleans, so `yes` and `on` stay text. A key with no value, `~` or `null`
//! means its field's default where the field has one ([`or_default`]), and a
//! document with nothing in it is no value at all ([`parse`]). An error is one
//! line that names its line and quotes it.

use serde::Serialize;
use serde::de::{Deserialize, DeserializeOwned, Deserializer};

/// Parse `text` as a `T`; `None` for a document with nothing in it (no text,
/// only comments, `---` or `~`).
///
/// # Errors
/// Returns what makes `text` no `T`, with the line it is on.
pub fn parse<T: DeserializeOwned>(text: &str) -> Result<Option<T>, String> {
    let options = serde_saphyr::options! { strict_booleans: true };
    serde_saphyr::from_str_with_options::<Option<T>>(text, options)
        .map_err(|error| describe(&error, text))
}

/// `value` as YAML.
///
/// # Errors
/// Returns an error if `value` cannot be written as YAML.
pub fn to_string<T: Serialize>(value: &T) -> Result<String, serde_saphyr::SerializeError> {
    serde_saphyr::to_string(value)
}

/// One line saying what is wrong and where: the parser's message, then the
/// line it points at.
fn describe(error: &serde_saphyr::Error, text: &str) -> String {
    let message = error.render_with_options(serde_saphyr::render_options! {
        snippets: serde_saphyr::SnippetMode::Off,
    });
    let line = error
        .location()
        .map(|at| at.line())
        .filter(|&line| line > 0)
        .and_then(|line| text.lines().nth(usize::try_from(line - 1).ok()?))
        .map(str::trim)
        .filter(|line| !line.is_empty());
    match line {
        Some(line) => format!("{message}: `{line}`"),
        None => message,
    }
}

/// Field attribute (`deserialize_with`): a value of `null` is the field's
/// default.
///
/// # Errors
/// Returns the deserializer's error for a value that is neither null nor a `T`.
pub fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Settings {
        #[serde(default, deserialize_with = "or_default")]
        include: Vec<String>,
        #[serde(default, deserialize_with = "or_default")]
        nested: Nested,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        label: Option<String>,
    }

    #[derive(Debug, Default, PartialEq, Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Nested {
        #[serde(default)]
        size: Option<u64>,
    }

    fn settings(text: &str) -> Result<Settings, String> {
        parse(text).map(Option::unwrap_or_default)
    }

    /// A key left without a value, or set to `~` or `null`, is its default;
    /// a document with nothing in it is no value.
    #[test]
    fn nothing_means_the_default() {
        let read = settings("include:\nnested: ~\nlabel: null\n").unwrap();
        assert_eq!(read, Settings::default());
        for empty in ["", "\n", "# only a comment\n", "---\n", "~"] {
            assert_eq!(parse::<Settings>(empty).unwrap(), None, "{empty:?}");
        }
    }

    /// Only YAML 1.2's spellings are booleans: `yes` is no boolean, and a
    /// text field keeps it as text.
    #[test]
    fn booleans_are_yaml_1_2s() {
        for (text, enabled) in [("true", true), ("False", false), ("TRUE", true)] {
            let read = settings(&format!("enabled: {text}")).unwrap();
            assert_eq!(read.enabled, Some(enabled), "{text}");
        }
        for text in ["yes", "off", "y"] {
            assert!(settings(&format!("enabled: {text}")).is_err(), "{text}");
            let read = settings(&format!("label: {text}")).unwrap();
            assert_eq!(read.label.as_deref(), Some(text));
        }
    }

    /// An error is one line that names its line and quotes it.
    #[test]
    fn an_error_names_and_quotes_its_line() {
        let error = settings("include: [rs]\nnested:\n  sise: 3\n").unwrap_err();
        assert!(error.contains("unknown field `sise`"), "{error}");
        assert!(error.contains("line 3, column 3"), "{error}");
        assert!(error.ends_with(": `sise: 3`"), "{error}");
        assert!(!error.contains('\n'), "{error}");
    }

    /// What is written reads back the same, text that looks like another
    /// kind of value included.
    #[test]
    fn written_yaml_reads_back() {
        let written = Settings {
            include: ["*.min.js", "**/target", "yes", "1.0", "null", "~", "#x", ""]
                .map(String::from)
                .to_vec(),
            nested: Nested { size: Some(5) },
            enabled: Some(false),
            label: Some("a: b".into()),
        };
        let yaml = to_string(&written).unwrap();
        assert_eq!(settings(&yaml).unwrap(), written, "{yaml}");
    }
}
