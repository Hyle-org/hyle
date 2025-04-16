use std::net::SocketAddr;

use borsh::{BorshDeserialize, BorshSerialize};
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use tokio_util::codec::{Decoder, Encoder, Framed};

use crate::{net::TcpStream, tcp::get_current_timestamp};
use anyhow::{bail, Result};
use tracing::{debug, info, trace, warn};

use super::{TcpMessage, TcpMessageCodec};

type TcpSender<ClientCodec, Req> =
    SplitSink<Framed<TcpStream, TcpMessageCodec<ClientCodec>>, TcpMessage<Req>>;
type TcpReceiver<ClientCodec> = SplitStream<Framed<TcpStream, TcpMessageCodec<ClientCodec>>>;

pub struct TcpClient<ClientCodec, Req, Res>
where
    ClientCodec: Decoder<Item = Res> + Encoder<Req> + Default,
    Req: Clone,
    Res: Clone + std::fmt::Debug,
{
    pub id: String,
    pub sender: TcpSender<ClientCodec, Req>,
    pub receiver: TcpReceiver<ClientCodec>,
    pub last_ping: u64,
    pub socket_addr: SocketAddr,
}

impl<ClientCodec, Req, Res> TcpClient<ClientCodec, Req, Res>
where
    ClientCodec: Decoder<Item = Res> + Encoder<Req> + Default + Send + 'static,
    <ClientCodec as Decoder>::Error: std::fmt::Debug + Send,
    <ClientCodec as Encoder<Req>>::Error: std::fmt::Debug + Send,
    Res: BorshDeserialize + std::fmt::Debug + Clone + Send + 'static,
    Req: BorshSerialize + Clone + Send + 'static,
{
    pub async fn connect<
        Id: std::fmt::Display,
        A: crate::net::ToSocketAddrs + std::fmt::Display,
    >(
        id: Id,
        target: A,
    ) -> Result<TcpClient<ClientCodec, Req, Res>> {
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        let tcp_stream = loop {
            debug!("TcpClient {} - Trying to connect to {}", id, &target);
            match TcpStream::connect(&target).await {
                Ok(stream) => break stream,
                Err(e) => {
                    if start.elapsed() >= timeout {
                        bail!(
                            "TcpClient {} - Failed to connect to {}: {}. Timeout reached.",
                            id,
                            &target,
                            e
                        );
                    }
                    warn!(
                        "TcpClient {} - Failed to connect to {}: {}. Retrying in 1 second...",
                        id, target, e
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        };
        let addr = tcp_stream.local_addr()?;
        info!("TcpClient {} - Connected to data stream on {}.", id, addr);

        let (sender, receiver) =
            Framed::new(tcp_stream, TcpMessageCodec::<ClientCodec>::default()).split();

        Ok(TcpClient {
            id: id.to_string(),
            sender,
            receiver,
            last_ping: get_current_timestamp(),
            socket_addr: addr,
        })
    }

    pub async fn send<T: Into<Req>>(&mut self, msg: T) -> Result<()> {
        self.sender
            .send(TcpMessage::<Req>::Data(msg.into()))
            .await?;

        Ok(())
    }
    pub async fn ping(&mut self) -> Result<()> {
        self.sender.send(TcpMessage::<Req>::Ping).await?;

        Ok(())
    }

    pub async fn recv(&mut self) -> Option<Res> {
        loop {
            match self.receiver.next().await {
                Some(Ok(TcpMessage::Data(data))) => {
                    // Interesting message
                    trace!("Some data for client {}", self.id);
                    return Some(data);
                }
                None => {
                    // End of stream
                    warn!("End of stream for client {}", self.id);
                    return None;
                }
                Some(Err(e)) => {
                    warn!("Error while streaming data from peer: {:#}", e);
                    return None;
                }
                Some(Ok(TcpMessage::Ping)) => {
                    trace!("Ping received for client {}", self.id);
                }
            }
        }
    }

    pub fn split(self) -> (TcpSender<ClientCodec, Req>, TcpReceiver<ClientCodec>) {
        (self.sender, self.receiver)
    }

    pub async fn close(mut self) -> Result<()> {
        self.sender.close().await?;
        Ok(())
    }
}
