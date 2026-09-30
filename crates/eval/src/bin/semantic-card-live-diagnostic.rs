//! Explicitly authorized, bounded single-source semantic-card diagnostic.
//!
//! This command validates the complete frozen M6 config before opening any
//! State/Auth/Provider runtime. It never changes manifest, gold, run-config,
//! or M6 acceptance state and reserves at most two real Provider requests.

use mcp_vault_eval::{
    ComparisonArm, DiagnosticSelection, LiveCliConfig, build_live_runtime,
    run_live_semantic_diagnostic, validate_live_semantic_diagnostic_config,
};
use serde_json::to_string;
use std::{env, error::Error, fs, path::PathBuf};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn usage() -> &'static str {
    "usage: semantic-card-live-diagnostic --run-authorized-real-semantic-diagnostic CONFIG_JSON --arm B|C --source-id SOURCE_ID"
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    if args.next().as_deref() != Some("--run-authorized-real-semantic-diagnostic") {
        return Err(format!(
            "{usage}\nexplicit diagnostic flag is required",
            usage = usage()
        )
        .into());
    }
    let Some(config_path) = args.next() else {
        return Err(usage().into());
    };
    if args.next().as_deref() != Some("--arm") {
        return Err(usage().into());
    }
    let Some(arm) = args.next() else {
        return Err(usage().into());
    };
    if args.next().as_deref() != Some("--source-id") {
        return Err(usage().into());
    }
    let Some(source_id) = args.next() else {
        return Err(usage().into());
    };
    if args.next().is_some() {
        return Err(usage().into());
    }
    let arm = match arm.as_str() {
        "B" => ComparisonArm::B,
        "C" => ComparisonArm::C,
        _ => return Err("diagnostic arm must be B or C".into()),
    };

    let mut config: LiveCliConfig = serde_json::from_slice(&fs::read(PathBuf::from(config_path))?)?;
    config.evaluation.provider_model_id = Some(config.model_id.clone());
    config.evaluation.provider_templates = config.templates.clone();
    let selection = DiagnosticSelection { arm, source_id };
    validate_live_semantic_diagnostic_config(&config.evaluation, &selection)?;

    // The builder performs the same isolated runtime construction as the full
    // M6 CLI, but the diagnostic always binds its transport budget to two.
    let runtime = build_live_runtime(config, Some(2)).await?;
    let result = run_live_semantic_diagnostic(
        &runtime.evaluation,
        &runtime.verifier,
        &runtime.provider,
        &runtime.semantic,
        selection,
    )
    .await?;
    println!("{}", to_string(&result)?);
    Ok(())
}
