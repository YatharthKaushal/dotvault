//! Cross-cutting HTTP middleware: rate limits, JSON-ified framework errors and
//! the access log.

use std::net::SocketAddr;
use std::time::Instant;

use axum::{
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::error::ApiError;
use crate::ratelimit::{client_ip, RateLimiter};
use crate::AppState;

async fn limit(limiter: &RateLimiter, trust_proxy: bool, req: Request, next: Next) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let ip = client_ip(trust_proxy, req.headers(), peer);
    match limiter.check(ip) {
        Ok(()) => next.run(req).await,
        Err(wait) => {
            let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
            tracing::warn!(%ip, retry_after_secs = secs, "rate limit exceeded");
            ApiError::rate_limited(secs.max(1)).into_response()
        }
    }
}

/// register, prelogin, login and recover/*.
pub async fn rate_limit_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    limit(&state.auth_limiter, state.config.trust_proxy, req, next).await
}

/// Share open.
pub async fn rate_limit_share_open(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    limit(
        &state.share_open_limiter,
        state.config.trust_proxy,
        req,
        next,
    )
    .await
}

/// Rewrites error responses produced by the framework itself (unknown route,
/// wrong method, unreadable path parameter, ...) into the API's JSON error
/// body, so clients only ever have to parse one error format.
pub async fn json_errors(resp: Response) -> Response {
    let status = resp.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return resp;
    }
    let is_json = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if is_json {
        return resp;
    }
    let (code, message) = match status {
        StatusCode::NOT_FOUND => ("not_found", "not found"),
        StatusCode::METHOD_NOT_ALLOWED => ("method_not_allowed", "method not allowed"),
        StatusCode::PAYLOAD_TOO_LARGE => ("payload_too_large", "request body is too large"),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => (
            "unsupported_media_type",
            "expected Content-Type: application/json",
        ),
        s if s.is_server_error() => ("internal", "internal server error"),
        _ => ("bad_request", "bad request"),
    };
    let allow = resp.headers().get(header::ALLOW).cloned();
    let mut out = ApiError::new(status, code, message).into_response();
    if let Some(allow) = allow {
        out.headers_mut().insert(header::ALLOW, allow);
    }
    out
}

/// One line per request: method, route template (never the real path, which
/// can contain ids), status and latency. No headers, no bodies, no query.
pub async fn access_log(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "<unmatched>".to_string(), |m| m.as_str().to_string());
    let start = Instant::now();
    let resp = next.run(req).await;
    let status = resp.status().as_u16();
    let latency_ms = start.elapsed().as_millis() as u64;
    if route == "/healthz" {
        tracing::debug!(%method, %route, status, latency_ms, "request");
    } else {
        tracing::info!(%method, %route, status, latency_ms, "request");
    }
    resp
}
