//! SQLite connection setup, migrations and the small `meta` key-value table.

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, SqlitePool};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("could not prepare the database location: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not read secure random bytes")]
    Random,
}

/// Unix seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn is_memory(db: &str) -> bool {
    db == ":memory:" || db.starts_with("sqlite::memory:") || db.contains("mode=memory")
}

/// Opens (creating if needed) the database, applies pragmas and runs the
/// migrations.
///
/// `db_url_or_path` is a file path (`./dotvault.db`), a `sqlite:` URL, or
/// `:memory:` / `sqlite::memory:` for a private in-memory database (which
/// is limited to a single connection, because every in-memory connection
/// would otherwise see its own empty database).
pub async fn connect(db_url_or_path: &str) -> Result<SqlitePool, DbError> {
    let memory = is_memory(db_url_or_path);

    let options = if memory {
        SqliteConnectOptions::from_str("sqlite::memory:")?
    } else if db_url_or_path.starts_with("sqlite:") {
        SqliteConnectOptions::from_str(db_url_or_path)?
    } else {
        if let Some(parent) = std::path::Path::new(db_url_or_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        SqliteConnectOptions::new().filename(db_url_or_path)
    };

    let mut options = options
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        // Statement text is harmless but noisy; keep logs to our own events.
        .log_statements(log::LevelFilter::Off)
        .log_slow_statements(log::LevelFilter::Warn, Duration::from_secs(2));
    if !memory {
        options = options
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);
    }

    let pool_options = if memory {
        SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
    } else {
        SqlitePoolOptions::new().max_connections(8)
    };
    let pool = pool_options.connect_with(options).await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Returns the secret used to derive fake prelogin salts, creating and
/// persisting a random one on first use.
pub async fn load_or_create_server_secret(pool: &SqlitePool) -> Result<Vec<u8>, DbError> {
    const KEY: &str = "server_secret";
    let read = || async {
        sqlx::query_scalar::<_, Vec<u8>>("SELECT value FROM meta WHERE key = ?1")
            .bind(KEY)
            .fetch_optional(pool)
            .await
    };
    if let Some(v) = read().await? {
        return Ok(v);
    }
    let fresh = dotvault_core::crypto::random_bytes::<32>().map_err(|_| DbError::Random)?;
    // OR IGNORE: if another process won the race, use its value.
    sqlx::query("INSERT OR IGNORE INTO meta (key, value) VALUES (?1, ?2)")
        .bind(KEY)
        .bind(&fresh[..])
        .execute(pool)
        .await?;
    Ok(read().await?.unwrap_or_else(|| fresh.to_vec()))
}
