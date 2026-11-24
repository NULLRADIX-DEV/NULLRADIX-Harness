use nullradix_domain::Id;
use uuid::Uuid;

/// Source of new entity ids, swappable for deterministic tests.
pub trait IdGenerator: Send + Sync {
    fn next_uuid(&self) -> Uuid;
}

impl dyn IdGenerator {
    #[must_use]
    pub fn next<T>(&self) -> Id<T> {
        Id::from_uuid(self.next_uuid())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UuidV7Generator;

impl IdGenerator for UuidV7Generator {
    fn next_uuid(&self) -> Uuid {
        Uuid::now_v7()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Thing;

    #[test]
    fn generates_ordered_version_seven_ids() {
        let generator: &dyn IdGenerator = &UuidV7Generator;
        let first: Id<Thing> = generator.next();
        let second: Id<Thing> = generator.next();
        assert_eq!(first.as_uuid().get_version_num(), 7);
        assert!(first < second);
    }
}
