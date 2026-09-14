//! Paired, isolated source-unit selection prompt comparison.
//!
//! This runner reads frozen production batches and calls the production
//! ProviderService/transport. It never reads the expected labels and never
//! publishes memory, starts HTTP/workers, or runs embeddings/overview.
use async_trait::async_trait;
use mcp_vault_auth::{AuthService, MasterKeyRing, load_or_create_master_key};
use mcp_vault_domain::{Revision, VaultContext, VaultId, VaultSlug};
use mcp_vault_memory::units::{SelectionOutput, SourceUnit, resolve_selection};
use mcp_vault_providers::{
    ModelCapabilities, ModelInput, ModelSettings, ProviderInput, ProviderKind, ProviderMode,
    ProviderService, ProviderSettings, RequestBudget, StructuredGenerationRequest,
};
use mcp_vault_state::{ModelRecord, StateStore, VaultStatus};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    error::Error,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn save_atomic(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_fixture() -> (Value, Value) {
        let unit = json!({"id":"u:0:10","headings":["Procedure"],"body":{"start_byte":0,"end_byte":10,"start_line":1,"end_line":1,"text":"# Procedure\n"},"context":[]});
        let source = json!({"source_id":"F","path":"f.md","candidates":[unit]});
        let batch = json!({"index":0,"unit_ids":["u:0:10"],"user":"{}","schema":{}});
        (source, batch)
    }

    #[test]
    fn offline_selection_validation_accepts_nonempty_and_rejects_unknown_or_duplicate() {
        let (source, batch) = source_fixture();
        let valid =
            json!({"selections":[{"unit_id":"u:0:10","kind":"procedure","retrieval_hint":"步骤"}]});
        assert_eq!(
            validate_selected(&source, &batch, &valid).unwrap(),
            ["u:0:10"]
        );
        let unknown =
            json!({"selections":[{"unit_id":"forged","kind":"procedure","retrieval_hint":""}]});
        assert!(validate_selected(&source, &batch, &unknown).is_err());
        let duplicate = json!({"selections":[{"unit_id":"u:0:10","kind":"procedure","retrieval_hint":""},{"unit_id":"u:0:10","kind":"procedure","retrieval_hint":""}]});
        assert!(validate_selected(&source, &batch, &duplicate).is_err());
    }

    #[tokio::test]
    async fn offline_attempt_budget_never_counts_rejected_request() {
        let budget = AttemptBudget {
            limit: 2,
            used: AtomicU64::new(0),
        };
        assert!(budget.reserve(1).await.is_ok());
        assert!(budget.reserve(1).await.is_ok());
        assert!(budget.reserve(1).await.is_err());
        assert_eq!(budget.used.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn offline_failure_snapshot_keeps_completed_raw_and_failure() {
        let directory = tempfile::tempdir().unwrap();
        let mut raw = BTreeMap::new();
        raw.insert(
            "F:v3".into(),
            vec![json!({"usage":{"prompt_tokens":3},"output":{"selections":[]}})],
        );
        let failures = vec![
            json!({"sequence":2,"code":"provider_schema_invalid","schema_issue":"enum","schema_path":"$.selections"}),
        ];
        write_results(
            directory.path(),
            "m",
            "i",
            "p",
            "deepseek-flash",
            "failed",
            2,
            &raw,
            &BTreeMap::new(),
            &failures,
            12,
        )
        .unwrap();
        let result: Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("results.json")).unwrap())
                .unwrap();
        assert_eq!(result["raw"]["F:v3"][0]["usage"]["prompt_tokens"], 3);
        assert_eq!(result["failures"][0]["code"], "provider_schema_invalid");
    }
}

#[derive(Debug)]
struct AttemptBudget {
    limit: u64,
    used: AtomicU64,
}

#[async_trait]
impl RequestBudget for AttemptBudget {
    async fn reserve(
        &self,
        _body_bytes: usize,
    ) -> std::result::Result<(), mcp_vault_providers::ProviderError> {
        loop {
            let current = self.used.load(Ordering::SeqCst);
            if current >= self.limit {
                return Err(mcp_vault_providers::ProviderError::InvalidConfiguration(
                    "selection comparison transport budget exhausted",
                ));
            }
            if self
                .used
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Ok(());
            }
        }
    }
}

async fn clone_model(
    source: &StateStore,
    source_auth: &AuthService,
    source_context: &VaultContext,
    target: &ProviderService,
    target_context: &VaultContext,
) -> Result<ModelRecord> {
    let binding = source
        .providers()
        .resolve_binding(source_context, "memory_extraction")
        .await?
        .ok_or("source memory_extraction binding missing")?;
    let model = source
        .providers()
        .get_model(binding.model_id)
        .await?
        .ok_or("source model missing")?;
    if !model.enabled {
        return Err("source model is disabled".into());
    }
    if model.external_model_id != "deepseek-flash" {
        return Err(format!("unexpected source model {}", model.external_model_id).into());
    }
    let provider = source
        .providers()
        .get_provider(model.provider_id)
        .await?
        .ok_or("source provider missing")?;
    if !provider.enabled {
        return Err("source provider is disabled".into());
    }
    if provider.provider_type != "deepseek" {
        return Err(format!("unexpected source provider {}", provider.provider_type).into());
    }
    let secret = match provider.secret_id {
        Some(id) => Some(
            source_auth
                .read_installation_secret(
                    id,
                    "provider-api-key",
                    "provider",
                    Some(&provider.id.to_string()),
                )
                .await?,
        ),
        None => None,
    };
    let mut settings = ProviderSettings::from_json(&provider.settings)?;
    settings.max_retries = 0;
    let created = target
        .create_provider(ProviderInput {
            name: "Selection prompt comparison deepseek".into(),
            kind: ProviderKind::try_from(provider.provider_type.as_str())?,
            base_url: url::Url::parse(&provider.base_url)?,
            settings,
            enabled: true,
            secret,
        })
        .await?;
    let created = target
        .register_model(ModelInput {
            provider_id: created.id,
            external_model_id: model.external_model_id,
            capabilities: ModelCapabilities::from_json(&model.capabilities)?,
            settings: ModelSettings::from_json(&model.settings)?,
            enabled: true,
        })
        .await?;
    target
        .bind_model(
            Some(target_context),
            "memory_extraction",
            created.id,
            json!({}),
            None,
        )
        .await?;
    Ok(created)
}

fn source_and_batch<'a>(
    inputs: &'a Value,
    source_id: &str,
    batch_index: usize,
) -> Result<(&'a Value, &'a Value)> {
    let source = inputs["sources"]
        .as_array()
        .ok_or("inputs sources missing")?
        .iter()
        .find(|source| source["source_id"] == source_id)
        .ok_or("plan source missing")?;
    let batch = source["batches"]
        .as_array()
        .ok_or("source batches missing")?
        .get(batch_index)
        .ok_or("plan batch missing")?;
    Ok((source, batch))
}

fn validate_selected(source: &Value, batch: &Value, output: &Value) -> Result<Vec<String>> {
    let units = batch["unit_ids"]
        .as_array()
        .ok_or("batch unit_ids missing")?
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    let candidates = source["candidates"]
        .as_array()
        .ok_or("source candidates missing")?
        .iter()
        .filter(|candidate| {
            candidate["id"]
                .as_str()
                .is_some_and(|id| units.iter().any(|unit| unit == id))
        })
        .cloned()
        .collect::<Vec<_>>();
    let candidates = candidates
        .into_iter()
        .map(serde_json::from_value::<SourceUnit>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let parsed: SelectionOutput = serde_json::from_value(output.clone())?;
    let resolved = resolve_selection(&candidates, parsed)?;
    let ids = resolved
        .into_iter()
        .map(|(unit, _)| unit.id)
        .collect::<Vec<_>>();
    if ids.iter().any(|id| !units.contains(id)) {
        return Err("resolved unit outside batch".into());
    }
    Ok(ids)
}

fn validate_plan(inputs: &Value, plan: &Value) -> Result<()> {
    let requests = plan["requests"].as_array().ok_or("request plan missing")?;
    if requests.len() != 44 || plan["request_count"] != 44 || plan["pair_count"] != 22 {
        return Err("request plan must contain exactly 44 requests and 22 pairs".into());
    }
    let mut seen = HashSet::new();
    for (offset, request) in requests.iter().enumerate() {
        if request["sequence"].as_u64() != Some(offset as u64 + 1) {
            return Err("request plan sequence is not contiguous".into());
        }
        let source_id = request["source_id"]
            .as_str()
            .ok_or("request source_id missing")?;
        let batch_index = request["batch_index"]
            .as_u64()
            .ok_or("request batch index missing")? as usize;
        let arm = request["arm"].as_str().ok_or("request arm missing")?;
        if !matches!(arm, "v3" | "v4") {
            return Err("request plan arm invalid".into());
        }
        let (source, batch) = source_and_batch(inputs, source_id, batch_index)?;
        if request["source_path"] != source["path"]
            || request["input_ref"]["source_id"] != source_id
            || request["input_ref"]["batch_index"] != batch_index
        {
            return Err("request plan input reference mismatch".into());
        }
        let key = format!("{source_id}:{batch_index}:{arm}");
        if !seen.insert(key) {
            return Err("request plan duplicate source/batch/arm".into());
        }
        if batch["index"] != batch_index as u64 {
            return Err("frozen batch index mismatch".into());
        }
    }
    for source in inputs["sources"]
        .as_array()
        .ok_or("inputs sources missing")?
    {
        let source_id = source["source_id"].as_str().ok_or("source ID missing")?;
        let count = source["batch_count"]
            .as_u64()
            .ok_or("source batch count missing")? as usize;
        for index in 0..count {
            for arm in ["v3", "v4"] {
                if !seen.contains(&format!("{source_id}:{index}:{arm}")) {
                    return Err("request plan does not cover every source batch arm".into());
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_results(
    root: &Path,
    manifest_hash: &str,
    inputs_hash: &str,
    plan_hash: &str,
    model: &str,
    status: &str,
    attempts: u64,
    raw: &BTreeMap<String, Vec<Value>>,
    final_resolve: &BTreeMap<String, Value>,
    failures: &[Value],
    elapsed_ms: u128,
) -> Result<()> {
    save_atomic(
        &root.join("results.json"),
        &json!({
            "schema":"memory-selection-prompt-comparison-results/v1", "status":status,
            "manifest_sha256":manifest_hash, "inputs_sha256":inputs_hash,
            "request_plan_sha256":plan_hash, "model":model, "provider_attempts":attempts,
            "raw":raw, "final_resolve":final_resolve, "failures":failures, "elapsed_ms":elapsed_ms,
        }),
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 || args[0] != "--run-authorized-selection-comparison" {
        return Err("usage: live_memory_selection_compare --run-authorized-selection-comparison SOURCE_DB MASTER_KEY FROZEN_DIR NEW_RUN_ROOT".into());
    }
    let source_db = PathBuf::from(&args[1]).canonicalize()?;
    let source_key = PathBuf::from(&args[2]).canonicalize()?;
    let frozen = PathBuf::from(&args[3]).canonicalize()?;
    let root = PathBuf::from(&args[4]);
    if root.exists() {
        return Err("comparison run root must not already exist".into());
    }
    std::fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    let manifest_bytes = std::fs::read(frozen.join("comparison-manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&manifest_bytes)?;
    let read_frozen = |name: &str| -> Result<(Vec<u8>, Value)> {
        let bytes = std::fs::read(frozen.join(name))?;
        Ok((bytes.clone(), serde_json::from_slice(&bytes)?))
    };
    let (inputs_bytes, inputs) = read_frozen("production-inputs.json")?;
    let plan_bytes = std::fs::read(frozen.join("request-plan.json"))?;
    let plan: Value = serde_json::from_slice(&plan_bytes)?;
    if hash(&inputs_bytes) != manifest["inputs"]["sha256"]
        || hash(&plan_bytes) != manifest["request_plan"]["sha256"]
    {
        return Err("frozen input or request plan hash mismatch".into());
    }
    validate_plan(&inputs, &plan)?;
    let v3_system = std::fs::read_to_string(frozen.join("system-v3.txt"))?;
    let v4_system = std::fs::read_to_string(frozen.join("system-v4.txt"))?;
    if hash(v3_system.as_bytes()) != manifest["systems"]["v3"]["prompt_sha256"]
        || hash(v4_system.as_bytes()) != manifest["systems"]["v4"]["prompt_sha256"]
    {
        return Err("system prompt hash mismatch".into());
    }

    let source =
        StateStore::connect_read_only(&format!("sqlite://{}", source_db.display())).await?;
    let source_auth = AuthService::new(source.auth(), MasterKeyRing::load_file(&source_key).await?);
    let source_context = source
        .vaults()
        .list()
        .await?
        .into_iter()
        .find(|v| v.slug.as_str() == "default")
        .ok_or("source Vault missing")?
        .context()?;
    let state = StateStore::connect_and_migrate(&format!(
        "sqlite://{}/state/mcp-vault.sqlite3",
        root.display()
    ))
    .await?;
    let auth = AuthService::new(
        state.auth(),
        load_or_create_master_key(&root.join("secrets/master-key")).await?,
    );
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("default")?,
        root.join("vaults/default"),
        Revision::ZERO,
    )?;
    state
        .vaults()
        .insert(&context, "Selection prompt comparison", VaultStatus::Active)
        .await?;
    let budget = Arc::new(AttemptBudget {
        limit: 44,
        used: AtomicU64::new(0),
    });
    let providers =
        ProviderService::new(state.clone(), auth).with_generation_budget(budget.clone());
    let model = clone_model(&source, &source_auth, &source_context, &providers, &context).await?;
    providers
        .set_provider_mode(&context, ProviderMode::RemoteAllowed, None)
        .await?;

    let mut raw: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut failures = Vec::new();
    let started = std::time::Instant::now();
    for request in plan["requests"].as_array().ok_or("request plan missing")? {
        let seq = request["sequence"]
            .as_u64()
            .ok_or("request sequence missing")?;
        let source_id = request["source_id"]
            .as_str()
            .ok_or("request source_id missing")?;
        let batch_index = request["batch_index"]
            .as_u64()
            .ok_or("request batch index missing")? as usize;
        let arm = request["arm"].as_str().ok_or("request arm missing")?;
        let (source, batch) = source_and_batch(&inputs, source_id, batch_index)?;
        let system = match arm {
            "v3" => &v3_system,
            "v4" => &v4_system,
            _ => return Err("unknown prompt arm".into()),
        };
        let req = StructuredGenerationRequest {
            model: model.external_model_id.clone(),
            system: system.clone(),
            user: batch["user"]
                .as_str()
                .ok_or("batch user missing")?
                .to_owned(),
            schema_name: manifest["request"]["schema_name"]
                .as_str()
                .ok_or("schema name missing")?
                .to_owned(),
            schema: batch["schema"].clone(),
            allow_additional_output_properties: false,
            missing_required_string_fallbacks: Vec::new(),
            max_output_tokens: manifest["request"]["max_output_tokens_requested"]
                .as_u64()
                .ok_or("max output token setting missing")? as u32,
            temperature: Some(0.0),
            timeout: Some(Duration::from_secs(
                manifest["request"]["timeout_seconds"]
                    .as_u64()
                    .ok_or("timeout setting missing")?,
            )),
        };
        let generated = match providers
            .generate_structured(&context, model.id, &req)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let (issue, path) = error
                    .schema_diagnostic()
                    .map_or((Value::Null, Value::Null), |(issue, path)| {
                        (json!(issue), json!(path))
                    });
                failures.push(json!({"sequence":seq,"pair":request["pair"],"source_id":source_id,"batch_index":batch_index,"arm":arm,"code":error.code(),"schema_issue":issue,"schema_path":path}));
                write_results(
                    &root,
                    &hash(&manifest_bytes),
                    &hash(&inputs_bytes),
                    &hash(&plan_bytes),
                    &model.external_model_id,
                    "failed",
                    budget.used.load(Ordering::SeqCst),
                    &raw,
                    &BTreeMap::new(),
                    &failures,
                    started.elapsed().as_millis(),
                )?;
                break;
            }
        };
        let resolved_ids = match validate_selected(source, batch, &generated.value) {
            Ok(ids) => ids,
            Err(error) => {
                failures.push(json!({"sequence":seq,"pair":request["pair"],"source_id":source_id,"batch_index":batch_index,"arm":arm,"code":"reference_or_schema_validation","detail":error.to_string(),"output":generated.value,"usage":generated.usage}));
                write_results(
                    &root,
                    &hash(&manifest_bytes),
                    &hash(&inputs_bytes),
                    &hash(&plan_bytes),
                    &model.external_model_id,
                    "failed",
                    budget.used.load(Ordering::SeqCst),
                    &raw,
                    &BTreeMap::new(),
                    &failures,
                    started.elapsed().as_millis(),
                )?;
                break;
            }
        };
        let key = format!("{source_id}:{arm}");
        raw.entry(key).or_default().push(json!({"sequence":seq,"pair":request["pair"],"source_id":source_id,"batch_index":batch_index,"arm":arm,"unit_ids":batch["unit_ids"],"output":generated.value,"resolved_unit_ids":resolved_ids,"usage":generated.usage}));
        write_results(
            &root,
            &hash(&manifest_bytes),
            &hash(&inputs_bytes),
            &hash(&plan_bytes),
            &model.external_model_id,
            "in_progress",
            budget.used.load(Ordering::SeqCst),
            &raw,
            &BTreeMap::new(),
            &failures,
            started.elapsed().as_millis(),
        )?;
        save_atomic(
            &root.join("progress.json"),
            &json!({"schema":"memory-selection-comparison-progress/v1","completed_requests":raw.values().map(Vec::len).sum::<usize>(),"failures":failures,"provider_attempts":budget.used.load(Ordering::SeqCst)}),
        )?;
    }

    let mut final_resolve = BTreeMap::new();
    for source in inputs["sources"]
        .as_array()
        .ok_or("inputs sources missing")?
    {
        let source_id = source["source_id"].as_str().ok_or("source ID missing")?;
        for arm in ["v3", "v4"] {
            let key = format!("{source_id}:{arm}");
            let rows = raw.get(&key).map(Vec::as_slice).unwrap_or_default();
            let expected_batches = source["batch_count"]
                .as_u64()
                .ok_or("source batch count missing")? as usize;
            if rows.len() != expected_batches {
                final_resolve.insert(key, json!({"status":"partial","completed_batches":rows.len(),"expected_batches":expected_batches,"resolved_unit_ids":Value::Null}));
                continue;
            }
            let mut selections = Vec::new();
            for row in rows {
                selections.extend(
                    row["output"]["selections"]
                        .as_array()
                        .ok_or("selection output missing")?
                        .iter()
                        .cloned(),
                );
            }
            let candidates = source["candidates"]
                .as_array()
                .ok_or("source candidates missing")?
                .iter()
                .cloned()
                .map(serde_json::from_value::<SourceUnit>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            match resolve_selection(
                &candidates,
                SelectionOutput {
                    selections: serde_json::from_value(Value::Array(selections))?,
                },
            ) {
                Ok(resolved) => {
                    final_resolve.insert(key, json!({"status":"complete","raw_selection_count":rows.iter().map(|r| r["output"]["selections"].as_array().map_or(0,Vec::len)).sum::<usize>(),"resolved_unit_ids":resolved.into_iter().map(|(unit,_)|unit.id).collect::<Vec<_>>() }));
                }
                Err(error) => {
                    failures.push(json!({"source_id":source_id,"arm":arm,"code":"reference_or_schema_validation","detail":error.to_string()}));
                    final_resolve.insert(
                        key,
                        json!({"status":"union_resolve_error","resolved_unit_ids":Value::Null}),
                    );
                }
            }
        }
    }
    let failed = !failures.is_empty();
    write_results(
        &root,
        &hash(&manifest_bytes),
        &hash(&inputs_bytes),
        &hash(&plan_bytes),
        &model.external_model_id,
        if failed { "failed" } else { "completed" },
        budget.used.load(Ordering::SeqCst),
        &raw,
        &final_resolve,
        &failures,
        started.elapsed().as_millis(),
    )?;
    state.close().await;
    source.close().await;
    if failed {
        return Err("selection comparison stopped on first failed request".into());
    }
    Ok(())
}
