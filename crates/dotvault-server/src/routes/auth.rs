//! Accounts and sessions.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use dotvault_core::{
    api::{
        ChangePasswordRequest, LoginRequest, MeResponse, PreloginResponse, RecoverRequest,
        RecoverUnlockRequest, RecoverUnlockResponse, RegisterRequest, SessionResponse,
    },
    crypto::{KdfParams, SALT_LEN},
};
use serde::Deserialize;

use super::{require_len, NO_CONTENT};
use crate::auth::{
    auth_hash, create_session, fake_salt, hashes_match, recovery_hash, AuthUser, DUMMY_HASH,
};
use crate::config::MAX_WRAPPED_KEY_BYTES;
use crate::db::now;
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::AppState;

const KEY_LEN: usize = 32;
const USERNAME_MIN: usize = 3;
const USERNAME_MAX: usize = 32;

/// Usernames are 3-32 characters of `[A-Za-z0-9_.-]` and are stored
/// lowercased; the returned value is the canonical (lowercase) form.
pub fn validate_username(raw: &str) -> ApiResult<String> {
    let ok_len = (USERNAME_MIN..=USERNAME_MAX).contains(&raw.len());
    let ok_chars = raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if ok_len && ok_chars {
        Ok(raw.to_ascii_lowercase())
    } else {
        Err(ApiError::bad_request(
            "username must be 3-32 characters from A-Z a-z 0-9 _ . -",
        ))
    }
}

fn require_wrapped(field: &str, bytes: &[u8]) -> ApiResult<()> {
    if bytes.is_empty() || bytes.len() > MAX_WRAPPED_KEY_BYTES {
        return Err(ApiError::bad_request(format!(
            "{field} must be 1-{MAX_WRAPPED_KEY_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Salt, KDF parameters, derived auth key and wrapped key: the material that
/// register, password change and recovery all replace.
fn validate_password_material(
    salt: &[u8],
    params: &KdfParams,
    auth_key: &[u8],
    wrapped_vault_key: &[u8],
) -> ApiResult<()> {
    require_len("kdf_salt", salt, SALT_LEN)?;
    params
        .validate()
        .map_err(|_| ApiError::bad_request("kdf_params are outside the accepted range"))?;
    require_len("auth_key", auth_key, KEY_LEN)?;
    require_wrapped("wrapped_vault_key", wrapped_vault_key)
}

fn params_json(params: &KdfParams) -> ApiResult<String> {
    serde_json::to_string(params).map_err(|e| {
        tracing::error!(error = %e, "could not serialize kdf params");
        ApiError::internal()
    })
}

// --- register --------------------------------------------------------------

pub async fn register(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RegisterRequest>,
) -> ApiResult<Json<SessionResponse>> {
    let username = validate_username(&req.username)?;
    validate_password_material(
        &req.kdf_salt,
        &req.kdf_params,
        &req.auth_key,
        &req.wrapped_vault_key,
    )?;
    require_len("recovery_auth_key", &req.recovery_auth_key, KEY_LEN)?;
    require_wrapped(
        "wrapped_vault_key_recovery",
        &req.wrapped_vault_key_recovery,
    )?;

    let params = params_json(&req.kdf_params)?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let inserted: Result<i64, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO users
            (username, auth_hash, kdf_salt, kdf_params, wrapped_vault_key,
             recovery_hash, wrapped_vault_key_recovery, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         RETURNING id",
    )
    .bind(&username)
    .bind(&auth_hash(&req.auth_key)[..])
    .bind(&req.kdf_salt)
    .bind(params)
    .bind(&req.wrapped_vault_key)
    .bind(&recovery_hash(&req.recovery_auth_key)[..])
    .bind(&req.wrapped_vault_key_recovery)
    .bind(now())
    .fetch_one(&mut *tx)
    .await;
    let user_id = match inserted {
        Ok(id) => id,
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::username_taken())
        }
        Err(e) => return Err(e.into()),
    };
    let (token, expires_at) = create_session(&mut *tx, user_id, None).await?;
    tx.commit().await?;

    tracing::info!("account registered");
    Ok(Json(SessionResponse {
        token,
        expires_at,
        wrapped_vault_key: req.wrapped_vault_key,
    }))
}

// --- prelogin --------------------------------------------------------------

#[derive(Deserialize)]
pub struct PreloginQuery {
    username: String,
}

pub async fn prelogin(
    State(state): State<AppState>,
    Query(q): Query<PreloginQuery>,
) -> ApiResult<Json<PreloginResponse>> {
    let username = q.username.to_ascii_lowercase();
    let row: Option<(Vec<u8>, String)> =
        sqlx::query_as("SELECT kdf_salt, kdf_params FROM users WHERE username = ?1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await?;
    match row {
        Some((kdf_salt, params)) => {
            let kdf_params = serde_json::from_str(&params).map_err(|e| {
                tracing::error!(error = %e, "stored kdf params are corrupt");
                ApiError::internal()
            })?;
            Ok(Json(PreloginResponse {
                kdf_salt,
                kdf_params,
            }))
        }
        // Unknown (or malformed) usernames get a stable fake salt, so
        // prelogin cannot be used to enumerate accounts.
        None => Ok(Json(PreloginResponse {
            kdf_salt: fake_salt(&state.server_secret, &username).to_vec(),
            kdf_params: KdfParams::default(),
        })),
    }
}

// --- login / logout / me ---------------------------------------------------

pub async fn login(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<LoginRequest>,
) -> ApiResult<Json<SessionResponse>> {
    require_len("auth_key", &req.auth_key, KEY_LEN)?;
    let username = req.username.to_ascii_lowercase();
    let supplied = auth_hash(&req.auth_key);

    let row: Option<(i64, Vec<u8>, Vec<u8>)> =
        sqlx::query_as("SELECT id, auth_hash, wrapped_vault_key FROM users WHERE username = ?1")
            .bind(&username)
            .fetch_optional(&state.db)
            .await?;

    // Unknown user and wrong password take the same path and give the same
    // answer.
    let Some((user_id, stored, wrapped_vault_key)) = row else {
        let _ = hashes_match(&DUMMY_HASH, &supplied);
        return Err(ApiError::invalid_credentials());
    };
    if !hashes_match(&stored, &supplied) {
        return Err(ApiError::invalid_credentials());
    }

    let (token, expires_at) =
        create_session(&state.db, user_id, req.device_name.as_deref()).await?;
    Ok(Json(SessionResponse {
        token,
        expires_at,
        wrapped_vault_key,
    }))
}

pub async fn logout(State(state): State<AppState>, user: AuthUser) -> ApiResult<StatusCode> {
    sqlx::query("DELETE FROM sessions WHERE id = ?1")
        .bind(user.session_id)
        .execute(&state.db)
        .await?;
    Ok(NO_CONTENT)
}

pub async fn me(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<MeResponse>> {
    let created_at: i64 = sqlx::query_scalar("SELECT created_at FROM users WHERE id = ?1")
        .bind(user.user_id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(MeResponse {
        username: user.username,
        created_at,
    }))
}

// --- password change -------------------------------------------------------

pub async fn change_password(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(req): ApiJson<ChangePasswordRequest>,
) -> ApiResult<StatusCode> {
    require_len("current_auth_key", &req.current_auth_key, KEY_LEN)?;
    validate_password_material(
        &req.new_kdf_salt,
        &req.new_kdf_params,
        &req.new_auth_key,
        &req.new_wrapped_vault_key,
    )?;
    let params = params_json(&req.new_kdf_params)?;

    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let stored: Vec<u8> = sqlx::query_scalar("SELECT auth_hash FROM users WHERE id = ?1")
        .bind(user.user_id)
        .fetch_one(&mut *tx)
        .await?;
    if !hashes_match(&stored, &auth_hash(&req.current_auth_key)) {
        return Err(ApiError::invalid_credentials());
    }
    sqlx::query(
        "UPDATE users
         SET kdf_salt = ?1, kdf_params = ?2, auth_hash = ?3, wrapped_vault_key = ?4
         WHERE id = ?5",
    )
    .bind(&req.new_kdf_salt)
    .bind(params)
    .bind(&auth_hash(&req.new_auth_key)[..])
    .bind(&req.new_wrapped_vault_key)
    .bind(user.user_id)
    .execute(&mut *tx)
    .await?;
    // Every other device has to log in again with the new password.
    sqlx::query("DELETE FROM sessions WHERE user_id = ?1 AND id != ?2")
        .bind(user.user_id)
        .bind(user.session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    tracing::info!("password changed");
    Ok(NO_CONTENT)
}

// --- recovery --------------------------------------------------------------

/// Looks the user up and checks the recovery key in constant time. Unknown
/// user and wrong key are indistinguishable. Returns the user id.
async fn verify_recovery<'e, E>(
    executor: E,
    username: &str,
    recovery_auth_key: &[u8],
) -> ApiResult<(i64, Vec<u8>)>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    require_len("recovery_auth_key", recovery_auth_key, KEY_LEN)?;
    let supplied = recovery_hash(recovery_auth_key);
    let row: Option<(i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT id, recovery_hash, wrapped_vault_key_recovery FROM users WHERE username = ?1",
    )
    .bind(username.to_ascii_lowercase())
    .fetch_optional(executor)
    .await?;
    let Some((id, stored, wrapped)) = row else {
        let _ = hashes_match(&DUMMY_HASH, &supplied);
        return Err(ApiError::invalid_credentials());
    };
    if !hashes_match(&stored, &supplied) {
        return Err(ApiError::invalid_credentials());
    }
    Ok((id, wrapped))
}

pub async fn recover_unlock(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RecoverUnlockRequest>,
) -> ApiResult<Json<RecoverUnlockResponse>> {
    let (_, wrapped) = verify_recovery(&state.db, &req.username, &req.recovery_auth_key).await?;
    Ok(Json(RecoverUnlockResponse {
        wrapped_vault_key_recovery: wrapped,
    }))
}

pub async fn recover(
    State(state): State<AppState>,
    ApiJson(req): ApiJson<RecoverRequest>,
) -> ApiResult<StatusCode> {
    validate_password_material(
        &req.new_kdf_salt,
        &req.new_kdf_params,
        &req.new_auth_key,
        &req.new_wrapped_vault_key,
    )?;
    let params = params_json(&req.new_kdf_params)?;

    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let (user_id, _) = verify_recovery(&mut *tx, &req.username, &req.recovery_auth_key).await?;
    sqlx::query(
        "UPDATE users
         SET kdf_salt = ?1, kdf_params = ?2, auth_hash = ?3, wrapped_vault_key = ?4
         WHERE id = ?5",
    )
    .bind(&req.new_kdf_salt)
    .bind(params)
    .bind(&auth_hash(&req.new_auth_key)[..])
    .bind(&req.new_wrapped_vault_key)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = ?1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    tracing::info!("account recovered with recovery key");
    Ok(NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_rules() {
        for ok in ["abc", "Alice", "a.b-c_d", "x".repeat(32).as_str(), "A1_"] {
            assert!(validate_username(ok).is_ok(), "{ok}");
        }
        assert_eq!(validate_username("MiXeD.Case").unwrap(), "mixed.case");

        for bad in [
            "",
            "ab",
            &"x".repeat(33),
            "has space",
            "semi;colon",
            "tab\t",
            "new\nline",
            "emoji\u{1F600}",
            "\u{e9}clair",
            "slash/y",
            "at@sign",
            "nul\0",
        ] {
            let err = validate_username(bad).unwrap_err();
            assert_eq!(err.status, StatusCode::BAD_REQUEST, "{bad:?}");
        }
    }

    #[test]
    fn password_material_validation() {
        let ok_params = KdfParams {
            m_kib: 19 * 1024,
            t: 2,
            p: 1,
        };
        let salt = [0u8; SALT_LEN];
        let key = [0u8; KEY_LEN];
        let wrapped = [0u8; 73];
        assert!(validate_password_material(&salt, &ok_params, &key, &wrapped).is_ok());
        assert!(validate_password_material(&salt[..15], &ok_params, &key, &wrapped).is_err());
        assert!(validate_password_material(&salt, &ok_params, &key[..31], &wrapped).is_err());
        assert!(validate_password_material(&salt, &ok_params, &key, &[]).is_err());
        assert!(validate_password_material(&salt, &ok_params, &key, &[0u8; 257]).is_err());
        let weak = KdfParams {
            m_kib: 1024,
            ..ok_params
        };
        assert!(validate_password_material(&salt, &weak, &key, &wrapped).is_err());
    }
}
