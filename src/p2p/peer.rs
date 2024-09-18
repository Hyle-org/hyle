use std::time::Duration;
use std::time::SystemTime;

use anyhow::{anyhow, Context, Error, Result};
use bloomfilter::Bloom;
use tokio::net::TcpStream;
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, info, trace, warn};

use super::network::HandshakeNetMessage;
use super::network::OutboundMessage;
use super::network::{MempoolNetMessage, NetMessage, Version};
use super::stream::send_net_message;
use crate::bus::SharedMessageBus;
use crate::p2p::network::ConsensusNetMessage;
use crate::p2p::stream::read_stream;
use crate::p2p::stream::send_binary;
use crate::utils::conf::SharedConf;

pub struct Peer {
    id: u64,
    stream: TcpStream,
    bus: SharedMessageBus,
    last_pong: SystemTime,
    conf: SharedConf,
    bloom_filter: Bloom<Vec<u8>>,

    // peer internal channel
    internal_cmd_tx: mpsc::Sender<Cmd>,
    internal_cmd_rx: mpsc::Receiver<Cmd>,
}

enum Cmd {
    Ping,
}

impl Peer {
    pub fn new(id: u64, stream: TcpStream, bus: SharedMessageBus, conf: SharedConf) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(100);
        let bloom_filter = Bloom::new_for_fp_rate(10_000, 0.01);

        Peer {
            id,
            stream,
            bus,
            last_pong: SystemTime::now(),
            conf,
            bloom_filter,
            internal_cmd_tx: cmd_tx,
            internal_cmd_rx: cmd_rx,
        }
    }

    async fn handle_send_message(&mut self, peer_id: u64, msg: NetMessage) -> Result<(), Error> {
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
                send_net_message(
                    &mut self.stream,
                    NetMessage::from(HandshakeNetMessage::Verack),
                )
                .await
            }
            HandshakeNetMessage::Verack => self.ping_pong(),
            HandshakeNetMessage::Ping => {
                debug!("Got ping");
                send_net_message(
                    &mut self.stream,
                    NetMessage::from(HandshakeNetMessage::Pong),
                )
                .await
            }
            HandshakeNetMessage::Pong => {
                debug!("pong");
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
                self.handle_handshake_message(handshake_msg).await
            }
            NetMessage::MempoolMessage(mempool_msg) => {
                debug!("Received new mempool net message {:?}", mempool_msg);
                self.bus
                    .sender::<MempoolNetMessage>()
                    .await
                    .send(mempool_msg)
                    .map(|_| ())
                    .context("Receiving mempool net message")
            }
            NetMessage::ConsensusMessage(consensus_msg) => {
                debug!("Received new consensus net message {:?}", consensus_msg);
                self.bus
                    .sender::<ConsensusNetMessage>()
                    .await
                    .send(consensus_msg)
                    .map(|_| ())
                    .context("Receiving consensus net message")
            }
        }
    }

    fn ping_pong(&self) -> Result<(), Error> {
        let tx = self.internal_cmd_tx.clone();
        let interval = self.conf.p2p.ping_interval;
        info!("Starting ping pong");

        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(interval)).await;
                tx.send(Cmd::Ping).await.ok();
            }
        });

        Ok(())
    }

    pub async fn start(&mut self) -> Result<(), Error> {
        let mut outbound_rx = self.bus.receiver::<OutboundMessage>().await;

        loop {
            let wait_tcp = read_stream(&mut self.stream); //FIXME: make read_stream cancel safe !
            let wait_cmd = self.internal_cmd_rx.recv();
            let wait_outbound = outbound_rx.recv();

            // select! waits on multiple concurrent branches, returning when the **first** branch
            // completes, cancelling the remaining branches.
            select! {
                res = wait_outbound => {
                    match res {
                        Ok(OutboundMessage::SendMessage { peer_id, msg }) => match self.handle_send_message(peer_id, msg).await {
                            Ok(_) => continue,
                            Err(e) => {
                                warn!("Error while sending net message: {}", e);
                            }
                        }
                        Ok(OutboundMessage::BroadcastMessage(message)) => match self.handle_broadcast_message(message).await {
                            Ok(_) => continue,
                            Err(e) => {
                                warn!("Error while broadcasting net message: {}", e);
                            }
                        }
                        Err(e) => {
                            warn!("Error while receiving outbound message: {}", e);
                        }
                    }
                }
                res = wait_tcp => {
                    match res {
                        Ok((message, _)) => match self.handle_stream_message(message).await {
                            Ok(_) => continue,
                            Err(e) => {
                                warn!("Error while handling net message: {}", e);
                            }
                        },
                        Err(e) => {
                            warn!("Error reading tcp stream: {:?}", e);
                            return Err(e);
                        }
                    }
                }
                res = wait_cmd => {
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
                                send_net_message(&mut self.stream, NetMessage::from(HandshakeNetMessage::Ping)).await
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
            NetMessage::from(HandshakeNetMessage::Version(Version { id: 1 })),
        )
        .await
    }
}
