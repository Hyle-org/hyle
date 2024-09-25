//! Networking layer

use std::{sync::Arc, time::Duration};

use crate::{
    bus::SharedMessageBus,
    model::SharedRunContext,
    utils::{conf::SharedConf, crypto::SharedBlstCrypto, modules::Module},
};
use anyhow::{Error, Result};
use tokio::{net::TcpListener, time::sleep};
use tracing::{debug, error, info, warn};

pub mod network; // FIXME(Bertrand): NetMessage should be private
mod peer;
pub mod stream;

pub struct P2P {}
impl Module for P2P {
    fn name() -> &'static str {
        "P2P"
    }

    type Context = SharedRunContext;
    type Store = ();

    async fn build(_ctx: &Self::Context) -> Result<Self> {
        Ok(P2P {})
    }

    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        p2p_server(ctx.config.clone(), ctx.bus.new_handle(), ctx.crypto.clone())
    }
}

pub async fn p2p_server(
    config: SharedConf,
    bus: SharedMessageBus,
    crypto: SharedBlstCrypto,
) -> Result<(), Error> {
    let mut peer_id = 1u64;

    // Wait all other threads to start correctly
    sleep(Duration::from_secs(1)).await;

    for peer in &config.peers {
        let config = config.clone();
        let bus = bus.new_handle();
        let crypto = crypto.clone();
        let peer_address = peer.clone();
        let id = peer_id;
        peer_id += 1;

        tokio::spawn(async move {
            let mut retry_count = 20;
            while retry_count > 0 {
                info!("Connecting to peer #{}: {}", id, peer_address);
                if let Ok(stream) = peer::Peer::connect(peer_address.as_str()).await {
                    let mut peer = peer::Peer::new(
                        id,
                        stream,
                        bus.new_handle(),
                        crypto.clone(),
                        config.clone(),
                    )
                    .await;

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
        let crypto = crypto.clone();
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
            let mut peer_server = peer::Peer::new(id, socket, bus, crypto, conf).await;
            _ = peer_server.handshake().await;
            debug!("Handshake done !");
            match peer_server.start().await {
                Ok(_) => info!("Peer thread exited"),
                Err(e) => info!("Peer thread exited: {}", e),
            }
            anyhow::Ok(())
        });
    }
}
