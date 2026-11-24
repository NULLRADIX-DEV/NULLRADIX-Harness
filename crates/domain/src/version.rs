/// Optimistic concurrency counter of a stored entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a version is a whole number from 1")]
pub struct InvalidVersion;

impl Version {
    pub const INITIAL: Self = Self(1);

    /// Validates a version sent by a client.
    pub const fn parse(value: i64) -> Result<Self, InvalidVersion> {
        if value >= 1 {
            Ok(Self(value))
        } else {
            Err(InvalidVersion)
        }
    }

    /// Trusts a value read back from storage.
    #[must_use]
    pub const fn from_stored(value: i64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    #[must_use]
    pub const fn value(self) -> i64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_positive_versions() {
        assert_eq!(Version::parse(3), Ok(Version::from_stored(3)));
        assert_eq!(Version::parse(0), Err(InvalidVersion));
        assert_eq!(Version::parse(-1), Err(InvalidVersion));
        assert_eq!(Version::INITIAL.next(), Version::from_stored(2));
    }

    #[test]
    fn round_trips_stored_values() {
        assert_eq!(Version::from_stored(7).value(), 7);
        assert_eq!(Version::INITIAL.value(), 1);
    }
}
