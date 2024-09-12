use std::sync::Arc;

use crate::{bus::SharedMessageBus, utils::conf::SharedConf};
use anyhow::{Error, Result};
use tokio::net::TcpListener;
use tracing::info;

pub mod network; // FIXME(Bertrand): NetMessage should be private
mod peer;

pub async fn p2p_server(
    config: SharedConf,
    mempool: SharedMessageBus,
    consensus: SharedMessageBus,
) -> Result<(), Error> {
    if config.peers.is_empty() {
        let listener = TcpListener::bind(config.addr()).await?;
        let (addr, port) = config.addr();
        info!("p2p listening on {}:{}", addr, port);

        loop {
            let (socket, _) = listener.accept().await?;
            let conf = Arc::clone(&config);

            let mempool = mempool.new_handle();
            let consensus = consensus.new_handle();

            tokio::spawn(async move {
                info!(
                    "New peer: {}",
                    socket
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or("no address".to_string())
                );
                let mut peer_server = peer::Peer::new(socket, mempool, consensus, conf).await?;
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
        let mut peer = peer::Peer::new(stream, mempool, consensus, config).await?;

        peer.handshake().await?;
        peer.start().await
    }
}
