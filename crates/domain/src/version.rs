/// Optimistic concurrency counter of a stored entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(i64);

impl Version {
    pub const INITIAL: Self = Self(1);

    #[must_use]
    pub const fn from_stored(value: i64) -> Self {
        Self(value)
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
    fn round_trips_stored_values() {
        assert_eq!(Version::from_stored(7).value(), 7);
        assert_eq!(Version::INITIAL.value(), 1);
    }
}
