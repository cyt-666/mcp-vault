use std::sync::{Arc, atomic::AtomicBool};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use mcp_vault_admin_api::{AdminApiConfig, AdminApiState, stateful_router};
use mcp_vault_auth::{AuthService, MasterKeyRing, OriginPolicy};
use mcp_vault_backup::BackupLimits;
use mcp_vault_core::{VaultCore, VaultCoreRuntime};
use mcp_vault_domain::{
    Actor, MaintenanceGate, Revision, SourcePlane, VaultContext, VaultId, VaultPath,
    VaultPathPolicy, VaultSlug, WritePrecondition,
};
use mcp_vault_memory::SemanticMemoryService;
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::StorageOptions;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

struct Fixture {
    router: Router,
    _root: TempDir,
    maintenance: MaintenanceGate,
    cookie: String,
    csrf: String,
    card_id: String,
    foreign_card_id: String,
}

async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let state = StateStore::connect_and_migrate("sqlite::memory:")
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("work").unwrap(),
        root.path().join("vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, "Work", VaultStatus::Active)
        .await
        .unwrap();
    state
        .settings()
        .set_vault(
            &context,
            "memory.units.policy",
            &json!({"enabled":true,"request_timeout_seconds":300}),
            WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    let auth = AuthService::new(
        state.auth(),
        MasterKeyRing::from_bytes(1, &[41_u8; 32]).unwrap(),
    );
    let key_version_ids = auth.key_version_ids();
    let maintenance = MaintenanceGate::new();
    let core_runtime = VaultCoreRuntime::new(maintenance.clone());
    let admin = AdminApiState::new(
        state.clone(),
        auth,
        AdminApiConfig {
            origin_policy: OriginPolicy::new(["http://localhost:8081"]).unwrap(),
            data_hosts: ["localhost".to_owned()].into_iter().collect(),
            data_origins: Vec::new(),
            data_public_origin: None,
            data_bind: "127.0.0.1:8080".parse().unwrap(),
            admin_bind: "127.0.0.1:8081".parse().unwrap(),
            data_dir: root.path().to_owned(),
            history_root: root.path().join("history"),
            storage_options: StorageOptions::default(),
            backup_root: root.path().join("backups"),
            backup_limits: BackupLimits::default(),
            key_version_ids,
            maintenance: maintenance.clone(),
            core_runtime,
            readiness: Arc::new(AtomicBool::new(true)),
            version: "test".to_owned(),
        },
    );
    let router = stateful_router(admin);
    let core = VaultCore::new(
        state.clone(),
        root.path().join("history"),
        VaultPathPolicy::default(),
        StorageOptions::default(),
        Default::default(),
    );
    let path = VaultPath::parse("notes/semantic.md").unwrap();
    core.create_bytes(
        &context,
        &path,
        b"# Semantic\nAdmin semantic source.\n",
        Actor::system(),
        SourcePlane::System,
        None,
    )
    .await
    .unwrap();
    let memory = SemanticMemoryService::new(state.clone());
    let input = memory.prepare_source(&context, &core, &path).await.unwrap();
    let block = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Admin semantic source.","scope":"project","assertion_status":"source_asserted","conditions":[],"exceptions":[],"ordered_steps":[],"admission_reason":"admin fixture","value_for_future_work":"retain","body_block_ids":[block.local_id.clone()]}],
        "cards":[{"title":"Admin semantic","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    memory
        .submit_proposal_json(&context, &core, &path, &proposal.to_string())
        .await
        .unwrap();
    let card = state
        .semantic_memory()
        .list_cards(&context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap();
    let other_context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new("other").unwrap(),
        root.path().join("other-vault"),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&other_context, "Other", VaultStatus::Active)
        .await
        .unwrap();
    state
        .settings()
        .set_vault(
            &other_context,
            "memory.units.policy",
            &json!({"enabled":true,"request_timeout_seconds":300}),
            WritePrecondition::Unconditional,
            None,
        )
        .await
        .unwrap();
    let other_core = VaultCore::new(
        state.clone(),
        root.path().join("other-history"),
        VaultPathPolicy::default(),
        StorageOptions::default(),
        Default::default(),
    );
    other_core
        .create_bytes(
            &other_context,
            &path,
            b"# Other\nOther semantic source.\n",
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let other_input = memory
        .prepare_source(&other_context, &other_core, &path)
        .await
        .unwrap();
    let other_block = other_input.blocks.last().unwrap();
    let other_proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"Other semantic source.","scope":"project","assertion_status":"source_asserted","conditions":[],"exceptions":[],"ordered_steps":[],"admission_reason":"admin fixture","value_for_future_work":"retain","body_block_ids":[other_block.local_id.clone()]}],
        "cards":[{"title":"Other semantic","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    memory
        .submit_proposal_json(
            &other_context,
            &other_core,
            &path,
            &other_proposal.to_string(),
        )
        .await
        .unwrap();
    let foreign_card_id = state
        .semantic_memory()
        .list_cards(&other_context, 20)
        .await
        .unwrap()
        .pop()
        .unwrap()
        .id
        .to_string();

    let setup = router
        .clone()
        .oneshot(request(
            "POST",
            "/setup",
            json!({"username":"owner","password":"correct horse battery staple"}),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(setup.status(), StatusCode::CREATED);
    let login = router
        .clone()
        .oneshot(request(
            "POST",
            "/session",
            json!({"username":"owner","password":"correct horse battery staple"}),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = body_json(login).await["data"]["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    Fixture {
        router,
        _root: root,
        maintenance,
        cookie,
        csrf,
        card_id: card.id.to_string(),
        foreign_card_id,
    }
}

fn request(
    method: &str,
    uri: &str,
    body: Value,
    cookie: Option<&str>,
    csrf: Option<&str>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("origin", "http://localhost:8081")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    if let Some(cookie) = cookie {
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
    }
    if let Some(csrf) = csrf {
        request
            .headers_mut()
            .insert("x-csrf-token", csrf.parse().unwrap());
    }
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap(),
    ));
    request
}

async fn body_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn semantic_admin_routes_use_selected_vault_and_safe_mutation_boundary() {
    let fixture = fixture().await;
    let cards = fixture
        .router
        .clone()
        .oneshot(request(
            "GET",
            "/vaults/work/semantic/cards?limit=20",
            json!({}),
            Some(&fixture.cookie),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(cards.status(), StatusCode::OK);
    let cards = body_json(cards).await;
    assert_eq!(cards["data"]["cards"][0]["card_id"], fixture.card_id);
    let foreign = fixture
        .router
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/vaults/work/semantic/card?card_id={}",
                fixture.foreign_card_id
            ),
            json!({}),
            Some(&fixture.cookie),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    let invalid_kind = fixture
        .router
        .clone()
        .oneshot(request(
            "GET",
            &format!(
                "/vaults/work/semantic/card?card_id={}&card_kind=bogus",
                fixture.card_id
            ),
            json!({}),
            Some(&fixture.cookie),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_kind.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let invalid_kind = body_json(invalid_kind).await;
    assert_eq!(invalid_kind["error"]["code"], "validation_failed");

    let status = fixture
        .router
        .clone()
        .oneshot(request(
            "GET",
            "/vaults/work/semantic/status?limit=20",
            json!({}),
            Some(&fixture.cookie),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let unknown_pack = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/pack",
            json!({"task":"Admin semantic", "unknown":true}),
            Some(&fixture.cookie),
            Some(&fixture.csrf),
        ))
        .await
        .unwrap();
    assert_eq!(unknown_pack.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let csrf_rejected = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/correct",
            json!({"target_ref":format!("card:{}", fixture.card_id), "mutation":"correction"}),
            Some(&fixture.cookie),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(csrf_rejected.status(), StatusCode::FORBIDDEN);
    let pack = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/pack",
            json!({"task":"Admin semantic"}),
            Some(&fixture.cookie),
            Some(&fixture.csrf),
        ))
        .await
        .unwrap();
    assert_eq!(pack.status(), StatusCode::OK);

    let explicit_csrf_rejected = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/remember-explicit",
            json!({"content":"blocked","idempotency_key":"admin-explicit-csrf"}),
            Some(&fixture.cookie),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(explicit_csrf_rejected.status(), StatusCode::FORBIDDEN);
    let mut bad_origin = request(
        "POST",
        "/vaults/work/semantic/remember-explicit",
        json!({"content":"blocked","idempotency_key":"admin-explicit-origin"}),
        Some(&fixture.cookie),
        Some(&fixture.csrf),
    );
    bad_origin
        .headers_mut()
        .insert("origin", "http://evil.invalid".parse().unwrap());
    let explicit_origin_rejected = fixture.router.clone().oneshot(bad_origin).await.unwrap();
    assert_eq!(explicit_origin_rejected.status(), StatusCode::FORBIDDEN);
    let missing_key = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/remember-explicit",
            json!({"content":"missing key"}),
            Some(&fixture.cookie),
            Some(&fixture.csrf),
        ))
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let empty_key = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/remember-explicit",
            json!({"content":"empty key","idempotency_key":"  "}),
            Some(&fixture.cookie),
            Some(&fixture.csrf),
        ))
        .await
        .unwrap();
    assert_eq!(empty_key.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let explicit = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/remember-explicit",
            json!({
                "content":"Admin-owned explicit assertion.",
                "idempotency_key":"admin-explicit-v1"
            }),
            Some(&fixture.cookie),
            Some(&fixture.csrf),
        ))
        .await
        .unwrap();
    assert_eq!(explicit.status(), StatusCode::OK);
    let explicit = body_json(explicit).await;
    assert_eq!(
        explicit["data"]["explicit"]["memory"]["ownership"],
        "explicit"
    );
    assert_eq!(
        explicit["data"]["explicit"]["memory"]["embedding_binding_present"],
        false
    );
    let audit = fixture
        .router
        .clone()
        .oneshot(request(
            "GET",
            "/vaults/work/audit?limit=100",
            json!({}),
            Some(&fixture.cookie),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(audit.status(), StatusCode::OK);
    let audit_body = body_json(audit).await;
    let explicit_audit = audit_body["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["action"] == "admin.semantic.explicit_remembered")
        .expect("explicit remember audit entry");
    assert_eq!(explicit_audit["actor_type"], "admin");
    assert!(
        explicit_audit["actor_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    let audit_text = audit_body.to_string();
    assert!(!audit_text.contains("Admin-owned explicit assertion"));
    assert!(!audit_text.contains("source_path"));

    let correction = fixture.router.clone().oneshot(request("POST", "/vaults/work/semantic/correct", json!({
        "target_ref": format!("card:{}", fixture.card_id),
        "mutation":"correction",
        "payload":{"replace":"Admin semantic source after correction.","remove":"Admin semantic source."},
        "expected_parent_revision":1,
        "expected_rules_revision":0,
        "idempotency_key":"admin-semantic-correction"
    }), Some(&fixture.cookie), Some(&fixture.csrf))).await.unwrap();
    assert_eq!(correction.status(), StatusCode::OK);

    let forgotten = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/forget",
            json!({
                "target_ref": format!("card:{}", fixture.card_id),
                "mutation":"forget_current",
                "payload":{"reason":"admin test"},
                "expected_parent_revision":1,
                "expected_rules_revision":1,
                "idempotency_key":"admin-semantic-forget"
            }),
            Some(&fixture.cookie),
            Some(&fixture.csrf),
        ))
        .await
        .unwrap();
    assert_eq!(forgotten.status(), StatusCode::OK);
    assert!(
        fixture
            ._root
            .path()
            .join("vault/notes/semantic.md")
            .is_file()
    );

    fixture
        .maintenance
        .set(mcp_vault_domain::MaintenanceMode::ReadOnly);
    let rejected = fixture.router.clone().oneshot(request("POST", "/vaults/work/semantic/correct", json!({
        "target_ref": format!("card:{}", fixture.card_id), "mutation":"correction", "payload":{"replace":"blocked","remove":"Admin semantic source work."},
        "expected_parent_revision":1, "expected_rules_revision":2, "idempotency_key":"admin-semantic-maintenance"
    }), Some(&fixture.cookie), Some(&fixture.csrf))).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    let rejected = body_json(rejected).await;
    assert_eq!(rejected["error"]["code"], "maintenance");
    let rejected = fixture
        .router
        .clone()
        .oneshot(request(
            "POST",
            "/vaults/work/semantic/remember-explicit",
            json!({"content":"blocked","idempotency_key":"admin-explicit-maintenance"}),
            Some(&fixture.cookie),
            Some(&fixture.csrf),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_json(rejected).await["error"]["code"], "maintenance");
}
