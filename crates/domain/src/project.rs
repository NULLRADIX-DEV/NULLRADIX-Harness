use std::fmt;

use crate::{Id, Timestamp, Version};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectError {
    #[error("a project key has 2 to 6 characters, starts with A-Z and uses only A-Z and 0-9")]
    InvalidKey,
    #[error("a project name has 1 to 200 characters")]
    InvalidName,
}

/// Prefix of the project's human keys, such as `ARC` in `ARC-0042`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectKey(String);

impl ProjectKey {
    pub fn parse(text: &str) -> Result<Self, ProjectError> {
        let valid = (2..=6).contains(&text.len())
            && text.starts_with(|c: char| c.is_ascii_uppercase())
            && text
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        valid
            .then(|| Self(text.to_owned()))
            .ok_or(ProjectError::InvalidKey)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectName(String);

impl ProjectName {
    /// Trims surrounding whitespace before checking the length.
    pub fn parse(text: &str) -> Result<Self, ProjectError> {
        let trimmed = text.trim();
        let length = trimmed.chars().count();
        (1..=200)
            .contains(&length)
            .then(|| Self(trimmed.to_owned()))
            .ok_or(ProjectError::InvalidName)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: Id<Project>,
    pub key: ProjectKey,
    pub name: ProjectName,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

impl Project {
    #[must_use]
    pub fn new(id: Id<Self>, key: ProjectKey, name: ProjectName, now: Timestamp) -> Self {
        Self {
            id,
            key,
            name,
            created_at: now,
            updated_at: now,
            version: Version::INITIAL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_keys() {
        for key in ["AR", "ARC", "A1", "NULLR6"] {
            assert_eq!(ProjectKey::parse(key).unwrap().as_str(), key);
        }
    }

    #[test]
    fn rejects_invalid_keys() {
        for key in ["", "A", "1AB", "arc", "AR-C", "TOOLONG", "AB C"] {
            assert_eq!(
                ProjectKey::parse(key),
                Err(ProjectError::InvalidKey),
                "{key}"
            );
        }
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(ProjectName::parse("  Arches ").unwrap().as_str(), "Arches");
        assert_eq!(ProjectName::parse("   "), Err(ProjectError::InvalidName));
        assert!(ProjectName::parse(&"x".repeat(200)).is_ok());
        assert_eq!(
            ProjectName::parse(&"x".repeat(201)),
            Err(ProjectError::InvalidName)
        );
    }

    #[test]
    fn new_projects_start_at_the_initial_version() {
        let now = Timestamp::from_millis(5);
        let project = Project::new(
            Id::from_uuid(uuid::Uuid::from_u128(1)),
            ProjectKey::parse("ARC").unwrap(),
            ProjectName::parse("Arches").unwrap(),
            now,
        );
        assert_eq!(project.version, Version::INITIAL);
        assert_eq!(project.created_at, now);
        assert_eq!(project.updated_at, now);
    }
}
