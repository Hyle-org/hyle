use std::time::Duration;
use std::time::SystemTime;

use anyhow::Context;
use anyhow::{anyhow, Error, Result};
use bloomfilter::Bloom;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, info, trace, warn};

use super::network::HandshakeNetMessage;
use super::network::OutboundMessage;
use super::network::{NetMessage, Version};
use super::stream::send_net_message;
use crate::bus::bus_client;
use crate::bus::SharedMessageBus;
use crate::consensus::ConsensusNetMessage;
use crate::handle_messages;
use crate::mempool::MempoolNetMessage;
use crate::p2p::network::SignedWithId;
use crate::p2p::stream::read_stream;
use crate::p2p::stream::send_binary;
use crate::utils::conf::SharedConf;
use crate::utils::crypto::SharedBlstCrypto;
use crate::validator_registry::ConsensusValidator;
use crate::validator_registry::ValidatorId;
use crate::validator_registry::ValidatorRegistryNetMessage;

bus_client! {
struct PeerBusClient {
    sender(SignedWithId<MempoolNetMessage>),
    sender(SignedWithId<ConsensusNetMessage>),
    sender(ValidatorRegistryNetMessage),
    receiver(OutboundMessage),
}
}

pub struct Peer {
    id: u64,
    stream: TcpStream,
    bus: PeerBusClient,
    last_pong: SystemTime,
    conf: SharedConf,
    bloom_filter: Bloom<Vec<u8>>,
    self_validator: ConsensusValidator,

    // peer internal channel
    internal_cmd_tx: mpsc::Sender<Cmd>,
    internal_cmd_rx: mpsc::Receiver<Cmd>,
}

enum Cmd {
    Ping,
}

impl Peer {
    pub async fn new(
        id: u64,
        stream: TcpStream,
        bus: SharedMessageBus,
        crypto: SharedBlstCrypto,
        conf: SharedConf,
    ) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(100);
        let bloom_filter = Bloom::new_for_fp_rate(10_000, 0.01);
        let self_validator = crypto.as_validator();
        Peer {
            id,
            stream,
            bus: PeerBusClient::new_from_bus(bus).await,
            last_pong: SystemTime::now(),
            conf,
            bloom_filter,
            self_validator,
            internal_cmd_tx: cmd_tx,
            internal_cmd_rx: cmd_rx,
        }
    }

    async fn handle_send_message(
        &mut self,
        _validator_id: ValidatorId,
        msg: NetMessage,
    ) -> Result<(), Error> {
        // FIXME: extract peer_id from validator_id
        let peer_id = 1;
        if peer_id != self.id {
            return Ok(());
        }
        send_net_message(&mut self.stream, msg).await
    }

    async fn handle_broadcast_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        let binary = msg.to_binary();
        if !self.bloom_filter.check(&binary) {
            self.bloom_filter.set(&binary);
            debug!("Broadcast message to #{}: {:?}", self.id, msg);
            send_binary(&mut self.stream, binary.as_slice()).await
        } else {
            trace!("Message to #{} already broadcasted", self.id);
            Ok(())
        }
    }

    async fn handle_handshake_message(&mut self, msg: HandshakeNetMessage) -> Result<(), Error> {
        match msg {
            HandshakeNetMessage::Version(v) => {
                info!("Got peer version {:?}", v);
                send_net_message(&mut self.stream, HandshakeNetMessage::Verack.into()).await
            }
            HandshakeNetMessage::Verack => {
                self.ping_pong();
                send_net_message(
                    &mut self.stream,
                    ValidatorRegistryNetMessage::NewValidator(self.self_validator.clone()).into(),
                )
                .await
            }
            HandshakeNetMessage::Ping => {
                send_net_message(&mut self.stream, HandshakeNetMessage::Pong.into()).await
            }
            HandshakeNetMessage::Pong => {
                self.last_pong = SystemTime::now();
                Ok(())
            }
        }
    }

    async fn handle_stream_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        trace!("RECV: {:?}", msg);
        match msg {
            NetMessage::HandshakeMessage(handshake_msg) => {
                debug!("Received new handshake net message {:?}", handshake_msg);
                self.handle_handshake_message(handshake_msg).await?;
            }
            NetMessage::MempoolMessage(mempool_msg) => {
                debug!("Received new mempool net message {:?}", mempool_msg);
                self.bus
                    .send(mempool_msg)
                    .context("Receiving mempool net message")?;
            }
            NetMessage::ConsensusMessage(consensus_msg) => {
                debug!("Received new consensus net message {:?}", consensus_msg);
                self.bus
                    .send(consensus_msg)
                    .context("Receiving consensus net message")?;
            }
            NetMessage::ValidatorRegistryMessage(validator_registry_msg) => {
                debug!(
                    "Received new validator registry net message {:?}",
                    validator_registry_msg
                );
                self.bus
                    .send(validator_registry_msg)
                    .context("Receiving validator registry net message")?;
            }
        }
        Ok(())
    }

    fn ping_pong(&self) {
        let tx = self.internal_cmd_tx.clone();
        let interval = self.conf.p2p.ping_interval;
        info!("Starting ping pong");

        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(interval)).await;
                tx.send(Cmd::Ping).await.ok();
            }
        });
    }

    pub async fn start(&mut self) -> Result<(), Error> {
        handle_messages! {
            on_bus self.bus,
            listen<OutboundMessage> res => {
                match res {
                    OutboundMessage::SendMessage { validator_id, msg } => match self.handle_send_message(validator_id, msg).await {
                        Ok(_) => continue,
                        Err(e) => {
                            warn!("Error while sending net message: {}", e);
                        }
                    }
                    OutboundMessage::BroadcastMessage(message) => match self.handle_broadcast_message(message).await {
                        Ok(_) => continue,
                        Err(e) => {
                            warn!("Error while broadcasting net message: {}", e);
                        }
                    }
                }
            }

            res = read_stream(&mut self.stream) => match res {
                Ok((message, _)) => match self.handle_stream_message(message).await {
                    Ok(_) => continue,
                    Err(e) => {
                        warn!("Error while handling net message: {:#}", e);
                    }
                },
                Err(e) => {
                    warn!("Error reading tcp stream: {:?}", e);
                    return Err(e);
                }
            },

            res =  self.internal_cmd_rx.recv() => {
                if let Some(cmd) = res {
                    let cmd_res = match cmd {
                        Cmd::Ping => {
                            if let Ok(d) = SystemTime::now().duration_since(self.last_pong) {
                                if d > Duration::from_secs(self.conf.p2p.ping_interval * 5) {
                                    warn!("Peer did not respond to last 5 pings. Disconnecting.");
                                    return Ok(())
                                }
                            }
                            debug!("ping");
                            send_net_message(&mut self.stream, HandshakeNetMessage::Ping.into()).await
                        }
                    };

                    match cmd_res {
                            Ok(_) => (),
                            Err(e) => {
                            warn!("Error while handling cmd: {}", e);
                        },
                        }
                }
            }
        }
    }

    pub async fn connect(addr: &str) -> Result<TcpStream, Error> {
        match TcpStream::connect(addr).await {
            Ok(conn) => {
                info!("Connected to peer: {}", addr);
                Ok(conn)
            }
            Err(e) => Err(anyhow!("Failed to connect to peer: {}", e)),
        }
    }

    pub async fn handshake(&mut self) -> Result<(), Error> {
        send_net_message(
            &mut self.stream,
            HandshakeNetMessage::Version(Version { id: 1 }).into(),
        )
        .await
    }
}
