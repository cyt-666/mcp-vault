//! Typed bootstrap configuration for the two service planes.

use std::{
    collections::BTreeSet,
    env,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use mcp_vault_auth::OriginPolicy;
use mcp_vault_backup::BackupLimits;
use thiserror::Error;

const DEFAULT_DATA_BIND: &str = "0.0.0.0:8080";
const DEFAULT_ADMIN_BIND: &str = "127.0.0.1:8081";
const DEFAULT_DATA_DIR: &str = "./data";
const DEFAULT_DATABASE_URL: &str = "sqlite://./data/state/mcp-vault.sqlite3";
const DEFAULT_SHUTDOWN_TIMEOUT_SECONDS: u64 = 30;
const MAX_SHUTDOWN_TIMEOUT_SECONDS: u64 = 300;
const DEFAULT_RECONCILIATION_INTERVAL_SECONDS: u64 = 300;
const MAX_RECONCILIATION_INTERVAL_SECONDS: u64 = 86_400;
const DEFAULT_ADMIN_ORIGINS: &str = "http://127.0.0.1:8081,http://localhost:8081";
const DEFAULT_DATA_ORIGINS: &str = "";

/// The output format used by the structured tracing subscriber.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogFormat {
    /// JSON records suitable for production collection.
    Json,
    /// Human-readable compact records for local development.
    Pretty,
}

/// Listener addresses and process bootstrap settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    /// Data-plane address for MCP, WebDAV, and public health routes.
    pub data_bind: SocketAddr,
    /// Control-plane address for Admin UI and Admin API.
    pub admin_bind: SocketAddr,
    /// Root directory reserved for application state and future Vault roots.
    pub data_dir: PathBuf,
    /// Directory for the application-managed installation key. Ordinary
    /// service backups exclude this directory.
    pub secrets_dir: PathBuf,
    /// Bootstrap SQLite URL. Long-term settings remain in SQLite.
    pub database_url: String,
    /// Optional operator-managed installation master-key file override.
    pub master_key_file: Option<PathBuf>,
    /// Exact browser origins accepted by the Admin listener.
    pub admin_origins: OriginPolicy,
    /// Exact browser origins accepted by data-plane protocol adapters.
    pub data_origins: OriginPolicy,
    /// Canonical external data-plane origin advertised in Admin connection
    /// cards. When absent, the server derives a direct-listener origin.
    pub data_public_origin: Option<String>,
    /// Exact Host authorities accepted by the MCP transport.
    pub data_hosts: BTreeSet<String>,
    /// Structured log output mode.
    pub log_format: LogFormat,
    /// Maximum graceful-shutdown period used by the eventual worker supervisor.
    pub shutdown_timeout: Duration,
    /// Delay between authoritative full Vault reconciliation passes.
    pub reconciliation_interval: Duration,
    /// Service-owned backup artifact root.
    pub backup_root: PathBuf,
    /// Archive and retention limits.
    pub backup_limits: BackupLimits,
    /// Whether the non-sensitive Prometheus endpoint is exposed.
    pub metrics_enabled: bool,
    /// Optional OTLP HTTP endpoint; disabled when absent.
    pub otlp_endpoint: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            data_bind: DEFAULT_DATA_BIND.parse().expect("valid data bind default"),
            admin_bind: DEFAULT_ADMIN_BIND
                .parse()
                .expect("valid admin bind default"),
            data_dir: PathBuf::from(DEFAULT_DATA_DIR),
            secrets_dir: PathBuf::from(DEFAULT_DATA_DIR).join("secrets"),
            database_url: DEFAULT_DATABASE_URL.to_owned(),
            master_key_file: None,
            admin_origins: OriginPolicy::new(DEFAULT_ADMIN_ORIGINS.split(','))
                .expect("default Admin origins are valid"),
            data_origins: OriginPolicy::new(std::iter::empty::<&str>())
                .expect("empty data origin policy is valid"),
            data_public_origin: None,
            data_hosts: default_data_hosts(DEFAULT_DATA_BIND.parse().expect("valid data bind")),
            log_format: LogFormat::Json,
            shutdown_timeout: Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECONDS),
            reconciliation_interval: Duration::from_secs(DEFAULT_RECONCILIATION_INTERVAL_SECONDS),
            backup_root: PathBuf::from("./data/backups"),
            backup_limits: BackupLimits::default(),
            metrics_enabled: false,
            otlp_endpoint: None,
        }
    }
}

impl AppConfig {
    /// Load and validate configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// Build configuration from a lookup function.
    ///
    /// Keeping parsing independent from the global environment makes startup
    /// validation deterministic and keeps configuration tests isolated.
    pub fn from_lookup<F>(lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let defaults = Self::default();
        if lookup("MCP_VAULT_ADMIN_ALLOWED_CIDRS").is_some() {
            return Err(ConfigError::InvalidValue {
                key: "MCP_VAULT_ADMIN_ALLOWED_CIDRS",
                message: "is no longer enforced by MCP Vault; remove it and configure listener publication, firewall/VPN, or reverse-proxy policy instead".to_owned(),
            });
        }
        for key in [
            "MCP_VAULT_BOOTSTRAP_TOKEN",
            "MCP_VAULT_BOOTSTRAP_TOKEN_FILE",
        ] {
            if lookup(key).is_some() {
                return Err(ConfigError::InvalidValue {
                    key,
                    message: "is obsolete because first-Admin setup now accepts only username and password; remove this setting".to_owned(),
                });
            }
        }
        let data_bind = parse_socket_addr(&lookup, "MCP_VAULT_DATA_BIND", defaults.data_bind)?;
        let admin_bind = parse_socket_addr(&lookup, "MCP_VAULT_ADMIN_BIND", defaults.admin_bind)?;
        let data_dir = parse_path(&lookup, "MCP_VAULT_DATA_DIR", defaults.data_dir)?;
        let secrets_dir = parse_path(&lookup, "MCP_VAULT_SECRETS_DIR", data_dir.join("secrets"))?;
        let database_url =
            lookup("MCP_VAULT_DATABASE_URL").unwrap_or_else(|| default_database_url(&data_dir));
        let master_key_file = optional_path(&lookup, "MCP_VAULT_MASTER_KEY_FILE")?;
        let admin_origins =
            parse_origins(&lookup, "MCP_VAULT_ADMIN_ORIGINS", DEFAULT_ADMIN_ORIGINS)?;
        validate_admin_origin_transports(&admin_origins)?;
        let data_origins = parse_origins(&lookup, "MCP_VAULT_DATA_ORIGINS", DEFAULT_DATA_ORIGINS)?;
        let data_public_origin = parse_optional_origin(&lookup, "MCP_VAULT_DATA_PUBLIC_ORIGIN")?;
        let data_hosts = parse_data_hosts(&lookup, default_data_hosts(data_bind))?;
        let log_format = parse_log_format(&lookup)?;
        let shutdown_timeout = parse_shutdown_timeout(&lookup)?;
        let reconciliation_interval = parse_reconciliation_interval(&lookup)?;
        let backup_root = parse_path(&lookup, "MCP_VAULT_BACKUP_DIR", data_dir.join("backups"))?;
        let backup_limits = parse_backup_limits(&lookup)?;
        let metrics_enabled = parse_bool(&lookup, "MCP_VAULT_METRICS_ENABLED", false)?;
        let otlp_endpoint = optional_text(&lookup, "MCP_VAULT_OTEL_ENDPOINT")?;

        Self {
            data_bind,
            admin_bind,
            data_dir,
            secrets_dir,
            database_url,
            master_key_file,
            admin_origins,
            data_origins,
            data_public_origin,
            data_hosts,
            log_format,
            shutdown_timeout,
            reconciliation_interval,
            backup_root,
            backup_limits,
            metrics_enabled,
            otlp_endpoint,
        }
        .validate()
    }

    /// Validate cross-field bootstrap invariants.
    pub fn validate(self) -> Result<Self, ConfigError> {
        if self.data_bind == self.admin_bind {
            return Err(ConfigError::ListenersShareAddress(self.data_bind));
        }

        if self.data_dir.as_os_str().is_empty() {
            return Err(ConfigError::EmptyValue("MCP_VAULT_DATA_DIR"));
        }
        if self.secrets_dir.as_os_str().is_empty() {
            return Err(ConfigError::EmptyValue("MCP_VAULT_SECRETS_DIR"));
        }
        if self.database_url.is_empty() {
            return Err(ConfigError::EmptyValue("MCP_VAULT_DATABASE_URL"));
        }
        if !self.database_url.starts_with("sqlite:")
            || self.database_url.chars().any(char::is_control)
        {
            return Err(ConfigError::InvalidValue {
                key: "MCP_VAULT_DATABASE_URL",
                message: "must be a non-control SQLite URL".to_owned(),
            });
        }

        if self.data_hosts.is_empty() {
            return Err(ConfigError::InvalidValue {
                key: "MCP_VAULT_DATA_HOSTS",
                message: "must contain at least one exact host authority".to_owned(),
            });
        }

        if self.backup_root.as_os_str().is_empty()
            || self.backup_limits.max_entry_bytes == 0
            || self.backup_limits.max_total_bytes < self.backup_limits.max_entry_bytes
            || self.backup_limits.max_archive_bytes < self.backup_limits.max_total_bytes
            || self.backup_limits.max_entries == 0
            || self.backup_limits.keep_count == 0
        {
            return Err(ConfigError::InvalidValue {
                key: "MCP_VAULT_BACKUP_*",
                message: "backup root and limits are invalid".to_owned(),
            });
        }

        if let Some(endpoint) = self.otlp_endpoint.as_deref() {
            let parsed = url::Url::parse(endpoint).map_err(|_| ConfigError::InvalidValue {
                key: "MCP_VAULT_OTEL_ENDPOINT",
                message: "must be an absolute HTTP(S) URL without userinfo".to_owned(),
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                return Err(ConfigError::InvalidValue {
                    key: "MCP_VAULT_OTEL_ENDPOINT",
                    message: "must be an absolute HTTP(S) URL without userinfo".to_owned(),
                });
            }
        }

        Ok(self)
    }

    /// Default service-owned installation-key path used when no explicit file
    /// override is configured.
    pub fn managed_master_key_file(&self) -> PathBuf {
        self.secrets_dir.join("master-key")
    }
}

fn default_database_url(data_dir: &Path) -> String {
    format!("sqlite://{}/state/mcp-vault.sqlite3", data_dir.display())
}

fn default_data_hosts(bind: SocketAddr) -> BTreeSet<String> {
    let port = bind.port();
    let mut hosts = BTreeSet::from([
        "localhost".to_owned(),
        format!("localhost:{port}"),
        "127.0.0.1".to_owned(),
        format!("127.0.0.1:{port}"),
        "::1".to_owned(),
        "[::1]".to_owned(),
        format!("[::1]:{port}"),
    ]);
    if !bind.ip().is_unspecified() {
        hosts.insert(bind.ip().to_string());
        hosts.insert(match bind.ip() {
            IpAddr::V4(address) => format!("{address}:{port}"),
            IpAddr::V6(address) => format!("[{address}]:{port}"),
        });
    }
    hosts
}

/// Errors raised before the server binds a listener.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ConfigError {
    #[error("invalid value for {key}: {message}")]
    InvalidValue { key: &'static str, message: String },
    #[error("{0} must not be empty")]
    EmptyValue(&'static str),
    #[error("data and control listeners cannot share {0}")]
    ListenersShareAddress(SocketAddr),
}

fn parse_socket_addr<F>(
    lookup: &F,
    key: &'static str,
    default: SocketAddr,
) -> Result<SocketAddr, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = lookup(key) else {
        return Ok(default);
    };

    value.parse().map_err(|error| ConfigError::InvalidValue {
        key,
        message: format!("{error}"),
    })
}

fn parse_path<F>(lookup: &F, key: &'static str, default: PathBuf) -> Result<PathBuf, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(key) {
        Some(value) if value.is_empty() => Err(ConfigError::EmptyValue(key)),
        Some(value) => Ok(PathBuf::from(value)),
        None => Ok(default),
    }
}

fn optional_path<F>(lookup: &F, key: &'static str) -> Result<Option<PathBuf>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(key) {
        Some(value) if value.is_empty() => Err(ConfigError::EmptyValue(key)),
        Some(value) => Ok(Some(PathBuf::from(value))),
        None => Ok(None),
    }
}

fn optional_text<F>(lookup: &F, key: &'static str) -> Result<Option<String>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(key) {
        Some(value) if value.is_empty() || value.chars().any(char::is_control) => {
            Err(ConfigError::InvalidValue {
                key,
                message: "must be non-empty and contain no control characters".to_owned(),
            })
        }
        Some(value) => Ok(Some(value)),
        None => Ok(None),
    }
}

fn parse_bool<F>(lookup: &F, key: &'static str, default: bool) -> Result<bool, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(key).as_deref() {
        None => Ok(default),
        Some("1" | "true" | "yes" | "on") => Ok(true),
        Some("0" | "false" | "no" | "off") => Ok(false),
        Some(value) => Err(ConfigError::InvalidValue {
            key,
            message: format!("expected boolean, got {value}"),
        }),
    }
}

fn parse_backup_limits<F>(lookup: &F) -> Result<BackupLimits, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let defaults = BackupLimits::default();
    Ok(BackupLimits {
        max_entry_bytes: parse_u64_limit(
            lookup,
            "MCP_VAULT_BACKUP_MAX_ENTRY_BYTES",
            defaults.max_entry_bytes,
            1,
            4 * 1024 * 1024 * 1024,
        )?,
        max_total_bytes: parse_u64_limit(
            lookup,
            "MCP_VAULT_BACKUP_MAX_TOTAL_BYTES",
            defaults.max_total_bytes,
            1,
            16 * 1024 * 1024 * 1024,
        )?,
        max_archive_bytes: parse_u64_limit(
            lookup,
            "MCP_VAULT_BACKUP_MAX_ARCHIVE_BYTES",
            defaults.max_archive_bytes,
            1,
            16 * 1024 * 1024 * 1024,
        )?,
        max_entries: parse_u64_limit(
            lookup,
            "MCP_VAULT_BACKUP_MAX_ENTRIES",
            defaults.max_entries,
            1,
            1_000_000,
        )?,
        keep_count: parse_u64_limit(
            lookup,
            "MCP_VAULT_BACKUP_KEEP_COUNT",
            u64::from(defaults.keep_count),
            1,
            100,
        )? as u32,
    })
}

fn parse_u64_limit<F>(
    lookup: &F,
    key: &'static str,
    default: u64,
    minimum: u64,
    maximum: u64,
) -> Result<u64, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = lookup(key) else {
        return Ok(default);
    };
    let parsed = value
        .parse::<u64>()
        .map_err(|error| ConfigError::InvalidValue {
            key,
            message: format!("{error}"),
        })?;
    if parsed < minimum || parsed > maximum {
        return Err(ConfigError::InvalidValue {
            key,
            message: format!("must be between {minimum} and {maximum}"),
        });
    }
    Ok(parsed)
}

fn parse_origins<F>(
    lookup: &F,
    key: &'static str,
    default: &str,
) -> Result<OriginPolicy, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let value = lookup(key).unwrap_or_else(|| default.to_owned());
    let origins = value
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .collect::<Vec<_>>();
    OriginPolicy::new(origins).map_err(|_| ConfigError::InvalidValue {
        key,
        message: "must contain exact http(s) origins separated by commas".to_owned(),
    })
}

fn validate_admin_origin_transports(policy: &OriginPolicy) -> Result<(), ConfigError> {
    for origin in policy.allowed_origins() {
        let parsed = url::Url::parse(origin).map_err(|_| ConfigError::InvalidValue {
            key: "MCP_VAULT_ADMIN_ORIGINS",
            message: "must contain valid exact Admin origins".to_owned(),
        })?;
        if parsed.scheme() != "http" {
            continue;
        }
        let is_local = match parsed.host() {
            Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(address)) => is_private_or_local_ip(IpAddr::V4(address)),
            Some(url::Host::Ipv6(address)) => is_private_or_local_ip(IpAddr::V6(address)),
            None => false,
        };
        if !is_local {
            return Err(ConfigError::InvalidValue {
                key: "MCP_VAULT_ADMIN_ORIGINS",
                message: "cleartext HTTP is allowed only for localhost or a literal private, loopback, or link-local IP address".to_owned(),
            });
        }
    }
    Ok(())
}

fn is_private_or_local_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        }
        IpAddr::V6(address) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
    }
}

fn parse_optional_origin<F>(lookup: &F, key: &'static str) -> Result<Option<String>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = lookup(key) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(ConfigError::EmptyValue(key));
    }
    let policy = OriginPolicy::new([value.as_str()]).map_err(|_| ConfigError::InvalidValue {
        key,
        message: "must be one exact http(s) origin without a path, query, fragment, or userinfo"
            .to_owned(),
    })?;
    Ok(policy.allowed_origins().next().map(str::to_owned))
}

fn parse_log_format<F>(lookup: &F) -> Result<LogFormat, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup("MCP_VAULT_LOG_FORMAT").as_deref() {
        None | Some("json") => Ok(LogFormat::Json),
        Some("pretty") => Ok(LogFormat::Pretty),
        Some(value) => Err(ConfigError::InvalidValue {
            key: "MCP_VAULT_LOG_FORMAT",
            message: format!("expected json or pretty, got {value}"),
        }),
    }
}

fn parse_shutdown_timeout<F>(lookup: &F) -> Result<Duration, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = lookup("MCP_VAULT_SHUTDOWN_TIMEOUT_SECONDS") else {
        return Ok(Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECONDS));
    };

    let seconds: u64 = value.parse().map_err(|error| ConfigError::InvalidValue {
        key: "MCP_VAULT_SHUTDOWN_TIMEOUT_SECONDS",
        message: format!("{error}"),
    })?;

    if seconds == 0 || seconds > MAX_SHUTDOWN_TIMEOUT_SECONDS {
        return Err(ConfigError::InvalidValue {
            key: "MCP_VAULT_SHUTDOWN_TIMEOUT_SECONDS",
            message: format!("must be between 1 and {MAX_SHUTDOWN_TIMEOUT_SECONDS} seconds"),
        });
    }

    Ok(Duration::from_secs(seconds))
}

fn parse_reconciliation_interval<F>(lookup: &F) -> Result<Duration, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = lookup("MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS") else {
        return Ok(Duration::from_secs(DEFAULT_RECONCILIATION_INTERVAL_SECONDS));
    };

    let seconds: u64 = value.parse().map_err(|error| ConfigError::InvalidValue {
        key: "MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS",
        message: format!("{error}"),
    })?;

    if seconds == 0 || seconds > MAX_RECONCILIATION_INTERVAL_SECONDS {
        return Err(ConfigError::InvalidValue {
            key: "MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS",
            message: format!("must be between 1 and {MAX_RECONCILIATION_INTERVAL_SECONDS} seconds"),
        });
    }

    Ok(Duration::from_secs(seconds))
}

fn parse_data_hosts<F>(
    lookup: &F,
    defaults: BTreeSet<String>,
) -> Result<BTreeSet<String>, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let Some(value) = lookup("MCP_VAULT_DATA_HOSTS") else {
        return Ok(defaults);
    };
    let hosts = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.chars().any(|character| {
                character.is_control() || character.is_whitespace() || matches!(character, '/' | '?' | '#')
            }) || value.contains('@')
            {
                return Err(ConfigError::InvalidValue {
                    key: "MCP_VAULT_DATA_HOSTS",
                    message: "host authorities must not contain whitespace, paths, queries, fragments, or userinfo".to_owned(),
                });
            }
            Ok(value.to_owned())
        })
        .collect::<Result<BTreeSet<_>, ConfigError>>()?;
    if hosts.is_empty() {
        return Err(ConfigError::InvalidValue {
            key: "MCP_VAULT_DATA_HOSTS",
            message: "must contain at least one exact host authority".to_owned(),
        });
    }
    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, net::SocketAddr, path::PathBuf, time::Duration};

    use super::{AppConfig, ConfigError, LogFormat};

    fn config(values: &[(&str, &str)]) -> Result<AppConfig, ConfigError> {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>();

        AppConfig::from_lookup(|key| values.get(key).cloned())
    }

    #[test]
    fn defaults_keep_control_plane_on_loopback() {
        let result = config(&[]).unwrap();

        assert_eq!(
            result.data_bind,
            "0.0.0.0:8080".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            result.admin_bind,
            "127.0.0.1:8081".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(result.data_dir, PathBuf::from("./data"));
        assert_eq!(result.secrets_dir, PathBuf::from("./data/secrets"));
        assert_eq!(
            result.database_url,
            "sqlite://./data/state/mcp-vault.sqlite3"
        );
        assert_eq!(result.log_format, LogFormat::Json);
        assert_eq!(result.shutdown_timeout, Duration::from_secs(30));
        assert_eq!(result.reconciliation_interval, Duration::from_secs(300));
        assert_eq!(
            result.admin_origins.allowed_origins().collect::<Vec<_>>(),
            vec!["http://127.0.0.1:8081", "http://localhost:8081"]
        );
        assert_eq!(result.data_origins.allowed_origins().count(), 0);
        assert!(result.data_public_origin.is_none());
        for authority in [
            "localhost",
            "localhost:8080",
            "127.0.0.1",
            "127.0.0.1:8080",
            "::1",
            "[::1]",
            "[::1]:8080",
        ] {
            assert!(result.data_hosts.contains(authority), "missing {authority}");
        }
    }

    #[test]
    fn loopback_host_defaults_follow_an_overridden_data_port() {
        let result = config(&[("MCP_VAULT_DATA_BIND", "127.0.0.1:18080")]).unwrap();

        for authority in ["localhost:18080", "127.0.0.1:18080", "[::1]:18080"] {
            assert!(result.data_hosts.contains(authority), "missing {authority}");
        }
    }

    #[test]
    fn parses_typed_overrides_without_loading_secret_contents() {
        let result = config(&[
            ("MCP_VAULT_DATA_BIND", "127.0.0.1:18080"),
            ("MCP_VAULT_ADMIN_BIND", "127.0.0.1:18081"),
            ("MCP_VAULT_DATA_DIR", "/srv/mcp-vault"),
            ("MCP_VAULT_SECRETS_DIR", "/srv/mcp-vault-private"),
            (
                "MCP_VAULT_DATABASE_URL",
                "sqlite:///var/lib/mcp-vault/state.db",
            ),
            ("MCP_VAULT_MASTER_KEY_FILE", "/run/secrets/master-key"),
            (
                "MCP_VAULT_ADMIN_ORIGINS",
                "https://admin.example.test,https://admin.example.test:443",
            ),
            ("MCP_VAULT_DATA_ORIGINS", "https://agent.example.test"),
            (
                "MCP_VAULT_DATA_PUBLIC_ORIGIN",
                "https://vault.example.test:8443",
            ),
            ("MCP_VAULT_LOG_FORMAT", "pretty"),
            ("MCP_VAULT_SHUTDOWN_TIMEOUT_SECONDS", "12"),
            ("MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS", "42"),
            ("MCP_VAULT_BACKUP_DIR", "/srv/mcp-vault/backups"),
            ("MCP_VAULT_BACKUP_MAX_ENTRY_BYTES", "1048576"),
            ("MCP_VAULT_BACKUP_MAX_TOTAL_BYTES", "2097152"),
            ("MCP_VAULT_BACKUP_MAX_ARCHIVE_BYTES", "2097152"),
            ("MCP_VAULT_BACKUP_MAX_ENTRIES", "100"),
            ("MCP_VAULT_BACKUP_KEEP_COUNT", "2"),
            ("MCP_VAULT_METRICS_ENABLED", "true"),
            (
                "MCP_VAULT_OTEL_ENDPOINT",
                "http://otel-collector:4318/v1/traces",
            ),
            (
                "MCP_VAULT_DATA_HOSTS",
                "vault.example.test,vault.example.test:8443",
            ),
        ])
        .unwrap();

        assert_eq!(result.data_bind, "127.0.0.1:18080".parse().unwrap());
        assert_eq!(result.admin_bind, "127.0.0.1:18081".parse().unwrap());
        assert_eq!(result.data_dir, PathBuf::from("/srv/mcp-vault"));
        assert_eq!(result.secrets_dir, PathBuf::from("/srv/mcp-vault-private"));
        assert_eq!(result.database_url, "sqlite:///var/lib/mcp-vault/state.db");
        assert_eq!(
            result.master_key_file,
            Some(PathBuf::from("/run/secrets/master-key"))
        );
        assert_eq!(result.log_format, LogFormat::Pretty);
        assert_eq!(result.shutdown_timeout, Duration::from_secs(12));
        assert_eq!(result.reconciliation_interval, Duration::from_secs(42));
        assert_eq!(result.backup_root, PathBuf::from("/srv/mcp-vault/backups"));
        assert_eq!(result.backup_limits.max_entry_bytes, 1_048_576);
        assert_eq!(result.backup_limits.max_total_bytes, 2_097_152);
        assert_eq!(result.backup_limits.keep_count, 2);
        assert!(result.metrics_enabled);
        assert_eq!(
            result.otlp_endpoint.as_deref(),
            Some("http://otel-collector:4318/v1/traces")
        );
        assert!(result.data_hosts.contains("vault.example.test"));
        assert!(result.data_hosts.contains("vault.example.test:8443"));
        assert_eq!(
            result.admin_origins.allowed_origins().collect::<Vec<_>>(),
            vec!["https://admin.example.test"]
        );
        assert_eq!(
            result.data_origins.allowed_origins().collect::<Vec<_>>(),
            vec!["https://agent.example.test"]
        );
        assert_eq!(
            result.data_public_origin.as_deref(),
            Some("https://vault.example.test:8443")
        );
    }

    #[test]
    fn permits_explicit_private_http_admin_origins_but_rejects_public_cleartext() {
        let private = config(&[(
            "MCP_VAULT_ADMIN_ORIGINS",
            "https://admin.example.test,http://192.168.1.20:8081,http://[fd00::20]:8081",
        )])
        .unwrap();
        assert_eq!(
            private.admin_origins.allowed_origins().collect::<Vec<_>>(),
            vec![
                "http://192.168.1.20:8081",
                "http://[fd00::20]:8081",
                "https://admin.example.test",
            ]
        );

        for origin in ["http://203.0.113.10:8081", "http://admin.example.test"] {
            let error = config(&[("MCP_VAULT_ADMIN_ORIGINS", origin)]).unwrap_err();
            assert!(matches!(
                error,
                ConfigError::InvalidValue {
                    key: "MCP_VAULT_ADMIN_ORIGINS",
                    ..
                }
            ));
        }
    }

    #[test]
    fn rejects_shared_listener_address() {
        let error = config(&[
            ("MCP_VAULT_DATA_BIND", "127.0.0.1:18080"),
            ("MCP_VAULT_ADMIN_BIND", "127.0.0.1:18080"),
        ])
        .unwrap_err();

        assert_eq!(
            error,
            ConfigError::ListenersShareAddress("127.0.0.1:18080".parse().unwrap())
        );
    }

    #[test]
    fn rejects_invalid_log_format_and_shutdown_timeout() {
        let error = config(&[("MCP_VAULT_LOG_FORMAT", "yaml")]).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                key: "MCP_VAULT_LOG_FORMAT",
                ..
            }
        ));

        let error = config(&[("MCP_VAULT_SHUTDOWN_TIMEOUT_SECONDS", "0")]).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                key: "MCP_VAULT_SHUTDOWN_TIMEOUT_SECONDS",
                ..
            }
        ));

        let error = config(&[("MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS", "0")]).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                key: "MCP_VAULT_RECONCILIATION_INTERVAL_SECONDS",
                ..
            }
        ));

        let error = config(&[("MCP_VAULT_ADMIN_ALLOWED_CIDRS", "127.0.0.0/8")]).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                key: "MCP_VAULT_ADMIN_ALLOWED_CIDRS",
                ..
            }
        ));

        let error = config(&[("MCP_VAULT_DATA_HOSTS", "https://vault.example.test")]).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                key: "MCP_VAULT_DATA_HOSTS",
                ..
            }
        ));

        let error = config(&[(
            "MCP_VAULT_DATA_PUBLIC_ORIGIN",
            "https://vault.example.test/path",
        )])
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                key: "MCP_VAULT_DATA_PUBLIC_ORIGIN",
                ..
            }
        ));

        for key in [
            "MCP_VAULT_BOOTSTRAP_TOKEN",
            "MCP_VAULT_BOOTSTRAP_TOKEN_FILE",
        ] {
            let error = config(&[(key, "obsolete")]).unwrap_err();
            assert!(matches!(
                error,
                ConfigError::InvalidValue {
                    key: rejected,
                    ..
                } if rejected == key
            ));
        }

        let error = config(&[("MCP_VAULT_BACKUP_KEEP_COUNT", "0")]).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                key: "MCP_VAULT_BACKUP_KEEP_COUNT",
                ..
            }
        ));

        let error = config(&[(
            "MCP_VAULT_OTEL_ENDPOINT",
            "http://user:password@otel.example.test/v1/traces",
        )])
        .unwrap_err();
        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                key: "MCP_VAULT_OTEL_ENDPOINT",
                ..
            }
        ));
    }
}
