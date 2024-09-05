use anyhow::{anyhow, Context, Error, Result};
use tokio::io::Interest;
use tracing::{error, warn};
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

    async fn handle_net_message(&mut self, msg: NetMessage) -> Result<(), Error> {
        trace!("RECV: {:?}", msg);
        match msg {
            NetMessage::Version(v) => {
                info!("Got peer version {:?}", v);
                self.send_message(NetMessage::Verack).await
            }
            NetMessage::Verack => Ok(()),
            NetMessage::Ping => todo!(),
            NetMessage::Pong => todo!(),
            NetMessage::NewTransaction(_) => todo!(),
        }
    }

    pub async fn start(&mut self) -> Result<(), Error> {
        loop {
            let ready = self
                .stream
                .ready(Interest::READABLE | Interest::WRITABLE | Interest::ERROR)
                .await?;

            if ready.is_error() {
                return Err(anyhow!("Stream not ready"));
            }

            let res = match self.stream.read_u32().await {
                Ok(msg_size) => self.read_next_message(msg_size).await,
                Err(e) => Err(anyhow!(e)),
            };

            match res {
                Ok(_) => continue,
                Err(e) => {
                    trace!("err: {:?}", e);
                    return Err(e);
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

    async fn read_next_message(&mut self, msg_size: u32) -> Result<(), Error> {
        if msg_size == 0 {
            return Err(anyhow!("Connection closed by remote (1)"));
        }

        trace!("Reading {} bytes from buffer", msg_size);
        let mut buf = vec![0; msg_size as usize];
        trace!("buf before: {:?}", buf);

        let data = self.stream.read_exact(&mut buf).await?;
        if data == 0 {
            return Err(anyhow!("Connection closed by remote (2)"));
        }

        trace!("got buff {:?}", buf);
        let message = Self::handle_read(&buf).await;

        match message {
            Ok(msg) => self.handle_net_message(msg).await,
            Err(e) => {
                warn!("Error while handling net message: {}", e);
                Ok(())
            }
        }
    }

    async fn handle_read(buf: &[u8]) -> Result<NetMessage, Error> {
        match bincode::deserialize::<NetMessage>(buf) {
            std::result::Result::Ok(msg) => Ok(msg),
            std::result::Result::Err(_) => Err(anyhow!("Could not decode NetMessage")),
        }
    }
}