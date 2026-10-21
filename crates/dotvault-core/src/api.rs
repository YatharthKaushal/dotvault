//! Request and response bodies of the HTTP API (`/v1`), shared by the CLI and
//! the server. Binary fields travel as unpadded base64url strings; times are
//! unix seconds.

use serde::{Deserialize, Serialize};

use crate::crypto::KdfParams;

pub const API_PREFIX: &str = "/v1";

// Limits enforced by the server. They live here so the CLI can reject a
// request with a clear message instead of a round trip to a 4xx.

/// Largest sealed vault item payload.
pub const MAX_ITEM_PAYLOAD_BYTES: usize = 256 * 1024;
/// Largest sealed vault item name.
pub const MAX_ITEM_NAME_BYTES: usize = 4 * 1024;
/// Largest sealed share payload.
pub const MAX_SHARE_PAYLOAD_BYTES: usize = 64 * 1024;
pub const SHARE_MIN_TTL_SECS: u64 = 60;
pub const SHARE_MAX_TTL_SECS: u64 = 30 * 24 * 3600;
pub const SHARE_MAX_VIEWS: u32 = 1000;

/// Serde adapter: `Vec<u8>` <-> base64url string.
pub mod b64 {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        URL_SAFE_NO_PAD.decode(s).map_err(serde::de::Error::custom)
    }
}

// --- errors ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Stable machine-readable code, e.g. `username_taken`.
    pub code: String,
    pub message: String,
}

// --- auth ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    #[serde(with = "b64")]
    pub kdf_salt: Vec<u8>,
    pub kdf_params: KdfParams,
    #[serde(with = "b64")]
    pub auth_key: Vec<u8>,
    #[serde(with = "b64")]
    pub wrapped_vault_key: Vec<u8>,
    #[serde(with = "b64")]
    pub recovery_auth_key: Vec<u8>,
    #[serde(with = "b64")]
    pub wrapped_vault_key_recovery: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreloginResponse {
    #[serde(with = "b64")]
    pub kdf_salt: Vec<u8>,
    pub kdf_params: KdfParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    #[serde(with = "b64")]
    pub auth_key: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

/// Returned by both register and login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    pub token: String,
    pub expires_at: i64,
    #[serde(with = "b64")]
    pub wrapped_vault_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeResponse {
    pub username: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    #[serde(with = "b64")]
    pub current_auth_key: Vec<u8>,
    #[serde(with = "b64")]
    pub new_kdf_salt: Vec<u8>,
    pub new_kdf_params: KdfParams,
    #[serde(with = "b64")]
    pub new_auth_key: Vec<u8>,
    #[serde(with = "b64")]
    pub new_wrapped_vault_key: Vec<u8>,
}

/// Recovery step 1: prove possession of the recovery key, receive the vault
/// key wrapped under it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverUnlockRequest {
    pub username: String,
    #[serde(with = "b64")]
    pub recovery_auth_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverUnlockResponse {
    #[serde(with = "b64")]
    pub wrapped_vault_key_recovery: Vec<u8>,
}

/// Recovery step 2: set a new password. Ends every existing session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverRequest {
    pub username: String,
    #[serde(with = "b64")]
    pub recovery_auth_key: Vec<u8>,
    #[serde(with = "b64")]
    pub new_kdf_salt: Vec<u8>,
    pub new_kdf_params: KdfParams,
    #[serde(with = "b64")]
    pub new_auth_key: Vec<u8>,
    #[serde(with = "b64")]
    pub new_wrapped_vault_key: Vec<u8>,
}

// --- vault -----------------------------------------------------------------

/// `id` is chosen by the client because it is part of the encryption context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateItemRequest {
    pub id: String,
    #[serde(with = "b64")]
    pub name_ct: Vec<u8>,
    #[serde(with = "b64")]
    pub payload_ct: Vec<u8>,
}

/// `version` is the new version number (current + 1); the server rejects the
/// update with a conflict if it is not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateItemRequest {
    #[serde(with = "b64")]
    pub name_ct: Vec<u8>,
    #[serde(with = "b64")]
    pub payload_ct: Vec<u8>,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemSummary {
    pub id: String,
    #[serde(with = "b64")]
    pub name_ct: Vec<u8>,
    pub version: u32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListItemsResponse {
    pub items: Vec<ItemSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDetail {
    pub id: String,
    #[serde(with = "b64")]
    pub name_ct: Vec<u8>,
    #[serde(with = "b64")]
    pub payload_ct: Vec<u8>,
    pub version: u32,
    pub created_at: i64,
    pub updated_at: i64,
}

// --- shares ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShareRequest {
    #[serde(with = "b64")]
    pub payload_ct: Vec<u8>,
    /// Lifetime in seconds. The server applies a default and a ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_views: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShareResponse {
    pub id: String,
    pub expires_at: i64,
}

/// Opening a share counts as a view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenShareResponse {
    pub id: String,
    pub owner: String,
    #[serde(with = "b64")]
    pub payload_ct: Vec<u8>,
    pub created_at: i64,
    pub expires_at: i64,
    /// Views left after this one, when a limit was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub views_left: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareSummary {
    pub id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub max_views: Option<u32>,
    pub view_count: u32,
    pub revoked: bool,
    pub expired: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSharesResponse {
    pub shares: Vec<ShareSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareAction {
    View,
    Save,
}

/// Clients may only report `save`; `view` is recorded by the server on open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportEventRequest {
    pub action: ShareAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareEvent {
    pub user: String,
    pub action: ShareAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareEventsResponse {
    pub events: Vec<ShareEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_fields_are_base64url_strings() {
        let req = LoginRequest {
            username: "ada".into(),
            auth_key: vec![0xfb, 0xff, 0xfe],
            device_name: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"username":"ada","auth_key":"-__-"}"#);
        let back: LoginRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.auth_key, vec![0xfb, 0xff, 0xfe]);
    }

    #[test]
    fn rejects_bad_base64() {
        let r: Result<LoginRequest, _> =
            serde_json::from_str(r#"{"username":"a","auth_key":"not base64!!"}"#);
        assert!(r.is_err());
    }

    #[test]
    fn share_action_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&ShareAction::View).unwrap(),
            r#""view""#
        );
        let a: ShareAction = serde_json::from_str(r#""save""#).unwrap();
        assert_eq!(a, ShareAction::Save);
    }
}
