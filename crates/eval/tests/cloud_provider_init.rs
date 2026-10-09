#![cfg(unix)]

use mcp_vault_auth::{AuthService, MasterKeyRing};
use mcp_vault_domain::VaultSlug;
use mcp_vault_providers::{PROVIDER_SECRET_OWNER, PROVIDER_SECRET_PURPOSE};
use mcp_vault_state::StateStore;
use serde_json::Value;
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    process::{Command, Output},
};

const SYNTHETIC_PLACEHOLDER: &str = "offline-network-secret-placeholder-fixture";

fn invoke(root: &Path, authorized: bool, value: Option<&str>) -> Output {
    // Exercise permissions under a permissive umask, independently of the
    // task shell's defaults. Arguments pass through "$@" without expansion.
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("umask 022\nexec \"$@\"")
        .arg("m6-init-fixture")
        .arg(env!("CARGO_BIN_EXE_init-m6-cloud-provider"));
    command.env_remove("MIMO_API_KEY");
    if let Some(value) = value {
        command.env("MIMO_API_KEY", value);
    }
    if authorized {
        command.arg("--initialize-authorized-m6-provider");
    }
    command.arg(root).output().unwrap()
}

#[test]
fn missing_authorization_or_placeholder_does_not_create_state_or_disclose_values() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("config");
    for (authorized, value) in [
        (false, Some(SYNTHETIC_PLACEHOLDER)),
        (true, None),
        (true, Some("")),
        (true, Some("invalid\nheader")),
    ] {
        let output = invoke(&root, authorized, value);
        assert!(!output.status.success());
        assert!(!root.exists());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(SYNTHETIC_PLACEHOLDER));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("invalid\nheader"));
    }
}

#[test]
fn sqlite_uri_metacharacters_are_rejected_before_directory_creation() {
    let temp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(temp.path()).unwrap();
    for name in [
        "config%41",
        "config?cache=shared",
        "config#fragment",
        "config\ncontrol",
    ] {
        let root = parent.join(name);
        assert!(
            !invoke(&root, true, Some(SYNTHETIC_PLACEHOLDER))
                .status
                .success()
        );
        assert!(!root.exists());
    }
    assert!(std::fs::read_dir(parent).unwrap().next().is_none());
}

#[test]
fn existing_roots_and_symlinked_parents_are_not_modified() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let sentinel = root.join("sentinel");
    std::fs::write(&sentinel, b"keep existing state").unwrap();
    assert!(
        !invoke(&root, true, Some(SYNTHETIC_PLACEHOLDER))
            .status
            .success()
    );
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep existing state");
    let link = root.join("linked-parent");
    symlink(&root, &link).unwrap();
    assert!(
        !invoke(&link.join("new-config"), true, Some(SYNTHETIC_PLACEHOLDER))
            .status
            .success()
    );
    assert!(!root.join("new-config").exists());
}

#[tokio::test]
async fn synthetic_placeholder_is_encrypted_and_bound_without_provider_requests() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap().join("config");
    let output = invoke(&root, true, Some(SYNTHETIC_PLACEHOLDER));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SYNTHETIC_PLACEHOLDER));
    assert!(output.stderr.is_empty());
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["real_provider_requests_started"], 0);
    assert_eq!(summary["authentication_status"], "not_tested");
    let db = Path::new(summary["source_database_path"].as_str().unwrap());
    let key = Path::new(summary["source_master_key_path"].as_str().unwrap());
    for path in [&root, &root.join("state"), &root.join("source")] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    for path in [db, key] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let state = StateStore::connect_read_only(&format!("sqlite://{}", db.display()))
        .await
        .unwrap();
    let record = state
        .vaults()
        .find_by_slug(&VaultSlug::new("m6-cloud-config").unwrap())
        .await
        .unwrap()
        .unwrap();
    let context = record.context().unwrap();
    let binding = state
        .providers()
        .resolve_binding(&context, "memory_extraction")
        .await
        .unwrap()
        .unwrap();
    let model = state
        .providers()
        .get_model(binding.model_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(model.external_model_id, "mimo-v2.6-flash");
    assert_eq!(model.settings["generation_token_limit"], 32_768);
    assert_eq!(model.capabilities["structured_output"], true);
    let provider = state
        .providers()
        .get_provider(model.provider_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(provider.provider_type, "xiaomi_mimo");
    assert_eq!(provider.base_url, "https://api.xiaomimimo.com/v1/");
    assert_eq!(provider.settings["max_retries"], 0);
    assert_eq!(provider.settings["max_concurrency"], 1);
    assert_eq!(provider.settings["timeout_ms"], 600_000);
    let secret_id = provider.secret_id.unwrap();
    let encrypted = state.auth().get_secret(secret_id).await.unwrap().unwrap();
    assert_ne!(encrypted.ciphertext, SYNTHETIC_PLACEHOLDER.as_bytes());
    let auth = AuthService::new(state.auth(), MasterKeyRing::load_file(key).await.unwrap());
    let restored = auth
        .read_installation_secret(
            secret_id,
            PROVIDER_SECRET_PURPOSE,
            PROVIDER_SECRET_OWNER,
            Some(&provider.id.to_string()),
        )
        .await
        .unwrap();
    assert_eq!(restored.expose_secret(), SYNTHETIC_PLACEHOLDER);
    let db_before = std::fs::read(db).unwrap();
    assert!(
        !invoke(&root, true, Some("another-offline-placeholder"))
            .status
            .success()
    );
    assert_eq!(std::fs::read(db).unwrap(), db_before);
}
