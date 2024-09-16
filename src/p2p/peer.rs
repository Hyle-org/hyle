use std::time::Duration;
use std::time::SystemTime;

use anyhow::{anyhow, Context, Error, Result};
use tokio::net::TcpStream;
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, info, trace, warn};

use super::network::{MempoolNetMessage, NetMessage, Version};
use super::stream::send_net_message;
use crate::bus::SharedMessageBus;
use crate::p2p::network::Broadcast;
use crate::p2p::network::ConsensusNetMessage;
use crate::p2p::network::NetInput;
use crate::p2p::stream::read_stream;
use crate::utils::conf::SharedConf;

pub struct Peer {
    id: u64,
    stream: TcpStream,
    bus: SharedMessageBus,
    last_pong: SystemTime,
    conf: SharedConf,

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
        conf: SharedConf,
    ) -> Result<Self, Error> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(100);

        Ok(Peer {
            id,
            stream,
            bus,
            last_pong: SystemTime::now(),
            conf,
            internal_cmd_tx: cmd_tx,
            internal_cmd_rx: cmd_rx,
        })
    }

    async fn handle_consensus_message(&mut self, msg: ConsensusNetMessage) -> Result<(), Error> {
        send_net_message(&mut self.stream, NetMessage::ConsensusMessage(msg)).await
    }

    async fn handle_mempool_message(&mut self, msg: MempoolNetMessage) -> Result<(), Error> {
        send_net_message(&mut self.stream, NetMessage::MempoolMessage(msg)).await
    }

    async fn handle_broadcast_message(&mut self, msg: Broadcast) -> Result<(), Error> {
        if msg.peer_id == self.id {
            return Ok(());
        }
        send_net_message(&mut self.stream, msg.msg).await
    }

    async fn handle_stream_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        trace!("RECV: {:?}", msg);
        match msg {
            NetMessage::Version(v) => {
                info!("Got peer version {:?}", v);
                send_net_message(&mut self.stream, NetMessage::Verack).await
            }
            NetMessage::Verack => self.ping_pong(),
            NetMessage::Ping => {
                debug!("Got ping");
                send_net_message(&mut self.stream, NetMessage::Pong).await
            }
            NetMessage::Pong => {
                debug!("pong");
                self.last_pong = SystemTime::now();
                Ok(())
            }
            NetMessage::MempoolMessage(mempool_msg) => {
                debug!("Received new mempool net message {:?}", mempool_msg);
                self.bus
                    .sender::<NetInput<MempoolNetMessage>>()
                    .await
                    .send(NetInput::new(mempool_msg.clone()))
                    .map(|_| ())
                    .context("Receiving mempool net message")?;
                self.bus
                    .sender::<Broadcast>()
                    .await
                    .send(Broadcast {
                        peer_id: self.id,
                        msg: NetMessage::MempoolMessage(mempool_msg),
                    })
                    .map(|_| ())
                    .context("Broadcasting mempool net message")
            }
            NetMessage::ConsensusMessage(consensus_msg) => {
                debug!("Received new consensus net message {:?}", consensus_msg);
                self.bus
                    .sender::<NetInput<ConsensusNetMessage>>()
                    .await
                    .send(NetInput::new(consensus_msg))
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
        let mut mempool_rx = self.bus.receiver::<MempoolNetMessage>().await;
        let mut consensus_rx = self.bus.receiver::<ConsensusNetMessage>().await;
        let mut broadcast_rx = self.bus.receiver::<Broadcast>().await;

        loop {
            let wait_tcp = read_stream(&mut self.stream); //FIXME: make read_stream cancel safe !
            let wait_cmd = self.internal_cmd_rx.recv();
            let wait_mempool = mempool_rx.recv();
            let wait_consensus = consensus_rx.recv();
            let wait_broadcast = broadcast_rx.recv();

            // select! waits on multiple concurrent branches, returning when the **first** branch
            // completes, cancelling the remaining branches.
            select! {
                res = wait_mempool => {
                    if let Ok(message) = res {
                        match self.handle_mempool_message(message).await {
                            Ok(_) => continue,
                            Err(e) => {
                                warn!("Error while handling net message: {}", e);
                            }
                        }
                    }
                }
                res = wait_consensus => {
                    if let Ok(message) = res {
                        match self.handle_consensus_message(message).await {
                            Ok(_) => continue,
                            Err(e) => {
                                warn!("Error while handling net message: {}", e);
                            }
                        }
                    }
                }
                res = wait_broadcast => {
                    if let Ok(message) = res {
                        match self.handle_broadcast_message(message).await {
                            Ok(_) => continue,
                            Err(e) => {
                                warn!("Error while handling net message: {}", e);
                            }
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
                            trace!("err: {:?}", e);
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
                                send_net_message(&mut self.stream, NetMessage::Ping).await
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
        send_net_message(&mut self.stream, NetMessage::Version(Version { id: 1 })).await
    }
}
