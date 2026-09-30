//! Explicit Provider capability probe entry point.
//!
//! No flag means no config read, State open, key load, Provider construction,
//! Vault creation, or network request.

use std::{env, error::Error, path::PathBuf};

use mcp_vault_eval::{
    ProviderCapabilityProbeConfig, prepare_provider_capability_probe, run_capability_probe,
    validate_provider_capability_probe_config,
};
use serde_json::{json, to_string};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn usage() -> &'static str {
    "usage: provider-capability-probe (--preflight-provider-capability-probe | --run-authorized-provider-capability-probe) CONFIG_JSON"
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let Some(flag) = args.next() else {
        return Err(usage().into());
    };
    let preflight_only = flag == "--preflight-provider-capability-probe";
    if !preflight_only && flag != "--run-authorized-provider-capability-probe" {
        return Err(format!(
            "{usage}\nunknown or missing explicit probe flag",
            usage = usage()
        )
        .into());
    }
    let Some(config_path) = args.next() else {
        return Err(usage().into());
    };
    if args.next().is_some() {
        return Err(usage().into());
    }
    let config: ProviderCapabilityProbeConfig =
        serde_json::from_slice(&std::fs::read(PathBuf::from(config_path))?)?;

    // This is deliberately before all State/key/Provider construction.
    validate_provider_capability_probe_config(&config)
        .map_err(|_| "probe configuration is invalid or unsafe")?;
    if preflight_only {
        println!("{}", to_string(&json!({"status": "preflight_ok"}))?);
        return Ok(());
    }
    let (provider, budget) = prepare_provider_capability_probe(&config)
        .await
        .map_err(|_| "probe preparation failed")?;
    let checkpoint = run_capability_probe(&config, &provider, budget)
        .await
        .map_err(|_| "provider capability probe failed")?;
    // Only the redaction-safe checkpoint shape is printed.
    println!("{}", to_string(&checkpoint)?);
    Ok(())
}
