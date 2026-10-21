//! dotvault server: account sessions, an end-to-end encrypted vault and
//! expiring share links.
//!
//! The server only ever stores ciphertext and hashes of derived secrets. It
//! never receives a password, a vault key or a share key, so it has nothing
//! it could decrypt.
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use dotvault_server::{app, connect, AppState, Config};
//!
//! let config = Config::from_env()?;
//! let pool = connect(&config.db).await?;
//! let state = AppState::new(pool, config).await?;
//! let router = app(state); // an axum::Router
//! # let _ = router;
//! # Ok(()) }
//! ```

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    http::{header, HeaderValue},
    middleware::{from_fn, from_fn_with_state, map_response},
    routing::{delete, get, post},
    Router,
};
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tower_http::set_header::SetResponseHeaderLayer;

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod harness;
pub mod janitor;
pub mod middleware;
pub mod ratelimit;
pub mod routes;

pub use config::Config;
pub use db::{connect, DbError};

use ratelimit::RateLimiter;

/// Shared by every handler. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    /// Key for the fake prelogin salts of unknown users.
    pub server_secret: Arc<Vec<u8>>,
    pub auth_limiter: Arc<RateLimiter>,
    pub share_open_limiter: Arc<RateLimiter>,
}

impl AppState {
    /// Builds the state for an already-migrated pool (see [`connect`]).
    /// Resolves the server secret: `config.server_secret` if set, otherwise
    /// the one persisted in the database (generated on first use).
    pub async fn new(db: SqlitePool, config: Config) -> Result<Self, DbError> {
        let server_secret = match &config.server_secret {
            Some(s) => s.as_bytes().to_vec(),
            None => db::load_or_create_server_secret(&db).await?,
        };
        Ok(Self {
            auth_limiter: Arc::new(RateLimiter::new(config.auth_rate)),
            share_open_limiter: Arc::new(RateLimiter::new(config.share_open_rate)),
            server_secret: Arc::new(server_secret),
            config: Arc::new(config),
            db,
        })
    }
}

/// The complete HTTP application.
///
/// To rate-limit by client address the server must be run with
/// connect info, as [`serve`] does; a router driven without it (e.g. via
/// `tower::ServiceExt::oneshot`) puts every client in one bucket unless the
/// request carries an `axum::extract::ConnectInfo<SocketAddr>` extension.
pub fn app(state: AppState) -> Router {
    use routes::{auth as auth_routes, landing, shares, vault};

    let rate_limited_auth = Router::new()
        .route("/auth/register", post(auth_routes::register))
        .route("/auth/prelogin", get(auth_routes::prelogin))
        .route("/auth/login", post(auth_routes::login))
        .route("/auth/recover/unlock", post(auth_routes::recover_unlock))
        .route("/auth/recover", post(auth_routes::recover))
        .route_layer(from_fn_with_state(
            state.clone(),
            middleware::rate_limit_auth,
        ));

    let rate_limited_open = Router::new()
        .route("/shares/{id}/open", post(shares::open))
        .route_layer(from_fn_with_state(
            state.clone(),
            middleware::rate_limit_share_open,
        ));

    let v1 = Router::new()
        .merge(rate_limited_auth)
        .merge(rate_limited_open)
        .route("/auth/logout", post(auth_routes::logout))
        .route("/auth/password", post(auth_routes::change_password))
        .route("/me", get(auth_routes::me))
        .route("/vault/items", get(vault::list).post(vault::create))
        .route(
            "/vault/items/{id}",
            get(vault::get_one).put(vault::update).delete(vault::remove),
        )
        .route("/shares", get(shares::list).post(shares::create))
        .route("/shares/{id}", delete(shares::revoke))
        .route(
            "/shares/{id}/events",
            get(shares::events).post(shares::report_event),
        )
        // API responses are private and must not be sniffed or cached.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ));

    Router::new()
        .nest("/v1", v1)
        .route("/s/{id}", get(landing::page))
        .route("/assets/share.js", get(landing::script))
        .route("/healthz", get(routes::healthz))
        .fallback(routes::not_found)
        .layer(DefaultBodyLimit::max(config::MAX_BODY_BYTES))
        .layer(map_response(middleware::json_errors))
        .layer(from_fn(middleware::access_log))
        .with_state(state)
}

/// Serves `app(state)` on `listener` until `shutdown` completes, letting
/// in-flight requests finish. Client addresses come from the socket.
pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}

/// Completes on SIGINT (Ctrl-C) or, on Unix, SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
