use std::time::Duration;
use std::time::SystemTime;

use anyhow::Context;
use anyhow::{anyhow, Error, Result};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::codec::Framed;
use tokio_util::codec::LengthDelimitedCodec;
use tracing::{debug, info, trace, warn};

use super::fifo_filter::FifoFilter;
use super::network::HandshakeNetMessage;
use super::network::OutboundMessage;
use super::network::PeerEvent;
use super::network::{Hello, NetMessage};
use super::stream::send_net_message;
use crate::bus::bus_client;
use crate::bus::BusClientSender;
use crate::bus::SharedMessageBus;
use crate::consensus::ConsensusNetMessage;
use crate::mempool::MempoolNetMessage;
use crate::model::ValidatorPublicKey;
use crate::module_handle_messages;
use crate::p2p::stream::read_stream;
use crate::utils::conf::SharedConf;
use crate::utils::crypto::SharedBlstCrypto;
use crate::utils::crypto::SignedByValidator;
use crate::utils::modules::signal::ShutdownModule;

bus_client! {
struct PeerBusClient {
    sender(SignedByValidator<MempoolNetMessage>),
    sender(SignedByValidator<ConsensusNetMessage>),
    sender(PeerEvent),
    receiver(OutboundMessage),
    receiver(ShutdownModule),
}
}

pub struct Peer {
    id: u64,
    stream: Framed<TcpStream, LengthDelimitedCodec>,
    bus: PeerBusClient,
    last_pong: SystemTime,
    conf: SharedConf,
    fifo_filter: FifoFilter<Vec<u8>>,
    self_pubkey: ValidatorPublicKey,
    peer_pubkey: Option<ValidatorPublicKey>,
    peer_name: Option<String>,
    peer_da_address: Option<String>,

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
        let fifo_filter = FifoFilter::new(1000);
        let self_validator = crypto.validator_pubkey().clone();
        let framed = Framed::new(stream, LengthDelimitedCodec::new());

        Peer {
            id,
            stream: framed,
            bus: PeerBusClient::new_from_bus(bus).await,
            last_pong: SystemTime::now(),
            conf,
            fifo_filter,
            self_pubkey: self_validator,
            peer_pubkey: None,
            internal_cmd_tx: cmd_tx,
            internal_cmd_rx: cmd_rx,
            peer_name: None,
            peer_da_address: None,
        }
    }

    async fn handle_send_message(
        &mut self,
        validator_id: ValidatorPublicKey,
        msg: NetMessage,
    ) -> Result<(), Error> {
        if let Some(peer_validator) = &self.peer_pubkey {
            if *peer_validator == validator_id {
                return send_net_message(&mut self.stream, msg).await;
            }
        } else {
            warn!("Peer validator not set. Ignoring message");
        }
        Ok(())
    }

    async fn handle_broadcast_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        let binary = msg.to_binary();
        if !self.fifo_filter.check(&binary) {
            self.fifo_filter.set(binary);
            debug!("Broadcast message to #{}: {}", self.id, msg);
            send_net_message(&mut self.stream, msg).await
        } else {
            trace!("Message to #{} already broadcasted", self.id);
            Ok(())
        }
    }

    async fn handle_handshake_message(&mut self, msg: HandshakeNetMessage) -> Result<(), Error> {
        match msg {
            HandshakeNetMessage::Hello(v) => {
                info!("👋 Got peer hello message {:?}", v);
                self.peer_pubkey = Some(v.validator_pubkey);
                self.peer_name = Some(v.name);
                self.peer_da_address = Some(v.da_address);
                send_net_message(&mut self.stream, HandshakeNetMessage::Verack.into()).await
            }
            HandshakeNetMessage::Verack => {
                debug!("Got peer verack message");
                if let Some(pubkey) = &self.peer_pubkey {
                    self.bus.send(PeerEvent::NewPeer {
                        name: self.peer_name.clone().unwrap_or("unknown".to_string()),
                        pubkey: pubkey.clone(),
                        da_address: self.peer_da_address.clone().unwrap(),
                    })?;
                }
                self.ping_pong();
                Ok(())
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

    async fn handle_peer_stream_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        trace!("RECV: {:?}", msg);
        match msg {
            NetMessage::HandshakeMessage(handshake_msg) => {
                debug!("Received new handshake net message {:?}", handshake_msg);
                self.handle_handshake_message(handshake_msg).await?;
            }
            NetMessage::MempoolMessage(mempool_msg) => {
                debug!("Received new mempool net message {}", mempool_msg);
                self.bus
                    .send(mempool_msg)
                    .context("Receiving mempool net message")?;
            }
            NetMessage::ConsensusMessage(consensus_msg) => {
                debug!("Received new consensus net message {}", consensus_msg);
                self.bus
                    .send(consensus_msg)
                    .context("Receiving consensus net message")?;
            }
        }
        Ok(())
    }

    fn ping_pong(&self) {
        let tx = self.internal_cmd_tx.clone();
        let interval = self.conf.p2p.ping_interval;
        info!("Starting ping pong");

        tokio::task::Builder::new()
            .name(&format!("ping-peer-{}", self.id))
            .spawn(async move {
                loop {
                    sleep(Duration::from_secs(interval)).await;
                    tx.send(Cmd::Ping).await.ok();
                }
            })
            .ok();
    }

    pub async fn start(&mut self) -> Result<()> {
        module_handle_messages! {
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
                    OutboundMessage::BroadcastMessageOnlyFor(only_for, message) => {
                        if let Some(ref pubkey) = self.peer_pubkey {
                            if only_for.contains(pubkey) {
                            match self.handle_broadcast_message(message).await {
                                Ok(_) => continue,
                                Err(e) => {
                                    warn!("Error while broadcasting net message: {}", e);
                                }
                            }
                            }
                            }
                    }
                }
            }

            res = read_stream(&mut self.stream) => match res {
                Ok(message) => match self.handle_peer_stream_message(message).await {
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
        Ok(())
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
            HandshakeNetMessage::Hello(Hello {
                version: 1,
                validator_pubkey: self.self_pubkey.clone(),
                name: self.conf.id.clone(),
                da_address: self.conf.da_address.clone(),
            })
            .into(),
        )
        .await
    }
}
