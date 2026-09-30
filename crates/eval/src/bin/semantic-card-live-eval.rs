//! Explicitly authorized semantic-card M6 live evaluation entry point.
//!
//! Runtime construction is shared with the bounded single-source diagnostic;
//! without the exact live flag this binary does not open State/Auth or build a
//! Provider service.

use mcp_vault_eval::{LiveCliConfig, build_live_runtime, run_live_evaluation};
use std::{env, error::Error, fs, path::PathBuf};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn usage() -> &'static str {
    "usage: semantic-card-live-eval --run-authorized-real-semantic-evaluation CONFIG_JSON"
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    if args.next().as_deref() != Some("--run-authorized-real-semantic-evaluation") {
        return Err(format!(
            "{usage}\nunknown or missing explicit live flag",
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
    let config: LiveCliConfig = serde_json::from_slice(&fs::read(PathBuf::from(config_path))?)?;
    let runtime = build_live_runtime(config, None).await?;
    let result = run_live_evaluation(
        &runtime.evaluation,
        &runtime.verifier,
        &runtime.provider,
        &runtime.semantic,
    )
    .await?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
