use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use mcp_vault_auth::SecretString;
use mcp_vault_providers::{
    AuthStyle, ProviderError, ProviderMode, ProviderSettings, ProviderTransport, RequestBudget,
    RequestOptions, SseEventAction,
};
use reqwest::Method;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};

#[derive(Default)]
struct Attempts(AtomicUsize);

#[async_trait::async_trait]
impl RequestBudget for Attempts {
    async fn reserve(&self, _body_bytes: usize) -> Result<(), ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// Re-execute network tests in a child so host proxies cannot intercept their
// localhost fixtures, without mutating the environment of parallel tests.
fn is_child_with_isolated_proxy_environment(test_name: &str) -> bool {
    if std::env::var("MCP_VAULT_TRANSPORT_SECURITY_CHILD").as_deref() == Ok(test_name) {
        return true;
    }
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
    let output = command
        .args(["--exact", test_name, "--nocapture"])
        .env("MCP_VAULT_TRANSPORT_SECURITY_CHILD", test_name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

#[tokio::test]
async fn disabled_and_legacy_local_only_send_nothing_and_consume_no_transport_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    )
    .parse()
    .unwrap();
    let attempts = Arc::new(Attempts::default());
    let transport = ProviderTransport::new(ProviderSettings::default())
        .unwrap()
        .with_budget(attempts.clone());
    for stored_mode in ["disabled", "local_only"] {
        let mode = serde_json::from_value::<ProviderMode>(json!(stored_mode)).unwrap();
        let json_error = transport
            .request_json(
                Method::POST,
                &endpoint,
                mode,
                &json!({}),
                RequestOptions::new(AuthStyle::None, None),
            )
            .await
            .unwrap_err();
        assert!(matches!(json_error, ProviderError::PrivacyDenied));
        let sse_error = transport
            .request_sse(
                Method::POST,
                &endpoint,
                mode,
                &json!({}),
                RequestOptions::new(AuthStyle::None, None),
                |_| async { Ok(SseEventAction::Progress) },
            )
            .await
            .unwrap_err();
        assert!(matches!(sse_error, ProviderError::PrivacyDenied));
    }
    assert_eq!(attempts.0.load(Ordering::SeqCst), 0);
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn json_and_sse_reject_untrusted_tls_before_sending_provider_credentials() {
    if !is_child_with_isolated_proxy_environment(
        "json_and_sse_reject_untrusted_tls_before_sending_provider_credentials",
    ) {
        return;
    }
    // Deliberately public synthetic self-signed certificate/key, never a real credential.
    let certificate =
        CertificateDer::from(include_bytes!("fixtures/untrusted-provider-cert.der").to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        include_bytes!("fixtures/untrusted-provider-key.der").to_vec(),
    ));
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    for kind in ["json", "sse"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "https://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        )
        .parse()
        .unwrap();
        let acceptor = acceptor.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            match timeout(Duration::from_secs(3), acceptor.accept(socket))
                .await
                .unwrap()
            {
                Err(_) => false,
                Ok(mut tls) => {
                    // If TLS validation were disabled, serve a valid response so the
                    // test cannot accidentally pass because the fake server closed.
                    let mut headers = Vec::new();
                    while !headers.ends_with(b"\r\n\r\n") {
                        headers.push(tls.read_u8().await.unwrap());
                        assert!(headers.len() < 8192);
                    }
                    let body = if kind == "json" { "{}" } else { "data: {}\n\n" };
                    let content_type = if kind == "json" {
                        "application/json"
                    } else {
                        "text/event-stream"
                    };
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    tls.write_all(reply.as_bytes()).await.unwrap();
                    true
                }
            }
        });
        let transport = ProviderTransport::new(ProviderSettings {
            max_retries: 0,
            ..ProviderSettings::default()
        })
        .unwrap();
        let secret = SecretString::new("synthetic-tls-provider-secret");
        let options = RequestOptions::new(AuthStyle::Bearer, Some(&secret));
        let error = if kind == "json" {
            transport
                .request_json(
                    Method::POST,
                    &endpoint,
                    ProviderMode::Enabled,
                    &json!({}),
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
                    &json!({}),
                    options,
                    |_| async { Ok(SseEventAction::Terminal) },
                )
                .await
                .unwrap_err()
        };
        assert!(matches!(error, ProviderError::Transport { .. }));
        assert!(
            !server.await.unwrap(),
            "untrusted TLS must fail before any HTTP headers"
        );
    }
}

#[tokio::test]
async fn json_and_sse_never_forward_credentials_or_body_to_redirect_targets() {
    if !is_child_with_isolated_proxy_environment(
        "json_and_sse_never_forward_credentials_or_body_to_redirect_targets",
    ) {
        return;
    }
    for status in [301, 302, 303, 307, 308] {
        for kind in ["json", "sse"] {
            let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let redirect_url = format!("http://{}/captured", destination.local_addr().unwrap());
            let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!(
                "http://{}/v1/chat/completions",
                origin.local_addr().unwrap()
            )
            .parse()
            .unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = timeout(Duration::from_secs(5), origin.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") {
                    headers.push(
                        timeout(Duration::from_secs(3), socket.read_u8())
                            .await
                            .unwrap()
                            .unwrap(),
                    );
                    assert!(headers.len() < 8192);
                }
                let headers = String::from_utf8(headers).unwrap().to_ascii_lowercase();
                assert!(headers.contains("authorization: bearer synthetic-redirect-secret\r\n"));
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                assert!(length < 4096);
                let mut body = vec![0; length];
                timeout(Duration::from_secs(3), socket.read_exact(&mut body))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                    json!({"prompt": "synthetic-redirect-body"})
                );
                let reply = format!(
                    "HTTP/1.1 {status} Redirect\r\nLocation: {redirect_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            });
            let attempts = Arc::new(Attempts::default());
            let transport = ProviderTransport::new(ProviderSettings {
                max_retries: 0,
                timeout_ms: 2_000,
                connect_timeout_ms: 1_000,
                stream_first_event_timeout_ms: 2_000,
                stream_idle_timeout_ms: 2_000,
                stream_total_timeout_ms: 2_000,
                ..ProviderSettings::default()
            })
            .unwrap()
            .with_budget(attempts.clone());
            let secret = SecretString::new("synthetic-redirect-secret");
            let options = RequestOptions::new(AuthStyle::Bearer, Some(&secret));
            let body = json!({"prompt": "synthetic-redirect-body"});
            let error = if kind == "json" {
                transport
                    .request_json(
                        Method::POST,
                        &endpoint,
                        ProviderMode::Enabled,
                        &body,
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
                        &body,
                        options,
                        |_| async { Ok(SseEventAction::Progress) },
                    )
                    .await
                    .unwrap_err()
            };
            assert!(matches!(error, ProviderError::EndpointDenied));
            server.await.unwrap();
            assert_eq!(attempts.0.load(Ordering::SeqCst), 1);
            assert!(
                timeout(Duration::from_millis(30), destination.accept())
                    .await
                    .is_err(),
                "redirect destination must receive no connection"
            );
        }
    }
}
