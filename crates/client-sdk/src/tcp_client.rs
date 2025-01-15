use anyhow::{Context, Result};
use futures::SinkExt;
use sdk::{TcpServerNetMessage, Transaction};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

pub struct NodeTcpClient {
    pub framed: Framed<TcpStream, LengthDelimitedCodec>,
}

impl NodeTcpClient {
    pub async fn new(url: String) -> Result<Self> {
        let stream = TcpStream::connect(url).await?;
        let framed = Framed::new(stream, LengthDelimitedCodec::new());
        Ok(Self { framed })
    }

    pub async fn send_transaction(&mut self, transaction: Transaction) -> Result<()> {
        let msg: TcpServerNetMessage = transaction.into();
        self.framed
            .send(msg.to_binary()?.into())
            .await
            .context("Failed to send NetMessage")?;
        Ok(())
    }

    pub async fn send_encoded_message_no_response(&mut self, encoded_msg: Vec<u8>) -> Result<()> {
        self.framed
            .send(encoded_msg.into())
            .await
            .context("Failed to send NetMessage")?;
        Ok(())
    }
}
