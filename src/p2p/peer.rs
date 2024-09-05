use std::mem::size_of;

use anyhow::{anyhow, Context, Error, Result};
use tokio::io::Interest;
use tracing::error;
use tracing::{info, trace};

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::{net::TcpStream, sync::mpsc};

use super::network::{NetMessage, Version};
use crate::ctx::CtxCommand;
#[derive(Debug)]
pub struct Peer {
    stream: TcpStream,
    ctx: mpsc::Sender<CtxCommand>,
}

impl Peer {
    pub async fn new(stream: TcpStream, ctx: mpsc::Sender<CtxCommand>) -> Result<Self, Error> {
        Ok(Peer { stream, ctx })
    }

    pub async fn start(&mut self) -> Result<(), Error> {
        loop {
            let ready = self
                .stream
                .ready(Interest::READABLE | Interest::WRITABLE | Interest::ERROR)
                .await?;

            if ready.is_error() {
                panic!("Error on stream");
            }

            let msg_size = self.stream.read_u32().await;

            match msg_size {
                Ok(0) => {
                    info!("Connection closed by remote (1)");
                    return Ok(());
                }
                Ok(exptected_msg_size) => {
                    trace!("Reading {} bytes from buffer", exptected_msg_size);
                    let mut buf = vec![0; exptected_msg_size as usize];
                    trace!("buf before: {:?}", buf);
                    let data = self.stream.read_exact(&mut buf).await;
                    match data {
                        Ok(0) => {
                            info!("Connection closed by remote (2)");
                            return Ok(());
                        }
                        Ok(_) => {
                            trace!("got buff {:?}", buf);
                            let message = Self::handle_read(&buf).await;

                            match message {
                                Ok(msg) => self.handle_net_message(msg).await,
                                Err(e) => error!("{}", e),
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            continue;
                        }
                        Err(e) => {
                            return Err(e.into());
                        }
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    return Err(e.into());
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

    async fn handle_net_message(&mut self, msg: NetMessage) {
        trace!("RECV: {:?}", msg);
        match msg {
            NetMessage::Version(v) => info!("Got peer version {:?}", v),
            NetMessage::Verack => todo!(),
            NetMessage::Ping => todo!(),
            NetMessage::Pong => todo!(),
            NetMessage::NewTransaction(_) => todo!(),
        }
    }

    pub async fn handshake(&mut self) -> Result<(), Error> {
        self.send_message(NetMessage::Version(Version { id: 1 }))
            .await
    }

    async fn send_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        let binary = msg.as_binary();
        trace!("SEND {} bytes: {:?}", binary.len(), binary);
        self.stream
            .write_u32(binary.len() as u32)
            .await
            .context("Failed to write size on stream")?;
        self.stream
            .write(msg.as_binary().as_ref())
            .await
            .context("Failed to write data on stream")?;
        Ok(())
    }

    async fn handle_read(buf: &[u8]) -> Result<NetMessage, Error> {
        match bincode::deserialize::<NetMessage>(buf) {
            std::result::Result::Ok(msg) => Ok(msg),
            std::result::Result::Err(_) => Err(anyhow!("Could not decode NetMessage")),
        }
    }
}
