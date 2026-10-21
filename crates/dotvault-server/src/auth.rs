//! Credential hashing, session tokens and the [`AuthUser`] extractor.
//!
//! What the server keeps of each secret:
//!
//! * `auth_key` / `recovery_auth_key` are already 256-bit outputs of a KDF
//!   (Argon2id then HKDF, computed on the client), so a fast salted-by-domain
//!   SHA-256 is the right storage hash: there is nothing to brute-force.
//! * session tokens are 32 random bytes; only their SHA-256 is stored.

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderMap},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use sqlx::{Executor, Sqlite};
use subtle::ConstantTimeEq;

use crate::config::{SESSION_TOUCH_INTERVAL_SECS, SESSION_TTL_SECS};
use crate::db::now;
use crate::error::ApiError;
use crate::AppState;

const AUTH_HASH_DOMAIN: &[u8] = b"dotvault/v1/auth-hash";
const RECOVERY_HASH_DOMAIN: &[u8] = b"dotvault/v1/recovery-hash";
pub const SESSION_TOKEN_LEN: usize = 32;

pub type Hash = [u8; 32];

fn domain_hash(domain: &[u8], secret: &[u8]) -> Hash {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(secret);
    h.finalize().into()
}

/// The value stored in `users.auth_hash`.
pub fn auth_hash(auth_key: &[u8]) -> Hash {
    domain_hash(AUTH_HASH_DOMAIN, auth_key)
}

/// The value stored in `users.recovery_hash`.
pub fn recovery_hash(recovery_auth_key: &[u8]) -> Hash {
    domain_hash(RECOVERY_HASH_DOMAIN, recovery_auth_key)
}

/// Constant-time comparison of a stored hash with a freshly computed one.
pub fn hashes_match(stored: &[u8], computed: &Hash) -> bool {
    stored.ct_eq(computed.as_slice()).into()
}

/// What a login attempt for an unknown user is compared against, so that the
/// unknown-user path does the same work as the wrong-password path.
pub const DUMMY_HASH: Hash = [0u8; 32];

/// The salt handed out by prelogin for a username that does not exist:
/// the first 16 bytes of `HMAC-SHA256(server_secret, lowercase(username))`.
/// Deterministic, so it cannot be told apart from a real salt by asking twice.
pub fn fake_salt(server_secret: &[u8], username: &str) -> [u8; 16] {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(server_secret)
        .expect("HMAC accepts keys of any length");
    mac.update(username.to_ascii_lowercase().as_bytes());
    let tag = mac.finalize().into_bytes();
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&tag[..16]);
    salt
}

pub fn token_hash(token: &[u8]) -> Hash {
    Sha256::digest(token).into()
}

/// Decodes a bearer token string into its raw bytes, if well formed.
pub fn decode_token(token: &str) -> Option<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD.decode(token.trim()).ok()?;
    (bytes.len() == SESSION_TOKEN_LEN).then_some(bytes)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

/// Inserts a new session and returns `(token, expires_at)`.
pub async fn create_session<'e, E>(
    executor: E,
    user_id: i64,
    device_name: Option<&str>,
) -> Result<(String, i64), ApiError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let raw = dotvault_core::crypto::random_bytes::<SESSION_TOKEN_LEN>()?;
    let token = URL_SAFE_NO_PAD.encode(raw);
    let now = now();
    let expires_at = now + SESSION_TTL_SECS;
    let device: Option<String> = device_name
        .map(|d| d.chars().take(64).collect::<String>())
        .filter(|d| !d.is_empty());
    sqlx::query(
        "INSERT INTO sessions (user_id, token_hash, device_name, created_at, last_used_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
    )
    .bind(user_id)
    .bind(&token_hash(&raw)[..])
    .bind(device)
    .bind(now)
    .bind(expires_at)
    .execute(executor)
    .await?;
    Ok((token, expires_at))
}

/// The authenticated caller. Extracting it checks the bearer token against
/// the sessions table, so adding it to a handler is what makes a route
/// require login.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub username: String,
    pub session_id: i64,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: i64,
    user_id: i64,
    username: String,
    last_used_at: i64,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer_token(&parts.headers).ok_or_else(ApiError::unauthorized)?;
        let raw = decode_token(token).ok_or_else(ApiError::unauthorized)?;
        let hash = token_hash(&raw);
        let now = now();

        let row: Option<SessionRow> = sqlx::query_as(
            "SELECT s.id, s.user_id, u.username, s.last_used_at
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > ?2",
        )
        .bind(&hash[..])
        .bind(now)
        .fetch_optional(&state.db)
        .await?;
        let row = row.ok_or_else(ApiError::unauthorized)?;

        // Sliding expiry, written at most once per interval.
        if now - row.last_used_at >= SESSION_TOUCH_INTERVAL_SECS {
            let touched =
                sqlx::query("UPDATE sessions SET last_used_at = ?1, expires_at = ?2 WHERE id = ?3")
                    .bind(now)
                    .bind(now + SESSION_TTL_SECS)
                    .bind(row.id)
                    .execute(&state.db)
                    .await;
            if let Err(e) = touched {
                // Not worth failing the request over.
                tracing::warn!(error = %e, "could not extend session");
            }
        }

        Ok(Self {
            user_id: row.user_id,
            username: row.username,
            session_id: row.id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_domain_separated_and_deterministic() {
        let key = [7u8; 32];
        assert_eq!(auth_hash(&key), auth_hash(&key));
        assert_ne!(auth_hash(&key), recovery_hash(&key));
        assert_ne!(auth_hash(&key), auth_hash(&[8u8; 32]));
        // never the key itself
        assert_ne!(&auth_hash(&key)[..], &key[..]);
    }

    #[test]
    fn hash_comparison() {
        let h = auth_hash(&[1u8; 32]);
        assert!(hashes_match(&h, &h));
        assert!(!hashes_match(&h, &auth_hash(&[2u8; 32])));
        assert!(
            !hashes_match(&h[..31], &h),
            "length mismatch must not match"
        );
        assert!(!hashes_match(&[], &h));
    }

    #[test]
    fn fake_salt_is_stable_per_username_and_secret() {
        let s = fake_salt(b"secret-one", "Alice");
        assert_eq!(s, fake_salt(b"secret-one", "alice"), "case-insensitive");
        assert_eq!(s, fake_salt(b"secret-one", "ALICE"));
        assert_ne!(s, fake_salt(b"secret-one", "bob"));
        assert_ne!(s, fake_salt(b"secret-two", "alice"));
    }

    #[test]
    fn token_decoding() {
        let raw = [9u8; 32];
        let t = URL_SAFE_NO_PAD.encode(raw);
        assert_eq!(decode_token(&t).as_deref(), Some(&raw[..]));
        assert!(decode_token("short").is_none());
        assert!(decode_token(&URL_SAFE_NO_PAD.encode([1u8; 31])).is_none());
        assert!(decode_token("!!!!").is_none());
    }

    #[test]
    fn bearer_parsing() {
        let mut h = HeaderMap::new();
        assert!(bearer_token(&h).is_none());
        h.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(bearer_token(&h), Some("abc"));
        h.insert(header::AUTHORIZATION, "bearer abc".parse().unwrap());
        assert_eq!(bearer_token(&h), Some("abc"));
        h.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert!(bearer_token(&h).is_none());
        h.insert(header::AUTHORIZATION, "Bearer".parse().unwrap());
        assert!(bearer_token(&h).is_none());
    }
}
