//! Runtime configuration, read from `DOTVAULT_*` environment variables.

use std::net::SocketAddr;
use std::time::Duration;

/// Largest request body the server will read at all.
pub const MAX_BODY_BYTES: usize = 512 * 1024;
pub use dotvault_core::api::{
    MAX_ITEM_NAME_BYTES, MAX_ITEM_PAYLOAD_BYTES, MAX_SHARE_PAYLOAD_BYTES, SHARE_MAX_TTL_SECS,
    SHARE_MAX_VIEWS, SHARE_MIN_TTL_SECS,
};
/// Largest wrapped key accepted from a client.
pub const MAX_WRAPPED_KEY_BYTES: usize = 256;
/// How many versions of a vault item are kept.
pub const KEEP_ITEM_VERSIONS: i64 = 20;

pub const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
/// A session row is rewritten at most this often.
pub const SESSION_TOUCH_INTERVAL_SECS: i64 = 3600;

pub const SHARE_DEFAULT_TTL_SECS: u64 = 24 * 3600;

/// A fixed-window limit: at most `max` requests per `window` per client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub max: u32,
    pub window: Duration,
}

impl RateLimit {
    pub const fn per_minute(max: u32) -> Self {
        Self {
            max,
            window: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// `DOTVAULT_BIND`, default `127.0.0.1:8080`.
    pub bind: SocketAddr,
    /// `DOTVAULT_DB`: a file path, or a `sqlite:` URL. Default `./dotvault.db`.
    pub db: String,
    /// `DOTVAULT_SERVER_SECRET`. When `None`, a random secret is generated
    /// once and kept in the database.
    pub server_secret: Option<String>,
    /// `DOTVAULT_PUBLIC_URL`, shown on the landing page.
    pub public_url: Option<String>,
    /// `DOTVAULT_TRUST_PROXY=1`: take the client address from the last
    /// `X-Forwarded-For` entry (the one the trusted proxy appended).
    pub trust_proxy: bool,
    /// Shared budget of register, prelogin, login and recover/*.
    pub auth_rate: RateLimit,
    /// Budget of share open.
    pub share_open_rate: RateLimit,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 8080)),
            db: "./dotvault.db".to_string(),
            server_secret: None,
            public_url: None,
            trust_proxy: false,
            auth_rate: RateLimit::per_minute(20),
            share_open_rate: RateLimit::per_minute(60),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{var}: {msg}")]
pub struct ConfigError {
    pub var: &'static str,
    pub msg: String,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Like [`Config::from_env`] but reads variables through `get`, so the
    /// parsing can be tested without touching the process environment.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let get = |k: &str| {
            get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut cfg = Self::default();

        if let Some(v) = get("DOTVAULT_BIND") {
            cfg.bind = v.parse().map_err(|e| ConfigError {
                var: "DOTVAULT_BIND",
                msg: format!("expected host:port, e.g. 127.0.0.1:8080 ({e})"),
            })?;
        }
        if let Some(v) = get("DOTVAULT_DB") {
            cfg.db = v;
        }
        if let Some(v) = get("DOTVAULT_SERVER_SECRET") {
            if v.len() < 16 {
                return Err(ConfigError {
                    var: "DOTVAULT_SERVER_SECRET",
                    msg: "must be at least 16 characters (try: openssl rand -base64 32)".into(),
                });
            }
            cfg.server_secret = Some(v);
        }
        if let Some(v) = get("DOTVAULT_PUBLIC_URL") {
            cfg.public_url = Some(v.trim_end_matches('/').to_string());
        }
        if let Some(v) = get("DOTVAULT_TRUST_PROXY") {
            cfg.trust_proxy = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults() {
        let c = Config::from_lookup(lookup(&[])).unwrap();
        assert_eq!(c.bind, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(c.db, "./dotvault.db");
        assert!(c.server_secret.is_none() && c.public_url.is_none() && !c.trust_proxy);
        assert_eq!(c.auth_rate, RateLimit::per_minute(20));
        assert_eq!(c.share_open_rate, RateLimit::per_minute(60));
    }

    #[test]
    fn reads_every_variable() {
        let c = Config::from_lookup(lookup(&[
            ("DOTVAULT_BIND", "0.0.0.0:9000"),
            ("DOTVAULT_DB", "/data/x.db"),
            ("DOTVAULT_SERVER_SECRET", "0123456789abcdef0123"),
            ("DOTVAULT_PUBLIC_URL", "https://vault.example.com/"),
            ("DOTVAULT_TRUST_PROXY", "1"),
        ]))
        .unwrap();
        assert_eq!(c.bind.port(), 9000);
        assert_eq!(c.db, "/data/x.db");
        assert_eq!(c.server_secret.as_deref(), Some("0123456789abcdef0123"));
        assert_eq!(c.public_url.as_deref(), Some("https://vault.example.com"));
        assert!(c.trust_proxy);
    }

    #[test]
    fn rejects_bad_values_and_treats_empty_as_unset() {
        assert!(Config::from_lookup(lookup(&[("DOTVAULT_BIND", "nonsense")])).is_err());
        assert!(Config::from_lookup(lookup(&[("DOTVAULT_SERVER_SECRET", "short")])).is_err());
        let c = Config::from_lookup(lookup(&[
            ("DOTVAULT_SERVER_SECRET", "  "),
            ("DOTVAULT_TRUST_PROXY", "0"),
        ]))
        .unwrap();
        assert!(c.server_secret.is_none() && !c.trust_proxy);
    }
}
