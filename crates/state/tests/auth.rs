use std::path::PathBuf;

use mcp_vault_domain::{
    AdminSessionId, AdminUserId, CredentialId, OAuthGrantId, OAuthIssuerId, Revision, SecretId,
    VaultContext, VaultId, VaultSlug,
};
use mcp_vault_state::{StateStore, VaultStatus};

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

async fn register(store: &StateStore, context: &VaultContext) {
    store
        .vaults()
        .insert(context, context.slug().as_str(), VaultStatus::Active)
        .await
        .unwrap();
}

#[tokio::test]
async fn auth_repository_keeps_secret_metadata_and_credentials_redactable() {
    let store = store().await;
    let secret = store
        .auth()
        .insert_secret(
            SecretId::new(),
            "provider-api-key",
            "vault",
            Some("vault-a"),
            1,
            &[1; 24],
            b"ciphertext-only",
            Some("sk-…9ab2"),
        )
        .await
        .unwrap();
    let debug = format!("{secret:?}");
    assert!(!debug.contains("ciphertext-only"));
    assert_eq!(secret.hint.as_deref(), Some("sk-…9ab2"));

    let user = store
        .auth()
        .insert_admin_user(AdminUserId::new(), "admin", "$argon2id$redacted")
        .await
        .unwrap();
    assert!(!format!("{user:?}").contains("$argon2id$redacted"));
}

#[tokio::test]
async fn vault_credentials_and_oauth_grants_are_scoped_in_every_lookup() {
    let store = store().await;
    let first = context("first", "/srv/first");
    let second = context("second", "/srv/second");
    register(&store, &first).await;
    register(&store, &second).await;

    let webdav = store
        .auth()
        .insert_webdav_credential(
            &first,
            CredentialId::new(),
            "Laptop",
            "laptop",
            "$argon2id$redacted",
            r#"["read"]"#,
            None,
        )
        .await
        .unwrap();
    assert!(
        store
            .auth()
            .find_webdav_credential(&second, &webdav.username)
            .await
            .unwrap()
            .is_none()
    );

    let pat = store
        .auth()
        .insert_mcp_token(
            &first,
            CredentialId::new(),
            "Agent",
            "mcpv_pat_abc",
            &[9; 32],
            1,
            r#"["vault:read"]"#,
            None,
        )
        .await
        .unwrap();
    assert!(
        store
            .auth()
            .find_mcp_token(&second, &pat.token_prefix, &pat.token_digest)
            .await
            .unwrap()
            .is_none()
    );

    let issuer = store
        .auth()
        .insert_oauth_issuer(
            OAuthIssuerId::new(),
            "Issuer",
            "https://issuer.example.test",
            None,
            "mcp-vault",
            Some("https://vault.example.test/mcp"),
            Some(r#"{"keys":[]}"#),
            true,
        )
        .await
        .unwrap();
    let grant = store
        .auth()
        .insert_oauth_grant(
            &first,
            OAuthGrantId::new(),
            issuer.id,
            "agent",
            r#"["vault:read"]"#,
        )
        .await
        .unwrap();
    assert!(
        store
            .auth()
            .find_oauth_grant(&second, issuer.id, &grant.subject)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .auth()
            .list_oauth_grants(&first, false, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .auth()
            .list_oauth_grants(&second, true, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .auth()
            .revoke_oauth_grant(&second, grant.id)
            .await
            .is_err()
    );
    assert!(
        store
            .auth()
            .find_oauth_grant(&first, issuer.id, &grant.subject)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none()
    );
}

#[tokio::test]
async fn admin_session_digest_key_version_round_trips() {
    let store = store().await;
    let user = store
        .auth()
        .insert_admin_user(AdminUserId::new(), "admin", "$argon2id$redacted")
        .await
        .unwrap();
    let session = store
        .auth()
        .insert_admin_session(
            AdminSessionId::new(),
            user.id,
            &[1; 32],
            &[2; 32],
            3,
            1,
            100,
            Some("127.0.0.1"),
            Some(&[3; 32]),
        )
        .await
        .unwrap();
    let loaded = store
        .auth()
        .find_admin_session(&[1; 32])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.digest_key_version, 3);
}
