//! Customizations: skills and output styles a person replaced or added, kept in the database.

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_domain::{
    Base, BaseState, ExtensionPoint, Finding, FrontMatter, Overlay, OverlayError, OverlayKind,
    OverlayName, OverlayState, Timestamp, Version, base_state, check_front_matter, overlay_content,
    overlay_state,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::hex;
use crate::{AppError, Change, ChangeSignal, Clock, IdGenerator};

/// The shipped files a customization can replace.
#[async_trait]
pub trait StandardCatalog: Send + Sync {
    /// The shipped file of that name; `None` when there is none.
    async fn get(
        &self,
        point: ExtensionPoint,
        name: &OverlayName,
    ) -> Result<Option<String>, AppError>;
}

/// A host that ships nothing yet: every customization is a new one.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoStandards;

#[async_trait]
impl StandardCatalog for NoStandards {
    async fn get(&self, _: ExtensionPoint, _: &OverlayName) -> Result<Option<String>, AppError> {
        Ok(None)
    }
}

/// One write; dropping it without commit rolls back.
#[async_trait]
pub trait CustomizationWrite: Send {
    async fn overlay(
        &mut self,
        point: ExtensionPoint,
        name: &OverlayName,
    ) -> Result<Option<Overlay>, AppError>;
    async fn insert(&mut self, overlay: &Overlay, feed_id: Uuid) -> Result<Change, AppError>;
    /// Fails with a version conflict when the stored overlay is not at `previous`.
    async fn update(&mut self, overlay: &Overlay, previous: Version) -> Result<Change, AppError>;
    /// Removes the overlay; `None` when there was none.
    async fn delete(
        &mut self,
        point: ExtensionPoint,
        name: &OverlayName,
        expected: Version,
        at: Timestamp,
    ) -> Result<Option<Change>, AppError>;
    async fn commit(self: Box<Self>) -> Result<(), AppError>;
}

#[async_trait]
pub trait CustomizationRepository: Send + Sync {
    async fn begin(&self) -> Result<Box<dyn CustomizationWrite>, AppError>;
    /// Sorted by point, then name.
    async fn list(&self) -> Result<Vec<Overlay>, AppError>;
    async fn get(
        &self,
        point: ExtensionPoint,
        name: &OverlayName,
    ) -> Result<Option<Overlay>, AppError>;
}

/// A customization with what is known about it today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayLine {
    pub overlay: Overlay,
    pub state: OverlayState,
    /// Reasons the shipped file stays in charge.
    pub problems: Vec<Finding>,
    /// Hints that leave the customization in charge.
    pub warnings: Vec<Finding>,
    pub base: BaseState,
    /// Whether a shipped file of that name exists.
    pub standard: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Custom,
    Standard,
}

/// The text that is in charge of a name right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub source: Source,
    pub content: String,
    /// Why the customization does not apply, when the shipped file took over.
    pub problems: Vec<Finding>,
}

#[must_use]
pub fn name_invalid(error: &OverlayError) -> AppError {
    let code = match error {
        OverlayError::InvalidName | OverlayError::ReservedName => "customization-name-invalid",
        OverlayError::EmptyContent => "customization-content-empty",
        OverlayError::ContentTooLong => "value-too-long",
    };
    AppError::invalid(code, error.to_string())
}

fn label(point: ExtensionPoint, name: &OverlayName) -> String {
    format!("{} {name}", point.as_str())
}

#[must_use]
pub fn exists(point: ExtensionPoint, name: &OverlayName) -> AppError {
    AppError::conflict(
        "customization-exists",
        format!("{} is already customized", label(point, name)),
    )
}

#[must_use]
pub fn missing(point: ExtensionPoint, name: &OverlayName) -> AppError {
    AppError::not_found(
        "customization-missing",
        format!("{} is not customized", label(point, name)),
    )
}

#[must_use]
pub fn inactive(point: ExtensionPoint, name: &OverlayName) -> AppError {
    AppError::not_found(
        "customization-inactive",
        format!(
            "{} does not apply and no shipped file takes over",
            label(point, name)
        ),
    )
}

fn no_standard(point: ExtensionPoint, name: &OverlayName) -> AppError {
    AppError::invalid(
        "customization-no-standard",
        format!("there is no shipped {}", label(point, name)),
    )
}

/// The head between the first two `---` lines, read as YAML.
#[must_use]
pub fn parse_front_matter(text: &str) -> FrontMatter {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return FrontMatter::default();
    };
    let mut head = Vec::new();
    let mut closed = false;
    for line in rest.split_inclusive('\n') {
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        if line == "---" {
            closed = true;
            break;
        }
        head.push(line);
    }
    if !closed {
        return FrontMatter::default();
    }
    match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&head.join("\n")) {
        Ok(serde_yaml_ng::Value::Mapping(fields)) => {
            let text = |key: &str| {
                fields
                    .get(key)
                    .and_then(serde_yaml_ng::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            };
            FrontMatter {
                name: text("name"),
                description: text("description"),
                error: None,
            }
        }
        Ok(_) => FrontMatter::default(),
        Err(error) => FrontMatter {
            error: Some(
                error
                    .to_string()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
            ),
            ..FrontMatter::default()
        },
    }
}

fn hash(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

fn snapshot(content: &str) -> Base {
    Base {
        content: content.to_owned(),
        hash: hash(content),
    }
}

/// Evaluates a customization against the shipped file of its name.
#[must_use]
pub fn evaluate(overlay: Overlay, standard: Option<&str>) -> OverlayLine {
    let record = overlay.record();
    let problems = check_front_matter(&record.name, &parse_front_matter(&record.content));
    let base = base_state(
        record.kind,
        record.base.as_ref(),
        standard.map(hash).as_deref(),
    );
    let mut warnings = Vec::new();
    match base {
        BaseState::Outdated => warnings.push(Finding::new(
            "base-outdated",
            "the shipped file changed since this customization was started",
        )),
        BaseState::Unknown => warnings.push(Finding::new(
            "base-unknown",
            "the shipped file this customization replaces is not known",
        )),
        BaseState::Current | BaseState::None => {}
    }
    if record.kind == OverlayKind::New && standard.is_some() {
        warnings.push(Finding::new(
            "standard-exists",
            "a shipped file of this name exists and is replaced; run copy --rebase to base this on it",
        ));
    }
    let state = overlay_state(record.enabled, &problems);
    OverlayLine {
        state,
        problems,
        warnings,
        base,
        standard: standard.is_some(),
        overlay,
    }
}

pub struct CustomizationService {
    repository: Arc<dyn CustomizationRepository>,
    standards: Arc<dyn StandardCatalog>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    signal: ChangeSignal,
}

impl std::fmt::Debug for CustomizationService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomizationService")
            .finish_non_exhaustive()
    }
}

impl CustomizationService {
    #[must_use]
    pub fn new(
        repository: Arc<dyn CustomizationRepository>,
        standards: Arc<dyn StandardCatalog>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        signal: ChangeSignal,
    ) -> Self {
        Self {
            repository,
            standards,
            clock,
            ids,
            signal,
        }
    }

    async fn line(&self, overlay: Overlay) -> Result<OverlayLine, AppError> {
        let record = overlay.record();
        let standard = self.standards.get(record.point, &record.name).await?;
        Ok(evaluate(overlay, standard.as_deref()))
    }

    pub async fn list(&self) -> Result<Vec<OverlayLine>, AppError> {
        let mut lines = Vec::new();
        for overlay in self.repository.list().await? {
            lines.push(self.line(overlay).await?);
        }
        Ok(lines)
    }

    pub async fn get(&self, point: ExtensionPoint, name: &str) -> Result<OverlayLine, AppError> {
        let name = OverlayName::parse(name).map_err(|error| name_invalid(&error))?;
        let overlay = self
            .repository
            .get(point, &name)
            .await?
            .ok_or_else(|| missing(point, &name))?;
        self.line(overlay).await
    }

    /// Starts a customization from `content`, or from the shipped file when none is given.
    pub async fn create(
        &self,
        point: ExtensionPoint,
        name: &str,
        content: Option<&str>,
    ) -> Result<OverlayLine, AppError> {
        let name = OverlayName::parse(name).map_err(|error| name_invalid(&error))?;
        let standard = self.standards.get(point, &name).await?;
        let (content, kind, base) = match (content, &standard) {
            (Some(text), Some(shipped)) => (
                overlay_content(text)
                    .map_err(|error| name_invalid(&error))?
                    .to_owned(),
                OverlayKind::Replace,
                Some(snapshot(shipped)),
            ),
            (Some(text), None) => (
                overlay_content(text)
                    .map_err(|error| name_invalid(&error))?
                    .to_owned(),
                OverlayKind::New,
                None,
            ),
            (None, Some(shipped)) => (
                shipped.clone(),
                OverlayKind::Replace,
                Some(snapshot(shipped)),
            ),
            (None, None) => return Err(no_standard(point, &name)),
        };
        let overlay = Overlay::create(point, name.clone(), kind, content, base, self.clock.now());
        let mut write = self.repository.begin().await?;
        if write.overlay(point, &name).await?.is_some() {
            return Err(exists(point, &name));
        }
        let change = write.insert(&overlay, self.ids.next_uuid()).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(evaluate(overlay, standard.as_deref()))
    }

    /// Changes the content and the switch; what is left out stays.
    pub async fn update(
        &self,
        point: ExtensionPoint,
        name: &str,
        content: Option<&str>,
        enabled: Option<bool>,
        expected: Version,
    ) -> Result<OverlayLine, AppError> {
        let name = OverlayName::parse(name).map_err(|error| name_invalid(&error))?;
        let content = content
            .map(|text| overlay_content(text).map_err(|error| name_invalid(&error)))
            .transpose()?;
        let now = self.clock.now();
        let mut write = self.repository.begin().await?;
        let current = write
            .overlay(point, &name)
            .await?
            .ok_or_else(|| missing(point, &name))?;
        if current.record().version != expected {
            return Err(AppError::version_conflict(
                "customization",
                label(point, &name),
            ));
        }
        let mut next = current.clone();
        if let Some(text) = content.filter(|text| *text != current.record().content) {
            next = next.with_content(text.to_owned(), now);
        }
        if let Some(flag) = enabled.filter(|flag| *flag != current.record().enabled) {
            next = next.with_enabled(flag, now);
        }
        if next != current {
            let change = write.update(&next, expected).await?;
            write.commit().await?;
            self.signal.notify(&change);
        }
        self.line(next).await
    }

    /// Takes the shipped file as the new starting point; the content stays.
    pub async fn rebase(
        &self,
        point: ExtensionPoint,
        name: &str,
        expected: Version,
    ) -> Result<OverlayLine, AppError> {
        let name = OverlayName::parse(name).map_err(|error| name_invalid(&error))?;
        let standard = self
            .standards
            .get(point, &name)
            .await?
            .ok_or_else(|| no_standard(point, &name))?;
        let now = self.clock.now();
        let mut write = self.repository.begin().await?;
        let current = write
            .overlay(point, &name)
            .await?
            .ok_or_else(|| missing(point, &name))?;
        if current.record().version != expected {
            return Err(AppError::version_conflict(
                "customization",
                label(point, &name),
            ));
        }
        let base = snapshot(&standard);
        if current.record().base.as_ref().map(|known| &known.hash) == Some(&base.hash) {
            // already on the shipped file: no new version and no change entry
            return Ok(evaluate(current, Some(&standard)));
        }
        let next = current.rebased(base, now);
        let change = write.update(&next, expected).await?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(evaluate(next, Some(&standard)))
    }

    /// Drops the customization; the shipped file is in charge again.
    pub async fn delete(
        &self,
        point: ExtensionPoint,
        name: &str,
        expected: Version,
    ) -> Result<(), AppError> {
        let name = OverlayName::parse(name).map_err(|error| name_invalid(&error))?;
        let mut write = self.repository.begin().await?;
        let change = write
            .delete(point, &name, expected, self.clock.now())
            .await?
            .ok_or_else(|| missing(point, &name))?;
        write.commit().await?;
        self.signal.notify(&change);
        Ok(())
    }

    /// The customization when it is enabled and has no problems, else the shipped file.
    pub async fn effective(
        &self,
        point: ExtensionPoint,
        name: &str,
    ) -> Result<Effective, AppError> {
        let name = OverlayName::parse(name).map_err(|error| name_invalid(&error))?;
        let standard = self.standards.get(point, &name).await?;
        let overlay = self.repository.get(point, &name).await?;
        let Some(overlay) = overlay else {
            return standard
                .map(|content| Effective {
                    source: Source::Standard,
                    content,
                    problems: Vec::new(),
                })
                .ok_or_else(|| missing(point, &name));
        };
        let line = evaluate(overlay, standard.as_deref());
        if line.state == OverlayState::Active {
            return Ok(Effective {
                source: Source::Custom,
                content: line.overlay.record().content.clone(),
                problems: Vec::new(),
            });
        }
        standard
            .map(|content| Effective {
                source: Source::Standard,
                content,
                problems: line.problems,
            })
            .ok_or_else(|| inactive(point, &name))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use parking_lot::Mutex;

    use super::*;
    use crate::{ChangeOp, Entity, SystemClock, UuidV7Generator};

    const SKILL: &str = "---\nname: review\ndescription: Reviews a change\n---\nBody\n";

    #[derive(Default)]
    struct Memory {
        rows: Mutex<BTreeMap<(ExtensionPoint, String), Overlay>>,
        feed: Mutex<Vec<(Entity, ChangeOp)>>,
    }

    impl Memory {
        fn record(&self, op: ChangeOp) -> Change {
            let mut feed = self.feed.lock();
            feed.push((Entity::Customization, op));
            Change {
                seq: i64::try_from(feed.len()).unwrap(),
                entity: Entity::Customization,
                entity_id: Uuid::nil(),
                op,
                at: Timestamp::from_millis(0),
            }
        }
    }

    struct Write(Arc<Memory>);

    fn key(point: ExtensionPoint, name: &OverlayName) -> (ExtensionPoint, String) {
        (point, name.to_string())
    }

    #[async_trait]
    impl CustomizationWrite for Write {
        async fn overlay(
            &mut self,
            point: ExtensionPoint,
            name: &OverlayName,
        ) -> Result<Option<Overlay>, AppError> {
            Ok(self.0.rows.lock().get(&key(point, name)).cloned())
        }

        async fn insert(&mut self, overlay: &Overlay, _: Uuid) -> Result<Change, AppError> {
            let record = overlay.record();
            self.0
                .rows
                .lock()
                .insert(key(record.point, &record.name), overlay.clone());
            Ok(self.0.record(ChangeOp::Insert))
        }

        async fn update(&mut self, overlay: &Overlay, _: Version) -> Result<Change, AppError> {
            let record = overlay.record();
            self.0
                .rows
                .lock()
                .insert(key(record.point, &record.name), overlay.clone());
            Ok(self.0.record(ChangeOp::Update))
        }

        async fn delete(
            &mut self,
            point: ExtensionPoint,
            name: &OverlayName,
            expected: Version,
            _: Timestamp,
        ) -> Result<Option<Change>, AppError> {
            let found = self.0.rows.lock().get(&key(point, name)).cloned();
            let Some(found) = found else {
                return Ok(None);
            };
            if found.record().version != expected {
                return Err(AppError::version_conflict("customization", name));
            }
            self.0.rows.lock().remove(&key(point, name));
            Ok(Some(self.0.record(ChangeOp::Delete)))
        }

        async fn commit(self: Box<Self>) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait]
    impl CustomizationRepository for Arc<Memory> {
        async fn begin(&self) -> Result<Box<dyn CustomizationWrite>, AppError> {
            Ok(Box::new(Write(self.clone())))
        }

        async fn list(&self) -> Result<Vec<Overlay>, AppError> {
            Ok(self.rows.lock().values().cloned().collect())
        }

        async fn get(
            &self,
            point: ExtensionPoint,
            name: &OverlayName,
        ) -> Result<Option<Overlay>, AppError> {
            Ok(self.rows.lock().get(&key(point, name)).cloned())
        }
    }

    #[derive(Default)]
    struct Shipped(Mutex<HashMap<String, String>>);

    #[async_trait]
    impl StandardCatalog for Shipped {
        async fn get(
            &self,
            point: ExtensionPoint,
            name: &OverlayName,
        ) -> Result<Option<String>, AppError> {
            Ok(self
                .0
                .lock()
                .get(&format!("{}/{name}", point.as_str()))
                .cloned())
        }
    }

    struct Fixture {
        service: CustomizationService,
        memory: Arc<Memory>,
        shipped: Arc<Shipped>,
    }

    fn fixture() -> Fixture {
        let memory = Arc::new(Memory::default());
        let shipped = Arc::new(Shipped::default());
        let service = CustomizationService::new(
            Arc::new(memory.clone()),
            shipped.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
            ChangeSignal::new(),
        );
        Fixture {
            service,
            memory,
            shipped,
        }
    }

    impl Fixture {
        fn ship(&self, point: &str, name: &str, content: &str) {
            self.shipped
                .0
                .lock()
                .insert(format!("{point}/{name}"), content.to_owned());
        }
    }

    #[test]
    fn heads_are_read_as_yaml() {
        let head = parse_front_matter(
            "---\nname: review\ndescription: >-\n  Reviews\n  things\n---\nBody",
        );
        assert_eq!(head.name.as_deref(), Some("review"));
        assert_eq!(head.description.as_deref(), Some("Reviews things"));
        assert_eq!(head.error, None);
        let windows = parse_front_matter("---\r\ndescription: Hi\r\n---\r\nBody");
        assert_eq!(windows.description.as_deref(), Some("Hi"));
    }

    #[test]
    fn missing_unclosed_or_non_mapping_heads_are_empty_and_broken_yaml_is_reported() {
        for text in [
            "Body only",
            "---\nname: x\nBody",
            "---\n- a\n- b\n---\nBody",
            "---\n---\nBody",
        ] {
            assert_eq!(parse_front_matter(text), FrontMatter::default(), "{text}");
        }
        let broken = parse_front_matter("---\nname: [unclosed\n---\nBody");
        assert!(broken.error.is_some());
        assert!(!broken.error.unwrap().contains('\n'));
        let blank = parse_front_matter("---\nname: '  '\ndescription: 7\n---\n");
        assert_eq!((blank.name, blank.description), (None, None));
    }

    #[tokio::test]
    async fn a_name_without_a_shipped_file_starts_a_new_customization() {
        let fixture = fixture();
        let line = fixture
            .service
            .create(ExtensionPoint::Skill, "review", Some(SKILL))
            .await
            .unwrap();
        assert_eq!(line.overlay.record().kind, OverlayKind::New);
        assert_eq!(line.state, OverlayState::Active);
        assert_eq!(line.base, BaseState::None);
        assert!(!line.standard);
        assert_eq!(fixture.memory.feed.lock().len(), 1);
    }

    #[tokio::test]
    async fn a_name_with_a_shipped_file_replaces_it_and_keeps_its_base() {
        let fixture = fixture();
        fixture.ship("skill", "review", "---\ndescription: shipped\n---\nv1");
        let line = fixture
            .service
            .create(ExtensionPoint::Skill, "review", Some(SKILL))
            .await
            .unwrap();
        assert_eq!(line.overlay.record().kind, OverlayKind::Replace);
        assert_eq!(line.base, BaseState::Current);
        assert!(line.standard);
        assert_eq!(
            line.overlay.record().base.as_ref().unwrap().content,
            "---\ndescription: shipped\n---\nv1"
        );
    }

    #[tokio::test]
    async fn copying_without_content_takes_the_shipped_file() {
        let fixture = fixture();
        fixture.ship(
            "output-style",
            "terse",
            "---\ndescription: Short\n---\nBe brief",
        );
        let line = fixture
            .service
            .create(ExtensionPoint::OutputStyle, "terse", None)
            .await
            .unwrap();
        assert_eq!(
            line.overlay.record().content,
            "---\ndescription: Short\n---\nBe brief"
        );
        assert_eq!(line.overlay.record().kind, OverlayKind::Replace);
        let error = fixture
            .service
            .create(ExtensionPoint::Skill, "nothing", None)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "customization-no-standard");
    }

    #[tokio::test]
    async fn creating_checks_name_content_and_duplicates() {
        let fixture = fixture();
        let create = |name: &'static str, content: Option<&'static str>| {
            fixture.service.create(ExtensionPoint::Skill, name, content)
        };
        assert_eq!(
            create("Bad Name", Some(SKILL)).await.unwrap_err().code(),
            "customization-name-invalid"
        );
        assert_eq!(
            create("review", Some("  ")).await.unwrap_err().code(),
            "customization-content-empty"
        );
        create("review", Some(SKILL)).await.unwrap();
        assert_eq!(
            create("review", Some(SKILL)).await.unwrap_err().code(),
            "customization-exists"
        );
        let other = fixture
            .service
            .create(ExtensionPoint::OutputStyle, "review", Some(SKILL))
            .await;
        assert!(other.is_ok());
        let huge = "a".repeat(nullradix_domain::OVERLAY_CONTENT_MAX + 1);
        let error = fixture
            .service
            .create(ExtensionPoint::Skill, "huge", Some(&huge))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "value-too-long");
    }

    #[tokio::test]
    async fn problems_in_the_head_send_the_overlay_back_to_the_shipped_file() {
        let fixture = fixture();
        fixture.ship("skill", "review", "---\ndescription: shipped\n---\nv1");
        let line = fixture
            .service
            .create(ExtensionPoint::Skill, "review", Some("no head at all"))
            .await
            .unwrap();
        assert_eq!(line.state, OverlayState::Fallback);
        assert_eq!(line.problems[0].code, "description-missing");
        let effective = fixture
            .service
            .effective(ExtensionPoint::Skill, "review")
            .await
            .unwrap();
        assert_eq!(effective.source, Source::Standard);
        assert_eq!(effective.content, "---\ndescription: shipped\n---\nv1");
        assert_eq!(effective.problems[0].code, "description-missing");

        let fixed = fixture
            .service
            .update(
                ExtensionPoint::Skill,
                "review",
                Some(SKILL),
                None,
                line.overlay.record().version,
            )
            .await
            .unwrap();
        assert_eq!(fixed.state, OverlayState::Active);
        let effective = fixture
            .service
            .effective(ExtensionPoint::Skill, "review")
            .await
            .unwrap();
        assert_eq!(
            (effective.source, effective.content.as_str()),
            (Source::Custom, SKILL)
        );
    }

    #[tokio::test]
    async fn a_disabled_overlay_leaves_the_shipped_file_or_nothing_in_charge() {
        let fixture = fixture();
        let line = fixture
            .service
            .create(ExtensionPoint::Skill, "review", Some(SKILL))
            .await
            .unwrap();
        let off = fixture
            .service
            .update(
                ExtensionPoint::Skill,
                "review",
                None,
                Some(false),
                line.overlay.record().version,
            )
            .await
            .unwrap();
        assert_eq!(off.state, OverlayState::Disabled);
        let error = fixture
            .service
            .effective(ExtensionPoint::Skill, "review")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "customization-inactive");
        fixture.ship("skill", "review", "shipped");
        let effective = fixture
            .service
            .effective(ExtensionPoint::Skill, "review")
            .await
            .unwrap();
        assert_eq!(effective.source, Source::Standard);
        let none = fixture
            .service
            .effective(ExtensionPoint::Skill, "absent")
            .await;
        assert_eq!(none.unwrap_err().code(), "customization-missing");
        fixture.ship("skill", "only-shipped", "x");
        let shipped = fixture
            .service
            .effective(ExtensionPoint::Skill, "only-shipped")
            .await
            .unwrap();
        assert_eq!(shipped.source, Source::Standard);
    }

    #[tokio::test]
    async fn updates_check_the_version_and_ignore_changes_to_nothing() {
        let fixture = fixture();
        let line = fixture
            .service
            .create(ExtensionPoint::Skill, "review", Some(SKILL))
            .await
            .unwrap();
        let version = line.overlay.record().version;
        let stale = fixture
            .service
            .update(
                ExtensionPoint::Skill,
                "review",
                Some("x"),
                None,
                Version::from_stored(9),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
        let same = fixture
            .service
            .update(
                ExtensionPoint::Skill,
                "review",
                Some(SKILL),
                Some(true),
                version,
            )
            .await
            .unwrap();
        assert_eq!(same.overlay.record().version, version);
        assert_eq!(fixture.memory.feed.lock().len(), 1);
        let gone = fixture
            .service
            .update(ExtensionPoint::Skill, "absent", None, None, version)
            .await
            .unwrap_err();
        assert_eq!(gone.code(), "customization-missing");
        let blank = fixture
            .service
            .update(ExtensionPoint::Skill, "review", Some(" "), None, version)
            .await
            .unwrap_err();
        assert_eq!(blank.code(), "customization-content-empty");
    }

    #[tokio::test]
    async fn the_base_goes_outdated_when_the_shipped_file_moves_and_rebasing_catches_up() {
        let fixture = fixture();
        fixture.ship("skill", "review", "v1");
        fixture
            .service
            .create(ExtensionPoint::Skill, "review", Some(SKILL))
            .await
            .unwrap();
        fixture.ship("skill", "review", "v2");
        let line = fixture
            .service
            .get(ExtensionPoint::Skill, "review")
            .await
            .unwrap();
        assert_eq!(line.base, BaseState::Outdated);
        assert_eq!(line.warnings[0].code, "base-outdated");
        assert_eq!(line.state, OverlayState::Active);
        let version = line.overlay.record().version;
        let rebased = fixture
            .service
            .rebase(ExtensionPoint::Skill, "review", version)
            .await
            .unwrap();
        assert_eq!(rebased.base, BaseState::Current);
        assert_eq!(rebased.overlay.record().content, SKILL);
        assert_eq!(rebased.overlay.record().version, version.next());
        let stale = fixture
            .service
            .rebase(ExtensionPoint::Skill, "review", version)
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
    }

    #[tokio::test]
    async fn a_new_customization_that_shadows_a_shipped_file_warns_and_rebases_into_a_replacement()
    {
        let fixture = fixture();
        fixture
            .service
            .create(ExtensionPoint::Skill, "mine", Some(SKILL))
            .await
            .unwrap();
        fixture.ship("skill", "mine", "now shipped");
        let line = fixture
            .service
            .get(ExtensionPoint::Skill, "mine")
            .await
            .unwrap();
        assert_eq!(line.overlay.record().kind, OverlayKind::New);
        assert!(line.warnings.iter().any(|w| w.code == "standard-exists"));
        let rebased = fixture
            .service
            .rebase(ExtensionPoint::Skill, "mine", Version::INITIAL)
            .await
            .unwrap();
        assert_eq!(rebased.overlay.record().kind, OverlayKind::Replace);
        assert_eq!(rebased.base, BaseState::Current);
        assert_eq!(rebased.overlay.record().content, SKILL);
        assert_eq!(rebased.warnings.len(), 0);
    }

    #[tokio::test]
    async fn rebasing_onto_an_unchanged_shipped_file_changes_nothing() {
        let fixture = fixture();
        fixture.ship("skill", "review", "v1");
        fixture
            .service
            .create(ExtensionPoint::Skill, "review", Some(SKILL))
            .await
            .unwrap();
        let before = fixture.memory.feed.lock().len();
        let line = fixture
            .service
            .rebase(ExtensionPoint::Skill, "review", Version::INITIAL)
            .await
            .unwrap();
        assert_eq!(line.overlay.record().version, Version::INITIAL);
        assert_eq!(fixture.memory.feed.lock().len(), before);
    }

    #[tokio::test]
    async fn rebasing_needs_a_shipped_file_and_a_customization() {
        let fixture = fixture();
        fixture
            .service
            .create(ExtensionPoint::Skill, "mine", Some(SKILL))
            .await
            .unwrap();
        let error = fixture
            .service
            .rebase(ExtensionPoint::Skill, "mine", Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "customization-no-standard");
        fixture.ship("skill", "other", "x");
        let error = fixture
            .service
            .rebase(ExtensionPoint::Skill, "other", Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "customization-missing");
    }

    #[tokio::test]
    async fn deleting_checks_version_and_presence_and_lists_come_sorted() {
        let fixture = fixture();
        for (point, name) in [
            (ExtensionPoint::OutputStyle, "alpha"),
            (ExtensionPoint::Skill, "zeta"),
            (ExtensionPoint::Skill, "beta"),
        ] {
            fixture
                .service
                .create(point, name, Some(SKILL))
                .await
                .unwrap();
        }
        let order: Vec<String> = fixture
            .service
            .list()
            .await
            .unwrap()
            .iter()
            .map(|line| {
                let record = line.overlay.record();
                format!("{}/{}", record.point.as_str(), record.name)
            })
            .collect();
        assert_eq!(order, ["skill/beta", "skill/zeta", "output-style/alpha"]);
        let stale = fixture
            .service
            .delete(ExtensionPoint::Skill, "beta", Version::from_stored(4))
            .await
            .unwrap_err();
        assert_eq!(stale.code(), "version-conflict");
        fixture
            .service
            .delete(ExtensionPoint::Skill, "beta", Version::INITIAL)
            .await
            .unwrap();
        let gone = fixture
            .service
            .delete(ExtensionPoint::Skill, "beta", Version::INITIAL)
            .await
            .unwrap_err();
        assert_eq!(gone.code(), "customization-missing");
        assert_eq!(fixture.service.list().await.unwrap().len(), 2);
    }
}
