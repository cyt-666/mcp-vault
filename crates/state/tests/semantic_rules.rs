use std::path::PathBuf;

use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_state::{StateError, StateStore, VaultStatus};
use serde_json::json;

fn context(slug: &str) -> VaultContext {
    VaultContext::new(
        VaultId::new(),
        VaultSlug::new(slug).unwrap(),
        PathBuf::from(format!("/tmp/mcp-vault-rules-{slug}")),
        Revision::ZERO,
    )
    .unwrap()
}

async fn store(slug: &str) -> (StateStore, VaultContext) {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = context(slug);
    state
        .vaults()
        .insert(&context, slug, VaultStatus::Active)
        .await
        .unwrap();
    (state, context)
}

#[tokio::test]
async fn rules_are_vault_scoped_revision_checked_and_idempotent() {
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let first = context("first");
    let second = context("second");
    for context in [&first, &second] {
        state
            .vaults()
            .insert(context, context.slug().as_str(), VaultStatus::Active)
            .await
            .unwrap();
    }
    let rules = state.semantic_rules();
    let target = rules
        .register_target(&first, "memory_card", "project", 1, "source:topic:decision")
        .await
        .unwrap();
    let suppression = rules
        .apply_suppression(
            &first,
            "memory_card",
            "project",
            1,
            "source:topic:decision",
            &json!({"reason":"user_forbid"}),
            "suppress_read",
            None,
            None,
            None,
            None,
            None,
            Some(0),
            "test-actor",
            "suppress-once",
        )
        .await
        .unwrap();
    assert_eq!(suppression.target_key, target.target_key);
    assert_eq!(rules.current_rules_revision(&first).await.unwrap(), 1);
    let replay = rules
        .apply_suppression(
            &first,
            "memory_card",
            "project",
            1,
            "source:topic:decision",
            &json!({"reason":"user_forbid"}),
            "suppress_read",
            None,
            None,
            None,
            None,
            None,
            Some(0),
            "test-actor",
            "suppress-once",
        )
        .await
        .unwrap();
    assert_eq!(replay.id, suppression.id);
    assert!(
        rules
            .apply_suppression(
                &first,
                "memory_card",
                "project",
                1,
                "source:topic:decision",
                &json!({"reason":"changed"}),
                "suppress_read",
                None,
                None,
                None,
                None,
                None,
                Some(0),
                "test-actor",
                "different-request",
            )
            .await
            .is_err()
    );
    assert_eq!(rules.current_rules_revision(&second).await.unwrap(), 0);
    assert!(
        rules
            .target(&second, &target.target_key)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn task_completion_requires_current_evidence() {
    let (state, context) = store("tasks").await;
    let error = state
        .semantic_rules()
        .set_task_state(
            &context,
            "task-1",
            "task",
            "project",
            1,
            "task-fingerprint",
            "completed",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StateError::InvalidInput(_)));
}

#[tokio::test]
async fn correction_rejects_old_and_new_lines_that_normalize_to_the_same_markdown() {
    let (state, context) = store("correction-lines").await;
    let error = state
        .semantic_rules()
        .apply_correction(
            &context,
            "memory_card",
            "project",
            1,
            "same-line",
            &json!({"replace":"- Keep the boundary", "remove":"1. Keep\u{00a0}the boundary"}),
            None,
            None,
            None,
            None,
            None,
            None,
            "test-actor",
            "same-normalized-lines",
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StateError::InvalidInput(_)));
}
