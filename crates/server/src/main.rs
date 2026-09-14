use std::process::ExitCode;

use mcp_vault_server::{config::AppConfig, init_tracing, run};

#[tokio::main]
async fn main() -> Result<ExitCode, mcp_vault_server::ServerError> {
    let config = AppConfig::from_env()?;
    let command = std::env::args().nth(1);

    match command.as_deref() {
        Some("initialize-memory") if std::env::args().nth(2).as_deref() == Some("--inspect") => {
            let arguments = std::env::args().skip(2).collect::<Vec<_>>();
            if arguments != ["--inspect"] {
                eprintln!("usage: mcp-vault initialize-memory --inspect");
                return Ok(ExitCode::FAILURE);
            }
            let report = mcp_vault_server::inspect_memory_initialization(&config).await?;
            println!("{report}");
            return Ok(ExitCode::SUCCESS);
        }
        Some("initialize-memory") => {
            let arguments = std::env::args().skip(2).collect::<Vec<_>>();
            if arguments != ["--discard-legacy-memory"] {
                eprintln!(
                    "usage: mcp-vault initialize-memory --discard-legacy-memory (stop the service and back up SQLite, Vaults and history first)"
                );
                return Ok(ExitCode::FAILURE);
            }
            let report = mcp_vault_server::initialize_memory(&config).await?;
            println!("{report}");
            return Ok(ExitCode::SUCCESS);
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
