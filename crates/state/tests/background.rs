use std::path::PathBuf;

use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_state::{JobStatus, ScanStatus, StateStore, VaultStatus};
use serde_json::json;
use tokio::time::{Duration, sleep};

async fn store_and_context() -> (StateStore, VaultContext) {
    let store = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("work").unwrap(),
        PathBuf::from("/srv/work"),
        Revision::new(1),
    )
    .unwrap();
    store
        .vaults()
        .insert(&context, "Work", VaultStatus::Active)
        .await
        .unwrap();
    (store, context)
}

#[tokio::test]
async fn jobs_deduplicate_claim_retry_and_reclaim_expired_leases() {
    let (store, context) = store_and_context().await;
    let first = store
        .jobs()
        .enqueue(
            &context,
            "index.note",
            "file:one:1",
            &json!({"file": "one"}),
            5,
            2,
            0,
        )
        .await
        .unwrap();
    let duplicate = store
        .jobs()
        .enqueue(
            &context,
            "index.note",
            "file:one:1",
            &json!({"ignored": true}),
            1,
            2,
            0,
        )
        .await
        .unwrap();
    assert_eq!(first.id, duplicate.id);

    let claimed = store
        .jobs()
        .claim_batch("worker-a", 10, 20, 8)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempts, 1);
    assert!(
        store
            .jobs()
            .claim_batch("worker-b", 10, 20, 8)
            .await
            .unwrap()
            .is_empty()
    );
    store
        .jobs()
        .update_progress(first.id, "worker-a", &json!({"done": 1}))
        .await
        .unwrap();
    assert_eq!(
        store
            .jobs()
            .retry_or_fail(first.id, "worker-a", 30, "temporary provider failure")
            .await
            .unwrap(),
        JobStatus::RetryWait
    );

    let reclaimed = store
        .jobs()
        .claim_batch("worker-b", 30, 40, 8)
        .await
        .unwrap();
    assert_eq!(reclaimed.len(), 1);
    store.jobs().complete(first.id, "worker-b").await.unwrap();
    assert_eq!(
        store
            .jobs()
            .get(&context, first.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        JobStatus::Completed
    );
}

#[tokio::test]
async fn active_jobs_remain_queryable_outside_bounded_terminal_history() {
    let (store, context) = store_and_context().await;
    let long_running = store
        .jobs()
        .enqueue(
            &context,
            "memory.extract",
            "jobs:old-running",
            &json!({"scope": "all"}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    store
        .jobs()
        .claim_batch("long-worker", 1, i64::MAX / 2, 1)
        .await
        .unwrap();
    sleep(Duration::from_millis(2)).await;

    for index in 0..55_u32 {
        store
            .jobs()
            .enqueue(
                &context,
                "index.note",
                &format!("jobs:terminal:{index}"),
                &json!({"index": index}),
                0,
                3,
                0,
            )
            .await
            .unwrap();
    }
    let terminal = store
        .jobs()
        .claim_batch("short-worker", 2, i64::MAX / 2, 55)
        .await
        .unwrap();
    assert_eq!(terminal.len(), 55);
    for job in terminal {
        store.jobs().complete(job.id, "short-worker").await.unwrap();
    }

    let recent = store
        .jobs()
        .list(&context, None, None, 50, 0)
        .await
        .unwrap();
    assert_eq!(recent.len(), 50);
    assert!(recent.iter().all(|job| job.id != long_running.id));
    let running = store
        .jobs()
        .list(&context, Some(JobStatus::Running), None, 200, 0)
        .await
        .unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, long_running.id);
    let history = store.jobs().list_terminal(&context, 50, 0).await.unwrap();
    assert_eq!(history.len(), 50);
    assert!(history.iter().all(|job| job.status == JobStatus::Completed));
    let counts = store.jobs().status_counts(&context).await.unwrap();
    assert_eq!(counts.running, 1);
    assert_eq!(counts.completed, 55);
    assert_eq!(counts.queued, 0);
}

#[tokio::test]
async fn exhausted_jobs_and_cancelled_jobs_are_terminal_and_visible() {
    let (store, context) = store_and_context().await;
    let failed = store
        .jobs()
        .enqueue(&context, "index.note", "file:failed", &json!({}), 0, 1, 0)
        .await
        .unwrap();
    store.jobs().claim_batch("worker", 1, 2, 1).await.unwrap();
    assert_eq!(
        store
            .jobs()
            .retry_or_fail(failed.id, "worker", 3, "permanent failure")
            .await
            .unwrap(),
        JobStatus::Failed
    );
    store
        .jobs()
        .request_retry(&context, failed.id)
        .await
        .unwrap();
    let retried = store
        .jobs()
        .get(&context, failed.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.status, JobStatus::Queued);
    assert_eq!(retried.attempts, 0);
    let reclaimed = store
        .jobs()
        .claim_batch(
            "retry-worker",
            retried.available_at,
            retried.available_at + 10,
            1,
        )
        .await
        .unwrap();
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].attempts, 1);
    store
        .jobs()
        .complete(failed.id, "retry-worker")
        .await
        .unwrap();

    let cancelled = store
        .jobs()
        .enqueue(
            &context,
            "index.note",
            "file:cancelled",
            &json!({}),
            0,
            2,
            0,
        )
        .await
        .unwrap();
    store
        .jobs()
        .request_cancel(&context, cancelled.id)
        .await
        .unwrap();
    assert_eq!(
        store
            .jobs()
            .get(&context, cancelled.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        JobStatus::Cancelled
    );
}

#[tokio::test]
async fn manual_memory_retry_preserves_paid_work_cursor() {
    let (store, context) = store_and_context().await;
    let extraction = store
        .jobs()
        .enqueue(
            &context,
            "memory.extract",
            "memory:partial",
            &json!({"scope": "all"}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    store
        .jobs()
        .claim_batch("memory-worker", 1, 10, 1)
        .await
        .unwrap();
    let progress = json!({
        "phase": "failed",
        "completed": 10,
        "total": 178,
        "last_completed_path": "notes/ten.md",
        "generated_output_failures": 1,
    });
    store
        .jobs()
        .update_progress(extraction.id, "memory-worker", &progress)
        .await
        .unwrap();
    store
        .jobs()
        .fail_permanently(
            extraction.id,
            "memory-worker",
            "memory_extract_output_failure_limit",
        )
        .await
        .unwrap();

    store
        .jobs()
        .request_retry(&context, extraction.id)
        .await
        .unwrap();
    let retried = store
        .jobs()
        .get(&context, extraction.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried.status, JobStatus::Queued);
    assert_eq!(retried.progress, Some(progress));
    assert!(retried.last_error.is_none());
}

#[tokio::test]
async fn active_job_lookup_is_type_and_vault_scoped_and_ignores_terminal_rows() {
    let (store, context) = store_and_context().await;
    let first = store
        .jobs()
        .enqueue(
            &context,
            "memory.extract",
            "memory:first",
            &json!({"scope": "all"}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .jobs()
            .find_active_by_type(&context, "memory.extract")
            .await
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    assert!(
        store
            .jobs()
            .find_active_by_type(&context, "index.rebuild")
            .await
            .unwrap()
            .is_none()
    );

    store
        .jobs()
        .request_cancel(&context, first.id)
        .await
        .unwrap();
    assert!(
        store
            .jobs()
            .find_active_by_type(&context, "memory.extract")
            .await
            .unwrap()
            .is_none()
    );

    let other = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("other-active-job").unwrap(),
        "/srv/other-active-job".into(),
        Revision::new(1),
    )
    .unwrap();
    store
        .vaults()
        .insert(&other, "Other", VaultStatus::Active)
        .await
        .unwrap();
    store
        .jobs()
        .enqueue(
            &other,
            "memory.extract",
            "memory:other",
            &json!({"scope": "all"}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    assert!(
        store
            .jobs()
            .find_active_by_type(&context, "memory.extract")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn memory_consolidation_is_a_vault_scoped_non_cancellable_singleton() {
    let (store, context) = store_and_context().await;
    let first = store
        .jobs()
        .enqueue_singleton(
            &context,
            "memory.consolidate",
            "memory:consolidate:first",
            &json!({"generation": 1}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    let duplicate = store
        .jobs()
        .enqueue_singleton(
            &context,
            "memory.consolidate",
            "memory:consolidate:second-trigger",
            &json!({"generation": 1}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    assert_eq!(first.id, duplicate.id);

    let claimed = store
        .jobs()
        .claim_batch("consolidation-worker", 1, 10, 1)
        .await
        .unwrap();
    assert_eq!(claimed[0].id, first.id);
    assert!(
        store
            .jobs()
            .request_cancel(&context, first.id)
            .await
            .is_err()
    );
    store
        .jobs()
        .complete(first.id, "consolidation-worker")
        .await
        .unwrap();

    let next = store
        .jobs()
        .enqueue_singleton(
            &context,
            "memory.consolidate",
            "memory:consolidate:next-generation",
            &json!({"generation": 2}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    assert_ne!(next.id, first.id);

    let other = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("other-consolidation").unwrap(),
        "/srv/other-consolidation".into(),
        Revision::new(1),
    )
    .unwrap();
    store
        .vaults()
        .insert(&other, "Other", VaultStatus::Active)
        .await
        .unwrap();
    let other_job = store
        .jobs()
        .enqueue_singleton(
            &other,
            "memory.consolidate",
            "memory:consolidate:other",
            &json!({"generation": 1}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    assert_ne!(other_job.id, next.id);
}

#[tokio::test]
async fn newer_active_job_coalescing_is_vault_scoped() {
    let (store, context) = store_and_context().await;
    let first = store
        .jobs()
        .enqueue(
            &context,
            "index.rebuild",
            "index:first",
            &json!({}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let second = store
        .jobs()
        .enqueue(
            &context,
            "index.rebuild",
            "index:second",
            &json!({}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    assert!(
        store
            .jobs()
            .has_newer_active_job(&context, "index.rebuild", first.created_at, first.id)
            .await
            .unwrap()
    );
    assert!(
        !store
            .jobs()
            .has_newer_active_job(&context, "index.rebuild", second.created_at, second.id)
            .await
            .unwrap()
    );

    let other = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("other-coalesce").unwrap(),
        "/srv/other-coalesce".into(),
        Revision::new(1),
    )
    .unwrap();
    store
        .vaults()
        .insert(&other, "Other", VaultStatus::Active)
        .await
        .unwrap();
    store
        .jobs()
        .enqueue(&other, "index.rebuild", "index:other", &json!({}), 0, 3, 0)
        .await
        .unwrap();
    assert!(
        !store
            .jobs()
            .has_newer_active_job(&context, "index.rebuild", second.created_at, second.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn running_backup_job_renews_lease_and_rejects_unsafe_cancellation() {
    let (store, context) = store_and_context().await;
    let job = store
        .jobs()
        .enqueue(
            &context,
            "backup.create",
            "backup:test",
            &json!({}),
            0,
            3,
            0,
        )
        .await
        .unwrap();
    let claimed = store
        .jobs()
        .claim_batch("backup-worker", 1, 10, 1)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert!(store.jobs().request_cancel(&context, job.id).await.is_err());
    assert!(
        !store
            .jobs()
            .renew_claimed(job.id, "backup-worker", i64::MAX / 2)
            .await
            .unwrap()
    );
    let running = store.jobs().get(&context, job.id).await.unwrap().unwrap();
    assert_eq!(running.status, JobStatus::Running);
    assert!(!running.cancel_requested);
    assert_eq!(running.lease_until, Some(i64::MAX / 2));
    store
        .jobs()
        .complete(job.id, "backup-worker")
        .await
        .unwrap();
}

#[tokio::test]
async fn scan_checkpoints_reject_stale_generations_and_keep_vaults_isolated() {
    let (store, context) = store_and_context().await;
    let other = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("other").unwrap(),
        "/srv/other".into(),
        Revision::new(1),
    )
    .unwrap();
    store
        .vaults()
        .insert(&other, "Other", VaultStatus::Active)
        .await
        .unwrap();
    let checkpoint = store
        .scan_checkpoints()
        .start(&context, "reconciliation", "gen-a")
        .await
        .unwrap();
    store
        .scan_checkpoints()
        .update_progress(
            &context,
            "reconciliation",
            "gen-a",
            Some(&"notes/a.md".parse().unwrap()),
            3,
            1,
            2,
            1,
            0,
            false,
        )
        .await
        .unwrap();
    assert!(
        store
            .scan_checkpoints()
            .update_progress(
                &context,
                "reconciliation",
                "old",
                None,
                4,
                2,
                2,
                2,
                0,
                false,
            )
            .await
            .is_err()
    );
    store
        .scan_checkpoints()
        .finish(
            &context,
            "reconciliation",
            "gen-a",
            ScanStatus::Completed,
            None,
        )
        .await
        .unwrap();
    let loaded = store
        .scan_checkpoints()
        .get(&context, "reconciliation")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.id, checkpoint.id);
    assert_eq!(loaded.status, ScanStatus::Completed);
    assert_eq!(loaded.unsafe_entries_skipped, 0);
    assert!(!loaded.missing_deletes_skipped);
    assert!(
        store
            .scan_checkpoints()
            .get(&other, "reconciliation")
            .await
            .unwrap()
            .is_none()
    );
}
