//! Networking layer

use std::{sync::Arc, time::Duration};

use crate::{
    bus::{bus_client, BusMessage, SharedMessageBus, ShutdownSignal},
    handle_messages,
    model::SharedRunContext,
    utils::{conf::SharedConf, crypto::SharedBlstCrypto, modules::Module},
};
use anyhow::{bail, Result};
use tokio::{net::TcpListener, time::sleep};
use tracing::{debug, error, info, warn};

mod fifo_filter;
pub mod network;
mod peer;
pub mod stream;

#[derive(Debug, Clone)]
pub enum P2PCommand {
    ConnectTo { peer: String },
}
impl BusMessage for P2PCommand {}

bus_client! {
struct P2PBusClient {
    receiver(P2PCommand),
    receiver(ShutdownSignal),
}
}

pub struct P2P {
    config: SharedConf,
    bus: SharedMessageBus,
    bus_client: P2PBusClient,
    crypto: SharedBlstCrypto,
    peer_id: u64,
}

impl Module for P2P {
    fn name() -> &'static str {
        "P2P"
    }

    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus_client = P2PBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        Ok(P2P {
            config: ctx.common.config.clone(),
            bus: ctx.common.bus.new_handle(),
            bus_client,
            crypto: ctx.node.crypto.clone(),
            peer_id: 1u64,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.p2p_server()
    }
}

impl P2P {
    fn spawn_peer(&mut self, peer_address: String) {
        let config = self.config.clone();
        let bus = self.bus.new_handle();
        let crypto = self.crypto.clone();
        let id = self.peer_id;
        self.peer_id += 1;

        tokio::task::Builder::new()
            .name("connect-to-peer")
            .spawn(async move {
                let mut retry_count = 20;
                while retry_count > 0 {
                    info!("Connecting to peer #{}: {}", id, peer_address);
                    match peer::Peer::connect(peer_address.as_str()).await {
                        Ok(stream) => {
                            let mut peer = peer::Peer::new(
                                id,
                                stream,
                                bus.new_handle(),
                                crypto.clone(),
                                config.clone(),
                            )
                            .await;

                            if let Err(e) = peer.handshake().await {
                                warn!("Error in handshake: {}", e);
                            }
                            debug!("Handshake done !");
                            match peer.start().await {
                                Ok(_) => warn!("Peer #{} thread ended with success.", id),
                                Err(_) => warn!(
                                    "Peer #{}: {} disconnected ! Retry connection",
                                    id, peer_address
                                ),
                            };
                        }
                        Err(e) => {
                            warn!("Error while connecting to peer #{}: {}", id, e);
                        }
                    }

                    retry_count -= 1;
                    sleep(Duration::from_secs(2)).await;
                }
                error!("Can't reach peer #{}: {}.", id, peer_address);
            })
            .expect("Failed to spawn peer thread");
    }

    fn handle_command(&mut self, cmd: P2PCommand) {
        match cmd {
            P2PCommand::ConnectTo { peer } => self.spawn_peer(peer),
        }
    }

    pub async fn p2p_server(&mut self) -> Result<()> {
        // Wait all other threads to start correctly
        sleep(Duration::from_secs(1)).await;

        let listener = TcpListener::bind(&self.config.host).await?;
        info!("p2p listening on {}", listener.local_addr()?);

        // Wait some more so all peers (in tests) are listening.
        #[cfg(test)]
        sleep(Duration::from_secs(1)).await;

        for peer in self.config.peers.clone() {
            self.spawn_peer(peer);
        }

        handle_messages! {
            on_bus self.bus_client,
            break_on<ShutdownSignal>
            listen<P2PCommand> cmd => {
                 self.handle_command(cmd)
            }

            res = listener.accept() => {
                match res {
                    Ok((socket, _)) => {
                        let conf = Arc::clone(&self.config);
                        let bus = self.bus.new_handle();
                        let crypto = self.crypto.clone();
                        let id = self.peer_id;
                        self.peer_id += 1;
                        tokio::task::Builder::new()
                            .name(&format!("peer-{}", id))
                            .spawn(async move {
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
                            })?;
                    }
                    Err(e) => {
                        bail!("Error while accepting connection: {}", e);
                    }
                }
            }
        }
        Ok(())
    }
}
