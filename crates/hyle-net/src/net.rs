use std::net::Ipv4Addr;
use std::ops::Deref;

use axum::serve::Listener;
#[cfg(not(feature = "turmoil"))]
pub use tokio::net::*;

#[cfg(feature = "turmoil")]
pub use turmoil::net::*;
#[cfg(feature = "turmoil")]
pub use turmoil::*;

pub async fn bind_tcp_listener(port: u16) -> anyhow::Result<HyleNetTcpListener> {
    Ok(HyleNetTcpListener(
        TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?,
    ))
}
/// Turmoil Listener does not implement axum::Listener. Wrap it with this helper to spawn an Axum server with it.
pub struct HyleNetTcpListener(pub TcpListener);

impl Listener for HyleNetTcpListener {
    type Io = TcpStream;

    type Addr = std::net::SocketAddr;

    fn accept(&mut self) -> impl std::future::Future<Output = (Self::Io, Self::Addr)> + Send {
        async move { self.0.accept().await.unwrap() }
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
