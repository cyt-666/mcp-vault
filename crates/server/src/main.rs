use std::process::ExitCode;

use mcp_vault_server::{bootstrap_token_for_display, config::AppConfig, init_tracing, run};

#[tokio::main]
async fn main() -> Result<ExitCode, mcp_vault_server::ServerError> {
    let config = AppConfig::from_env()?;
    let command = std::env::args().nth(1);

    match command.as_deref() {
        Some("--check-config") => {
            println!("mcp-vault configuration is valid");
            return Ok(ExitCode::SUCCESS);
        }
        Some("bootstrap-token" | "show-bootstrap-token") => {
            let token = bootstrap_token_for_display(&config).await?;
            println!("{}", token.expose_secret());
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }

    init_tracing(&config)?;
    run(config).await?;
    Ok(ExitCode::SUCCESS)
}
