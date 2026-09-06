use std::process::ExitCode;

use mcp_vault_server::{config::AppConfig, init_tracing, run};

#[tokio::main]
async fn main() -> Result<ExitCode, mcp_vault_server::ServerError> {
    let config = AppConfig::from_env()?;
    let command = std::env::args().nth(1);

    match command.as_deref() {
        Some("diagnose-calibration") => {
            let arguments = std::env::args().skip(2).collect::<Vec<_>>();
            return match mcp_vault_server::diagnostics::calibration(
                &config.database_url,
                &arguments,
            )
            .await
            {
                Ok(report) => {
                    println!("{report}");
                    Ok(ExitCode::SUCCESS)
                }
                Err(error) => {
                    eprintln!("calibration diagnosis failed: {error}");
                    Ok(ExitCode::FAILURE)
                }
            };
        }
        Some("--check-config") => {
            println!("mcp-vault configuration is valid");
            return Ok(ExitCode::SUCCESS);
        }
        Some("bootstrap-token" | "show-bootstrap-token") => {
            eprintln!(
                "the bootstrap-token command has been removed; open the Admin listener and create the first Admin with a username and password"
            );
            return Ok(ExitCode::FAILURE);
        }
        Some(command) => {
            eprintln!("unknown mcp-vault command: {command}");
            return Ok(ExitCode::FAILURE);
        }
        None => {}
    }

    init_tracing(&config)?;
    run(config).await?;
    Ok(ExitCode::SUCCESS)
}
