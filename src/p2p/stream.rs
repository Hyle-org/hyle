use std::io::IoSlice;

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

    let mut buf = [0; 4];
    match stream.peek(&mut buf).await {
        Ok(msg_size) => {
            if msg_size != 4 {
                bail!("Invalid message size")
            }
            read_net_message_from_buffer(stream, u32::from_be_bytes(buf)).await
        }
        Err(e) => Err(anyhow!(e)),
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

async fn read_net_message_from_buffer(
    stream: &mut TcpStream,
    msg_size: u32,
) -> Result<(NetMessage, usize), Error> {
    if msg_size == 0 {
        bail!("Connection closed by remote (1)")
    }

    trace!("Reading {} bytes from buffer", msg_size);
    let mut buf = vec![0; 4 + msg_size as usize];

    let data = stream.read_exact(&mut buf).await?;
    if data == 0 {
        bail!("Connection closed by remote (2)")
    }

    parse_net_message(&buf[4..]).await
}

async fn parse_net_message(buf: &[u8]) -> Result<(NetMessage, usize), Error> {
    bincode::decode_from_slice(buf, bincode::config::standard())
        .map_err(|_| anyhow!("Could not decode NetMessage"))
}
