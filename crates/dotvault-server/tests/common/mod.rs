//! Shared test harness: drives the real router in-process (no sockets) and
//! plays the part of the CLI, doing the client-side crypto with
//! `dotvault_core::crypto` so the server sees realistic requests.
#![allow(dead_code, clippy::result_large_err)]

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, HeaderMap, Method, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use dotvault_core::{
    api::{
        CreateItemRequest, CreateShareRequest, ErrorBody, LoginRequest, PreloginResponse,
        RegisterRequest, SessionResponse,
    },
    crypto::{
        aad_item_name, aad_item_payload, create_account, derive_account_keys, derive_recovery_keys,
        format_recovery_key, parse_recovery_key, random_key, seal, unlock_with_password, KdfParams,
        Key, AAD_SHARE,
    },
    share_code,
};
use dotvault_server::{app, connect, AppState, Config};
use serde::{de::DeserializeOwned, Serialize};
use tempfile::TempDir;
use tower::ServiceExt;

/// The smallest KDF cost core accepts; keeps Argon2 quick in tests.
pub const FAST: KdfParams = KdfParams {
    m_kib: 19 * 1024,
    t: 2,
    p: 1,
};

pub const DEFAULT_PEER: &str = "203.0.113.1";

/// A server under test, backed by a temp-file SQLite DB (WAL, like
/// production) that lives as long as this value.
pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub db_path: PathBuf,
    _dir: TempDir,
}

impl TestApp {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    /// Rate limits are lifted by default so tests that make many auth calls
    /// do not trip them; tests of the limiter set them explicitly.
    pub async fn with_config(tweak: impl FnOnce(&mut Config)) -> Self {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let mut config = Config {
            db: db_path.to_string_lossy().into_owned(),
            auth_rate: dotvault_server::config::RateLimit::per_minute(100_000),
            share_open_rate: dotvault_server::config::RateLimit::per_minute(100_000),
            ..Config::default()
        };
        tweak(&mut config);
        let pool = connect(&config.db).await.unwrap();
        let state = AppState::new(pool, config).await.unwrap();
        Self {
            router: app(state.clone()),
            state,
            db_path,
            _dir: dir,
        }
    }

    /// A second router over the same database, as after a server restart.
    pub async fn restarted(&self) -> Router {
        let pool = connect(&self.state.config.db).await.unwrap();
        let state = AppState::new(pool, (*self.state.config).clone())
            .await
            .unwrap();
        app(state)
    }

    pub async fn send(&self, req: Req) -> Resp {
        send_to(&self.router, req).await
    }

    pub async fn get(&self, path: &str, token: Option<&str>) -> Resp {
        self.send(Req::new("GET", path).token_opt(token)).await
    }

    pub async fn delete(&self, path: &str, token: Option<&str>) -> Resp {
        self.send(Req::new("DELETE", path).token_opt(token)).await
    }

    pub async fn post<T: Serialize>(&self, path: &str, token: Option<&str>, body: &T) -> Resp {
        self.send(Req::new("POST", path).token_opt(token).json(body))
            .await
    }

    pub async fn put<T: Serialize>(&self, path: &str, token: Option<&str>, body: &T) -> Resp {
        self.send(Req::new("PUT", path).token_opt(token).json(body))
            .await
    }

    // --- accounts (client side of the protocol) ------------------------------

    /// Registers like the CLI does and returns the logged-in account.
    pub async fn register(&self, username: &str, password: &str) -> Account {
        self.try_register(username, password)
            .await
            .unwrap_or_else(|r| panic!("register {username} failed: {} {}", r.status, r.text()))
    }

    pub async fn try_register(&self, username: &str, password: &str) -> Result<Account, Resp> {
        let acct = create_account(password, FAST).unwrap();
        let req = RegisterRequest {
            username: username.to_string(),
            kdf_salt: acct.password.kdf_salt.to_vec(),
            kdf_params: acct.password.kdf_params,
            auth_key: acct.password.auth_key.to_vec(),
            wrapped_vault_key: acct.password.wrapped_vault_key.clone(),
            recovery_auth_key: acct.recovery_auth_key.to_vec(),
            wrapped_vault_key_recovery: acct.wrapped_vault_key_recovery.clone(),
        };
        let resp = self.post("/v1/auth/register", None, &req).await;
        if resp.status != StatusCode::OK {
            return Err(resp);
        }
        let session: SessionResponse = resp.json();
        Ok(Account {
            username: username.to_string(),
            password: password.to_string(),
            token: session.token,
            vault_key: acct.vault_key,
            recovery_key_text: format_recovery_key(&acct.recovery_key),
            recovery_key: acct.recovery_key,
            auth_key: acct.password.auth_key,
            recovery_auth_key: acct.recovery_auth_key,
        })
    }

    /// prelogin, derive keys, POST login. Returns the raw response.
    pub async fn try_login(&self, username: &str, password: &str) -> Resp {
        self.try_login_as(username, password, None).await
    }

    pub async fn try_login_as(
        &self,
        username: &str,
        password: &str,
        device_name: Option<&str>,
    ) -> Resp {
        let pre = self
            .get(&format!("/v1/auth/prelogin?username={username}"), None)
            .await;
        assert_eq!(pre.status, StatusCode::OK, "prelogin: {}", pre.text());
        let pre: PreloginResponse = pre.json();
        let keys = derive_account_keys(password, &pre.kdf_salt, &pre.kdf_params).unwrap();
        self.post(
            "/v1/auth/login",
            None,
            &LoginRequest {
                username: username.to_string(),
                auth_key: keys.auth_key.to_vec(),
                device_name: device_name.map(str::to_string),
            },
        )
        .await
    }

    /// A successful login, with the vault key unwrapped locally.
    pub async fn login(&self, username: &str, password: &str) -> Session {
        let resp = self.try_login(username, password).await;
        assert_eq!(resp.status, StatusCode::OK, "login: {}", resp.text());
        let session: SessionResponse = resp.json();
        let pre: PreloginResponse = self
            .get(&format!("/v1/auth/prelogin?username={username}"), None)
            .await
            .json();
        let keys = derive_account_keys(password, &pre.kdf_salt, &pre.kdf_params).unwrap();
        let vault_key = unlock_with_password(&keys, &session.wrapped_vault_key).unwrap();
        Session {
            token: session.token,
            expires_at: session.expires_at,
            vault_key,
        }
    }

    // --- direct DB access ---------------------------------------------------

    pub async fn user_id(&self, username: &str) -> i64 {
        sqlx::query_scalar("SELECT id FROM users WHERE username = ?1")
            .bind(username)
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    pub async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&self.state.db)
            .await
            .unwrap()
    }

    /// Flushes the WAL into the main database file.
    pub async fn checkpoint(&self) {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.state.db)
            .await
            .unwrap();
    }
}

#[derive(Debug)]
pub struct Account {
    pub username: String,
    pub password: String,
    pub token: String,
    pub vault_key: Key,
    pub recovery_key: Key,
    /// What the user was shown at registration: `DVR1-XXXX-...`.
    pub recovery_key_text: String,
    pub auth_key: Key,
    pub recovery_auth_key: Key,
}

pub struct Session {
    pub token: String,
    pub expires_at: i64,
    pub vault_key: Key,
}

// --- requests and responses ---------------------------------------------------

pub struct Req {
    method: Method,
    path: String,
    token: Option<String>,
    body: Option<(Vec<u8>, &'static str)>,
    headers: Vec<(String, String)>,
    peer: SocketAddr,
}

impl Req {
    pub fn new(method: &str, path: &str) -> Self {
        Self {
            method: method.parse().unwrap(),
            path: path.to_string(),
            token: None,
            body: None,
            headers: Vec::new(),
            peer: format!("{DEFAULT_PEER}:40000").parse().unwrap(),
        }
    }

    pub fn token(mut self, token: &str) -> Self {
        self.token = Some(token.to_string());
        self
    }

    pub fn token_opt(mut self, token: Option<&str>) -> Self {
        self.token = token.map(str::to_string);
        self
    }

    pub fn json<T: Serialize>(mut self, body: &T) -> Self {
        self.body = Some((serde_json::to_vec(body).unwrap(), "application/json"));
        self
    }

    pub fn raw(mut self, body: Vec<u8>, content_type: &'static str) -> Self {
        self.body = Some((body, content_type));
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn peer(mut self, ip: &str) -> Self {
        self.peer = format!("{ip}:40000").parse().unwrap();
        self
    }
}

pub async fn send_to(router: &Router, req: Req) -> Resp {
    let mut builder = Request::builder().method(req.method).uri(&req.path);
    if let Some(t) = &req.token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    let body = match req.body {
        Some((bytes, ct)) => {
            builder = builder.header(header::CONTENT_TYPE, ct);
            builder = builder.header(header::CONTENT_LENGTH, bytes.len());
            Body::from(bytes)
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    request.extensions_mut().insert(ConnectInfo(req.peer));

    let response = router.clone().oneshot(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: bytes.to_vec(),
    }
}

#[derive(Debug)]
pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("bad json ({e}): {}", self.text()))
    }

    pub fn error(&self) -> ErrorBody {
        self.json()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// Asserts the status and the stable error code.
    #[track_caller]
    pub fn assert_error(&self, status: StatusCode, code: &str) {
        assert_eq!(self.status, status, "body: {}", self.text());
        assert_eq!(self.error().code, code, "body: {}", self.text());
    }
}

// --- vault and share payloads -------------------------------------------------

pub fn new_item_id() -> String {
    share_code::new_id().unwrap()
}

/// Seals a name and payload for `id` at `version`, as the CLI does.
pub fn sealed_item(
    vault_key: &Key,
    id: &str,
    name: &str,
    payload: &str,
    version: u32,
) -> (Vec<u8>, Vec<u8>) {
    (
        seal(vault_key, name.as_bytes(), &aad_item_name(id)).unwrap(),
        seal(
            vault_key,
            payload.as_bytes(),
            &aad_item_payload(id, version),
        )
        .unwrap(),
    )
}

pub fn create_item_request(
    vault_key: &Key,
    id: &str,
    name: &str,
    payload: &str,
) -> CreateItemRequest {
    let (name_ct, payload_ct) = sealed_item(vault_key, id, name, payload, 1);
    CreateItemRequest {
        id: id.to_string(),
        name_ct,
        payload_ct,
    }
}

/// A share request for `plaintext` sealed under a fresh share key.
pub fn share_request(
    plaintext: &str,
    ttl_secs: Option<u64>,
    max_views: Option<u32>,
) -> (CreateShareRequest, Key) {
    let share_key = random_key().unwrap();
    let payload_ct = seal(&share_key, plaintext.as_bytes(), AAD_SHARE).unwrap();
    (
        CreateShareRequest {
            payload_ct,
            ttl_secs,
            max_views,
        },
        share_key,
    )
}

// --- misc ----------------------------------------------------------------------

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

pub fn now() -> i64 {
    dotvault_server::db::now()
}

/// Recovery keys derive exactly as the CLI does it after the user types the
/// `DVR1-...` text back in.
pub fn recovery_auth_key_from_text(text: &str) -> (Key, dotvault_core::crypto::RecoveryKeys) {
    let rk = parse_recovery_key(text).unwrap();
    let keys = derive_recovery_keys(&rk).unwrap();
    (rk, keys)
}
