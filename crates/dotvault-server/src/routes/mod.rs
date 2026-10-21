//! HTTP handlers.

use axum::http::StatusCode;

use crate::error::{ApiError, ApiResult};

pub mod auth;
pub mod landing;
pub mod shares;
pub mod vault;

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn not_found() -> ApiError {
    ApiError::not_found()
}

/// Response of endpoints that have nothing to return.
pub(crate) const NO_CONTENT: StatusCode = StatusCode::NO_CONTENT;

/// A binary field that must be exactly `len` bytes.
pub(crate) fn require_len(field: &str, bytes: &[u8], len: usize) -> ApiResult<()> {
    if bytes.len() == len {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "{field} must be {len} bytes"
        )))
    }
}

/// A ciphertext field: non-empty and at most `max` bytes (413 above that).
pub(crate) fn require_blob(field: &str, bytes: &[u8], max: usize) -> ApiResult<()> {
    if bytes.is_empty() {
        return Err(ApiError::bad_request(format!("{field} must not be empty")));
    }
    if bytes.len() > max {
        return Err(ApiError::payload_too_large(format!(
            "{field} is larger than {max} bytes"
        )));
    }
    Ok(())
}

/// Client-supplied text kept for the audit log: cut to `max` characters.
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_limits() {
        assert!(require_blob("x", &[1], 4).is_ok());
        assert!(require_blob("x", &[1; 4], 4).is_ok());
        assert_eq!(
            require_blob("x", &[], 4).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            require_blob("x", &[1; 5], 4).unwrap_err().status,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[test]
    fn exact_lengths() {
        assert!(require_len("k", &[0; 32], 32).is_ok());
        assert!(require_len("k", &[0; 31], 32).is_err());
        assert!(require_len("k", &[0; 33], 32).is_err());
    }

    #[test]
    fn truncation_counts_characters() {
        assert_eq!(truncate_chars("abcdef", 3), "abc");
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("ab", 5), "ab");
    }
}
