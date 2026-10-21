//! Share links. The payload is opaque ciphertext sealed under a key that only
//! ever lives in the link fragment; the server enforces expiry, view limits
//! and revocation, and keeps an audit log of who opened what.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    Json,
};
use dotvault_core::{
    api::{
        CreateShareRequest, CreateShareResponse, ListSharesResponse, OpenShareResponse,
        ReportEventRequest, ShareAction, ShareEvent, ShareEventsResponse, ShareSummary,
    },
    share_code,
};

use super::{require_blob, truncate_chars, NO_CONTENT};
use crate::auth::AuthUser;
use crate::config::{
    MAX_SHARE_PAYLOAD_BYTES, SHARE_DEFAULT_TTL_SECS, SHARE_MAX_TTL_SECS, SHARE_MAX_VIEWS,
    SHARE_MIN_TTL_SECS,
};
use crate::db::now;
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::AppState;

const CLIENT_MAX_CHARS: usize = 128;

/// Resolves the requested lifetime: default when absent, clamped otherwise.
pub fn effective_ttl(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(SHARE_DEFAULT_TTL_SECS)
        .clamp(SHARE_MIN_TTL_SECS, SHARE_MAX_TTL_SECS)
}

fn client_label(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| truncate_chars(s, CLIENT_MAX_CHARS))
        .filter(|s| !s.is_empty())
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(req): ApiJson<CreateShareRequest>,
) -> ApiResult<(StatusCode, Json<CreateShareResponse>)> {
    require_blob("payload_ct", &req.payload_ct, MAX_SHARE_PAYLOAD_BYTES)?;
    if let Some(n) = req.max_views {
        if !(1..=SHARE_MAX_VIEWS).contains(&n) {
            return Err(ApiError::bad_request(format!(
                "max_views must be between 1 and {SHARE_MAX_VIEWS}"
            )));
        }
    }

    let id = share_code::new_id()?;
    let now = now();
    let expires_at = now + effective_ttl(req.ttl_secs) as i64;
    sqlx::query(
        "INSERT INTO shares (id, owner_id, payload_ct, expires_at, max_views, view_count, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)",
    )
    .bind(&id)
    .bind(user.user_id)
    .bind(&req.payload_ct)
    .bind(expires_at)
    .bind(req.max_views.map(i64::from))
    .bind(now)
    .execute(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(CreateShareResponse { id, expires_at }),
    ))
}

#[derive(sqlx::FromRow)]
struct OpenedRow {
    owner_id: i64,
    payload_ct: Option<Vec<u8>>,
    created_at: i64,
    expires_at: i64,
    max_views: Option<i64>,
    view_count: i64,
}

/// Opening a share is a view: the check and the increment are one
/// statement, so concurrent opens can never exceed `max_views`.
pub async fn open(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<OpenShareResponse>> {
    let now = now();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;

    let opened: Option<OpenedRow> = sqlx::query_as(
        "UPDATE shares SET view_count = view_count + 1
         WHERE id = ?1
           AND revoked_at IS NULL
           AND payload_ct IS NOT NULL
           AND expires_at > ?2
           AND (max_views IS NULL OR view_count < max_views)
         RETURNING owner_id, payload_ct, created_at, expires_at, max_views, view_count",
    )
    .bind(&id)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = opened else {
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM shares WHERE id = ?1")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
        return Err(if exists.is_some() {
            ApiError::share_gone()
        } else {
            ApiError::not_found()
        });
    };
    let payload_ct = row.payload_ct.ok_or_else(ApiError::share_gone)?;

    sqlx::query(
        "INSERT INTO share_events (share_id, user_id, action, client, created_at)
         VALUES (?1, ?2, 'view', ?3, ?4)",
    )
    .bind(&id)
    .bind(user.user_id)
    .bind(client_label(&headers))
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let owner: String = sqlx::query_scalar("SELECT username FROM users WHERE id = ?1")
        .bind(row.owner_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(OpenShareResponse {
        id,
        owner,
        payload_ct,
        created_at: row.created_at,
        expires_at: row.expires_at,
        views_left: row
            .max_views
            .map(|max| (max - row.view_count).max(0) as u32),
    }))
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: String,
    created_at: i64,
    expires_at: i64,
    max_views: Option<i64>,
    view_count: i64,
    revoked_at: Option<i64>,
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<ListSharesResponse>> {
    let now = now();
    let rows: Vec<SummaryRow> = sqlx::query_as(
        "SELECT id, created_at, expires_at, max_views, view_count, revoked_at
         FROM shares WHERE owner_id = ?1
         ORDER BY created_at DESC, rowid DESC
         LIMIT 1000",
    )
    .bind(user.user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(ListSharesResponse {
        shares: rows
            .into_iter()
            .map(|r| ShareSummary {
                id: r.id,
                created_at: r.created_at,
                expires_at: r.expires_at,
                max_views: r.max_views.map(|v| v as u32),
                view_count: r.view_count as u32,
                revoked: r.revoked_at.is_some(),
                expired: r.expires_at <= now,
            })
            .collect(),
    }))
}

/// Deletes the ciphertext but keeps the row and its events.
pub async fn revoke(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let done = sqlx::query(
        "UPDATE shares SET revoked_at = COALESCE(revoked_at, ?1), payload_ct = NULL
         WHERE id = ?2 AND owner_id = ?3",
    )
    .bind(now())
    .bind(&id)
    .bind(user.user_id)
    .execute(&state.db)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(NO_CONTENT)
}

#[derive(sqlx::FromRow)]
struct EventRow {
    username: String,
    action: String,
    client: Option<String>,
    created_at: i64,
}

pub async fn events(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ShareEventsResponse>> {
    let owned: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM shares WHERE id = ?1 AND owner_id = ?2")
            .bind(&id)
            .bind(user.user_id)
            .fetch_optional(&state.db)
            .await?;
    if owned.is_none() {
        return Err(ApiError::not_found());
    }
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT u.username, e.action, e.client, e.created_at
         FROM share_events e JOIN users u ON u.id = e.user_id
         WHERE e.share_id = ?1
         ORDER BY e.id",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(ShareEventsResponse {
        events: rows
            .into_iter()
            .map(|r| ShareEvent {
                user: r.username,
                action: if r.action == "save" {
                    ShareAction::Save
                } else {
                    ShareAction::View
                },
                client: r.client,
                created_at: r.created_at,
            })
            .collect(),
    }))
}

/// Clients report `save` after re-encrypting a share into their own vault.
/// The server cannot verify that, so it only checks the reporter actually
/// opened the share.
pub async fn report_event(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    headers: HeaderMap,
    ApiJson(req): ApiJson<ReportEventRequest>,
) -> ApiResult<StatusCode> {
    if req.action != ShareAction::Save {
        return Err(ApiError::bad_request(
            "only the `save` action can be reported; views are recorded by the server",
        ));
    }
    let done = sqlx::query(
        "INSERT INTO share_events (share_id, user_id, action, client, created_at)
         SELECT ?1, ?2, 'save', ?3, ?4
         WHERE EXISTS (
             SELECT 1 FROM share_events
             WHERE share_id = ?1 AND user_id = ?2 AND action = 'view'
         )",
    )
    .bind(&id)
    .bind(user.user_id)
    .bind(client_label(&headers))
    .bind(now())
    .execute(&state.db)
    .await?;
    if done.rows_affected() == 1 {
        return Ok(NO_CONTENT);
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM shares WHERE id = ?1")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?;
    Err(if exists.is_some() {
        ApiError::forbidden("you can only report a save for a share you have opened")
    } else {
        ApiError::not_found()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_defaults_and_clamps() {
        assert_eq!(effective_ttl(None), 24 * 3600);
        assert_eq!(effective_ttl(Some(0)), 60);
        assert_eq!(effective_ttl(Some(59)), 60);
        assert_eq!(effective_ttl(Some(60)), 60);
        assert_eq!(effective_ttl(Some(3600)), 3600);
        assert_eq!(effective_ttl(Some(30 * 24 * 3600)), 30 * 24 * 3600);
        assert_eq!(effective_ttl(Some(30 * 24 * 3600 + 1)), 30 * 24 * 3600);
        assert_eq!(effective_ttl(Some(u64::MAX)), 30 * 24 * 3600);
    }
}
