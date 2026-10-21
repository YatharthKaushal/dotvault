//! `dotvault-server`: a thin wrapper around the library.

use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use dotvault_server::{connect, janitor, serve, shutdown_signal, AppState, Config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing_subscriber::EnvFilter;

/// dotvault sync and sharing server. It stores ciphertext only.
///
/// Configuration comes from the environment: DOTVAULT_BIND, DOTVAULT_DB,
/// DOTVAULT_SERVER_SECRET, DOTVAULT_PUBLIC_URL and DOTVAULT_TRUST_PROXY.
#[derive(Parser)]
#[command(name = "dotvault-server", version)]
struct Args {
    /// Address to listen on (overrides DOTVAULT_BIND).
    #[arg(long, value_name = "ADDR")]
    bind: Option<SocketAddr>,

    /// Probe GET /healthz on the bind address and exit 0 if it answers 200.
    /// Meant for container health checks, where there is no curl.
    #[arg(long)]
    healthcheck: bool,
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .with_target(false)
        .init();

    let result = if args.healthcheck {
        healthcheck(&args).await
    } else {
        run(&args).await
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn load_config(args: &Args) -> Result<Config, BoxError> {
    let mut config = Config::from_env()?;
    if let Some(bind) = args.bind {
        config.bind = bind;
    }
    Ok(config)
}

async fn run(args: &Args) -> Result<(), BoxError> {
    let config = load_config(args)?;
    let pool = connect(&config.db).await?;
    let state = AppState::new(pool.clone(), config.clone()).await?;
    let listener = TcpListener::bind(config.bind).await?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        bind = %listener.local_addr()?,
        db = %config.db,
        trust_proxy = config.trust_proxy,
        "dotvault-server listening"
    );

    let janitor = janitor::spawn(state.clone(), janitor::INTERVAL);
    serve(listener, state, async {
        shutdown_signal().await;
        tracing::info!("shutting down");
    })
    .await?;

    janitor.abort();
    // Closing the last connection checkpoints the WAL back into the main file.
    pool.close().await;
    Ok(())
}

async fn healthcheck(args: &Args) -> Result<(), BoxError> {
    let mut addr = load_config(args)?.bind;
    if addr.ip().is_unspecified() {
        addr.set_ip(IpAddr::from([127, 0, 0, 1]));
    }
    let probe = async {
        let mut stream = TcpStream::connect(addr).await?;
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).await?;
        Ok::<_, std::io::Error>(head)
    };
    let head = tokio::time::timeout(Duration::from_secs(3), probe).await??;
    if &head == b"HTTP/1.1 200" {
        Ok(())
    } else {
        Err("unhealthy: /healthz did not answer 200".into())
    }
}
