use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::str::FromStr;

use uuid::Uuid;

/// Time-ordered UUIDv7 identifier, typed by the entity it identifies.
pub struct Id<T> {
    value: Uuid,
    kind: PhantomData<fn() -> T>,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid id: {0}")]
pub struct ParseIdError(#[from] uuid::Error);

impl<T> Id<T> {
    #[must_use]
    pub fn generate() -> Self {
        Self::from_uuid(Uuid::now_v7())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self {
            value,
            kind: PhantomData,
        }
    }

    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.value
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Id<T> {}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T> Eq for Id<T> {}

impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}

impl<T> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.value)
    }
}

impl<T> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

impl<T> FromStr for Id<T> {
    type Err = ParseIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_uuid(Uuid::parse_str(text)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Thing;

    #[test]
    fn generated_ids_are_version_seven_and_ordered() {
        let first = Id::<Thing>::generate();
        let second = Id::<Thing>::generate();
        assert_eq!(first.as_uuid().get_version_num(), 7);
        assert!(first < second);
    }

    #[test]
    fn round_trips_through_text() {
        let id = Id::<Thing>::generate();
        let parsed: Id<Thing> = id.to_string().parse().unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn rejects_invalid_text() {
        assert!("not-an-id".parse::<Id<Thing>>().is_err());
    }
}
