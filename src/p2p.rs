use std::{sync::Arc, time::Duration};

use crate::{bus::SharedMessageBus, utils::conf::SharedConf};
use anyhow::{Error, Result};
use tokio::{net::TcpListener, time::sleep};
use tracing::{debug, error, info, warn};

pub mod network; // FIXME(Bertrand): NetMessage should be private
mod peer;
pub mod stream;

pub async fn p2p_server(config: SharedConf, bus: SharedMessageBus) -> Result<(), Error> {
    let mut peer_id = 1u64;
    for peer in &config.peers {
        let config_clone = config.clone();
        let bus_clone = bus.new_handle();
        let peer_address = peer.clone();
        let id = peer_id;
        peer_id += 1;

        tokio::spawn(async move {
            let mut retry_count = 20;
            while retry_count > 0 {
                info!("Connecting to peer #{}: {}", id, peer_address);
                if let Ok(stream) = peer::Peer::connect(peer_address.as_str()).await {
                    let mut peer =
                        peer::Peer::new(id, stream, bus_clone.new_handle(), config_clone.clone());

                    _ = peer.handshake().await;
                    debug!("Handshake done !");
                    match peer.start().await {
                        Ok(_) => warn!("Peer #{} thread ended with success.", id),
                        Err(_) => warn!(
                            "Peer #{}: {} disconnected ! Retry connection",
                            id, peer_address
                        ),
                    };
                }

                retry_count -= 1;
                sleep(Duration::from_secs(2)).await;
            }
            error!("Can't reach peer #{}: {}.", id, peer_address);
        });
    }

    let listener = TcpListener::bind(config.addr()).await?;
    let (addr, port) = config.addr();
    info!("p2p listening on {}:{}", addr, port);

    loop {
        let (socket, _) = listener.accept().await?;
        let conf = Arc::clone(&config);

        let bus = bus.new_handle();
        let id = peer_id;
        peer_id += 1;

        tokio::spawn(async move {
            info!(
                "New peer #{}: {}",
                id,
                socket
                    .peer_addr()
                    .map(|a| a.to_string())
                    .unwrap_or("no address".to_string())
            );
            let mut peer_server = peer::Peer::new(id, socket, bus, conf);
            match peer_server.start().await {
                Ok(_) => info!("Peer thread exited"),
                Err(e) => info!("Peer thread exited: {}", e),
            }
            anyhow::Ok(())
        });
    }
}
