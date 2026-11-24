//! Projects group boards and number tickets and questions.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{Id, Project, ProjectError, ProjectKey, ProjectName, Timestamp, Version};

use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

#[async_trait]
pub trait ProjectRepository: Send + Sync {
    /// Fails with [`key_taken`] when another project uses the key.
    async fn insert(&self, project: &Project) -> Result<Change, AppError>;
    async fn list(&self) -> Result<Vec<Project>, AppError>;
    async fn get(&self, id: Id<Project>) -> Result<Option<Project>, AppError>;
    /// Stores `project` if the stored row still has version `expected`.
    async fn update(&self, project: &Project, expected: Version) -> Result<Change, AppError>;
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

#[must_use]
pub fn not_found(id: Id<Project>) -> AppError {
    AppError::not_found("project-not-found", format!("project {id} does not exist"))
}

#[must_use]
pub fn key_taken(key: &ProjectKey) -> AppError {
    AppError::conflict(
        "project-key-taken",
        format!("another project already uses the key {key}"),
    )
}

pub struct ProjectService {
    repository: Arc<dyn ProjectRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
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
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            clock,
            ids,
            signal,
        }
    }

    pub async fn create(&self, key: &str, name: &str) -> Result<Project, AppError> {
        let key = ProjectKey::parse(key).map_err(|error| invalid(&error))?;
        let name = ProjectName::parse(name).map_err(|error| invalid(&error))?;
        let project = Project::new(self.ids.as_ref().next(), key, name, self.clock.now());
        let change = self.repository.insert(&project).await?;
        self.signal.notify(&change);
        Ok(project)
    }

    pub async fn list(&self) -> Result<Vec<Project>, AppError> {
        self.repository.list().await
    }

    pub async fn get(&self, id: Id<Project>) -> Result<Project, AppError> {
        self.repository.get(id).await?.ok_or_else(|| not_found(id))
    }

    /// Renames the project; the same name returns it unchanged.
    pub async fn rename(
        &self,
        id: Id<Project>,
        name: &str,
        expected: Version,
    ) -> Result<Project, AppError> {
        let name = ProjectName::parse(name).map_err(|error| invalid(&error))?;
        let current = self.get(id).await?;
        if current.version() != expected {
            return Err(AppError::version_conflict("project", id));
        }
        let Some(renamed) = current.renamed(name, self.clock.now()) else {
            return Ok(current);
        };
        let change = self.repository.update(&renamed, expected).await?;
        self.signal.notify(&change);
        Ok(renamed)
    }

    pub async fn delete(&self, id: Id<Project>, expected: Version) -> Result<(), AppError> {
        let change = self
            .repository
            .delete(id, expected, self.clock.now())
            .await?;
        self.signal.notify(&change);
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
        assert_eq!(
            key_taken(&ProjectKey::parse("ARC").unwrap()).code(),
            "project-key-taken"
        );
    }
}
