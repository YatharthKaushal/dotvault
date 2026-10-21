//! Blocking HTTP client for the dotvault API.

use std::fmt;
use std::time::Duration;

use anyhow::{bail, Result};
use dotvault_core::api::{ErrorBody, API_PREFIX};
use serde::{de::DeserializeOwned, Serialize};
use ureq::http::{Method, Request};
use ureq::Agent;

/// Whatever went wrong talking to the server.
#[derive(Debug)]
pub enum ApiError {
    /// The server answered with an error status.
    Http {
        status: u16,
        code: String,
        message: String,
    },
    /// No usable answer: DNS, TLS, timeout, garbage.
    Transport(String),
}

impl ApiError {
    pub fn code(&self) -> Option<&str> {
        match self {
            ApiError::Http { code, .. } => Some(code),
            ApiError::Transport(_) => None,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Http { message, .. } => write!(f, "{message}"),
            ApiError::Transport(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

pub struct Api {
    agent: Agent,
    base: String,
    token: Option<String>,
}

impl Api {
    pub fn new(server: &str, token: Option<String>) -> Result<Api> {
        let base = normalize_server(server)?;
        let agent: Agent = Agent::config_builder()
            // Error bodies carry the reason; read them instead of failing early.
            .http_status_as_error(false)
            // The API never redirects, and a redirect must not carry our token elsewhere.
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(format!("dotvault/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Ok(Api { agent, base, token })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn get<R: DeserializeOwned>(&self, path: &str) -> Result<R, ApiError> {
        self.json(Method::GET, path, None::<&()>)
    }

    pub fn post<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R, ApiError> {
        self.json(Method::POST, path, Some(body))
    }

    pub fn put<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<R, ApiError> {
        self.json(Method::PUT, path, Some(body))
    }

    /// POST whose success answer has no body (204).
    pub fn post_unit<B: Serialize>(&self, path: &str, body: &B) -> Result<(), ApiError> {
        self.send(Method::POST, path, Some(body)).map(|_| ())
    }

    pub fn post_empty_unit(&self, path: &str) -> Result<(), ApiError> {
        self.send(Method::POST, path, None::<&()>).map(|_| ())
    }

    /// POST with no request body and a JSON answer.
    pub fn post_empty<R: DeserializeOwned>(&self, path: &str) -> Result<R, ApiError> {
        self.json(Method::POST, path, None::<&()>)
    }

    pub fn delete(&self, path: &str) -> Result<(), ApiError> {
        self.send(Method::DELETE, path, None::<&()>).map(|_| ())
    }

    fn json<B: Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, ApiError> {
        let bytes = self.send(method, path, body)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::Transport(format!("the server sent an unreadable answer: {e}")))
    }

    fn send<B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<Vec<u8>, ApiError> {
        let transport = |e: &dyn fmt::Display| {
            ApiError::Transport(format!("could not reach {}: {e}", self.base))
        };

        let mut builder = Request::builder()
            .method(method)
            .uri(format!("{}{API_PREFIX}{path}", self.base))
            .header("accept", "application/json");
        if let Some(token) = &self.token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let payload = match body {
            Some(b) => {
                builder = builder.header("content-type", "application/json");
                serde_json::to_vec(b).map_err(|e| ApiError::Transport(e.to_string()))?
            }
            None => Vec::new(),
        };
        let request = builder
            .body(payload)
            .map_err(|e| ApiError::Transport(e.to_string()))?;

        let mut response = self.agent.run(request).map_err(|e| transport(&e))?;
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(2 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| transport(&e))?;

        if (200..300).contains(&status) {
            return Ok(bytes);
        }
        Err(match serde_json::from_slice::<ErrorBody>(&bytes) {
            Ok(body) => ApiError::Http {
                status,
                code: body.code,
                message: body.message,
            },
            Err(_) => ApiError::Http {
                status,
                code: "unexpected".into(),
                message: format!("the server answered with HTTP {status}"),
            },
        })
    }
}

/// Accepts `https://host[:port]` (any path or trailing slash is dropped) and
/// plain `http://` only for loopback, so tokens never travel unencrypted
/// across a network.
pub fn normalize_server(server: &str) -> Result<String> {
    let server = server.trim();
    let (scheme, rest) = server
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("server must start with https:// (got {server:?})"))?;
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') || authority.contains(char::is_whitespace) {
        bail!("invalid server address {server:?}");
    }
    let authority = authority.to_ascii_lowercase();

    match scheme.as_str() {
        "https" => {}
        "http" if is_loopback(&authority) => {}
        "http" => bail!(
            "refusing to talk to {server} over plain http; use https:// (http is only allowed for localhost)"
        ),
        other => bail!("unsupported scheme {other:?}; use https://"),
    }
    // `https://host:443` and `https://host` are the same origin.
    let authority = match (scheme.as_str(), authority.rsplit_once(':')) {
        ("https", Some((host, "443"))) | ("http", Some((host, "80"))) => host.to_string(),
        _ => authority,
    };
    Ok(format!("{scheme}://{authority}"))
}

fn is_loopback(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.rsplit_once(':').map_or(authority, |(h, _)| h)
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// Percent-encodes a query value.
pub fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_servers() {
        for (input, want) in [
            ("https://vault.example.com", "https://vault.example.com"),
            ("https://Vault.Example.com/", "https://vault.example.com"),
            (
                "https://vault.example.com/some/path?x=1#y",
                "https://vault.example.com",
            ),
            ("https://vault.example.com:443", "https://vault.example.com"),
            (
                "https://vault.example.com:8443",
                "https://vault.example.com:8443",
            ),
            ("http://localhost:8080", "http://localhost:8080"),
            ("http://127.0.0.1:3000/", "http://127.0.0.1:3000"),
            ("http://[::1]:3000", "http://[::1]:3000"),
            ("  https://x.io  ", "https://x.io"),
        ] {
            assert_eq!(normalize_server(input).unwrap(), want, "{input}");
        }
    }

    #[test]
    fn rejects_unsafe_servers() {
        for bad in [
            "vault.example.com",
            "http://vault.example.com",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "ftp://x.io",
            "https://",
            "https://user@evil.com",
            "https://exa mple.com",
        ] {
            assert!(normalize_server(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(encode_query("ada_l.o-v~e"), "ada_l.o-v~e");
        assert_eq!(encode_query("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }
}
