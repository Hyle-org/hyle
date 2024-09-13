use std::time::Duration;
use std::time::SystemTime;

use anyhow::{anyhow, bail, Context, Error, Result};
use tokio::select;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, Interest},
    net::TcpStream,
};
use tracing::{debug, info, trace, warn};

use super::network::{MempoolNetMessage, NetMessage, Version};
use crate::bus::SharedMessageBus;
use crate::p2p::network::ConsensusNetMessage;
use crate::p2p::network::NetInput;
use crate::utils::conf::SharedConf;
use crate::utils::logger::LogMe;

pub struct Peer {
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
        stream: TcpStream,
        bus: SharedMessageBus,
        conf: SharedConf,
    ) -> Result<Self, Error> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(100);

        Ok(Peer {
            stream,
            bus,
            last_pong: SystemTime::now(),
            conf,
            internal_cmd_tx: cmd_tx,
            internal_cmd_rx: cmd_rx,
        })
    }

    async fn handle_consensus_message(&mut self, msg: ConsensusNetMessage) -> Result<(), Error> {
        self.send_net_message(NetMessage::ConsensusMessage(msg))
            .await
    }

    async fn handle_mempool_message(&mut self, msg: MempoolNetMessage) -> Result<(), Error> {
        self.send_net_message(NetMessage::MempoolMessage(msg)).await
    }

    async fn handle_net_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        trace!("RECV: {:?}", msg);
        match msg {
            NetMessage::Version(v) => {
                info!("Got peer version {:?}", v);
                self.send_net_message(NetMessage::Verack).await
            }
            NetMessage::Verack => self.ping_pong(),
            NetMessage::Ping => {
                debug!("Got ping");
                self.send_net_message(NetMessage::Pong).await
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
                    .send(NetInput::new(mempool_msg))
                    .map(|_| ())
                    .context("Receiving mempool net message")
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

    async fn read_stream(stream: &mut TcpStream) -> Result<NetMessage, Error> {
        let ready = stream
            .ready(Interest::READABLE | Interest::WRITABLE | Interest::ERROR)
            .await
            .log_error("Reading from peer")?;

        if ready.is_error() {
            bail!("Stream not ready")
        }

        match stream.read_u32().await {
            Ok(msg_size) => Self::read_net_message_from_buffer(stream, msg_size).await,
            Err(e) => Err(anyhow!(e)),
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

        loop {
            let wait_tcp = Self::read_stream(&mut self.stream); //FIXME: make read_stream cancel safe !
            let wait_cmd = self.internal_cmd_rx.recv();
            let wait_mempool = mempool_rx.recv();
            let wait_consensus = consensus_rx.recv();

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
                res = wait_tcp => {
                    match res {
                        Ok(message) => match self.handle_net_message(message).await {
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
                                self.send_net_message(NetMessage::Ping).await
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
        self.send_net_message(NetMessage::Version(Version { id: 1 }))
            .await
    }

    async fn send_net_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        let binary = msg.to_binary();
        trace!("SEND {} bytes: {:?}", binary.len(), binary);
        self.stream
            .write_u32(binary.len() as u32)
            .await
            .context("Failed to write size on stream")?;
        self.stream
            .write(&binary)
            .await
            .context("Failed to write data on stream")?;
        Ok(())
    }

    async fn read_net_message_from_buffer(
        stream: &mut TcpStream,
        msg_size: u32,
    ) -> Result<NetMessage, Error> {
        if msg_size == 0 {
            bail!("Connection closed by remote (1)")
        }

        trace!("Reading {} bytes from buffer", msg_size);
        let mut buf = vec![0; msg_size as usize];
        trace!("buf before: {:?}", buf);

        let data = stream.read_exact(&mut buf).await?;
        if data == 0 {
            bail!("Connection closed by remote (2)")
        }

        trace!("got buff {:?}", buf);
        Self::parse_net_message(&buf).await
    }

    async fn parse_net_message(buf: &[u8]) -> Result<NetMessage, Error> {
        bincode::deserialize::<NetMessage>(buf).map_err(|_| anyhow!("Could not decode NetMessage"))
    }
}
