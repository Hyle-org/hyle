use anyhow::{anyhow, bail, Context, Error};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use super::network::NetMessage;

pub async fn read_stream(stream: &mut TcpStream) -> Result<NetMessage, Error> {
    let mut framed = FramedRead::new(stream, LengthDelimitedCodec::new());

    if let Some(result) = framed.next().await {
        match result {
            Ok(data) => {
                let (net_msg, _) = bincode::decode_from_slice(&data, bincode::config::standard())
                    .map_err(|_| anyhow::anyhow!("Could not decode NetMessage"))?;
                Ok(net_msg)
            }
            Err(e) => Err(anyhow!(e).context("Error while reading NetMessage")),
        }
    } else {
        bail!("Stream closed or no message available");
    }
}

pub async fn send_net_message(stream: &mut TcpStream, msg: NetMessage) -> Result<(), Error> {
    let mut framed = FramedWrite::new(stream, LengthDelimitedCodec::new());

    let binary = msg.to_binary();

    framed
        .send(binary.into())
        .await
        .context("Failed to send NetMessage")?;

    Ok(())
}
