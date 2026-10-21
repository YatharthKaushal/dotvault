//! Spawning a real server on an ephemeral port, for integration and
//! end-to-end tests (including ones in other crates).

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::{connect, serve, AppState, Config};

/// A server listening on `127.0.0.1:<random port>`.
///
/// ```no_run
/// # async fn demo() {
/// use dotvault_server::{harness, Config};
///
/// let config = Config { db: ":memory:".into(), ..Config::default() };
/// let server = harness::spawn(config).await.unwrap();
/// println!("listening on {}", server.base_url());
/// server.shutdown().await;
/// # }
/// ```
pub struct RunningServer {
    pub addr: SocketAddr,
    /// The state the server runs with; tests can use `state.db` to inspect or
    /// age rows directly.
    pub state: AppState,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}

impl RunningServer {
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Stops accepting connections, drains in-flight requests and waits for
    /// the server task to finish.
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Opens `config.db` (a path, `sqlite:` URL or `:memory:`), runs migrations
/// and serves on an ephemeral loopback port. `config.bind` is ignored. The
/// janitor is not started.
pub async fn spawn(
    config: Config,
) -> Result<RunningServer, Box<dyn std::error::Error + Send + Sync>> {
    let pool = connect(&config.db).await?;
    let state = AppState::new(pool, config).await?;
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let addr = listener.local_addr()?;
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(serve(listener, state.clone(), async move {
        let _ = stopped.await;
    }));
    Ok(RunningServer {
        addr,
        state,
        stop: Some(stop),
        task,
    })
}
