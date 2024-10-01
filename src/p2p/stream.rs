use anyhow::{anyhow, bail, Context, Error};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{FramedRead, FramedWrite};

use super::network::{NetMessage, NetMessageCodec};

pub async fn read_stream(stream: &mut TcpStream) -> Result<NetMessage, Error> {
    let mut framed = FramedRead::new(stream, NetMessageCodec);

    if let Some(result) = framed.next().await {
        match result {
            Ok(msg) => Ok(msg),
            Err(e) => Err(anyhow!(e).context("Error while reading NetMessage")),
        }
    } else {
        bail!("Stream closed or no message available");
    }
}

pub async fn send_net_message(stream: &mut TcpStream, msg: NetMessage) -> Result<(), Error> {
    let mut framed = FramedWrite::new(stream, NetMessageCodec);

    framed
        .send(msg)
        .await
        .context("Failed to send NetMessage")?;

    Ok(())
}
