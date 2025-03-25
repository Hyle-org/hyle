use axum::serve::Listener;
#[cfg(not(feature = "turmoil"))]
pub use tokio::net::*;

#[cfg(feature = "turmoil")]
pub use turmoil::net::*;
#[cfg(feature = "turmoil")]
pub use turmoil::*;

/// Turmoil Listener does not implement axum::Listener. Wrap it with this helper to spawn an Axum server with it.
pub struct HyleNetTcpListener(pub net::TcpListener);

impl Listener for HyleNetTcpListener {
    type Io = net::TcpStream;

    type Addr = std::net::SocketAddr;

    fn accept(&mut self) -> impl std::future::Future<Output = (Self::Io, Self::Addr)> + Send {
        async move { self.0.accept().await.unwrap() }
    }

    fn local_addr(&self) -> tokio::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

impl From<net::TcpListener> for HyleNetTcpListener {
    fn from(value: net::TcpListener) -> Self {
        HyleNetTcpListener(value)
    }
}
