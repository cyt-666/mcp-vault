use std::path::PathBuf;

use mcp_vault_domain::{
    DomainError, PathError, Revision, VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug,
    WritePrecondition,
};

#[test]
fn public_path_api_rejects_encoded_escape_attempts() {
    assert_eq!(
        VaultPath::from_url_path("/notes/%2e%2e/private.md").unwrap_err(),
        DomainError::InvalidPath(PathError::Traversal)
    );
    assert_eq!(
        VaultPath::from_url_path("/notes/%252e%252e/private.md").unwrap_err(),
        DomainError::InvalidPath(PathError::EncodedUnsafeComponent)
    );
}

#[test]
fn public_policy_requires_managed_access_for_reserved_paths() {
    let policy = VaultPathPolicy::default();
    let path = VaultPath::parse("_MCP-VAULT/memory/decision.md").unwrap();

    assert!(matches!(
        policy.validate_user_path(&path),
        Err(DomainError::ReservedPath(_))
    ));
    assert!(policy.validate_managed_path(&path).is_ok());
}

#[test]
fn public_contexts_keep_vault_identity_explicit() {
    let slug = VaultSlug::new("default").unwrap();
    let first = VaultContext::new(
        VaultId::new(),
        slug.clone(),
        PathBuf::from("/srv/vault-a"),
        Revision::ZERO,
    )
    .unwrap();
    let second = VaultContext::new(
        VaultId::new(),
        slug,
        PathBuf::from("/srv/vault-b"),
        Revision::ZERO,
    )
    .unwrap();

    assert!(!first.is_same_vault(&second));
    assert!(matches!(
        first.ensure_same_vault(&second),
        Err(DomainError::VaultMismatch { .. })
    ));
}

#[test]
fn public_precondition_api_preserves_conflicts() {
    let expected = Revision::new(4);

    assert!(
        WritePrecondition::ExactRevision(expected)
            .check(Some(expected))
            .is_ok()
    );
    assert!(matches!(
        WritePrecondition::ExactRevision(expected).check(Some(Revision::new(5))),
        Err(DomainError::RevisionConflict { .. })
    ));
}
