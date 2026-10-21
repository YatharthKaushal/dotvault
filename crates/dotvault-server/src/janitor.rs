//! Background housekeeping.
//!
//! Every [`INTERVAL`] the janitor
//!
//! * NULLs the ciphertext of expired shares (rows and events are kept so the
//!   owner's audit log survives), and
//! * deletes expired sessions.

use std::time::Duration;

use sqlx::SqlitePool;
use tokio::task::JoinHandle;

use crate::db::now;
use crate::AppState;

pub const INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    pub shares_purged: u64,
    pub sessions_deleted: u64,
}

/// One pass, as of `now` (unix seconds).
pub async fn run_once(pool: &SqlitePool, now: i64) -> Result<Swept, sqlx::Error> {
    let shares = sqlx::query(
        "UPDATE shares SET payload_ct = NULL WHERE payload_ct IS NOT NULL AND expires_at <= ?1",
    )
    .bind(now)
    .execute(pool)
    .await?;
    let sessions = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?1")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(Swept {
        shares_purged: shares.rows_affected(),
        sessions_deleted: sessions.rows_affected(),
    })
}

/// Runs the janitor forever, every `interval`. Abort the handle to stop it.
pub fn spawn(state: AppState, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match run_once(&state.db, now()).await {
                Ok(s) if s != Swept::default() => tracing::info!(
                    shares_purged = s.shares_purged,
                    sessions_deleted = s.sessions_deleted,
                    "janitor pass"
                ),
                Ok(_) => {}
                Err(e) => tracing::error!(error = %e, "janitor pass failed"),
            }
            state.auth_limiter.prune();
            state.share_open_limiter.prune();
        }
    })
}
