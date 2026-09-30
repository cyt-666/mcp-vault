//! No-Provider dry-run for the frozen synthetic semantic-memory corpus.
//!
//! This test reads only the fixture files under this test's own directory. It
//! does not construct StateStore, VaultCore, a Provider, or a network client.

use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

const MANIFEST_JSON: &str = include_str!("fixtures/semantic-memory/manifest.json");
const FIXTURE_DIR: &str = "tests/fixtures/semantic-memory";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: String,
    fixture_id: String,
    dataset_class: DatasetClass,
    contains_real_user_data: bool,
    execution_policy: ExecutionPolicy,
    comparison_protocol: ComparisonProtocol,
    sources: Vec<SourceFixture>,
    tasks: Vec<TaskFixture>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum DatasetClass {
    Synthetic,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum RunMode {
    Mock,
    Replay,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionPolicy {
    default_mode: RunMode,
    allowed_modes: Vec<RunMode>,
    allow_live: bool,
    external_request_budget: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComparisonProtocol {
    arms: Vec<String>,
    context_budget_tokens: u32,
    freeze_source_set: bool,
    freeze_task_set: bool,
    freeze_answer_model_per_run: bool,
    freeze_retrieval_model_per_run: bool,
    freeze_index_profile_per_run: bool,
    real_run_status: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    VaultNote,
    ImportedRecord,
    ConversationExcerpt,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum AssertionRole {
    Adopted,
    Observed,
    Imported,
    Mixed,
    TaskState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum AuthorizationState {
    Active,
    Revoked,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationScope {
    required_permissions: Vec<String>,
    state: AuthorizationState,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SourceState {
    Current,
    ChangedAfterObservation,
    MovedSameContent,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFixture {
    id: String,
    path: String,
    sha256: String,
    source_kind: SourceKind,
    assertion_role: AssertionRole,
    vault_id: String,
    authorization_scope: AuthorizationScope,
    source_state: SourceState,
    observed_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Caller {
    vault_id: String,
    permissions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ExpectedStatus {
    Available,
    SourceIneligible,
    HistoricalOnly,
    ConflictUnresolved,
    ProposedOnly,
    TaskScoped,
    Unanswered,
    AccessDenied,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum RelationKind {
    Duplicate,
    Supplements,
    DifferentScope,
    Supersedes,
    HistoricalReference,
    UnknownScopeOrTime,
    ProposedNotAdopted,
    SourceChanged,
    SameSource,
    ReferenceOnly,
    NavigationOnly,
    AuthorizationDenied,
    CrossVaultDenied,
    SourceIneligible,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedRelation {
    kind: RelationKind,
    source_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskFixture {
    id: String,
    q_refs: Vec<String>,
    coverage_tags: Vec<String>,
    question: String,
    source_ids: Vec<String>,
    caller: Caller,
    expected_usable_information: Vec<String>,
    must_preserve_conditions: Vec<String>,
    must_not_infer: Vec<String>,
    expected_status: ExpectedStatus,
    expected_source_relations: Vec<ExpectedRelation>,
    severity: Severity,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_relative_fixture_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && !value.contains(':')
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn parse_manifest() -> Manifest {
    serde_json::from_str(MANIFEST_JSON).expect("semantic-memory manifest matches its schema")
}

#[test]
fn semantic_memory_manifest_is_a_synthetic_no_provider_dry_run() {
    let manifest = parse_manifest();

    assert_eq!(manifest.schema_version, "semantic-memory-manifest-v1");
    assert_eq!(manifest.fixture_id, "semantic-memory-synthetic-m0-v1");
    assert_eq!(manifest.dataset_class, DatasetClass::Synthetic);
    assert!(!manifest.contains_real_user_data);
    assert_eq!(manifest.sources.len(), 12);
    assert_eq!(manifest.tasks.len(), 24);

    assert_eq!(manifest.execution_policy.default_mode, RunMode::Mock);
    assert_eq!(
        manifest.execution_policy.allowed_modes,
        [RunMode::Mock, RunMode::Replay]
    );
    assert!(!manifest.execution_policy.allow_live);
    assert_eq!(manifest.execution_policy.external_request_budget, 0);

    let mut live_manifest: serde_json::Value =
        serde_json::from_str(MANIFEST_JSON).expect("manifest JSON is valid");
    live_manifest["execution_policy"]["default_mode"] = serde_json::json!("live");
    assert!(
        serde_json::from_value::<Manifest>(live_manifest).is_err(),
        "live is outside this dry-run schema"
    );

    let protocol = manifest.comparison_protocol;
    assert_eq!(protocol.arms, ["A", "B", "C"]);
    assert_eq!(protocol.context_budget_tokens, 4096);
    assert!(protocol.freeze_source_set);
    assert!(protocol.freeze_task_set);
    assert!(protocol.freeze_answer_model_per_run);
    assert!(protocol.freeze_retrieval_model_per_run);
    assert!(protocol.freeze_index_profile_per_run);
    assert_eq!(protocol.real_run_status, "pending_authorization");
}

#[test]
fn semantic_memory_fixtures_have_safe_paths_current_hashes_and_complete_task_links() {
    let manifest = parse_manifest();
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_DIR);
    let canonical_root = fixture_root
        .canonicalize()
        .expect("fixture directory exists inside the crate");

    let mut source_ids = HashSet::new();
    let mut revoked_source_ids = HashSet::new();
    let mut changed_sources = HashSet::new();
    let mut moved_sources = HashSet::new();

    for source in &manifest.sources {
        assert!(source_ids.insert(source.id.as_str()), "duplicate source ID");
        assert!(
            safe_relative_fixture_path(&source.path),
            "unsafe source path"
        );
        assert!(is_sha256(&source.sha256), "invalid source SHA-256");
        assert!(!source.vault_id.is_empty());
        assert!(!source.authorization_scope.required_permissions.is_empty());
        let _source_classification = (source.source_kind, source.assertion_role);

        let relative = Path::new(&source.path);
        assert!(
            relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        );
        let path = fixture_root.join(relative);
        let metadata = fs::symlink_metadata(&path).expect("fixture source exists");
        assert!(
            metadata.file_type().is_file(),
            "fixture source must not be a symlink"
        );
        let canonical_path = path.canonicalize().expect("fixture source resolves");
        assert!(canonical_path.starts_with(&canonical_root));

        let bytes = fs::read(&canonical_path).expect("fixture source is readable");
        assert_eq!(sha256_hex(&bytes), source.sha256, "source hash mismatch");
        let text = std::str::from_utf8(&bytes).expect("fixture source is UTF-8");
        assert!(
            text.starts_with("# ") && text.contains("合成评测资料"),
            "each source must identify itself as synthetic"
        );

        match source.source_state {
            SourceState::Current => assert!(source.observed_sha256.is_none()),
            SourceState::ChangedAfterObservation => {
                let previous = source
                    .observed_sha256
                    .as_deref()
                    .expect("changed source records the observed hash");
                assert!(is_sha256(previous));
                assert_ne!(previous, source.sha256);
                changed_sources.insert(source.id.as_str());
            }
            SourceState::MovedSameContent => {
                let previous = source
                    .observed_sha256
                    .as_deref()
                    .expect("moved source records its prior hash");
                assert_eq!(previous, source.sha256);
                moved_sources.insert(source.id.as_str());
            }
        }

        if source.authorization_scope.state == AuthorizationState::Revoked {
            revoked_source_ids.insert(source.id.as_str());
        }
    }

    let mut task_ids = HashSet::new();
    let mut seen_coverage = HashSet::new();
    for task in &manifest.tasks {
        assert!(task_ids.insert(task.id.as_str()), "duplicate task ID");
        assert!(!task.question.is_empty());
        assert!(!task.expected_usable_information.is_empty());
        assert!(!task.must_preserve_conditions.is_empty());
        assert!(!task.must_not_infer.is_empty());
        assert!(!task.q_refs.is_empty());
        assert!(!task.caller.vault_id.is_empty());
        assert!(!task.caller.permissions.is_empty());
        let _expected_status = task.expected_status;
        let _severity = task.severity;

        let mut task_sources = HashSet::new();
        for source_id in &task.source_ids {
            assert!(
                source_ids.contains(source_id.as_str()),
                "unknown task source"
            );
            assert!(
                task_sources.insert(source_id.as_str()),
                "duplicate task source"
            );
        }
        for relation in &task.expected_source_relations {
            assert!(!relation.source_ids.is_empty());
            let _relation_kind = relation.kind;
            for source_id in &relation.source_ids {
                assert!(task_sources.contains(source_id.as_str()));
            }
        }
        for q_ref in &task.q_refs {
            let number = q_ref
                .strip_prefix('Q')
                .and_then(|number| number.parse::<u8>().ok())
                .expect("Q reference has the form Q01 through Q30");
            assert!((1..=30).contains(&number));
        }
        seen_coverage.extend(task.coverage_tags.iter().map(String::as_str));
    }

    for required in [
        "scope",
        "negation",
        "exception",
        "ordered_steps",
        "proposal_vs_adoption",
        "no_answer",
        "cross_vault",
        "source_changed",
        "permission_revoked",
        "prompt_injection",
    ] {
        assert!(
            seen_coverage.contains(required),
            "missing {required} coverage"
        );
    }

    assert_eq!(changed_sources.len(), 1);
    assert_eq!(moved_sources.len(), 1);
    assert_eq!(revoked_source_ids.len(), 1);
    assert!(
        manifest.tasks.iter().any(|task| {
            task.coverage_tags.contains(&"cross_vault".to_owned())
                && task.caller.vault_id != "vault-beta"
        }),
        "the corpus includes a cross-Vault access attempt"
    );
}

#[test]
fn semantic_memory_manifest_rejects_unknown_live_configuration_fields() {
    let mut value: serde_json::Value =
        serde_json::from_str(MANIFEST_JSON).expect("manifest JSON is valid");
    value["execution_policy"]["provider_endpoint"] = serde_json::json!("https://provider.invalid");
    assert!(
        serde_json::from_value::<Manifest>(value).is_err(),
        "the strict M0 manifest schema has no live Provider configuration"
    );
}
