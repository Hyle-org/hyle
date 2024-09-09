use crate::conf::Conf;
use crate::consensus::ConsensusCommand;
use crate::p2p::peer;
use crate::rest_endpoints;
use anyhow::{Context, Error, Result};
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::info;

pub async fn p2p_server(
    addr: &str,
    config: &Conf,
    consensus: mpsc::Sender<ConsensusCommand>,
) -> Result<(), Error> {
    if config.peers.is_empty() {
        let listener = TcpListener::bind(addr).await?;
        info!("p2p listening on {}", addr);

        loop {
            let (socket, _) = listener.accept().await?;
            let tx = consensus.clone();

            tokio::spawn(async move {
                info!(
                    "New peer: {}",
                    socket
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or("no address".to_string())
                );
                let mut peer_server = peer::Peer::new(socket, tx.clone()).await?;
                match peer_server.start().await {
                    Ok(_) => info!("Peer thread exited"),
                    Err(e) => info!("Peer thread exited: {}", e),
                }
                anyhow::Ok(())
            });
        }
    } else {
        let peer_address = config.peers.first().unwrap();
        info!("Connecting to peer {}", peer_address);
        let stream = peer::Peer::connect(peer_address).await?;
        let mut peer = peer::Peer::new(stream, consensus.clone()).await?;

        peer.handshake().await?;
        peer.start().await
    }
}

pub async fn rest_server(addr: &str) -> Result<()> {
    info!("rest listening on {}", addr);
    let app = Router::new()
        .route("/getTransaction", get(rest_endpoints::get_transaction))
        .route("/getBlock", get(rest_endpoints::get_block));

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("Starting rest server")?;

    axum::serve(listener, app)
        .await
        .context("Starting rest server")
}
