use std::{collections::BTreeSet, path::PathBuf};

use mcp_vault_eval::{
    EvalError, TaskSplit, convert_m6_synthetic_fixture_manifest, validate_m6_synthetic_manifest,
    validate_manifest, verify_synthetic_fixture_files,
};

const MANIFEST: &str = include_str!("fixtures/semantic-memory-m6/manifest.json");

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic-memory-m6")
}

fn fixture_manifest() -> mcp_vault_eval::EvaluationManifest {
    convert_m6_synthetic_fixture_manifest(MANIFEST).expect("M6 fixture is valid")
}

#[test]
fn m6_fixture_has_frozen_size_fences_and_disjoint_splits() {
    let manifest = fixture_manifest();
    assert_eq!(manifest.sources.len(), 30);
    assert_eq!(manifest.tasks.len(), 60);
    assert!(manifest.synthetic_only);
    assert_eq!(
        manifest
            .tasks
            .iter()
            .filter(|task| task.split == TaskSplit::Development)
            .count(),
        36
    );
    assert_eq!(
        manifest
            .tasks
            .iter()
            .filter(|task| task.split == TaskSplit::Holdout)
            .count(),
        24
    );
    verify_synthetic_fixture_files(&manifest, &fixture_root()).unwrap();

    let mut source_splits = std::collections::BTreeMap::new();
    for task in &manifest.tasks {
        for source_id in &task.source_ids {
            let previous = source_splits.insert(source_id, &task.split);
            assert!(previous.is_none_or(|split| split == &task.split));
        }
        assert_eq!(
            task.source_fence
                .iter()
                .map(|fence| fence.source_id.as_str())
                .collect::<BTreeSet<_>>(),
            task.source_ids.iter().map(String::as_str).collect()
        );
    }
}

#[test]
fn m6_fixture_exposes_ground_truth_for_support_conditions_and_no_answer() {
    let manifest = fixture_manifest();
    assert!(
        manifest
            .tasks
            .iter()
            .any(|task| !task.expected_usable_information.is_empty())
    );
    assert!(manifest.tasks.iter().any(|task| {
        !task.must_preserve.is_empty()
            && task
                .must_preserve
                .iter()
                .any(|value| value.contains("Scope"))
    }));
    assert!(manifest.tasks.iter().any(|task| {
        task.expected_no_answer
            && task.expected_status == "unanswered"
            && task.expected_usable_information.is_empty()
    }));
    assert!(manifest.tasks.iter().any(|task| {
        task.expected_source_relations
            .iter()
            .any(|relation| relation.kind == "duplicate")
    }));
}

#[test]
fn m6_fixture_validator_rejects_hash_fence_split_and_size_drift() {
    let mut manifest = fixture_manifest();
    manifest.sources[0].content_hash = "0".repeat(64);
    assert_eq!(
        verify_synthetic_fixture_files(&manifest, &fixture_root()),
        Err(EvalError::Manifest("fixture source hash mismatch"))
    );

    let mut manifest = fixture_manifest();
    manifest.tasks[0].source_fence[0].file_revision += 1;
    assert_eq!(
        validate_manifest(&manifest),
        Err(EvalError::Manifest("task source fence is stale"))
    );

    let mut manifest = fixture_manifest();
    manifest.tasks[0].split = TaskSplit::Holdout;
    assert_eq!(
        validate_m6_synthetic_manifest(&manifest),
        Err(EvalError::Manifest(
            "development and holdout tasks share a source"
        ))
    );

    let mut manifest = fixture_manifest();
    manifest.tasks.truncate(59);
    assert_eq!(
        validate_m6_synthetic_manifest(&manifest),
        Err(EvalError::Manifest(
            "M6 synthetic fixture requires at least 30 sources and 60 tasks"
        ))
    );
}

#[test]
fn m6_fixture_validator_rejects_duplicate_query_or_relation_refs() {
    let mut manifest = fixture_manifest();
    let query_ref = manifest.tasks[0].query_refs[0].clone();
    manifest.tasks[0].query_refs.push(query_ref);
    assert!(matches!(
        validate_m6_synthetic_manifest(&manifest),
        Err(EvalError::Manifest("M6 query references are duplicated"))
    ));

    let mut manifest = fixture_manifest();
    let relation_source = manifest.tasks[0].expected_source_relations[0].source_ids[0].clone();
    manifest.tasks[0].expected_source_relations[0]
        .source_ids
        .push(relation_source);
    assert!(matches!(
        validate_m6_synthetic_manifest(&manifest),
        Err(EvalError::Manifest(
            "M6 task references are incomplete or outside its source set"
        ))
    ));
}
