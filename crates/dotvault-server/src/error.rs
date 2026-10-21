//! The API error type. Every error leaves the server as a JSON
//! [`dotvault_core::api::ErrorBody`] with a stable `code`.

use std::borrow::Cow;

use axum::{
    extract::{rejection::JsonRejection, FromRequest, Request},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use dotvault_core::api::ErrorBody;
use serde::de::DeserializeOwned;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: Cow<'static, str>,
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing, invalid or expired session token",
        )
    }

    /// Used for every failed credential check, whatever the reason, so the
    /// response never reveals whether the user exists.
    pub fn invalid_credentials() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "invalid username or password",
        )
    }

    pub fn username_taken() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "username_taken",
            "that username is already taken",
        )
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }

    pub fn conflict(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn share_gone() -> Self {
        Self::new(
            StatusCode::GONE,
            "share_gone",
            "this share was revoked, has expired or has reached its view limit",
        )
    }

    pub fn payload_too_large(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", message)
    }

    pub fn rate_limited(retry_after_secs: u64) -> Self {
        let mut e = Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests, slow down",
        );
        e.retry_after = Some(retry_after_secs);
        e
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            code: self.code.to_string(),
            message: self.message.into_owned(),
        };
        let mut resp = (self.status, Json(body)).into_response();
        if let Some(secs) = self.retry_after {
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        resp
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        // sqlx errors describe the failure, never the bound values.
        tracing::error!(error = %e, "database error");
        Self::internal()
    }
}

impl From<dotvault_core::Error> for ApiError {
    fn from(e: dotvault_core::Error) -> Self {
        tracing::error!(error = %e, "core error");
        Self::internal()
    }
}

/// `Json<T>` whose rejections are [`ApiError`]s.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(Self(v)),
            Err(rej) => Err(match rej.status() {
                StatusCode::PAYLOAD_TOO_LARGE => {
                    ApiError::payload_too_large("request body is too large")
                }
                StatusCode::UNSUPPORTED_MEDIA_TYPE => ApiError::new(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "unsupported_media_type",
                    "expected Content-Type: application/json",
                ),
                _ => ApiError::bad_request(describe_json_rejection(&rej)),
            }),
        }
    }
}

fn describe_json_rejection(rej: &JsonRejection) -> String {
    // The text names the offending field and the serde error; it does not
    // contain the request body.
    rej.body_text()
}
