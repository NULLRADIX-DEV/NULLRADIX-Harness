//! CSS snippets: small stylesheets on top of the active theme, kept in the database.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Snippet, SnippetCss, SnippetError, SnippetId, SnippetName, Timestamp, Version,
    active_stylesheet,
};
use uuid::Uuid;

use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// One write; dropping it without commit rolls back.
#[async_trait]
pub trait SnippetWrite: Send {
    async fn snippet(&mut self, id: &SnippetId) -> Result<Option<Snippet>, AppError>;
    /// The position after the last snippet.
    async fn next_order(&mut self) -> Result<i64, AppError>;
    async fn insert(&mut self, snippet: &Snippet, feed_id: Uuid) -> Result<Change, AppError>;
    /// Fails with a version conflict when the stored snippet is not at `previous`.
    async fn update(&mut self, snippet: &Snippet, previous: Version) -> Result<Change, AppError>;
    /// Removes the snippet; `None` when there was none.
    async fn delete(
        &mut self,
        id: &SnippetId,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    async fn commit(self: Box<Self>) -> Result<(), AppError>;
}

#[async_trait]
pub trait SnippetRepository: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn SnippetWrite>, AppError>;
    /// In delivery order: position, then id.
    async fn list(&self) -> Result<Vec<Snippet>, AppError>;
}

#[must_use]
pub fn snippet_invalid(error: &SnippetError) -> AppError {
    let code = match error {
        SnippetError::InvalidId => "snippet-id-invalid",
        SnippetError::Name(_) => "snippet-name-invalid",
        SnippetError::CssTooLong => "snippet-too-large",
        SnippetError::ForbiddenCss(_) => "snippet-css-forbidden",
    };
    AppError::invalid(code, error.to_string())
}

#[must_use]
pub fn exists(id: &SnippetId) -> AppError {
    AppError::conflict("snippet-exists", format!("the snippet {id} already exists"))
}

#[must_use]
pub fn missing(id: &SnippetId) -> AppError {
    AppError::not_found("snippet-missing", format!("there is no snippet {id}"))
}

pub struct SnippetService {
    repository: Arc<dyn SnippetRepository>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for SnippetService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnippetService").finish_non_exhaustive()
    }
}

impl SnippetService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn SnippetRepository>,
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

    pub async fn list(&self) -> Result<Vec<Snippet>, AppError> {
        self.repository.list().await
    }

    /// The enabled snippets as one stylesheet.
    pub async fn active_css(&self) -> Result<String, AppError> {
        Ok(active_stylesheet(&self.repository.list().await?))
    }

    /// Adds a snippet; new ones are on unless `enabled` says otherwise.
    pub async fn create(
        &self,
        id: &str,
        name: &str,
        css: &str,
        enabled: Option<bool>,
    ) -> Result<Snippet, AppError> {
        let id = SnippetId::parse(id).map_err(|error| snippet_invalid(&error))?;
        let name = SnippetName::parse(name).map_err(|error| snippet_invalid(&error))?;
        let css = SnippetCss::parse(css).map_err(|error| snippet_invalid(&error))?;
        let mut write = self.repository.begin().await?;
        if write.snippet(&id).await?.is_some() {
            return Err(exists(&id));
        }
        let order = write.next_order().await?;
        let snippet = Snippet::create(
            id,
            &name,
            &css,
            enabled.unwrap_or(true),
            order,
            self.clock.now(),
        );
        let change = write.insert(&snippet, self.ids.next_uuid()).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(snippet)
    }

    /// Changes name, text and switch; what is left out stays.
    pub async fn update(
        &self,
        id: &str,
        name: Option<&str>,
        css: Option<&str>,
        enabled: Option<bool>,
        expected: Version,
    ) -> Result<Snippet, AppError> {
        let id = SnippetId::parse(id).map_err(|error| snippet_invalid(&error))?;
        let name = name
            .map(|text| SnippetName::parse(text).map_err(|error| snippet_invalid(&error)))
            .transpose()?;
        let css = css
            .map(|text| SnippetCss::parse(text).map_err(|error| snippet_invalid(&error)))
            .transpose()?;
        let mut write = self.repository.begin().await?;
        let current = write.snippet(&id).await?.ok_or_else(|| missing(&id))?;
        if current.record().version != expected {
            return Err(AppError::version_conflict("snippet", &id));
        }
        let next = current.changed(name.as_ref(), css.as_ref(), enabled, self.clock.now());
        if next != current {
            let change = write.update(&next, expected).await?;
            write.commit().await?;
            self.signal.notify(&change);
        }
        Ok(next)
    }

    pub async fn delete(&self, id: &str, expected: Version) -> Result<(), AppError> {
        let id = SnippetId::parse(id).map_err(|error| snippet_invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let change = write
            .delete(&id, expected, self.clock.now())
            .await?
            .ok_or_else(|| missing(&id))?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use parking_lot::Mutex;

    use super::*;
    use crate::{ChangeOp, Entity, SystemClock, UuidV7Generator};

    #[derive(Default)]
    struct Memory {
        rows: Mutex<BTreeMap<String, Snippet>>,
        feed: Mutex<Vec<ChangeOp>>,
    }

    impl Memory {
        fn record(&self, op: ChangeOp) -> Change {
            let mut feed = self.feed.lock();
            feed.push(op);
            Change {
                seq: i64::try_from(feed.len()).unwrap(),
                entity: Entity::Snippet,
                entity_id: Uuid::nil(),
                op,
                at: Timestamp::from_millis(0),
            }
        }
    }

    struct Write(Arc<Memory>);

    #[async_trait]
    impl SnippetWrite for Write {
        async fn snippet(&mut self, id: &SnippetId) -> Result<Option<Snippet>, AppError> {
            Ok(self.0.rows.lock().get(id.as_str()).cloned())
        }

        async fn next_order(&mut self) -> Result<i64, AppError> {
            let rows = self.0.rows.lock();
            Ok(rows
                .values()
                .map(|row| row.record().order)
                .max()
                .unwrap_or(0)
                + 1)
        }

        async fn insert(&mut self, snippet: &Snippet, _: Uuid) -> Result<Change, AppError> {
            let key = snippet.record().id.to_string();
            self.0.rows.lock().insert(key, snippet.clone());
            Ok(self.0.record(ChangeOp::Insert))
        }

        async fn update(&mut self, snippet: &Snippet, _: Version) -> Result<Change, AppError> {
            let key = snippet.record().id.to_string();
            self.0.rows.lock().insert(key, snippet.clone());
            Ok(self.0.record(ChangeOp::Update))
        }

        async fn delete(
            &mut self,
            id: &SnippetId,
            expected: Version,
            _: Timestamp,
        ) -> Result<Option<Change>, AppError> {
            let found = self.0.rows.lock().get(id.as_str()).cloned();
            let Some(found) = found else {
                return Ok(None);
            };
            if found.record().version != expected {
                return Err(AppError::version_conflict("snippet", id));
            }
            self.0.rows.lock().remove(id.as_str());
            Ok(Some(self.0.record(ChangeOp::Delete)))
        }

        async fn commit(self: Box<Self>) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait]
    impl SnippetRepository for Arc<Memory> {
        async fn begin(&self) -> Result<Box<dyn SnippetWrite>, AppError> {
            Ok(Box::new(Write(self.clone())))
        }

        async fn list(&self) -> Result<Vec<Snippet>, AppError> {
            let mut rows: Vec<Snippet> = self.rows.lock().values().cloned().collect();
            rows.sort_by_key(|row| row.record().order);
            Ok(rows)
        }
    }

    fn fixture() -> (SnippetService, Arc<Memory>) {
        let memory = Arc::new(Memory::default());
        let service = SnippetService::new(
            Arc::new(memory.clone()),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            ChangeSignal::new(),
        );
        (service, memory)
    }

    #[tokio::test]
    async fn a_snippet_is_created_on_and_in_order() {
        let (service, memory) = fixture();
        let first = service.create("one", " One ", "a{}", None).await.unwrap();
        let second = service
            .create("two", "Two", "b{}", Some(false))
            .await
            .unwrap();
        assert_eq!(first.record().name, "One");
        assert!(first.record().enabled && !second.record().enabled);
        assert_eq!((first.record().order, second.record().order), (1, 2));
        assert_eq!(
            service.active_css().await.unwrap(),
            "/* snippet: one */\na{}\n"
        );
        assert_eq!(
            *memory.feed.lock(),
            vec![ChangeOp::Insert, ChangeOp::Insert]
        );
    }

    #[tokio::test]
    async fn a_taken_id_conflicts() {
        let (service, _) = fixture();
        service.create("one", "One", "a{}", None).await.unwrap();
        let error = service
            .create("one", "Again", "b{}", None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "snippet-exists");
    }

    #[tokio::test]
    async fn bad_input_is_refused_before_anything_is_stored() {
        let (service, memory) = fixture();
        let big = "a".repeat(nullradix_domain::SNIPPET_CSS_MAX + 1);
        let cases = [
            ("A", "n", "a{}", "snippet-id-invalid"),
            ("ok", " ", "a{}", "snippet-name-invalid"),
            ("ok", "n", "@import 'x';", "snippet-css-forbidden"),
            (
                "ok",
                "n",
                "a{background:url(http://x)}",
                "snippet-css-forbidden",
            ),
            ("ok", "n", &big, "snippet-too-large"),
        ];
        for (id, name, css, code) in cases {
            let error = service.create(id, name, css, None).await.unwrap_err();
            assert_eq!(error.code(), code, "{id} {name}");
        }
        assert!(memory.rows.lock().is_empty());
        assert!(memory.feed.lock().is_empty());
    }

    #[tokio::test]
    async fn an_update_needs_the_current_version_and_keeps_what_is_left_out() {
        let (service, memory) = fixture();
        service.create("one", "One", "a{}", None).await.unwrap();
        let off = service
            .update("one", None, None, Some(false), Version::INITIAL)
            .await
            .unwrap();
        assert_eq!(off.record().version, Version::from_stored(2));
        assert_eq!(
            (off.record().name.as_str(), off.record().css.as_str()),
            ("One", "a{}")
        );
        let stale = service
            .update("one", None, Some("b{}"), None, Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
        let bad = service
            .update(
                "one",
                None,
                Some("@import 'x';"),
                None,
                Version::from_stored(2),
            )
            .await
            .unwrap_err();
        assert_eq!(bad.code(), "snippet-css-forbidden");
        let gone = service
            .update("other", None, None, None, Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(gone.code(), "snippet-missing");
        // nothing changed: no version bump and no feed entry
        let same = service
            .update("one", Some("One"), None, None, Version::from_stored(2))
            .await
            .unwrap();
        assert_eq!(same.record().version, Version::from_stored(2));
        assert_eq!(
            *memory.feed.lock(),
            vec![ChangeOp::Insert, ChangeOp::Update]
        );
    }

    #[tokio::test]
    async fn a_delete_needs_the_current_version() {
        let (service, _) = fixture();
        service.create("one", "One", "a{}", None).await.unwrap();
        let stale = service
            .delete("one", Version::from_stored(3))
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
        service.delete("one", Version::INITIAL).await.unwrap();
        let again = service.delete("one", Version::INITIAL).await.unwrap_err();
        assert_eq!(again.code(), "snippet-missing");
        let rest = service.list().await.unwrap();
        assert!(matches!(rest.as_slice(), []));
    }
}
