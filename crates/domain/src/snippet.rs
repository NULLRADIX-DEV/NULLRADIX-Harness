//! CSS snippets: small stylesheets a person adds on top of the active theme.

use std::fmt::{self, Write};

use crate::theme::{strip_comments, url_values};
use crate::{PackageError, TextError, Timestamp, Version, check_theme_css, required_text};

pub const SNIPPET_ID_MIN: usize = 2;
pub const SNIPPET_ID_MAX: usize = 40;
pub const SNIPPET_NAME_MAX: usize = 100;
/// Largest snippet in bytes.
pub const SNIPPET_CSS_MAX: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnippetError {
    #[error("a snippet id has {SNIPPET_ID_MIN} to {SNIPPET_ID_MAX} characters from a-z, 0-9 and -")]
    InvalidId,
    #[error("the name {0}")]
    Name(TextError),
    #[error("the snippet has more than {SNIPPET_CSS_MAX} bytes")]
    CssTooLong,
    #[error("the snippet is not allowed: {0}")]
    ForbiddenCss(String),
}

/// Identifier of a snippet, chosen by the person: lowercase letters, digits and hyphens.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SnippetId(String);

impl SnippetId {
    pub fn parse(text: &str) -> Result<Self, SnippetError> {
        let valid = (SNIPPET_ID_MIN..=SNIPPET_ID_MAX).contains(&text.len())
            && text
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        valid
            .then(|| Self(text.to_owned()))
            .ok_or(SnippetError::InvalidId)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SnippetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The label of a snippet: not blank, trimmed, at most [`SNIPPET_NAME_MAX`] characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetName(String);

impl SnippetName {
    pub fn parse(text: &str) -> Result<Self, SnippetError> {
        required_text(text, SNIPPET_NAME_MAX)
            .map(|name| Self(name.to_owned()))
            .map_err(SnippetError::Name)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Style text that passed the checks; line ends are `\n`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetCss(String);

impl SnippetCss {
    /// Refuses what a theme stylesheet refuses, and every `url()` that is not `data:`.
    pub fn parse(text: &str) -> Result<Self, SnippetError> {
        if text.len() > SNIPPET_CSS_MAX {
            return Err(SnippetError::CssTooLong);
        }
        // a lone CR stays refused by the theme check
        let css = text.replace("\r\n", "\n");
        check_theme_css(&css).map_err(|error| match error {
            PackageError::ForbiddenCss(reason) => SnippetError::ForbiddenCss(reason),
            other => SnippetError::ForbiddenCss(other.to_string()),
        })?;
        // a snippet has no files of its own to point at
        let plain = strip_comments(&css);
        if let Some(value) = url_values(&plain)
            .into_iter()
            .find(|value| !value.to_ascii_lowercase().starts_with("data:"))
        {
            return Err(SnippetError::ForbiddenCss(format!(
                "only data: urls are allowed: {value}"
            )));
        }
        Ok(Self(css))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Every field of a stored snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetRecord {
    pub id: SnippetId,
    pub name: String,
    pub css: String,
    pub enabled: bool,
    /// Position in the delivered stylesheet; later snippets win.
    pub order: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet(SnippetRecord);

impl Snippet {
    #[must_use]
    pub fn create(
        id: SnippetId,
        name: &SnippetName,
        css: &SnippetCss,
        enabled: bool,
        order: i64,
        now: Timestamp,
    ) -> Self {
        Self(SnippetRecord {
            id,
            name: name.as_str().to_owned(),
            css: css.as_str().to_owned(),
            enabled,
            order,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    /// The same snippet with the given fields changed; `None` leaves a field as it is.
    #[must_use]
    pub fn changed(
        &self,
        name: Option<&SnippetName>,
        css: Option<&SnippetCss>,
        enabled: Option<bool>,
        now: Timestamp,
    ) -> Self {
        let mut record = self.0.clone();
        if let Some(name) = name {
            name.as_str().clone_into(&mut record.name);
        }
        if let Some(css) = css {
            css.as_str().clone_into(&mut record.css);
        }
        if let Some(enabled) = enabled {
            record.enabled = enabled;
        }
        if record == self.0 {
            return self.clone();
        }
        record.updated_at = now.max(self.0.updated_at);
        record.version = self.0.version.next();
        Self(record)
    }

    #[must_use]
    pub const fn record(&self) -> &SnippetRecord {
        &self.0
    }

    /// For stores only: trusts the record as stored.
    #[must_use]
    pub const fn restore(record: SnippetRecord) -> Self {
        Self(record)
    }
}

/// The enabled snippets as one stylesheet, each behind a comment with its id.
#[must_use]
pub fn active_stylesheet(snippets: &[Snippet]) -> String {
    let mut ordered: Vec<&SnippetRecord> = snippets
        .iter()
        .map(Snippet::record)
        .filter(|record| record.enabled)
        .collect();
    ordered.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    ordered
        .into_iter()
        .fold(String::new(), |mut sheet, record| {
            let _ = writeln!(sheet, "/* snippet: {} */\n{}", record.id, record.css);
            sheet
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    fn snippet(id: &str, css: &str, enabled: bool, order: i64) -> Snippet {
        Snippet::create(
            SnippetId::parse(id).unwrap(),
            &SnippetName::parse("Name").unwrap(),
            &SnippetCss::parse(css).unwrap(),
            enabled,
            order,
            at(1),
        )
    }

    #[test]
    fn ids_follow_the_theme_rule() {
        for valid in ["ab", "red-links", "a1", &"a".repeat(SNIPPET_ID_MAX)] {
            assert!(SnippetId::parse(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "a",
            "A1",
            "a_b",
            "a b",
            "\u{e4}1",
            &"a".repeat(SNIPPET_ID_MAX + 1),
        ] {
            assert_eq!(
                SnippetId::parse(invalid),
                Err(SnippetError::InvalidId),
                "{invalid}"
            );
        }
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(SnippetName::parse("  Red  ").unwrap().as_str(), "Red");
        assert_eq!(
            SnippetName::parse(" "),
            Err(SnippetError::Name(TextError::Empty))
        );
        assert!(SnippetName::parse(&"n".repeat(SNIPPET_NAME_MAX)).is_ok());
        assert_eq!(
            SnippetName::parse(&"n".repeat(SNIPPET_NAME_MAX + 1)),
            Err(SnippetError::Name(TextError::TooLong(SNIPPET_NAME_MAX)))
        );
    }

    #[test]
    fn css_is_bounded_in_bytes() {
        let filler = "a{}".repeat(SNIPPET_CSS_MAX / 3);
        assert!(SnippetCss::parse(&filler).is_ok());
        let over = format!("{filler}a{{}}");
        assert_eq!(SnippetCss::parse(&over), Err(SnippetError::CssTooLong));
        // multi-byte letters count by bytes
        let wide = "/*".to_owned() + &"\u{e4}".repeat(SNIPPET_CSS_MAX / 2) + "*/";
        assert_eq!(SnippetCss::parse(&wide), Err(SnippetError::CssTooLong));
        assert!(SnippetCss::parse("").is_ok());
    }

    #[test]
    fn css_may_not_import_or_reach_out() {
        assert!(SnippetCss::parse("a { color: red }").is_ok());
        assert!(SnippetCss::parse("a { background: url(data:image/png;base64,AAAA) }").is_ok());
        assert!(SnippetCss::parse("/* @import 'x'; url(http://x) */ a {}").is_ok());
        for css in [
            "@import 'x.css';",
            "@IMPORT url(a.css);",
            "a { background: url(https://evil.test/a.png) }",
            "a { background: url('//evil.test/a.png') }",
            "a { background: url(javascript:alert(1)) }",
            "a { background: image-set(\"https://evil/x.png\" 1x) }",
            "a { background: \\75 rl(https://evil/x) }",
            "a { background: url(local.png) }",
            "a { background: url(\"assets/a.png\") }",
            "a { background: url(/abs.png) }",
        ] {
            assert!(
                matches!(SnippetCss::parse(css), Err(SnippetError::ForbiddenCss(_))),
                "{css}"
            );
        }
    }

    #[test]
    fn crlf_is_taken_as_a_newline_and_a_lone_cr_is_refused() {
        assert_eq!(
            SnippetCss::parse("a {\r\n}\r\n").unwrap().as_str(),
            "a {\n}\n"
        );
        assert!(matches!(
            SnippetCss::parse("a {\r}"),
            Err(SnippetError::ForbiddenCss(_))
        ));
    }

    #[test]
    fn a_change_bumps_the_version_only_when_something_differs() {
        let first = snippet("red", "a{}", true, 1);
        let same = first.changed(None, None, Some(true), at(5));
        assert_eq!(same, first);
        let off = first.changed(None, None, Some(false), at(5));
        assert_eq!(off.record().version, Version::from_stored(2));
        assert_eq!(off.record().updated_at, at(5));
        assert!(!off.record().enabled);
        let name = SnippetName::parse("Other").unwrap();
        let renamed = off.changed(Some(&name), None, None, at(2));
        assert_eq!(renamed.record().name, "Other");
        assert_eq!(renamed.record().updated_at, at(5));
        let css = SnippetCss::parse("b{}").unwrap();
        assert_eq!(
            first.changed(None, Some(&css), None, at(9)).record().css,
            "b{}"
        );
    }

    #[test]
    fn the_stylesheet_holds_enabled_snippets_in_order() {
        let list = [
            snippet("two", "b{}", true, 2),
            snippet("one", "a{}", true, 1),
            snippet("off", "c{}", false, 3),
            snippet("tie", "d{}", true, 2),
        ];
        assert_eq!(
            active_stylesheet(&list),
            "/* snippet: one */\na{}\n/* snippet: tie */\nd{}\n/* snippet: two */\nb{}\n"
        );
        assert_eq!(active_stylesheet(&[]), "");
    }
}
