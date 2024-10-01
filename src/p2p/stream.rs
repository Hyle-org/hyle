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

    read_net_message_from_buffer(stream).await
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
) -> Result<(NetMessage, usize), Error> {
    // Extract message size on 4 bytes and convert it to usize
    let size_buf = read_exact_cancel_safe(stream, 4).await?;
    if size_buf.len() != 4 {
        bail!("Invalid message size")
    }
    let byte_array: [u8; 4] = size_buf
        .try_into()
        .map_err(|_| anyhow!("Failed to convert Vec<u8> to [u8; 4]. Should never happen"))?;

    let msg_size = u32::from_be_bytes(byte_array) as usize;

    if msg_size == 0 {
        bail!("Connection closed by remote (2)");
    }

    // Extract the message in a cancel safe way
    trace!("Reading {} bytes from buffer", msg_size);
    let data = read_exact_cancel_safe(stream, msg_size).await?;

    parse_net_message(&data).await
}

async fn parse_net_message(buf: &[u8]) -> Result<(NetMessage, usize), Error> {
    bincode::decode_from_slice(buf, bincode::config::standard())
        .map_err(|_| anyhow!("Could not decode NetMessage"))
}

async fn read_exact_cancel_safe<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    waited_bytes: usize,
) -> Result<Vec<u8>, Error> {
    let mut all_bytes = Vec::with_capacity(waited_bytes);
    let mut bytes_read = 0;

    while bytes_read < waited_bytes {
        let mut buf: Vec<u8> = vec![0u8; waited_bytes - bytes_read];

        // Read bytes in temporary buffer
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            return Err(anyhow!(
                "{} {}",
                tokio::io::ErrorKind::UnexpectedEof,
                "early eof",
            ));
        }

        all_bytes.extend_from_slice(&buf[..n]);

        bytes_read += n;
    }

    Ok(all_bytes)
}
