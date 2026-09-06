//! Offline diagnosis from one existing synthetic cache; no Provider or mutation.
use super::*;
use serde_json::Value;

/// Recompute one completed calibration using only its saved synthetic vectors.
/// Callers supply the authorized Vault context; all reads use the repository.
/// This never opens a Vault, reads credentials, dispatches requests or publishes.
pub async fn diagnose_calibration(
    state: &StateStore,
    context: &VaultContext,
    channel: &str,
    signature: &str,
) -> Result<Value, MemoryError> {
    if !matches!(channel, "memory" | "note")
        || signature.len() != 71
        || !signature.starts_with("sha256:")
        || !signature[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(MemoryError::InvalidInput(
            "invalid calibration channel or signature",
        ));
    }
    let run = state
        .calibrations()
        .get(context, channel, signature)
        .await?
        .ok_or(MemoryError::NotFound)?;
    let report: CalibrationReport = serde_json::from_str(run.report_json.as_deref().ok_or(
        MemoryError::Configuration("diagnostic_completed_report_required"),
    )?)
    .map_err(|_| MemoryError::Configuration("diagnostic_report_invalid"))?;
    if report.benchmark_hash != hash(CORPUS)
        || report.profile.channel != channel
        || report.profile.signature != signature
        || report.execution != "server_provider_adapter"
    {
        return Err(MemoryError::Configuration(
            "diagnostic_benchmark_or_profile_mismatch",
        ));
    }
    if run.checkpoint_json.len() > 32 * 1024 * 1024 {
        return Err(MemoryError::Configuration("diagnostic_cache_exceeds_bound"));
    }
    let corpus: Corpus = serde_json::from_str(CORPUS)
        .map_err(|_| MemoryError::Configuration("calibration_benchmark_invalid"))?;
    let prepared = prepare(&corpus, channel)?;
    let cache: BTreeMap<String, Vec<f32>> = serde_json::from_str(&run.checkpoint_json)
        .map_err(|_| MemoryError::Configuration("calibration_checkpoint_invalid"))?;
    // Only known bundled inputs enter evaluation or output. Extra cache entries
    // are counted, never exported, to avoid turning diagnosis into a data dump.
    for input in &prepared.inputs {
        let vector = cache.get(&hash(input)).ok_or(MemoryError::Configuration(
            "calibration_checkpoint_incomplete",
        ))?;
        validate_vector(vector)?;
    }
    let scored = score_queries(context, &corpus, &prepared, &cache, channel).await?;
    let mut trials = Vec::new();
    let mut best: Option<(f64, CalibrationMetrics, Vec<&str>)> = None;
    let mut passing = 0;
    for floor in threshold_candidates(&scored) {
        let metrics = evaluate(&scored, "calibration", Some(floor), channel);
        let failed = failed_gates(&metrics);
        if metrics.passes() {
            passing += 1;
        }
        trials.push(json!({"min_cosine":floor,"recall_at_5":metrics.recall_at_5,
            "precision_at_5":metrics.returned_precision_at_5,
            "pure_semantic_recall_at_5":metrics.pure_semantic_recall_at_5,
            "no_answer_false_returns":metrics.no_answer_false_returns,
            "no_answer_queries":metrics.unanswered_queries,"failed_gates":failed}));
        if best.as_ref().is_none_or(|(_, old, old_failed)| {
            failed.len() < old_failed.len()
                || (failed.len() == old_failed.len()
                    && (metrics.recall_at_5 > old.recall_at_5
                        || (metrics.recall_at_5 == old.recall_at_5
                            && metrics.returned_precision_at_5 > old.returned_precision_at_5)))
        }) {
            best = Some((floor, metrics, failed));
        }
    }
    let (floor, best_metrics, best_failed) = best.ok_or(MemoryError::Configuration(
        "diagnostic_no_threshold_candidates",
    ))?;
    let cases = scored.iter().map(|entry| {
        let expected = &entry.query.relevant;
        let raw_top = entry.candidates.iter().take(5).enumerate().map(|(index, (id, cosine, lexical))| {
            json!({"id":id,"rank":index+1,"cosine":cosine,"expected":expected.contains(id),
                "lexical_admitted":*lexical>0.0 && entry.lexical_ranks.contains_key(id)})
        }).collect::<Vec<_>>();
        let expected_ranks = entry.candidates.iter().enumerate().filter(|(_, (id, _, _))| expected.contains(id))
            .map(|(index,(id,cosine,_))|json!({"id":id,"rank":index+1,"cosine":cosine})).collect::<Vec<_>>();
        let closest = ranked_candidates(entry, Some(floor), channel).iter().map(|(id,score)|json!({"id":id,"score":score})).collect::<Vec<_>>();
        json!({"id":entry.query.id,"split":entry.query.split,"query":entry.query.query,
            "expected":expected,"pure_semantic":entry.query.pure_semantic,"cross_language":entry.query.cross_language,
            "raw_semantic_top_5":raw_top,"expected_raw_ranks":expected_ranks,
            "closest_trial_top_5":closest})
    }).collect::<Vec<_>>();
    let replay = evaluate(&scored, "calibration", Some(report.min_cosine), channel);
    let replay_holdout = evaluate(&scored, "holdout", Some(report.min_cosine), channel);
    Ok(json!({
        "schema_version":1,"evaluation_scope":"builtin_benchmark_diagnostics",
        "read_only":true,"provider_requests_made":0,"published":false,
        "implementation_commit":env!("MCP_VAULT_BUILD_COMMIT"),
        "original_implementation_commit":report.implementation_commit,
        "original_implementation_version":report.implementation_version,
        "profile":report.profile,"benchmark_hash":hash(CORPUS),
        "cache":{"expected_inputs":prepared.inputs.len(),"cached_inputs":cache.len(),
            "dimension":cache[&hash(&prepared.inputs[0])].len(),"recorded_requests":run.requests},
        "original_result":{"status":run.status,"min_cosine":report.min_cosine,"passed":report.passed,
            "calibration":report.calibration,"holdout":report.holdout},
        "original_result_reproduced":json!(replay)==json!(report.calibration) && json!(replay_holdout)==json!(report.holdout),
        "raw_semantic":{"calibration":raw_recall(&scored,"calibration"),"holdout":raw_recall(&scored,"holdout")},
        "threshold_search":{"trials":trials,"passing_calibration_thresholds":passing,
            "closest_trial":{"selection_rule":"fewest_failed_gates_then_recall_then_precision_on_calibration_only",
                "min_cosine":floor,"calibration":best_metrics,"failed_gates":best_failed,
                "holdout":evaluate(&scored,"holdout",Some(floor),channel)}},
        "cases":cases,
        "limitations":["raw ranks diagnose cached vectors, not a published retrieval policy",
            "a closest trial is not a passing threshold",
            "cached vectors cannot prove original Provider response order; response index mapping was unchecked in 0.2.2"]
    }))
}

fn failed_gates(metrics: &CalibrationMetrics) -> Vec<&'static str> {
    let mut failed = Vec::new();
    if metrics.answered_queries < 40 {
        failed.push("answered_sample_count");
    }
    if metrics.unanswered_queries < 20 {
        failed.push("unanswered_sample_count");
    }
    if metrics.recall_at_5 < 0.70 {
        failed.push("recall_below_0.70");
    }
    if metrics.returned_precision_at_5 < 0.80 {
        failed.push("precision_below_0.80");
    }
    if metrics.no_answer_false_return_rate > 0.05 {
        failed.push("no_answer_false_return_above_0.05");
    }
    if metrics.pure_semantic_queries == 0 || metrics.pure_semantic_recall_at_5 < 0.70 {
        failed.push("pure_semantic_recall_below_0.70");
    }
    failed
}

fn raw_recall(scored: &[ScoredQuery<'_>], split: &str) -> Value {
    let mut answered = 0;
    let mut pure = 0;
    let mut total = 0.0;
    let mut pure_total = 0.0;
    for entry in scored
        .iter()
        .filter(|entry| entry.query.split == split && !entry.query.relevant.is_empty())
    {
        let correct = entry
            .candidates
            .iter()
            .take(5)
            .filter(|(id, _, _)| entry.query.relevant.contains(id))
            .count();
        let recall = correct as f64 / entry.query.relevant.len() as f64;
        answered += 1;
        total += recall;
        if entry.query.pure_semantic {
            pure += 1;
            pure_total += recall;
        }
    }
    json!({"answered_queries":answered,"recall_at_5":total/f64::from(answered.max(1)),
        "pure_semantic_queries":pure,"pure_semantic_recall_at_5":pure_total/f64::from(pure.max(1))})
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_vault_domain::{VaultId, VaultSlug};
    use mcp_vault_state::VaultStatus;

    #[tokio::test]
    async fn cached_diagnosis_distinguishes_correct_ranks_from_impossible_thresholds_without_writes()
     {
        let directory = tempfile::tempdir().unwrap();
        let database = format!(
            "sqlite://{}",
            directory.path().join("state.sqlite").display()
        );
        let state = StateStore::connect_and_migrate(&database).await.unwrap();
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("diagnose").unwrap(),
            directory.path().join("vault"),
            Revision::ZERO,
        )
        .unwrap();
        state
            .vaults()
            .insert(&context, "diagnose", VaultStatus::Active)
            .await
            .unwrap();
        let corpus: Corpus = serde_json::from_str(CORPUS).unwrap();
        let prepared = prepare(&corpus, "note").unwrap();
        let mut cache = BTreeMap::new();
        let dimension = corpus.documents.len() + 1;
        for (index, doc) in corpus.documents.iter().enumerate() {
            let mut vector = vec![0.0_f32; dimension];
            vector[index] = 1.0;
            for key in &prepared.documents[&doc.id] {
                cache.insert(key.clone(), vector.clone());
            }
        }
        for query in &corpus.queries {
            let mut vector = vec![0.0_f32; dimension];
            let index = if let Some(id) = query.relevant.first() {
                corpus
                    .documents
                    .iter()
                    .position(|doc| doc.id == *id)
                    .unwrap()
            } else {
                corpus
                    .documents
                    .iter()
                    .position(|doc| doc.split == query.split)
                    .unwrap()
            };
            // Answerable queries have perfect raw ranks at .8, but no-answer
            // queries score .99 against an unrelated document. No floor can
            // satisfy both gates; this does not mean raw answer recall is zero.
            let cosine: f32 = if query.relevant.is_empty() { 0.99 } else { 0.8 };
            vector[index] = cosine;
            vector[dimension - 1] = (1.0 - cosine * cosine).sqrt();
            cache.insert(prepared.queries[&query.id].clone(), vector);
        }
        let scored = score_queries(&context, &corpus, &prepared, &cache, "note")
            .await
            .unwrap();
        let signature = hash("diagnostic-contract");
        let report = CalibrationReport {
            evaluation_scope: "builtin_benchmark".into(),
            execution: "server_provider_adapter".into(),
            profile: CalibrationProfile {
                channel: "note".into(),
                model_id: ModelId::new(),
                external_model_id: "local-contract".into(),
                embedding_profile_hash: hash("profile"),
                signature: signature.clone(),
            },
            benchmark_version: corpus.version.clone(),
            benchmark_hash: hash(CORPUS),
            min_cosine: 0.999_999,
            calibration: evaluate(&scored, "calibration", Some(0.999_999), "note"),
            holdout: evaluate(&scored, "holdout", Some(0.999_999), "note"),
            lexical_holdout: evaluate(&scored, "holdout", None, "note"),
            passed: false,
            embedding_inputs: prepared.inputs.len(),
            requests: 10,
            request_bytes: 13759,
            implementation_version: "0.2.2".into(),
            implementation_commit: "unknown".into(),
            implementation_source_state: "unknown".into(),
            elapsed_ms: 100,
            evaluated_at: now(),
        };
        state
            .calibrations()
            .ensure(&context, "note", &signature)
            .await
            .unwrap();
        // An unexpected entry is never exported (only its existence is counted).
        cache.insert("do-not-export-this-private-marker".into(), vec![1.0]);
        state
            .calibrations()
            .checkpoint(&context, "note", &signature, &json!(cache))
            .await
            .unwrap();
        state
            .calibrations()
            .finish(
                &context,
                "note",
                &signature,
                "quality_failed",
                &json!(report),
            )
            .await
            .unwrap();
        let before = state
            .calibrations()
            .get(&context, "note", &signature)
            .await
            .unwrap()
            .unwrap();
        let reader = StateStore::connect_read_only(&database).await.unwrap();
        let diagnosis = diagnose_calibration(&reader, &context, "note", &signature)
            .await
            .unwrap();
        assert_eq!(diagnosis["raw_semantic"]["calibration"]["recall_at_5"], 1.0);
        assert_eq!(
            diagnosis["threshold_search"]["passing_calibration_thresholds"],
            0
        );
        assert_eq!(diagnosis["original_result_reproduced"], true);
        assert_eq!(diagnosis["provider_requests_made"], 0);
        assert!(
            !diagnosis
                .to_string()
                .contains("do-not-export-this-private-marker")
        );
        assert_eq!(diagnosis["cases"].as_array().unwrap().len(), 120);
        let after = state
            .calibrations()
            .get(&context, "note", &signature)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(json!(before), json!(after));
        assert_eq!(before.checkpoint_json, after.checkpoint_json);
        assert!(
            state
                .settings()
                .get_vault(
                    &context,
                    &format!("retrieval.calibration.active.note.{signature}")
                )
                .await
                .unwrap()
                .is_none()
        );
        let other = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("other").unwrap(),
            directory.path().join("other"),
            Revision::ZERO,
        )
        .unwrap();
        assert!(matches!(
            diagnose_calibration(&reader, &other, "note", &signature).await,
            Err(MemoryError::NotFound)
        ));

        // A missing known input must not be interpreted as a successful empty
        // result. Retry here only edits this test database, never sends requests.
        state
            .calibrations()
            .retry(&context, "note", &signature)
            .await
            .unwrap();
        cache.remove(&hash(&prepared.inputs[0]));
        state
            .calibrations()
            .checkpoint(&context, "note", &signature, &json!(cache))
            .await
            .unwrap();
        state
            .calibrations()
            .finish(
                &context,
                "note",
                &signature,
                "quality_failed",
                &json!(report),
            )
            .await
            .unwrap();
        assert_eq!(
            diagnose_calibration(&reader, &context, "note", &signature)
                .await
                .unwrap_err()
                .code(),
            "calibration_checkpoint_incomplete"
        );
    }
}
