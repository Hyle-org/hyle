use anyhow::{anyhow, bail, Context, Error};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::trace;

use super::network::NetMessage;

pub async fn read_stream<T: bincode::Decode>(
    stream: &mut Framed<TcpStream, LengthDelimitedCodec>,
) -> Result<T, Error> {
    trace!("Waiting for data");
    if let Some(result) = stream.next().await {
        match result {
            Ok(data) => {
                let (msg, _) = bincode::decode_from_slice(&data, bincode::config::standard())
                    .map_err(|_| anyhow::anyhow!("Could not decode message"))?;
                Ok(msg)
            }
            Err(e) => Err(anyhow!(e).context("Error while reading message")),
        }
    } else {
        bail!("Stream closed or no message available");
    }
}

pub async fn send_net_message(
    stream: &mut Framed<TcpStream, LengthDelimitedCodec>,
    msg: NetMessage,
) -> Result<(), Error> {
    stream
        .send(msg.to_binary()?.into())
        .await
        .context("Failed to send NetMessage")?;

    Ok(())
}
