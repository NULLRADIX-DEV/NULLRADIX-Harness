//! Customizations: a person's own skill or output style, replacing a shipped one or added to them.

use strum::{EnumString, IntoStaticStr};

use crate::{Timestamp, Version};

pub const OVERLAY_NAME_MAX: usize = 64;
pub const OVERLAY_CONTENT_MAX: usize = 200_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OverlayError {
    #[error(
        "a customization name uses lowercase letters, digits and single hyphens, at most {OVERLAY_NAME_MAX} characters"
    )]
    InvalidName,
    #[error("the content is empty")]
    EmptyContent,
    #[error("the content has more than {OVERLAY_CONTENT_MAX} characters")]
    ContentTooLong,
}

/// Where a customization takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum ExtensionPoint {
    Skill,
    OutputStyle,
}

impl ExtensionPoint {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Replaces a shipped file or comes on top of the shipped ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum OverlayKind {
    Replace,
    New,
}

impl OverlayKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// Name of a skill or output style: lowercase words joined by single hyphens.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OverlayName(String);

impl OverlayName {
    pub fn parse(text: &str) -> Result<Self, OverlayError> {
        let valid = !text.is_empty()
            && text.len() <= OVERLAY_NAME_MAX
            && text.split('-').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            });
        valid
            .then(|| Self(text.to_owned()))
            .ok_or(OverlayError::InvalidName)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OverlayName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The content of a customization: not blank, not huge.
pub fn overlay_content(text: &str) -> Result<&str, OverlayError> {
    if text.trim().is_empty() {
        Err(OverlayError::EmptyContent)
    } else if text.chars().count() > OVERLAY_CONTENT_MAX {
        Err(OverlayError::ContentTooLong)
    } else {
        Ok(text)
    }
}

/// The shipped file a customization started from, to tell later whether it moved on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    pub content: String,
    pub hash: String,
}

/// Every field of a stored customization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayRecord {
    pub point: ExtensionPoint,
    pub name: OverlayName,
    pub kind: OverlayKind,
    pub content: String,
    pub base: Option<Base>,
    pub enabled: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay(OverlayRecord);

impl Overlay {
    #[must_use]
    pub const fn create(
        point: ExtensionPoint,
        name: OverlayName,
        kind: OverlayKind,
        content: String,
        base: Option<Base>,
        now: Timestamp,
    ) -> Self {
        Self(OverlayRecord {
            point,
            name,
            kind,
            content,
            base,
            enabled: true,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    fn touched(&self, now: Timestamp) -> OverlayRecord {
        OverlayRecord {
            updated_at: now.max(self.0.updated_at),
            version: self.0.version.next(),
            ..self.0.clone()
        }
    }

    #[must_use]
    pub fn with_content(&self, content: String, now: Timestamp) -> Self {
        Self(OverlayRecord {
            content,
            ..self.touched(now)
        })
    }

    #[must_use]
    pub fn with_enabled(&self, enabled: bool, now: Timestamp) -> Self {
        Self(OverlayRecord {
            enabled,
            ..self.touched(now)
        })
    }

    /// The same overlay against a newer shipped file: only the base moves.
    #[must_use]
    pub fn rebased(&self, base: Base, now: Timestamp) -> Self {
        Self(OverlayRecord {
            base: Some(base),
            ..self.touched(now)
        })
    }

    #[must_use]
    pub const fn record(&self) -> &OverlayRecord {
        &self.0
    }

    /// For stores only: trusts the record as stored.
    #[must_use]
    pub const fn restore(record: OverlayRecord) -> Self {
        Self(record)
    }
}

/// A reason an overlay does not apply, or a hint that leaves it applying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub code: &'static str,
    pub message: String,
}

impl Finding {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The head of a Markdown file, already read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrontMatter {
    pub name: Option<String>,
    pub description: Option<String>,
    /// First line of the reading error, when the head is there but unreadable.
    pub error: Option<String>,
}

/// Head readable, `description` set, and a `name` that agrees with the overlay's name.
#[must_use]
pub fn check_front_matter(name: &OverlayName, head: &FrontMatter) -> Vec<Finding> {
    if let Some(error) = &head.error {
        return vec![Finding::new(
            "front-matter-invalid",
            format!("the head is no valid YAML: {error}"),
        )];
    }
    let mut problems = Vec::new();
    if head.description.is_none() {
        problems.push(Finding::new(
            "description-missing",
            "the head has no description",
        ));
    }
    if let Some(declared) = &head.name
        && declared != name.as_str()
    {
        problems.push(Finding::new(
            "name-mismatch",
            format!("the name in the head ({declared}) is not {name}"),
        ));
    }
    problems
}

/// How a customization's starting point compares with the shipped file today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum BaseState {
    /// New customizations have nothing to compare with.
    None,
    Current,
    Outdated,
    /// A replacement that never recorded its starting point.
    Unknown,
}

impl BaseState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// `standard_hash` is the hash of the shipped file now, `None` when there is none.
#[must_use]
pub fn base_state(
    kind: OverlayKind,
    base: Option<&Base>,
    standard_hash: Option<&str>,
) -> BaseState {
    match (kind, base, standard_hash) {
        (OverlayKind::New, ..) => BaseState::None,
        (OverlayKind::Replace, Some(base), Some(now)) => {
            if base.hash == now {
                BaseState::Current
            } else {
                BaseState::Outdated
            }
        }
        (OverlayKind::Replace, ..) => BaseState::Unknown,
    }
}

/// Whether the customization applies, replaces nothing yet, or the shipped file stays in charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum OverlayState {
    Active,
    /// Problems send the person back to the shipped file.
    Fallback,
    Disabled,
}

impl OverlayState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[must_use]
pub fn overlay_state(enabled: bool, problems: &[Finding]) -> OverlayState {
    if !enabled {
        OverlayState::Disabled
    } else if problems.is_empty() {
        OverlayState::Active
    } else {
        OverlayState::Fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> OverlayName {
        OverlayName::parse(text).unwrap()
    }

    #[test]
    fn names_are_lowercase_words_with_single_hyphens() {
        for good in ["review", "my-review", "a1-b2", &"a".repeat(64)] {
            assert!(OverlayName::parse(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "Review",
            "a--b",
            "-a",
            "a-",
            "a b",
            "a_b",
            "../x",
            "a/b",
            &"a".repeat(65),
        ] {
            assert_eq!(
                OverlayName::parse(bad),
                Err(OverlayError::InvalidName),
                "{bad}"
            );
        }
    }

    #[test]
    fn content_is_neither_blank_nor_huge() {
        assert_eq!(overlay_content("hello"), Ok("hello"));
        assert_eq!(overlay_content(" \n"), Err(OverlayError::EmptyContent));
        let huge = "a".repeat(OVERLAY_CONTENT_MAX + 1);
        assert_eq!(overlay_content(&huge), Err(OverlayError::ContentTooLong));
        assert!(overlay_content(&huge[1..]).is_ok());
    }

    #[test]
    fn points_and_kinds_have_kebab_case_names() {
        assert_eq!(ExtensionPoint::OutputStyle.as_str(), "output-style");
        assert_eq!("skill".parse(), Ok(ExtensionPoint::Skill));
        assert!("workflow".parse::<ExtensionPoint>().is_err());
        assert_eq!(OverlayKind::Replace.as_str(), "replace");
        assert_eq!(BaseState::Outdated.as_str(), "outdated");
        assert_eq!(OverlayState::Fallback.as_str(), "fallback");
    }

    #[test]
    fn a_head_needs_a_readable_description_and_a_matching_name() {
        let good = FrontMatter {
            name: Some(String::from("review")),
            description: Some(String::from("Reviews")),
            error: None,
        };
        assert_eq!(check_front_matter(&name("review"), &good).len(), 0);
        let no_name = FrontMatter {
            name: None,
            ..good.clone()
        };
        assert_eq!(check_front_matter(&name("review"), &no_name).len(), 0);
        let codes = |head: &FrontMatter| -> Vec<&'static str> {
            check_front_matter(&name("review"), head)
                .iter()
                .map(|problem| problem.code)
                .collect()
        };
        assert_eq!(
            codes(&FrontMatter {
                description: None,
                name: Some(String::from("other")),
                error: None
            }),
            ["description-missing", "name-mismatch"]
        );
        assert_eq!(
            codes(&FrontMatter {
                error: Some(String::from("mapping values")),
                ..good
            }),
            ["front-matter-invalid"]
        );
    }

    #[test]
    fn a_base_is_current_outdated_or_unknown() {
        let base = Base {
            content: String::from("old"),
            hash: String::from("h1"),
        };
        let state = |kind, base: Option<&Base>, now| base_state(kind, base, now);
        assert_eq!(state(OverlayKind::New, None, None), BaseState::None);
        assert_eq!(
            state(OverlayKind::New, Some(&base), Some("h2")),
            BaseState::None
        );
        assert_eq!(
            state(OverlayKind::Replace, Some(&base), Some("h1")),
            BaseState::Current
        );
        assert_eq!(
            state(OverlayKind::Replace, Some(&base), Some("h2")),
            BaseState::Outdated
        );
        assert_eq!(
            state(OverlayKind::Replace, None, Some("h1")),
            BaseState::Unknown
        );
        assert_eq!(
            state(OverlayKind::Replace, Some(&base), None),
            BaseState::Unknown
        );
    }

    #[test]
    fn problems_send_an_enabled_overlay_back_to_the_shipped_file() {
        let problem = [Finding::new("x", "y")];
        assert_eq!(overlay_state(true, &[]), OverlayState::Active);
        assert_eq!(overlay_state(true, &problem), OverlayState::Fallback);
        assert_eq!(overlay_state(false, &[]), OverlayState::Disabled);
        assert_eq!(overlay_state(false, &problem), OverlayState::Disabled);
    }

    #[test]
    fn changes_bump_the_version_and_keep_the_rest() {
        let first = Overlay::create(
            ExtensionPoint::Skill,
            name("review"),
            OverlayKind::New,
            String::from("a"),
            None,
            Timestamp::from_millis(5),
        );
        let edited = first.with_content(String::from("b"), Timestamp::from_millis(9));
        assert_eq!(edited.record().content, "b");
        assert_eq!(edited.record().version, Version::from_stored(2));
        assert_eq!(edited.record().created_at, Timestamp::from_millis(5));
        assert_eq!(edited.record().updated_at, Timestamp::from_millis(9));
        let off = edited.with_enabled(false, Timestamp::from_millis(1));
        assert!(!off.record().enabled);
        assert_eq!(off.record().updated_at, Timestamp::from_millis(9));
        let base = Base {
            content: String::from("s"),
            hash: String::from("h"),
        };
        let rebased = off.rebased(base.clone(), Timestamp::from_millis(10));
        assert_eq!(rebased.record().base, Some(base));
        assert_eq!(rebased.record().content, "b");
        assert_eq!(rebased.record().version, Version::from_stored(4));
    }
}
