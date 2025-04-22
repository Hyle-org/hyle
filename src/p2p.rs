//! Networking layer

use crate::{
    bus::{BusClientSender, BusMessage},
    log_error, log_warn,
    mempool::MempoolNetMessage,
    model::SharedRunContext,
    module_handle_messages,
    utils::{
        conf::SharedConf,
        modules::{module_bus_client, Module},
    },
};
use anyhow::{Context, Error, Result};
use hyle_crypto::SharedBlstCrypto;
use hyle_model::{ConsensusNetMessage, SignedByValidator};
use hyle_net::tcp::{
    p2p_server::{P2PServer, P2PServerEvent},
    P2PTcpMessage, TcpEvent,
};
use network::{codec_p2p_tcp_message, NetMessage, OutboundMessage, PeerEvent};
use tracing::{info, trace};

pub mod network;

#[derive(Debug, Clone)]
pub enum P2PCommand {
    ConnectTo { peer: String },
}
impl BusMessage for P2PCommand {}

module_bus_client! {
struct P2PBusClient {
    sender(SignedByValidator<MempoolNetMessage>),
    sender(SignedByValidator<ConsensusNetMessage>),
    sender(PeerEvent),
    receiver(P2PCommand),
    receiver(OutboundMessage),
}
}

pub struct P2P {
    config: SharedConf,
    bus: P2PBusClient,
    crypto: SharedBlstCrypto,
}

impl Module for P2P {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus_client = P2PBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        Ok(P2P {
            config: ctx.common.config.clone(),
            bus: bus_client,
            crypto: ctx.node.crypto.clone(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.p2p_server()
    }
}

impl P2P {
    pub async fn p2p_server(&mut self) -> Result<()> {
        let network_tcp_server =
            codec_p2p_tcp_message::start_server(self.config.p2p.server_port).await?;

        let mut p2p_server = P2PServer::new(
            self.crypto.clone(),
            self.config.id.clone(),
            self.config.p2p.public_address.clone(),
            self.config.da_public_address.clone(),
            network_tcp_server,
        );

        info!(
            "📡  Starting P2P module, listening on {}",
            self.config.p2p.public_address
        );

        for peer_ip in self.config.p2p.peers.clone() {
            let _ = log_error!(
                p2p_server.start_handshake(peer_ip).await,
                "Error while starting Handshake at startup"
            );
        }

        module_handle_messages! {
            on_bus self.bus,
            listen<P2PCommand> cmd => {
                match cmd {
                    P2PCommand::ConnectTo { peer } => {
                        let _ = log_error!(p2p_server.start_handshake(peer).await, "Error while starting Handshake with new peer");
                    }
                }
            }
            listen<OutboundMessage> res => {
                match res {
                    OutboundMessage::SendMessage { validator_id, msg } => {
                        let warn_msg = format!("P2P Sending net message to {}", validator_id);
                        _ = log_warn!(p2p_server.send(validator_id, msg).await, warn_msg);
                    }
                    OutboundMessage::BroadcastMessage(message) => {
                        _ = log_warn!(
                            p2p_server.broadcast(message.clone()).await,
                            "P2P Broadcasting net message"
                        );
                    }
                    OutboundMessage::BroadcastMessageOnlyFor(only_for, message) => {
                        _ = log_warn!(
                            p2p_server.broadcast_only_for(&only_for, message.clone()).await,
                            "P2P Broadcasting net message"
                        );
                    }
                };
            }

            Some(tcp_event) = p2p_server.listen_next() => {
                let TcpEvent { dest, data: p2p_tcp_message } = tcp_event;
                match p2p_tcp_message {
                    // When p2p server receives a handshake message; we process it in order to extract information of new peer
                    P2PTcpMessage::Handshake(handshake) => {
                        if let Ok(Some(p2p_server_event)) = log_error!(p2p_server.handle_handshake(dest, handshake).await, "Handling handshake") {
                            match p2p_server_event {
                                P2PServerEvent::NewPeer {
                                    name,
                                    pubkey,
                                    da_address,
                                } => {
                                    let _ = log_warn!(self.bus.send(PeerEvent::NewPeer {
                                        name,
                                        pubkey,
                                        da_address,
                                    }), "Sending new peer event");
                                }
                            }
                        }
                    },
                    P2PTcpMessage::Data(net_message) => {
                        let _ = log_warn!(self.handle_net_message(net_message).await, "Handling P2P net message");
                    },
                }
            }
        };
        Ok(())
    }

    async fn handle_net_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        trace!("RECV: {:?}", msg);
        match msg {
            NetMessage::MempoolMessage(mempool_msg) => {
                trace!("Received new mempool net message {}", mempool_msg);
                self.bus
                    .send(mempool_msg)
                    .context("Receiving mempool net message")?;
            }
            NetMessage::ConsensusMessage(consensus_msg) => {
                trace!("Received new consensus net message {}", consensus_msg);
                self.bus
                    .send(consensus_msg)
                    .context("Receiving consensus net message")?;
            }
        }
        Ok(())
    }
}
