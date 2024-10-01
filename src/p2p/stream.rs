use std::io::IoSlice;

use anyhow::{anyhow, bail, Context, Error};
use futures::StreamExt;
use tokio::{io::AsyncWriteExt, net::TcpStream};
use tokio_util::codec::FramedRead;
use tracing::trace;

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
    send_binary(stream, msg.to_binary().as_slice()).await
}

pub(super) async fn send_binary(stream: &mut TcpStream, binary: &[u8]) -> Result<(), Error> {
    trace!("SEND {} bytes: {:?}", binary.len(), binary);
    // Create a new buffer with the size of the message prepended
    let size: [u8; 4] = (binary.len() as u32).to_be_bytes();
    let bufs: &[_] = &[IoSlice::new(&size), IoSlice::new(binary)];
    stream
        .write_vectored(bufs)
        .await
        .context("Failed to write data on stream")?;
    Ok(())
}
