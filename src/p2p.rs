//! Networking layer

use crate::{
    bus::BusClientSender, mempool::MempoolNetMessage, model::SharedRunContext,
    utils::conf::SharedConf,
};
use anyhow::{Context, Error, Result};
use hyle_crypto::SharedBlstCrypto;
use hyle_model::{BlockHeight, ConsensusNetMessage, NodeStateEvent, ValidatorPublicKey};
use hyle_modules::{
    log_warn, module_handle_messages,
    modules::{module_bus_client, Module},
};
use hyle_net::tcp::{
    p2p_server::{P2PServer, P2PServerEvent},
    Canal,
};
use network::{
    p2p_server_consensus_mempool, MsgWithHeader, NetMessage, OutboundMessage, PeerEvent,
};
use tracing::{info, trace, warn};

pub mod network;

#[derive(Debug, Clone)]
pub enum P2PCommand {
    ConnectTo { peer: String },
}
module_bus_client! {
struct P2PBusClient {
    sender(MsgWithHeader<MempoolNetMessage>),
    sender(MsgWithHeader<ConsensusNetMessage>),
    sender(PeerEvent),
    receiver(P2PCommand),
    receiver(NodeStateEvent),
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
    pub fn choose_canal(msg: &NetMessage) -> Canal {
        match msg {
            NetMessage::MempoolMessage(_) => Canal::new("mempool"),
            NetMessage::ConsensusMessage(_) => Canal::new("consensus"),
        }
    }

    pub async fn p2p_server(&mut self) -> Result<()> {
        let mut p2p_server = p2p_server_consensus_mempool::start_server(
            self.crypto.clone(),
            self.config.id.clone(),
            self.config.p2p.server_port,
            Some(self.config.p2p.max_frame_length),
            self.config.p2p.public_address.clone(),
            self.config.da_public_address.clone(),
        )
        .await?;

        info!(
            "📡  Starting P2P module, listening on {}",
            self.config.p2p.public_address
        );

        for peer_ip in self.config.p2p.peers.clone() {
            _ = p2p_server.try_start_connection(peer_ip.clone(), Canal::new("mempool"));
            _ = p2p_server.try_start_connection(peer_ip, Canal::new("consensus"));
        }

        module_handle_messages! {
            on_bus self.bus,
            listen<NodeStateEvent> NodeStateEvent::NewBlock(b) => {
                if b.block_height.0 > p2p_server.current_height {
                    p2p_server.current_height = b.block_height.0;
                }
            }
            listen<P2PCommand> cmd => {
                match cmd {
                    P2PCommand::ConnectTo { peer } => {
                        _ = p2p_server.try_start_connection(peer, Canal::new("consensus"));
                    }
                }
            }
            listen<OutboundMessage> res => {
                match res {
                    OutboundMessage::SendMessage { validator_id, msg }  => {
                        let canal = Self::choose_canal(&msg);
                        if let Err(e) = p2p_server.send(validator_id.clone(), canal, msg.clone()).await {
                            self.handle_failed_send(
                                &mut p2p_server,
                                validator_id,
                                msg,
                                e
                            ).await;
                        }
                    }
                    OutboundMessage::BroadcastMessage(message) => {
                        let canal = Self::choose_canal(&message);
                        for (failed_peer, error) in p2p_server.broadcast(message.clone(), canal).await {
                            self.handle_failed_send(
                                &mut p2p_server,
                                failed_peer,
                                message.clone(),
                                error,
                            ).await;
                        }
                    }
                    OutboundMessage::BroadcastMessageOnlyFor(only_for, message) => {
                        let canal = Self::choose_canal(&message);
                        for (failed_peer, error) in p2p_server.broadcast_only_for(&only_for, canal, message.clone()).await {
                            self.handle_failed_send(
                                &mut p2p_server,
                                failed_peer,
                                message.clone(),
                                error,
                            ).await;
                        }
                    }
                };
            }

            p2p_tcp_event = p2p_server.listen_next() => {
                if let Ok(Some(p2p_server_event)) = log_warn!(p2p_server.handle_p2p_tcp_event(p2p_tcp_event).await, "Handling P2PTcpEvent") {
                    match p2p_server_event {
                        P2PServerEvent::NewPeer { name, pubkey, da_address, height } => {
                            let _ = log_warn!(self.bus.send(PeerEvent::NewPeer {
                                name,
                                pubkey,
                                da_address,
                                height: BlockHeight(height)
                            }), "Sending new peer event");
                        },
                        P2PServerEvent::P2PMessage { msg: net_message } => {
                            let _ = log_warn!(self.handle_net_message(net_message).await, "Handling P2P net message");
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
                trace!("Received new mempool net message {}", mempool_msg.msg);
                self.bus
                    .send(mempool_msg)
                    .context("Receiving mempool net message")?;
            }
            NetMessage::ConsensusMessage(consensus_msg) => {
                trace!("Received new consensus net message {:?}", consensus_msg);
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
        validator_id: ValidatorPublicKey,
        _msg: NetMessage,
        error: Error,
    ) {
        // TODO: add waiting list for failed messages
        let canal = Self::choose_canal(&_msg);
        warn!("{error}. Reconnecting to peer on canal {:?}...", canal);
        if let Some(peer_info) = p2p_server.peers.get(&validator_id) {
            p2p_server.start_connection_task(
                peer_info.node_connection_data.p2p_public_address.clone(),
                canal,
            );
        }
    }
}
