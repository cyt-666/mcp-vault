//! Local operator diagnostics that never start listeners, workers or Providers.
use mcp_vault_domain::VaultSlug;
use mcp_vault_state::StateStore;
use serde_json::Value;

/// Diagnose one exact stored calibration. Output contains only bundled sample
/// ranks/metrics and model identity; it excludes database paths, secrets and cache.
pub async fn calibration(database_url: &str, arguments: &[String]) -> Result<Value, &'static str> {
    let mut flags = std::collections::HashMap::new();
    for pair in arguments.chunks(2) {
        if pair.len() != 2
            || !matches!(pair[0].as_str(), "--vault" | "--channel" | "--signature")
            || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(
                "usage: diagnose-calibration --vault SLUG --channel memory|note --signature sha256:HASH",
            );
        }
    }
    let slug = VaultSlug::new(flags.get("--vault").ok_or("--vault is required")?)
        .map_err(|_| "invalid Vault slug")?;
    let channel = *flags.get("--channel").ok_or("--channel is required")?;
    let signature = *flags.get("--signature").ok_or("--signature is required")?;
    if !matches!(channel, "memory" | "note") {
        return Err("invalid calibration channel");
    }
    let state = StateStore::connect_read_only(database_url)
        .await
        .map_err(|_| "cannot open the existing database read-only")?;
    let vault = state
        .vaults()
        .find_by_slug(&slug)
        .await
        .map_err(|_| "cannot read Vault registry")?
        .ok_or("Vault not found")?;
    let context = vault
        .context()
        .map_err(|_| "invalid Vault registry context")?;
    mcp_vault_memory::diagnose_calibration(&state, &context, channel, signature)
        .await
        .map_err(|error| error.code())
}
