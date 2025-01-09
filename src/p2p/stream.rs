use anyhow::{anyhow, bail, Context, Error};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::trace;

use super::network::{NetMessage, PeerNetMessage};

pub async fn read_peer_stream(
    stream: &mut Framed<TcpStream, LengthDelimitedCodec>,
) -> Result<PeerNetMessage, Error> {
    trace!("Waiting for data");
    if let Some(result) = stream.next().await {
        match result {
            Ok(data) => {
                let (net_msg, _) =
                    bincode::decode_from_slice(&data, bincode::config::standard())
                        .map_err(|_| anyhow::anyhow!("Could not decode PeerNetMessage"))?;
                Ok(net_msg)
            }
            Err(e) => Err(anyhow!(e).context("Error while reading NetMessage")),
        }
    } else {
        bail!("Stream closed or no message available");
    }
}

pub async fn read_stream(
    stream: &mut Framed<TcpStream, LengthDelimitedCodec>,
) -> Result<NetMessage, Error> {
    trace!("Waiting for data");
    if let Some(result) = stream.next().await {
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

pub async fn send_net_message(
    stream: &mut Framed<TcpStream, LengthDelimitedCodec>,
    msg: PeerNetMessage,
) -> Result<(), Error> {
    stream
        .send(msg.to_binary().into())
        .await
        .context("Failed to send NetMessage")?;

    Ok(())
}
