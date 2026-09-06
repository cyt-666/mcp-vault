use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MCP_VAULT_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
    println!("cargo:rerun-if-changed=src");
    let commit = std::env::var("MCP_VAULT_BUILD_COMMIT")
        .ok()
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
        })
        .filter(|value| {
            let value = value.trim();
            matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MCP_VAULT_BUILD_COMMIT={}", commit.trim());
    let source_state = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map_or("unknown", |output| {
            if output.stdout.is_empty() {
                "clean"
            } else {
                "dirty"
            }
        });
    println!("cargo:rustc-env=MCP_VAULT_BUILD_SOURCE_STATE={source_state}");
}
