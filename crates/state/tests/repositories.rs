use std::path::PathBuf;

use mcp_vault_domain::{
    ActorId, DomainError, Revision, VaultContext, VaultId, VaultSlug, WritePrecondition,
};
use mcp_vault_state::{StateStore, VaultRepository, VaultStatus};
use serde_json::json;

async fn store() -> StateStore {
    StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap()
}

fn context(slug: &str, root: &str) -> VaultContext {
    VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        PathBuf::from(root),
        Revision::new(1),
    )
    .unwrap()
}

async fn insert(repository: &VaultRepository, context: &VaultContext) {
    repository
        .insert(context, context.slug().as_str(), VaultStatus::Active)
        .await
        .unwrap();
}

#[tokio::test]
async fn vault_repository_round_trips_typed_context_and_status() {
    let store = store().await;
    let repository = store.vaults();
    let context = context("work", "/srv/work");

    let inserted = repository
        .insert(&context, "Work Vault", VaultStatus::Active)
        .await
        .unwrap();
    let by_id = repository.find_by_id(context.id()).await.unwrap().unwrap();
    let by_slug = repository
        .find_by_slug(context.slug())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(inserted, by_id);
    assert_eq!(by_id, by_slug);
    assert_eq!(by_id.context().unwrap(), context);

    repository
        .set_status(&context, VaultStatus::Maintenance)
        .await
        .unwrap();
    assert_eq!(
        repository
            .find_by_id(context.id())
            .await
            .unwrap()
            .unwrap()
            .status,
        VaultStatus::Maintenance
    );
}

#[tokio::test]
async fn settings_repository_keeps_two_vaults_isolated() {
    let store = store().await;
    let vaults = store.vaults();
    let settings = store.settings();
    let first = context("first", "/srv/first");
    let second = context("second", "/srv/second");
    insert(&vaults, &first).await;
    insert(&vaults, &second).await;
    let actor = ActorId::new("test-actor").unwrap();

    let first_record = settings
        .set_vault(
            &first,
            "feature.mode",
            &json!({"vault": "first"}),
            WritePrecondition::CreateOnly,
            Some(&actor),
        )
        .await
        .unwrap();
    let second_record = settings
        .set_vault(
            &second,
            "feature.mode",
            &json!({"vault": "second"}),
            WritePrecondition::CreateOnly,
            Some(&actor),
        )
        .await
        .unwrap();

    assert_eq!(first_record.vault_id, Some(first.id()));
    assert_eq!(second_record.vault_id, Some(second.id()));
    assert_eq!(
        settings
            .get_vault(&first, "feature.mode")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!({"vault": "first"})
    );
    assert_eq!(
        settings
            .get_vault(&second, "feature.mode")
            .await
            .unwrap()
            .unwrap()
            .value,
        json!({"vault": "second"})
    );

    let updated = settings
        .set_vault(
            &first,
            "feature.mode",
            &json!({"vault": "first-updated"}),
            WritePrecondition::ExactRevision(first_record.revision),
            Some(&actor),
        )
        .await
        .unwrap();
    assert_eq!(updated.revision, Revision::new(2));

    let stale = settings
        .set_vault(
            &first,
            "feature.mode",
            &json!({"vault": "lost-update"}),
            WritePrecondition::ExactRevision(first_record.revision),
            Some(&actor),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        stale,
        mcp_vault_state::StateError::InvalidDomain(DomainError::RevisionConflict { .. })
    ));
}

#[tokio::test]
async fn recent_revisions_are_vault_scoped_and_bounded() {
    let store = store().await;
    let first = context("first", "/srv/first");
    let second = context("second", "/srv/second");
    insert(&store.vaults(), &first).await;
    insert(&store.vaults(), &second).await;

    assert!(
        store
            .files()
            .list_recent_revisions(&first, 1)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .files()
            .list_recent_revisions(&second, 1)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .files()
            .list_recent_revisions(&first, 0)
            .await
            .is_err()
    );
    assert!(
        store
            .files()
            .list_recent_revisions(&first, 201)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn system_settings_are_global_but_keep_revision_preconditions() {
    let store = store().await;
    let settings = store.settings();

    let first = settings
        .set_system(
            "system.locale",
            &json!("zh-CN"),
            WritePrecondition::CreateOnly,
            None,
        )
        .await
        .unwrap();
    assert_eq!(first.vault_id, None);
    assert_eq!(first.revision, Revision::new(1));

    let error = settings
        .set_system(
            "system.locale",
            &json!("en-US"),
            WritePrecondition::CreateOnly,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        mcp_vault_state::StateError::InvalidDomain(DomainError::PreconditionFailed { .. })
    ));
}

#[tokio::test]
async fn settings_require_a_registered_vault_context() {
    let store = store().await;
    let error = store
        .settings()
        .set_vault(
            &context("unregistered", "/srv/unregistered"),
            "key",
            &json!(true),
            WritePrecondition::CreateOnly,
            None,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, mcp_vault_state::StateError::Database(_)));
}
