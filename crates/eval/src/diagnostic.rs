//! Bounded, single-source live Provider diagnostics.
//!
//! This is intentionally separate from the complete M6 runner. It uses the
//! same frozen manifest, source fences, Provider boundary, and semantic
//! application boundary, but never evaluates tasks or writes a quality claim.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{
    ComparisonArm, EvalError, EvaluationRunConfig, LiveProviderOutput, LiveProviderRequest,
    ProviderAppBoundary, ProviderRuntimeSnapshot, SemanticMemoryAppBoundary,
    SourceSnapshotVerifier, StrictFunctionCallIssue, StructuredJsonDiagnostic, safe_code,
    validate_live_evaluation_config, verify_live_source,
};

const DIAGNOSTIC_SCHEMA: &str = "semantic-memory-eval-live-diagnostic-v2";
const DIAGNOSTIC_BUDGET: u32 = 2;

/// Explicit source selection supplied by the diagnostic CLI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticSelection {
    pub arm: ComparisonArm,
    pub source_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticFailure {
    sequence: u32,
    stage: String,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_issue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_json_diagnostic: Option<StructuredJsonDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol_issue: Option<StrictFunctionCallIssue>,
}

struct DiagnosticProviderCallError {
    code: String,
    schema_issue: Option<String>,
    schema_path: Option<String>,
    structured_json_diagnostic: Option<StructuredJsonDiagnostic>,
    protocol_issue: Option<StrictFunctionCallIssue>,
}

/// Safe machine-readable diagnostic result. It contains no prompt, source
/// body, Provider response, credential, header, or generated output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSemanticDiagnosticResult {
    pub schema_version: &'static str,
    pub status: String,
    pub quality_claim: &'static str,
    pub m6_acceptance: &'static str,
    pub manifest_hash: String,
    pub run_config_hash: String,
    pub arm: ComparisonArm,
    pub source_id: String,
    pub sequence: u32,
    pub stage: String,
    pub provider_requests: u32,
    pub body_context_overlap_removed_count: u64,
    pub first_error: Option<DiagnosticFailure>,
    pub artifact_root: String,
}

/// Perform the complete frozen M6 validation and diagnostic-only selection
/// checks before opening State/Auth or constructing a Provider service.
pub fn validate_live_semantic_diagnostic_config(
    config: &crate::LiveEvaluationConfig,
    selection: &DiagnosticSelection,
) -> Result<(String, String), EvalError> {
    let hashes = validate_live_evaluation_config(config)?;
    validate_selection(config, selection)?;
    let root = Path::new(&config.artifact_root);
    if !root.is_absolute() {
        return Err(EvalError::UnsafeArtifactPath);
    }
    if root.exists()
        && fs::read_dir(root)
            .map_err(|_| EvalError::ArtifactIo)?
            .next()
            .is_some()
    {
        return Err(EvalError::LiveConfig(
            "diagnostic artifact root must be a fresh empty directory",
        ));
    }
    crate::private_fs::validate_private_directory_if_exists(root)
        .map_err(|_| EvalError::LiveConfig("diagnostic artifact root must be private"))?;
    Ok(hashes)
}

/// Run one explicitly authorized, bounded source diagnostic.
pub async fn run_live_semantic_diagnostic<V, P, S>(
    config: &crate::LiveEvaluationConfig,
    verifier: &V,
    provider: &P,
    semantic: &S,
    selection: DiagnosticSelection,
) -> Result<LiveSemanticDiagnosticResult, EvalError>
where
    V: SourceSnapshotVerifier,
    P: ProviderAppBoundary,
    S: SemanticMemoryAppBoundary,
{
    let (manifest_hash, run_config_hash) =
        validate_live_semantic_diagnostic_config(config, &selection)?;
    let root = Path::new(&config.artifact_root);
    if !root.is_absolute() {
        return Err(EvalError::UnsafeArtifactPath);
    }
    if root.exists()
        && fs::read_dir(root)
            .map_err(|_| EvalError::ArtifactIo)?
            .next()
            .is_some()
    {
        return Err(EvalError::LiveConfig(
            "diagnostic artifact root must be a fresh empty directory",
        ));
    }
    if !root.exists() {
        fs::create_dir_all(root).map_err(|_| EvalError::ArtifactIo)?;
    }
    crate::private_fs::validate_private_directory_if_exists(root)
        .map_err(|_| EvalError::LiveConfig("diagnostic artifact root must be private"))?;

    let source = config
        .manifest
        .sources
        .iter()
        .find(|source| source.logical_id == selection.source_id)
        .ok_or(EvalError::LiveConfig("diagnostic source is missing"))?;
    let mut sequence = 0_u32;
    let mut provider_requests = 0_u32;
    let request_counter = AtomicU32::new(0);

    if verify_live_source(source, verifier).await.is_err()
        || semantic
            .verify_arm_task_sources(
                selection.arm.clone(),
                std::slice::from_ref(source),
                "diagnostic_before_prepare",
            )
            .await
            .is_err()
    {
        return finish(
            config,
            manifest_hash,
            run_config_hash,
            &selection,
            sequence,
            provider_requests,
            Some(failure(
                sequence,
                "source_fence",
                "source_mismatch",
                None,
                None,
            )),
        );
    }

    let prepared = match semantic
        .prepare_for_arm(selection.arm.clone(), source)
        .await
    {
        Ok(value) => value,
        Err(code) => {
            let _ = semantic
                .abort_semantic_for_arm(selection.arm.clone(), source)
                .await;
            return finish(
                config,
                manifest_hash,
                run_config_hash,
                &selection,
                sequence,
                provider_requests,
                Some(failure(sequence, "prepare", &code, None, None)),
            );
        }
    };

    let observation = match call_diagnostic_provider(
        provider,
        config.provider_runtime_snapshot.as_ref(),
        &config.run_config,
        &request_counter,
        &mut sequence,
        &mut provider_requests,
        "observation",
        selection.arm.clone(),
        Some(selection.source_id.clone()),
        None,
        prepared,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            let _ = semantic
                .abort_semantic_for_arm(selection.arm.clone(), source)
                .await;
            return finish(
                config,
                manifest_hash,
                run_config_hash,
                &selection,
                sequence,
                provider_requests,
                Some({
                    let mut diagnostic = failure(
                        sequence,
                        "provider_observation",
                        &error.code,
                        error.schema_issue,
                        error.schema_path,
                    );
                    diagnostic.structured_json_diagnostic = error.structured_json_diagnostic;
                    diagnostic.protocol_issue = error.protocol_issue;
                    diagnostic
                }),
            );
        }
    };

    if !verify_diagnostic_fences(
        verifier,
        semantic,
        selection.arm.clone(),
        source,
        "diagnostic_after_provider",
    )
    .await
    {
        let _ = semantic
            .abort_semantic_for_arm(selection.arm.clone(), source)
            .await;
        return finish(
            config,
            manifest_hash,
            run_config_hash,
            &selection,
            sequence,
            provider_requests,
            Some(failure(
                sequence,
                "source_fence_after_provider",
                "source_mismatch",
                None,
                None,
            )),
        );
    }

    let accepted_observation = semantic
        .accept_observation_for_arm(selection.arm.clone(), source, &observation.output)
        .await;
    let overlap_removed = semantic
        .take_observation_context_overlap_removed_for_arm(selection.arm.clone(), source)
        .await;
    let composition_input = match accepted_observation {
        Ok(value) => value,
        Err(code) => {
            let _ = semantic
                .abort_semantic_for_arm(selection.arm.clone(), source)
                .await;
            return finish_with_overlap_count(
                config,
                manifest_hash,
                run_config_hash,
                &selection,
                sequence,
                provider_requests,
                overlap_removed,
                Some(failure(sequence, "accept_observation", &code, None, None)),
            );
        }
    };

    if composition_input.get("status").and_then(Value::as_str) == Some("success_empty") {
        return finish_with_overlap_count(
            config,
            manifest_hash,
            run_config_hash,
            &selection,
            sequence,
            provider_requests,
            overlap_removed,
            None,
        );
    }
    if composition_input.get("status").and_then(Value::as_str) != Some("composition_required") {
        let _ = semantic
            .abort_semantic_for_arm(selection.arm.clone(), source)
            .await;
        return finish_with_overlap_count(
            config,
            manifest_hash,
            run_config_hash,
            &selection,
            sequence,
            provider_requests,
            overlap_removed,
            Some(failure(
                sequence,
                "accept_observation",
                "semantic_composition_required_status_invalid",
                None,
                None,
            )),
        );
    }

    if !verify_diagnostic_fences(
        verifier,
        semantic,
        selection.arm.clone(),
        source,
        "diagnostic_before_composition",
    )
    .await
    {
        let _ = semantic
            .abort_semantic_for_arm(selection.arm.clone(), source)
            .await;
        return finish_with_overlap_count(
            config,
            manifest_hash,
            run_config_hash,
            &selection,
            sequence,
            provider_requests,
            overlap_removed,
            Some(failure(
                sequence,
                "source_fence_before_composition",
                "source_mismatch",
                None,
                None,
            )),
        );
    }

    let composition = match call_diagnostic_provider(
        provider,
        config.provider_runtime_snapshot.as_ref(),
        &config.run_config,
        &request_counter,
        &mut sequence,
        &mut provider_requests,
        "composition",
        selection.arm.clone(),
        Some(selection.source_id.clone()),
        None,
        composition_input,
    )
    .await
    {
        Ok(output) => output,
        Err(error) => {
            let _ = semantic
                .abort_semantic_for_arm(selection.arm.clone(), source)
                .await;
            return finish_with_overlap_count(
                config,
                manifest_hash,
                run_config_hash,
                &selection,
                sequence,
                provider_requests,
                overlap_removed,
                Some({
                    let mut diagnostic = failure(
                        sequence,
                        "provider_composition",
                        &error.code,
                        error.schema_issue,
                        error.schema_path,
                    );
                    diagnostic.structured_json_diagnostic = error.structured_json_diagnostic;
                    diagnostic.protocol_issue = error.protocol_issue;
                    diagnostic
                }),
            );
        }
    };
    if !verify_diagnostic_fences(
        verifier,
        semantic,
        selection.arm.clone(),
        source,
        "diagnostic_after_composition_provider",
    )
    .await
    {
        let _ = semantic
            .abort_semantic_for_arm(selection.arm.clone(), source)
            .await;
        return finish_with_overlap_count(
            config,
            manifest_hash,
            run_config_hash,
            &selection,
            sequence,
            provider_requests,
            overlap_removed,
            Some(failure(
                sequence,
                "source_fence_after_composition_provider",
                "source_mismatch",
                None,
                None,
            )),
        );
    }
    let submitted = match semantic
        .submit_composition_for_arm(selection.arm.clone(), source, &composition.output)
        .await
    {
        Ok(value) => value,
        Err(code) => {
            let _ = semantic
                .abort_semantic_for_arm(selection.arm.clone(), source)
                .await;
            return finish_with_overlap_count(
                config,
                manifest_hash,
                run_config_hash,
                &selection,
                sequence,
                provider_requests,
                overlap_removed,
                Some(failure(sequence, "submit_composition", &code, None, None)),
            );
        }
    };
    let state = submitted.get("state").and_then(Value::as_str);
    if !matches!(state, Some("success_empty") | Some("success_nonempty")) {
        let _ = semantic
            .abort_semantic_for_arm(selection.arm.clone(), source)
            .await;
        return finish_with_overlap_count(
            config,
            manifest_hash,
            run_config_hash,
            &selection,
            sequence,
            provider_requests,
            overlap_removed,
            Some(failure(
                sequence,
                "submit_composition",
                "semantic_extraction_not_published",
                None,
                None,
            )),
        );
    }
    if !verify_diagnostic_fences(
        verifier,
        semantic,
        selection.arm.clone(),
        source,
        "diagnostic_after_submit",
    )
    .await
    {
        return finish_with_overlap_count(
            config,
            manifest_hash,
            run_config_hash,
            &selection,
            sequence,
            provider_requests,
            overlap_removed,
            Some(failure(
                sequence,
                "source_fence_after_submit",
                "source_mismatch",
                None,
                None,
            )),
        );
    }
    finish_with_overlap_count(
        config,
        manifest_hash,
        run_config_hash,
        &selection,
        sequence,
        provider_requests,
        overlap_removed,
        None,
    )
}

async fn verify_diagnostic_fences<V, S>(
    verifier: &V,
    semantic: &S,
    arm: ComparisonArm,
    source: &crate::EvalSource,
    boundary: &str,
) -> bool
where
    V: SourceSnapshotVerifier,
    S: SemanticMemoryAppBoundary,
{
    verify_live_source(source, verifier).await.is_ok()
        && semantic
            .verify_arm_task_sources(arm, std::slice::from_ref(source), boundary)
            .await
            .is_ok()
}

fn validate_selection(
    config: &crate::LiveEvaluationConfig,
    selection: &DiagnosticSelection,
) -> Result<(), EvalError> {
    if !matches!(selection.arm, ComparisonArm::B | ComparisonArm::C)
        || selection.source_id.trim().is_empty()
    {
        return Err(EvalError::LiveConfig(
            "diagnostic arm/source selection is invalid",
        ));
    }
    let comparison = config
        .run_config
        .comparisons
        .iter()
        .find(|comparison| comparison.arm == selection.arm)
        .ok_or(EvalError::LiveConfig("diagnostic arm is missing"))?;
    if !comparison.source_ids.contains(&selection.source_id) {
        return Err(EvalError::LiveConfig(
            "diagnostic source is outside arm allowlist",
        ));
    }
    let holdout_union = config
        .manifest
        .tasks
        .iter()
        .filter(|task| matches!(task.split, crate::TaskSplit::Holdout))
        .flat_map(|task| task.source_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    if !holdout_union.contains(&selection.source_id) {
        return Err(EvalError::LiveConfig(
            "diagnostic source is outside holdout source union",
        ));
    }
    if !config
        .manifest
        .sources
        .iter()
        .any(|source| source.logical_id == selection.source_id)
    {
        return Err(EvalError::LiveConfig(
            "diagnostic source is not in manifest",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn call_diagnostic_provider<P: ProviderAppBoundary>(
    provider: &P,
    provider_snapshot: Option<&ProviderRuntimeSnapshot>,
    run_config: &EvaluationRunConfig,
    request_counter: &AtomicU32,
    sequence: &mut u32,
    provider_requests: &mut u32,
    stage: &str,
    arm: ComparisonArm,
    source_id: Option<String>,
    task_id: Option<String>,
    input: Value,
) -> Result<LiveProviderOutput, DiagnosticProviderCallError> {
    if let Some(expected) = provider_snapshot {
        provider
            .verify_frozen_configuration(expected)
            .await
            .map_err(|error| DiagnosticProviderCallError {
                code: error.code,
                schema_issue: error.schema_issue,
                schema_path: error.schema_path,
                structured_json_diagnostic: error.structured_json_diagnostic,
                protocol_issue: error.protocol_issue,
            })?;
    }
    request_counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            current
                .checked_add(1)
                .filter(|next| *next <= DIAGNOSTIC_BUDGET)
        })
        .map_err(|_| DiagnosticProviderCallError {
            code: "diagnostic_request_budget_exhausted".into(),
            schema_issue: None,
            schema_path: None,
            structured_json_diagnostic: None,
            protocol_issue: None,
        })?;
    let arm_config = run_config
        .comparisons
        .iter()
        .find(|comparison| comparison.arm == arm)
        .ok_or_else(|| DiagnosticProviderCallError {
            code: "diagnostic_arm_missing".into(),
            schema_issue: None,
            schema_path: None,
            structured_json_diagnostic: None,
            protocol_issue: None,
        })?;
    *sequence = sequence.saturating_add(1);
    let attempts_before = provider.transport_attempt_count();
    let output = provider
        .generate(LiveProviderRequest {
            sequence: *sequence,
            stage: stage.to_owned(),
            arm,
            source_id,
            task_id,
            model_id: arm_config.answer_model_id.clone(),
            prompt_id: arm_config.prompt_id.clone(),
            schema_id: arm_config.schema_id.clone(),
            index_profile_id: arm_config.index_profile_id.clone(),
            input,
        })
        .await;
    let attempts = match (attempts_before, provider.transport_attempt_count()) {
        (Some(before), Some(after)) => after.saturating_sub(before),
        _ => 1,
    };
    *provider_requests = provider_requests.saturating_add(attempts);
    output.map_err(|error| DiagnosticProviderCallError {
        code: error.code,
        schema_issue: error.schema_issue,
        schema_path: error.schema_path,
        structured_json_diagnostic: error.structured_json_diagnostic,
        protocol_issue: error.protocol_issue,
    })
}

fn failure(
    sequence: u32,
    stage: &str,
    code: &str,
    schema_issue: Option<String>,
    schema_path: Option<String>,
) -> DiagnosticFailure {
    DiagnosticFailure {
        sequence,
        stage: stage.to_owned(),
        code: safe_code(code),
        schema_issue: schema_issue.map(|value| safe_code(&value)),
        schema_path: schema_path.map(|value| value.chars().take(256).collect()),
        structured_json_diagnostic: None,
        protocol_issue: None,
    }
}

fn finish(
    config: &crate::LiveEvaluationConfig,
    manifest_hash: String,
    run_config_hash: String,
    selection: &DiagnosticSelection,
    sequence: u32,
    provider_requests: u32,
    first_error: Option<DiagnosticFailure>,
) -> Result<LiveSemanticDiagnosticResult, EvalError> {
    finish_with_overlap_count(
        config,
        manifest_hash,
        run_config_hash,
        selection,
        sequence,
        provider_requests,
        0,
        first_error,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_with_overlap_count(
    config: &crate::LiveEvaluationConfig,
    manifest_hash: String,
    run_config_hash: String,
    selection: &DiagnosticSelection,
    sequence: u32,
    provider_requests: u32,
    body_context_overlap_removed_count: u64,
    first_error: Option<DiagnosticFailure>,
) -> Result<LiveSemanticDiagnosticResult, EvalError> {
    let status = match first_error {
        Some(_) => "diagnostic_failed",
        None if sequence <= 1 => "diagnostic_success_empty",
        None => "diagnostic_success_nonempty",
    };
    let stage = first_error
        .as_ref()
        .map(|error| error.stage.clone())
        .unwrap_or_else(|| "complete".to_owned());
    let result = LiveSemanticDiagnosticResult {
        schema_version: DIAGNOSTIC_SCHEMA,
        status: status.to_owned(),
        quality_claim: "not_evaluated",
        m6_acceptance: "not_run",
        manifest_hash,
        run_config_hash,
        arm: selection.arm.clone(),
        source_id: selection.source_id.clone(),
        sequence,
        stage,
        provider_requests,
        body_context_overlap_removed_count,
        first_error,
        artifact_root: config.artifact_root.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&result).map_err(|_| EvalError::ArtifactIo)?;
    crate::atomic_write(
        Path::new(&config.artifact_root)
            .join("diagnostic-report.json")
            .as_path(),
        &bytes,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Budget, ComparisonConfig, EvalMode, EvalSource, EvalTask, EvaluationManifest,
        EvaluationRunConfig, ExpectedRelation, LIVE_RUN_SCHEMA, SemanticArmRoot, SemanticArmRoots,
        TaskSourceFence, TaskSplit,
    };

    fn source(id: &str) -> EvalSource {
        EvalSource {
            logical_id: id.to_owned(),
            synthetic_placeholder: false,
            vault_id: "v".into(),
            file_id: format!("file-{id}"),
            path: format!("notes/{id}.md"),
            file_revision: 1,
            content_hash: "a".repeat(64),
            logical_block_count: 0,
            source_revision_id: format!("revision-{id}"),
            authorization_revision: 1,
            profile_id: "profile".into(),
            rules_revision: 1,
            source_generation: 1,
            extraction_commit_sequence: 1,
        }
    }

    fn selection_config() -> crate::LiveEvaluationConfig {
        let source = source("S01");
        let task = EvalTask {
            id: "Q01".into(),
            split: TaskSplit::Holdout,
            query_refs: vec!["q01".into()],
            query: "question".into(),
            source_ids: vec![source.logical_id.clone()],
            source_fence: vec![TaskSourceFence {
                source_id: source.logical_id.clone(),
                file_revision: source.file_revision,
                source_revision_id: source.source_revision_id.clone(),
                content_hash: source.content_hash.clone(),
            }],
            must_preserve: vec![],
            must_not_infer: vec![],
            expected_usable_information: vec![],
            expected_no_answer: false,
            expected_status: "supported".into(),
            expected_source_relations: Vec::<ExpectedRelation>::new(),
            severity: "high".into(),
        };
        let budget = Budget {
            max_entries: 1,
            max_bytes: 1,
            max_tokens: 1,
            external_request_budget: 2,
        };
        let comparison = |arm| ComparisonConfig {
            arm,
            source_ids: vec!["S01".into()],
            task_ids: vec!["Q01".into()],
            budget: budget.clone(),
            index_profile_id: "index".into(),
            prompt_id: "prompt".into(),
            schema_id: "schema".into(),
            answer_model_id: "model".into(),
        };
        crate::LiveEvaluationConfig {
            schema_version: LIVE_RUN_SCHEMA.into(),
            semantic_protocol: String::new(),
            manifest: EvaluationManifest {
                schema_version: "semantic-memory-eval-manifest-v1".into(),
                dataset_id: "diagnostic".into(),
                synthetic_only: false,
                sources: vec![source],
                tasks: vec![task],
            },
            run_config: EvaluationRunConfig {
                schema_version: "semantic-memory-eval-run-config-v1".into(),
                mode: EvalMode::Live,
                allow_live: true,
                explicit_live_authorization: true,
                source_allowlist: vec!["S01".into(), "file-S01".into()],
                cost_budget_minor: 0,
                artifact_root: Some("/tmp/diagnostic".into()),
                comparisons: vec![
                    comparison(ComparisonArm::A),
                    comparison(ComparisonArm::B),
                    comparison(ComparisonArm::C),
                ],
            },
            run_root: "/tmp/diagnostic".into(),
            source_root: "/tmp/diagnostic/source".into(),
            state_root: "/tmp/diagnostic/state".into(),
            history_root: "/tmp/diagnostic/history".into(),
            artifact_root: "/tmp/diagnostic/artifacts".into(),
            isolated_vault_id: "v".into(),
            semantic_arm_roots: SemanticArmRoots {
                b: SemanticArmRoot {
                    vault_slug: "b".into(),
                    vault_id: "b".into(),
                    source_root: "/tmp/diagnostic/b/source".into(),
                    state_root: "/tmp/diagnostic/b/state".into(),
                    history_root: "/tmp/diagnostic/b/history".into(),
                },
                c: SemanticArmRoot {
                    vault_slug: "c".into(),
                    vault_id: "c".into(),
                    source_root: "/tmp/diagnostic/c/source".into(),
                    state_root: "/tmp/diagnostic/c/state".into(),
                    history_root: "/tmp/diagnostic/c/history".into(),
                },
            },
            current_schema: LIVE_RUN_SCHEMA.into(),
            task_budget: 1,
            unbounded_cost_authorized: true,
            provider_model_id: None,
            provider_runtime_snapshot: None,
            provider_templates: vec![],
            isolated_master_key_path: None,
        }
    }

    #[test]
    fn selection_rejects_non_b_c_and_outside_allowlist() {
        let config = selection_config();
        assert!(
            validate_selection(
                &config,
                &DiagnosticSelection {
                    arm: ComparisonArm::A,
                    source_id: "S01".into(),
                }
            )
            .is_err()
        );
        assert!(
            validate_selection(
                &config,
                &DiagnosticSelection {
                    arm: ComparisonArm::B,
                    source_id: "S02".into(),
                }
            )
            .is_err()
        );
    }

    #[test]
    fn failure_projection_keeps_only_safe_schema_details() {
        let record = failure(
            1,
            "provider_observation",
            "provider_schema_invalid",
            Some("enum_mismatch".into()),
            Some("$.observations[0].body_block_indices[0]".into()),
        );
        assert_eq!(record.code, "provider_schema_invalid");
        assert_eq!(record.schema_issue.as_deref(), Some("enum_mismatch"));
        assert_eq!(
            record.schema_path.as_deref(),
            Some("$.observations[0].body_block_indices[0]")
        );
    }
}
