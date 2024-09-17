use anyhow::{anyhow, bail, Context, Error};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, Interest},
    net::TcpStream,
};
use tracing::trace;

use crate::utils::logger::LogMe;

use super::network::NetMessage;

pub async fn read_stream(stream: &mut TcpStream) -> Result<(NetMessage, usize), Error> {
    let ready = stream
        .ready(Interest::READABLE | Interest::WRITABLE | Interest::ERROR)
        .await
        .log_error("Reading from peer")?;

    if ready.is_error() {
        bail!("Stream not ready")
    }

    match stream.read_u32().await {
        Ok(msg_size) => read_net_message_from_buffer(stream, msg_size).await,
        Err(e) => Err(anyhow!(e)),
    }
}

pub async fn send_net_message(stream: &mut TcpStream, msg: NetMessage) -> Result<(), Error> {
    let binary = msg.to_binary();
    trace!("SEND {} bytes: {:?}", binary.len(), binary);
    stream
        .write_u32(binary.len() as u32)
        .await
        .context("Failed to write size on stream")?;
    stream
        .write(&binary)
        .await
        .context("Failed to write data on stream")?;
    Ok(())
}

async fn read_net_message_from_buffer(
    stream: &mut TcpStream,
    msg_size: u32,
) -> Result<(NetMessage, usize), Error> {
    if msg_size == 0 {
        bail!("Connection closed by remote (1)")
    }

    trace!("Reading {} bytes from buffer", msg_size);
    let mut buf = vec![0; msg_size as usize];

    let data = stream.read_exact(&mut buf).await?;
    if data == 0 {
        bail!("Connection closed by remote (2)")
    }

    parse_net_message(&buf).await
}

async fn parse_net_message(buf: &[u8]) -> Result<(NetMessage, usize), Error> {
    bincode::decode_from_slice(buf, bincode::config::standard())
        .map_err(|_| anyhow!("Could not decode NetMessage"))
}
