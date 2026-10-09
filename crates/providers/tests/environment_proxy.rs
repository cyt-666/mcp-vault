//! The child process isolates proxy environment variables from parallel tests.
//! Only a local fake proxy is contacted; the target never receives a request.

use std::{process::Command, time::Duration};

use mcp_vault_auth::SecretString;
use mcp_vault_providers::{
    AuthStyle, ProviderError, ProviderMode, ProviderSettings, ProviderTransport, RequestOptions,
    SseEventAction,
};
use reqwest::Method;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};

#[test]
fn proxy_client_child() {
    let Ok(kind) = std::env::var("MCP_VAULT_PROXY_TEST_CHILD") else {
        return;
    };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let transport = ProviderTransport::new(ProviderSettings {
            max_retries: 0,
            timeout_ms: 2_000,
            connect_timeout_ms: 1_000,
            stream_first_event_timeout_ms: 2_000,
            stream_idle_timeout_ms: 2_000,
            stream_total_timeout_ms: 2_000,
            ..ProviderSettings::default()
        })
        .unwrap();
        let endpoint = "https://provider-without-local-dns.invalid/v1/chat/completions"
            .parse()
            .unwrap();
        let secret = SecretString::new("synthetic-provider-token");
        let options = RequestOptions::new(AuthStyle::Bearer, Some(&secret));
        let error = if kind == "json" {
            transport
                .request_json(
                    Method::POST,
                    &endpoint,
                    ProviderMode::Enabled,
                    &json!({"prompt":"synthetic-provider-body"}),
                    options,
                )
                .await
                .unwrap_err()
        } else {
            transport
                .request_sse(
                    Method::POST,
                    &endpoint,
                    ProviderMode::Enabled,
                    &json!({"prompt":"synthetic-provider-body"}),
                    options,
                    |_| async { Ok(SseEventAction::Progress) },
                )
                .await
                .unwrap_err()
        };
        assert!(
            matches!(error, ProviderError::Transport { .. }),
            "{error:?}"
        );
    });
}

#[tokio::test]
async fn json_and_sse_use_environment_proxy_without_resolving_target_or_leaking_credentials() {
    for kind in ["json", "sse"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let child = tokio::task::spawn_blocking(move || {
            let mut command = Command::new(std::env::current_exe().unwrap());
            for name in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
                "NO_PROXY",
                "no_proxy",
                "REQUEST_METHOD",
                "MIMO_API_KEY",
            ] {
                command.env_remove(name);
            }
            command
                .args(["--exact", "proxy_client_child", "--nocapture"])
                .env("MCP_VAULT_PROXY_TEST_CHILD", kind)
                .env("HTTPS_PROXY", proxy)
                .output()
                .unwrap()
        });
        let (mut socket, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            let byte = timeout(Duration::from_secs(3), socket.read_u8())
                .await
                .unwrap()
                .unwrap();
            headers.push(byte);
            assert!(headers.len() < 4096);
        }
        let headers = String::from_utf8(headers).unwrap();
        assert!(headers.starts_with("CONNECT provider-without-local-dns.invalid:443 HTTP/1.1\r\n"));
        assert!(!headers.to_ascii_lowercase().contains("authorization"));
        assert!(!headers.contains("synthetic-provider"));
        socket
            .write_all(
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        drop(socket);
        let output = timeout(Duration::from_secs(5), child)
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
