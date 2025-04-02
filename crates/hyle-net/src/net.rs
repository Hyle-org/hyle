use std::net::Ipv4Addr;
use std::ops::Deref;

use anyhow::bail;
use axum::serve::Listener;
#[cfg(not(feature = "turmoil"))]
pub use tokio::net::*;

use tracing::warn;
#[cfg(feature = "turmoil")]
pub use turmoil::net::*;
#[cfg(feature = "turmoil")]
pub use turmoil::*;

pub async fn bind_tcp_listener(port: u16) -> anyhow::Result<HyleNetTcpListener> {
    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();
    let listener = loop {
        match TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await {
            Ok(l) => break l,
            Err(e) => {
                if start.elapsed() >= timeout {
                    bail!(
                        "Failed to bind TCPListener after {} seconds: {}",
                        timeout.as_secs(),
                        e
                    );
                }
                warn!("Failed to bind TCPListener: {}. Retrying in 1 second...", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    };

    Ok(HyleNetTcpListener(listener))
}
/// Turmoil Listener does not implement axum::Listener. Wrap it with this helper to spawn an Axum server with it.
pub struct HyleNetTcpListener(pub TcpListener);

impl Listener for HyleNetTcpListener {
    type Io = TcpStream;

    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        self.0.accept().await.unwrap()
    }

    fn local_addr(&self) -> tokio::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

impl From<TcpListener> for HyleNetTcpListener {
    fn from(value: TcpListener) -> Self {
        HyleNetTcpListener(value)
    }
}

impl Deref for HyleNetTcpListener {
    type Target = TcpListener;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
