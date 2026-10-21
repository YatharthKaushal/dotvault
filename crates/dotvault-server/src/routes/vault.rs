//! The encrypted vault. Every query is scoped by `user_id`, so another
//! user's item is indistinguishable from one that does not exist (404).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use dotvault_core::api::{
    CreateItemRequest, ItemDetail, ItemSummary, ListItemsResponse, UpdateItemRequest,
};

use super::{require_blob, NO_CONTENT};
use crate::auth::AuthUser;
use crate::config::{KEEP_ITEM_VERSIONS, MAX_ITEM_NAME_BYTES, MAX_ITEM_PAYLOAD_BYTES};
use crate::db::now;
use crate::error::{ApiError, ApiJson, ApiResult};
use crate::AppState;

const ITEM_ID_LEN: usize = 22;

/// Client-chosen ids are 22 base64url characters (a 128-bit random value).
pub fn validate_item_id(id: &str) -> ApiResult<()> {
    let ok = id.len() == ITEM_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "item id must be 22 base64url characters",
        ))
    }
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    id: String,
    name_ct: Vec<u8>,
    version: i64,
    updated_at: i64,
}

#[derive(sqlx::FromRow)]
struct DetailRow {
    id: String,
    name_ct: Vec<u8>,
    payload_ct: Vec<u8>,
    version: i64,
    created_at: i64,
    updated_at: i64,
}

impl From<DetailRow> for ItemDetail {
    fn from(r: DetailRow) -> Self {
        Self {
            id: r.id,
            name_ct: r.name_ct,
            payload_ct: r.payload_ct,
            version: r.version as u32,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<ListItemsResponse>> {
    let rows: Vec<SummaryRow> = sqlx::query_as(
        "SELECT id, name_ct, version, updated_at FROM vault_items
         WHERE user_id = ?1 AND deleted_at IS NULL
         ORDER BY created_at, id",
    )
    .bind(user.user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(ListItemsResponse {
        items: rows
            .into_iter()
            .map(|r| ItemSummary {
                id: r.id,
                name_ct: r.name_ct,
                version: r.version as u32,
                updated_at: r.updated_at,
            })
            .collect(),
    }))
}

async fn fetch_detail(state: &AppState, user: &AuthUser, id: &str) -> ApiResult<Option<DetailRow>> {
    Ok(sqlx::query_as(
        "SELECT i.id, i.name_ct, v.payload_ct, i.version, i.created_at, i.updated_at
         FROM vault_items i
         JOIN vault_item_versions v
           ON v.user_id = i.user_id AND v.item_id = i.id AND v.version = i.version
         WHERE i.user_id = ?1 AND i.id = ?2 AND i.deleted_at IS NULL",
    )
    .bind(user.user_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?)
}

pub async fn get_one(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ItemDetail>> {
    let row = fetch_detail(&state, &user, &id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(row.into()))
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(req): ApiJson<CreateItemRequest>,
) -> ApiResult<(StatusCode, Json<ItemDetail>)> {
    validate_item_id(&req.id)?;
    require_blob("name_ct", &req.name_ct, MAX_ITEM_NAME_BYTES)?;
    require_blob("payload_ct", &req.payload_ct, MAX_ITEM_PAYLOAD_BYTES)?;

    let now = now();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let inserted = sqlx::query(
        "INSERT INTO vault_items (user_id, id, name_ct, version, created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, ?4, ?4)",
    )
    .bind(user.user_id)
    .bind(&req.id)
    .bind(&req.name_ct)
    .bind(now)
    .execute(&mut *tx)
    .await;
    match inserted {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(ApiError::conflict("an item with this id already exists"))
        }
        Err(e) => return Err(e.into()),
    }
    sqlx::query(
        "INSERT INTO vault_item_versions (user_id, item_id, version, payload_ct, created_at)
         VALUES (?1, ?2, 1, ?3, ?4)",
    )
    .bind(user.user_id)
    .bind(&req.id)
    .bind(&req.payload_ct)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((
        StatusCode::CREATED,
        Json(ItemDetail {
            id: req.id,
            name_ct: req.name_ct,
            payload_ct: req.payload_ct,
            version: 1,
            created_at: now,
            updated_at: now,
        }),
    ))
}

pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<UpdateItemRequest>,
) -> ApiResult<Json<ItemDetail>> {
    require_blob("name_ct", &req.name_ct, MAX_ITEM_NAME_BYTES)?;
    require_blob("payload_ct", &req.payload_ct, MAX_ITEM_PAYLOAD_BYTES)?;

    let now = now();
    let new_version = i64::from(req.version);
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;

    // Optimistic concurrency: only succeeds if the client saw the current
    // version, i.e. is writing exactly current + 1.
    let created_at: Option<i64> = sqlx::query_scalar(
        "UPDATE vault_items SET name_ct = ?1, version = ?2, updated_at = ?3
         WHERE user_id = ?4 AND id = ?5 AND deleted_at IS NULL AND version = ?2 - 1
         RETURNING created_at",
    )
    .bind(&req.name_ct)
    .bind(new_version)
    .bind(now)
    .bind(user.user_id)
    .bind(&id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(created_at) = created_at else {
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM vault_items
             WHERE user_id = ?1 AND id = ?2 AND deleted_at IS NULL",
        )
        .bind(user.user_id)
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?;
        return Err(match current {
            None => ApiError::not_found(),
            Some(v) => ApiError::conflict(format!(
                "version conflict: the item is at version {v}, so the next version is {}",
                v + 1
            )),
        });
    };

    sqlx::query(
        "INSERT INTO vault_item_versions (user_id, item_id, version, payload_ct, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(user.user_id)
    .bind(&id)
    .bind(new_version)
    .bind(&req.payload_ct)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    // Keep the newest KEEP_ITEM_VERSIONS versions.
    sqlx::query(
        "DELETE FROM vault_item_versions
         WHERE user_id = ?1 AND item_id = ?2 AND version <= ?3",
    )
    .bind(user.user_id)
    .bind(&id)
    .bind(new_version - KEEP_ITEM_VERSIONS)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(ItemDetail {
        id,
        name_ct: req.name_ct,
        payload_ct: req.payload_ct,
        version: req.version,
        created_at,
        updated_at: now,
    }))
}

pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let now = now();
    let done = sqlx::query(
        "UPDATE vault_items SET deleted_at = ?1, updated_at = ?1
         WHERE user_id = ?2 AND id = ?3 AND deleted_at IS NULL",
    )
    .bind(now)
    .bind(user.user_id)
    .bind(&id)
    .execute(&state.db)
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_id_validation() {
        assert!(validate_item_id("AAAAAAAAAAAAAAAAAAAAAA").is_ok());
        assert!(validate_item_id("abc-_ABC012345678901-_").is_ok());
        for bad in [
            "",
            "short",
            "AAAAAAAAAAAAAAAAAAAAAAA",   // 23
            "AAAAAAAAAAAAAAAAAAAAA",     // 21
            "AAAAAAAAAAAAAAAAAAAAA=",    // padding
            "AAAAAAAAAAAAAAAAAAAA/A",    // not url-safe
            "AAAAAAAAAAAAAAAAAAAA+A",    // not url-safe
            "AAAAAAAAAAAAAAAAAAAA A",    // space
            "AAAAAAAAAAAAAAAAAAA\u{e9}", // multi-byte
        ] {
            assert!(validate_item_id(bad).is_err(), "{bad:?}");
        }
    }
}
