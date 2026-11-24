//! Projects group boards and number tickets and questions.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{Id, Project, ProjectError, ProjectKey, ProjectName, Timestamp, Version};

use crate::{AppError, Change, Clock, EventBus, IdGenerator};

#[async_trait]
pub trait ProjectRepository: Send + Sync {
    /// Fails with `project-key-taken` when another project uses the key.
    async fn insert(&self, project: &Project) -> Result<Change, AppError>;
    async fn list(&self) -> Result<Vec<Project>, AppError>;
    async fn get(&self, id: Id<Project>) -> Result<Option<Project>, AppError>;
    /// Fails with `version-conflict` when `expected` is outdated.
    async fn rename(
        &self,
        id: Id<Project>,
        name: &ProjectName,
        expected: Version,
        at: Timestamp,
    ) -> Result<(Project, Change), AppError>;
    async fn delete(
        &self,
        id: Id<Project>,
        expected: Version,
        at: Timestamp,
    ) -> Result<Change, AppError>;
}

fn invalid(error: &ProjectError) -> AppError {
    let code = match error {
        ProjectError::InvalidKey => "invalid-project-key",
        ProjectError::InvalidName => "invalid-project-name",
    };
    AppError::invalid(code, error.to_string())
}

/// Error for a project that does not exist.
#[must_use]
pub fn not_found(id: Id<Project>) -> AppError {
    AppError::not_found("project-not-found", format!("project {id} does not exist"))
}

pub struct ProjectService {
    repository: Arc<dyn ProjectRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    events: EventBus<Change>,
}

impl std::fmt::Debug for ProjectService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectService").finish_non_exhaustive()
    }
}

impl ProjectService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn ProjectRepository>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        events: EventBus<Change>,
    ) -> Self {
        Self {
            repository,
            clock,
            ids,
            events,
        }
    }

    pub async fn create(&self, key: &str, name: &str) -> Result<Project, AppError> {
        let key = ProjectKey::parse(key).map_err(|error| invalid(&error))?;
        let name = ProjectName::parse(name).map_err(|error| invalid(&error))?;
        let project = Project::new(self.ids.as_ref().next(), key, name, self.clock.now());
        let change = self.repository.insert(&project).await?;
        self.events.publish(change);
        Ok(project)
    }

    pub async fn list(&self) -> Result<Vec<Project>, AppError> {
        self.repository.list().await
    }

    pub async fn get(&self, id: Id<Project>) -> Result<Project, AppError> {
        self.repository.get(id).await?.ok_or_else(|| not_found(id))
    }

    pub async fn rename(
        &self,
        id: Id<Project>,
        name: &str,
        expected: Version,
    ) -> Result<Project, AppError> {
        let name = ProjectName::parse(name).map_err(|error| invalid(&error))?;
        let (project, change) = self
            .repository
            .rename(id, &name, expected, self.clock.now())
            .await?;
        self.events.publish(change);
        Ok(project)
    }

    pub async fn delete(&self, id: Id<Project>, expected: Version) -> Result<(), AppError> {
        let change = self
            .repository
            .delete(id, expected, self.clock.now())
            .await?;
        self.events.publish(change);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_validation_errors_to_stable_codes() {
        assert_eq!(
            invalid(&ProjectError::InvalidKey).code(),
            "invalid-project-key"
        );
        assert_eq!(
            invalid(&ProjectError::InvalidName).code(),
            "invalid-project-name"
        );
    }
}
