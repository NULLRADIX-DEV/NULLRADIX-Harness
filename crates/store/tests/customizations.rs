#![allow(clippy::unwrap_used, reason = "integration tests fail loudly")]

use std::sync::Arc;

use async_trait::async_trait;
use nullradix_app::customizations::{
    CustomizationRepository, CustomizationService, StandardCatalog,
};
use nullradix_app::{
    AppError, ChangeFeed, ChangeOp, ChangeSignal, Entity, SystemClock, UuidV7Generator,
};
use nullradix_domain::{
    Base, ExtensionPoint, Overlay, OverlayKind, OverlayName, Timestamp, Version,
};
use nullradix_store::Store;
use uuid::Uuid;

fn name(text: &str) -> OverlayName {
    OverlayName::parse(text).unwrap()
}

fn overlay(point: ExtensionPoint, text: &str, kind: OverlayKind, at: i64) -> Overlay {
    let base = (kind == OverlayKind::Replace).then(|| Base {
        content: String::from("shipped"),
        hash: String::from("abc"),
    });
    Overlay::create(
        point,
        name(text),
        kind,
        format!("---\ndescription: {text}\n---\nbody"),
        base,
        Timestamp::from_millis(at),
    )
}

async fn changes(store: &Store) -> Vec<(Entity, ChangeOp)> {
    store
        .changes_since(0, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|change| (change.entity, change.op))
        .collect()
}

#[tokio::test]
async fn an_overlay_round_trips_with_its_base() {
    let store = Store::in_memory().await.unwrap();
    let replaced = overlay(ExtensionPoint::Skill, "review", OverlayKind::Replace, 5);
    let fresh = overlay(ExtensionPoint::OutputStyle, "terse", OverlayKind::New, 6);
    let mut write = store.begin().await.unwrap();
    let change = write.insert(&replaced, Uuid::from_u128(1)).await.unwrap();
    assert_eq!(
        (change.entity, change.op),
        (Entity::Customization, ChangeOp::Insert)
    );
    write.insert(&fresh, Uuid::from_u128(2)).await.unwrap();
    write.commit().await.unwrap();

    let found = store
        .get(ExtensionPoint::Skill, &name("review"))
        .await
        .unwrap();
    assert_eq!(found, Some(replaced.clone()));
    let found = store
        .get(ExtensionPoint::OutputStyle, &name("terse"))
        .await
        .unwrap();
    assert_eq!(found, Some(fresh.clone()));
    assert_eq!(
        store
            .get(ExtensionPoint::Skill, &name("terse"))
            .await
            .unwrap(),
        None
    );
    assert_eq!(store.list().await.unwrap(), vec![replaced, fresh]);
}

#[tokio::test]
async fn lists_put_skills_first_and_sort_by_name() {
    let store = Store::in_memory().await.unwrap();
    let mut write = store.begin().await.unwrap();
    for (index, (point, text)) in [
        (ExtensionPoint::OutputStyle, "alpha"),
        (ExtensionPoint::Skill, "zeta"),
        (ExtensionPoint::Skill, "beta"),
    ]
    .into_iter()
    .enumerate()
    {
        let row = overlay(point, text, OverlayKind::New, 1);
        write
            .insert(&row, Uuid::from_u128(index as u128 + 1))
            .await
            .unwrap();
    }
    write.commit().await.unwrap();
    let order: Vec<String> = store
        .list()
        .await
        .unwrap()
        .iter()
        .map(|row| format!("{}/{}", row.record().point.as_str(), row.record().name))
        .collect();
    assert_eq!(order, ["skill/beta", "skill/zeta", "output-style/alpha"]);
}

#[tokio::test]
async fn a_name_is_taken_once_per_point() {
    let store = Store::in_memory().await.unwrap();
    let first = overlay(ExtensionPoint::Skill, "review", OverlayKind::New, 1);
    let mut write = store.begin().await.unwrap();
    write.insert(&first, Uuid::from_u128(1)).await.unwrap();
    let error = write.insert(&first, Uuid::from_u128(2)).await.unwrap_err();
    assert_eq!(error.code(), "customization-exists");
    let other_point = overlay(ExtensionPoint::OutputStyle, "review", OverlayKind::New, 1);
    write
        .insert(&other_point, Uuid::from_u128(3))
        .await
        .unwrap();
}

#[tokio::test]
async fn updates_check_the_version() {
    let store = Store::in_memory().await.unwrap();
    let first = overlay(ExtensionPoint::Skill, "review", OverlayKind::New, 1);
    let mut write = store.begin().await.unwrap();
    write.insert(&first, Uuid::from_u128(1)).await.unwrap();
    write.commit().await.unwrap();

    let edited = first
        .with_content(
            String::from("---\ndescription: new\n---\n"),
            Timestamp::from_millis(9),
        )
        .with_enabled(false, Timestamp::from_millis(10));
    let mut write = store.begin().await.unwrap();
    let error = write
        .update(&edited, Version::from_stored(7))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let change = write.update(&edited, Version::INITIAL).await.unwrap();
    assert_eq!(
        (change.entity, change.op),
        (Entity::Customization, ChangeOp::Update)
    );
    write.commit().await.unwrap();
    let stored = store
        .get(ExtensionPoint::Skill, &name("review"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored, edited);
    assert!(!stored.record().enabled);

    let ghost = overlay(ExtensionPoint::Skill, "ghost", OverlayKind::New, 1);
    let mut write = store.begin().await.unwrap();
    let error = write.update(&ghost, Version::INITIAL).await.unwrap_err();
    assert_eq!(error.code(), "customization-missing");
}

#[tokio::test]
async fn deleting_checks_the_version_and_reports_missing_overlays() {
    let store = Store::in_memory().await.unwrap();
    let row = overlay(ExtensionPoint::Skill, "review", OverlayKind::New, 1);
    let at = Timestamp::from_millis(2);
    let mut write = store.begin().await.unwrap();
    let none = write
        .delete(ExtensionPoint::Skill, &name("review"), Version::INITIAL, at)
        .await
        .unwrap();
    assert!(none.is_none());
    write.insert(&row, Uuid::from_u128(1)).await.unwrap();
    let error = write
        .delete(
            ExtensionPoint::Skill,
            &name("review"),
            Version::from_stored(3),
            at,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), "version-conflict");
    let change = write
        .delete(ExtensionPoint::Skill, &name("review"), Version::INITIAL, at)
        .await
        .unwrap()
        .unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        (change.entity, change.op),
        (Entity::Customization, ChangeOp::Delete)
    );
    assert_eq!(store.list().await.unwrap().len(), 0);
}

#[tokio::test]
async fn a_rolled_back_write_changes_nothing() {
    let store = Store::in_memory().await.unwrap();
    let mut write = store.begin().await.unwrap();
    let row = overlay(ExtensionPoint::Skill, "review", OverlayKind::New, 1);
    write.insert(&row, Uuid::from_u128(1)).await.unwrap();
    drop(write);
    assert_eq!(store.list().await.unwrap().len(), 0);
    assert_eq!(changes(&store).await.len(), 0);
}

struct Shipped;

#[async_trait]
impl StandardCatalog for Shipped {
    async fn get(
        &self,
        point: ExtensionPoint,
        name: &OverlayName,
    ) -> Result<Option<String>, AppError> {
        Ok(
            (point == ExtensionPoint::Skill && name.as_str() == "review")
                .then(|| String::from("---\ndescription: shipped\n---\nv1")),
        )
    }
}

#[tokio::test]
async fn rebasing_a_new_customization_stores_it_as_a_replacement() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let service = CustomizationService::new(
        store.clone(),
        Arc::new(Shipped),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    );
    let own = "---
name: review
description: mine
---
body";
    let name = OverlayName::parse("review").unwrap();
    let new = Overlay::create(
        ExtensionPoint::Skill,
        name.clone(),
        OverlayKind::New,
        own.to_owned(),
        None,
        Timestamp::from_millis(1),
    );
    let mut write = store.begin().await.unwrap();
    write.insert(&new, Uuid::now_v7()).await.unwrap();
    write.commit().await.unwrap();

    let rebased = service
        .rebase(ExtensionPoint::Skill, "review", Version::INITIAL)
        .await
        .unwrap();

    assert_eq!(rebased.overlay.record().kind, OverlayKind::Replace);
    let stored = store
        .get(ExtensionPoint::Skill, &name)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.record().kind, OverlayKind::Replace);
    assert!(stored.record().base.is_some());
    assert_eq!(stored.record().content, own);
}

#[tokio::test]
async fn a_stored_device_name_still_lists_warns_and_can_be_reset() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let service = CustomizationService::new(
        store.clone(),
        Arc::new(Shipped),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    );
    let name = OverlayName::parse_stored("nul").unwrap();
    let row = Overlay::create(
        ExtensionPoint::Skill,
        name,
        OverlayKind::New,
        String::from(
            "---
name: nul
description: old
---
body",
        ),
        None,
        Timestamp::from_millis(1),
    );
    let mut write = store.begin().await.unwrap();
    write.insert(&row, Uuid::now_v7()).await.unwrap();
    write.commit().await.unwrap();

    let lines = service.list().await.unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].warnings.iter().any(|w| w.code == "name-reserved"));
    let line = service.get(ExtensionPoint::Skill, "nul").await.unwrap();
    service
        .delete(ExtensionPoint::Skill, "nul", line.overlay.record().version)
        .await
        .unwrap();
    assert_eq!(store.list().await.unwrap().len(), 0);
    let again = service
        .create(ExtensionPoint::Skill, "nul", Some("x"))
        .await;
    assert!(again.is_err());
}

#[tokio::test]
async fn the_service_copies_edits_rebases_and_drops_against_the_database() {
    let store = Arc::new(Store::in_memory().await.unwrap());
    let service = CustomizationService::new(
        store.clone(),
        Arc::new(Shipped),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
        ChangeSignal::new(),
    );
    let copy = service
        .create(ExtensionPoint::Skill, "review", None)
        .await
        .unwrap();
    assert_eq!(copy.overlay.record().kind, OverlayKind::Replace);
    let edited = service
        .update(
            ExtensionPoint::Skill,
            "review",
            Some("---\ndescription: mine\n---\nbody"),
            None,
            copy.overlay.record().version,
        )
        .await
        .unwrap();
    let rebased = service
        .rebase(
            ExtensionPoint::Skill,
            "review",
            edited.overlay.record().version,
        )
        .await
        .unwrap();
    // the shipped file did not move, so rebasing is a no-op
    assert_eq!(rebased.overlay.record().version, Version::from_stored(2));
    service
        .delete(
            ExtensionPoint::Skill,
            "review",
            rebased.overlay.record().version,
        )
        .await
        .unwrap();
    assert_eq!(store.list().await.unwrap().len(), 0);
    assert_eq!(
        changes(&store).await,
        [
            (Entity::Customization, ChangeOp::Insert),
            (Entity::Customization, ChangeOp::Update),
            (Entity::Customization, ChangeOp::Delete),
        ]
    );
}
