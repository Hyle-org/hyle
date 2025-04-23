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
use hyle_model::{ConsensusNetMessage, SignedByValidator, ValidatorPublicKey};
use hyle_net::tcp::{
    p2p_server::{P2PServer, P2PServerEvent},
    tcp_client::TcpClient,
};
use network::{p2p_server_consensus_mempool, NetMessage, OutboundMessage, PeerEvent};
use tokio::task::JoinSet;
use tracing::{info, trace, warn};

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

type HandShakeJoinSet = JoinSet<
    Result<
        TcpClient<
            p2p_server_consensus_mempool::codec_tcp::ServerCodec,
            hyle_net::tcp::P2PTcpMessage<NetMessage>,
            hyle_net::tcp::P2PTcpMessage<NetMessage>,
        >,
    >,
>;

impl P2P {
    pub async fn p2p_server(&mut self) -> Result<()> {
        let mut p2p_server = p2p_server_consensus_mempool::start_server(
            self.crypto.clone(),
            self.config.id.clone(),
            self.config.p2p.server_port,
            self.config.p2p.public_address.clone(),
            self.config.da_public_address.clone(),
        )
        .await?;

        p2p_server.set_max_frame_length(256 * 1024 * 1024);

        info!(
            "📡  Starting P2P module, listening on {}",
            self.config.p2p.public_address
        );

        let mut handshake_clients_tasks = JoinSet::new();

        for peer_ip in self.config.p2p.peers.clone() {
            handshake_clients_tasks.spawn(async move {
                TcpClient::connect("p2p_server_handshake", peer_ip.clone()).await
            });
        }

        module_handle_messages! {
            on_bus self.bus,
            listen<P2PCommand> cmd => {
                match cmd {
                    P2PCommand::ConnectTo { peer } => {
                        handshake_clients_tasks.spawn(async move {
                                TcpClient::connect("p2p_server_new_peer", peer.clone()).await
                        });
                    }
                }
            }
            listen<OutboundMessage> res => {
                match res {
                    OutboundMessage::SendMessage { validator_id, msg } => {
                        if let Err(e) = p2p_server.send(validator_id.clone(), msg.clone()).await {
                            warn!("P2P Sending net message to {validator_id}: {e}");
                            self.handle_failed_send(
                                &mut p2p_server,
                                &mut handshake_clients_tasks,
                                validator_id,
                                msg,
                            ).await;
                        }
                    }
                    OutboundMessage::BroadcastMessage(message) => {
                        for failed_peer in p2p_server.broadcast(message.clone()).await {
                            self.handle_failed_send(
                                &mut p2p_server,
                                &mut handshake_clients_tasks,
                                failed_peer,
                                message.clone(),
                            ).await;
                        }
                    }
                    OutboundMessage::BroadcastMessageOnlyFor(only_for, message) => {
                        for failed_peer in p2p_server.broadcast_only_for(&only_for, message.clone()).await {
                            self.handle_failed_send(
                                &mut p2p_server,
                                &mut handshake_clients_tasks,
                                failed_peer,
                                message.clone(),
                            ).await;
                        }
                    }
                };
            }

            Some(task_result) = handshake_clients_tasks.join_next() => {
                if let Ok(tcp_client) = log_error!(task_result, "Processing Handshake Joinset") {
                    if let Ok(tcp_client) = log_error!(tcp_client, "Error while connecting TcpClient") {
                        let _ = log_error!(
                            p2p_server.start_handshake(tcp_client).await,
                            "Sending handshake's Hello message");
                    }
                }
            }

            Some(tcp_event) = p2p_server.listen_next() => {
                if let Ok(Some(p2p_tcp_event)) = log_warn!(p2p_server.handle_tcp_event(tcp_event).await, "Handling TCP event") {
                    match p2p_tcp_event {
                        P2PServerEvent::NewPeer { name, pubkey, da_address } => {
                            let _ = log_warn!(self.bus.send(PeerEvent::NewPeer {
                                name,
                                pubkey,
                                da_address,
                            }), "Sending new peer event");
                        },
                        P2PServerEvent::P2PMessage { msg: net_message } => {
                            let _ = log_warn!(self.handle_net_message(net_message).await, "Handling P2P net message");
                        },
                        P2PServerEvent::DisconnectedPeer { peer_ip } => {
                            handshake_clients_tasks.spawn(async move {
                                TcpClient::connect("p2p_server_disconnected_peer", peer_ip.clone()).await
                            });
                        },
                    }
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

    async fn handle_failed_send(
        &self,
        p2p_server: &mut P2PServer<
            p2p_server_consensus_mempool::codec_tcp::ServerCodec,
            NetMessage,
        >,
        handshake_clients_tasks: &mut HandShakeJoinSet,
        validator_id: ValidatorPublicKey,
        _msg: NetMessage,
    ) {
        // TODO: add waiting list for failed messages

        if let Some(validator_ip) = p2p_server
            .peers
            .get(&validator_id)
            .map(|peer| peer.node_connection_data.p2p_public_adress.clone())
        {
            handshake_clients_tasks.spawn(async move {
                TcpClient::connect("p2p_server_reconnect_peer", validator_ip).await
            });
        }
    }
}
