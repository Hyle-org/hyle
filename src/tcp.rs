use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use bytes::BytesMut;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use hyle_model::utils::get_current_timestamp;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};
use tokio_util::codec::{Decoder, Encoder, Framed, LengthDelimitedCodec};

use anyhow::{anyhow, bail, Context, Result};
use tracing::{debug, error, info, trace, warn};

use crate::utils::logger::LogMe;

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize, PartialEq)]
pub enum TcpMessage<Data: Clone> {
    Ping,
    Data(Data),
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub enum TcpCommand<Data: Clone> {
    Broadcast(Box<Data>),
    Send(String, Box<Data>),
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub struct TcpEvent<Data: Clone> {
    pub dest: String,
    pub data: Box<Data>,
}

// A Generic Codec to unwrap/wrap with TcpMessage<T>
#[derive(Debug)]
pub struct TcpMessageCodec<T> {
    _marker: std::marker::PhantomData<T>,
    ldc: LengthDelimitedCodec,
}

impl<T> Default for TcpMessageCodec<T> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
            ldc: LengthDelimitedCodec::default(),
        }
    }
}

impl<Codec, Decodable> Decoder for TcpMessageCodec<Codec>
where
    Codec: Decoder<Item = Decodable> + Send,
    Decodable: BorshDeserialize + Clone,
{
    type Item = TcpMessage<Decodable>;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }

        let Some(src_ldc) = self.ldc.decode(src)? else {
            return Ok(None);
        };

        let msg: TcpMessage<Decodable> =
            borsh::from_slice(&mut &src_ldc[..]).context("Decode TcpServerMessage wrapper type")?;

        Ok(Some(msg))
    }
}

impl<Codec, Encodable> Encoder<TcpMessage<Encodable>> for TcpMessageCodec<Codec>
where
    Codec: Encoder<Encodable> + Send,
    Encodable: BorshSerialize + Clone,
{
    type Error = anyhow::Error;

    fn encode(
        &mut self,
        item: TcpMessage<Encodable>,
        dst: &mut BytesMut,
    ) -> Result<(), Self::Error> {
        let serialized = borsh::to_vec(&item).context("Encoding to vec")?;

        self.ldc.encode(serialized.into(), dst)?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct TcpConnectionPool<Codec, In: Clone, Out: Clone + std::fmt::Debug>
where
    Codec: Decoder<Item = In> + Encoder<Out> + Default,
{
    new_peer_listener: tokio::net::TcpListener,
    peers: HashMap<String, PeerStream<Codec, In, Out>>,
}

impl<Codec, In, Out> TcpConnectionPool<Codec, In, Out>
where
    Codec: Decoder<Item = In> + Encoder<Out> + Default + Send + 'static,
    <Codec as Decoder>::Error: std::fmt::Debug + Send,
    <Codec as Encoder<Out>>::Error: std::fmt::Debug + Send,
    In: BorshDeserialize + Clone + Send + 'static + std::fmt::Debug,
    Out: BorshSerialize + Clone + Send + 'static + std::fmt::Debug,
{
    pub async fn listen(addr: String) -> Result<TcpConnectionPool<Codec, In, Out>> {
        let new_peer_listener = TcpListener::bind(&addr).await?;
        info!(
            "📡  Starting TcpServerConnection Pool, listening for stream requests on {}",
            addr
        );
        Ok(TcpConnectionPool {
            new_peer_listener,
            peers: HashMap::new(),
        })
    }

    pub async fn run_in_background(
        mut self,
    ) -> Result<(Sender<TcpCommand<Out>>, Receiver<TcpEvent<In>>)> {
        let (out_sender, out_receiver) = tokio::sync::mpsc::channel(100);
        let (in_sender, in_receiver) = tokio::sync::mpsc::channel(100);

        tokio::task::Builder::new()
            .name("tcp-connection-pool-loop")
            .spawn(async move {
                info!("Starting run loop in background");
                _ = self
                    .run(out_receiver, in_sender)
                    .await
                    .log_error("Running connection pool loop");
            })?;

        Ok((out_sender, in_receiver))
    }

    async fn run(
        &mut self,
        mut pool_recv: Receiver<TcpCommand<Out>>,
        pool_sender: Sender<TcpEvent<In>>,
    ) -> Result<()> {
        let (ping_sender, mut ping_receiver) = tokio::sync::mpsc::channel(100);
        loop {
            if false {
                break;
            }
            tokio::select! {
                Ok((stream, addr)) = self.new_peer_listener.accept() => {
                    _  = self.setup_peer(ping_sender.clone(), pool_sender.clone(), stream, &addr.ip().to_string());
                }

                Some(to_send) = pool_recv.recv() => {
                    _ = self.send(to_send).await.log_error("Sending message");
                }

                Some(peer_id) = ping_receiver.recv() => {
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.last_ping = get_current_timestamp();
                    }
                }
            }
        }

        Ok(())
    }

    async fn send(&mut self, msg: TcpCommand<Out>) -> Result<()> {
        match msg {
            TcpCommand::Broadcast(data) => {
                debug!("Broadcasting data {:?} to all", data);
                let mut to_remove = Vec::new();
                for (peer_id, peer) in self.peers.iter_mut() {
                    let last_ping = peer.last_ping;
                    if last_ping + 60 * 5 < get_current_timestamp() {
                        info!("peer {} timed out", &peer_id);
                        peer.abort.abort();
                        to_remove.push(peer_id.clone());
                    } else {
                        debug!("streaming event to peer {}", &peer_id);
                        match peer.sender.send(TcpMessage::Data((*data).clone())).await {
                            Ok(_) => {}
                            Err(e) => {
                                debug!(
                                    "Couldn't send new block to peer {}, stopping streaming  : {:?}",
                                    &peer_id, e
                                );
                                peer.abort.abort();
                                to_remove.push(peer_id.clone());
                            }
                        }
                    }
                }
                for peer_id in to_remove {
                    self.peers.remove(&peer_id);
                }
            }
            TcpCommand::Send(to, data) => {
                // Retry on error
                debug!("Sending data {:?} to {}", data, to);
                let peer_stream = self
                    .peers
                    .get_mut(&to)
                    .context(format!("Getting peer {} to send a message", &to))?;

                peer_stream
                    .sender
                    .send(TcpMessage::Data(*data))
                    .await
                    .map_err(|_| anyhow!("Sending message to peer {}", &to))?;
            }
        }

        Ok(())
    }

    fn setup_peer(
        &mut self,
        ping_sender: Sender<String>,
        sender_received_messages: Sender<TcpEvent<In>>,
        tcp_stream: TcpStream,
        peer_ip: &String,
    ) -> Result<()> {
        let (sender, mut receiver) =
            Framed::new(tcp_stream, TcpMessageCodec::<Codec>::default()).split();
        // Start a task to process pings from the peer.
        // We do the processing in the main select! loop to keep things synchronous.
        // This makes it easier to store data in the same struct without mutexing.
        let cloned_peer_ip = peer_ip.clone();
        let abort = tokio::task::Builder::new()
            .name("peer-stream-abort")
            .spawn(async move {
                while let Some(msg) = receiver.next().await {
                    debug!("Received message {:?}", &msg);
                    match msg {
                        Ok(TcpMessage::Ping) => {
                            _ = ping_sender.send(cloned_peer_ip.clone()).await;
                        }
                        Ok(TcpMessage::Data(data)) => {
                            _ = sender_received_messages
                                .send(TcpEvent {
                                    dest: cloned_peer_ip.clone(),
                                    data: Box::new(data),
                                })
                                .await;
                        }
                        Err(_) => {
                            error!("Decoding message in peer event loop");
                        }
                    }
                }
            })?;

        // Then store data so we can send new blocks as they come.
        self.peers.insert(
            peer_ip.to_string(),
            PeerStream {
                last_ping: get_current_timestamp(),
                sender,
                abort,
            },
        );

        Ok(())
    }
}

/// A peer we are streaming blocks to
#[derive(Debug)]
struct PeerStream<Codec, In, Out>
where
    In: Clone,
    Out: Clone + std::fmt::Debug,
    Codec: Decoder<Item = In> + Encoder<Out> + Default,
{
    /// Last timestamp we received a ping from the peer.
    last_ping: u64,
    /// Sender to stream blocks to the peer
    sender: SplitSink<Framed<TcpStream, TcpMessageCodec<Codec>>, TcpMessage<Out>>,
    /// Handle to abort the receiving side of the stream
    abort: JoinHandle<()>,
}

pub struct TcpClient<ClientCodec, In, Out>
where
    ClientCodec: Decoder<Item = Out> + Encoder<In> + Default,
    In: Clone,
    Out: Clone + std::fmt::Debug,
{
    id: String,
    sender: SplitSink<Framed<TcpStream, TcpMessageCodec<ClientCodec>>, TcpMessage<In>>,
    receiver: SplitStream<Framed<TcpStream, TcpMessageCodec<ClientCodec>>>,
}

impl<ClientCodec, In, Out> TcpClient<ClientCodec, In, Out>
where
    ClientCodec: Decoder<Item = Out> + Encoder<In> + Default + Send + 'static,
    <ClientCodec as Decoder>::Error: std::fmt::Debug + Send,
    <ClientCodec as Encoder<In>>::Error: std::fmt::Debug + Send,
    Out: BorshDeserialize + std::fmt::Debug + Clone + Send + 'static,
    In: BorshSerialize + Clone + Send + 'static,
{
    pub async fn connect(id: String, target: &String) -> Result<TcpClient<ClientCodec, In, Out>> {
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        let tcp_stream = loop {
            debug!("Trying to connect to {}", target);
            match TcpStream::connect(&target).await {
                Ok(stream) => break stream,
                Err(e) => {
                    if start.elapsed() >= timeout {
                        bail!("Failed to connect to {}: {}. Timeout reached.", target, e);
                    }
                    warn!(
                        "Failed to connect to {}: {}. Retrying in 1 second...",
                        target, e
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        };
        let addr = tcp_stream.local_addr()?;
        info!(
            "Client {} connected to data stream to {} on {}.",
            id, &target, addr
        );

        let (sender, receiver) =
            Framed::new(tcp_stream, TcpMessageCodec::<ClientCodec>::default()).split();

        Ok(TcpClient {
            id,
            sender,
            receiver,
        })
    }

    pub async fn send(&mut self, msg: In) -> Result<()> {
        self.sender.send(TcpMessage::<In>::Data(msg)).await?;

        Ok(())
    }
    pub async fn ping(&mut self) -> Result<()> {
        self.sender.send(TcpMessage::<In>::Ping).await?;

        Ok(())
    }

    pub async fn recv(&mut self) -> Option<Out> {
        loop {
            match self.receiver.next().await {
                Some(Ok(TcpMessage::Data(data))) => {
                    // Interesting message
                    trace!("Some interesting data for client {}", self.id);
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

    pub async fn close(mut self) -> Result<()> {
        self.sender.close().await?;
        Ok(())
    }
}

macro_rules! implem_tcp_codec {
    ($codec:ident, decode: $in:ty, encode: $out:ty) => {
        impl tokio_util::codec::Encoder<$out> for $codec {
            type Error = anyhow::Error;

            fn encode(
                &mut self,
                event: $out,
                dst: &mut bytes::BytesMut,
            ) -> Result<(), Self::Error> {
                let bytes: Vec<u8> = borsh::to_vec(&event)?;
                dst.put_slice(bytes.as_slice());
                Ok(())
            }
        }

        impl tokio_util::codec::Decoder for $codec {
            type Item = $in;
            type Error = anyhow::Error;

            fn decode(
                &mut self,
                src: &mut bytes::BytesMut,
            ) -> Result<Option<Self::Item>, Self::Error> {
                Ok(Some(
                    borsh::from_slice(&src).context(format!("Decoding bytes with borsh",))?,
                ))
            }
        }
    };
}

pub(crate) use implem_tcp_codec;

#[cfg(test)]
pub mod tests {
    use std::time::Duration;

    use crate::{
        data_availability::codec::{
            DataAvailabilityClientCodec, DataAvailabilityServerCodec, DataAvailabilityServerEvent,
            DataAvailabilityServerRequest,
        },
        tcp::{TcpClient, TcpCommand, TcpConnectionPool, TcpMessage},
    };

    use anyhow::Result;
    use futures::TryStreamExt;
    use hyle_model::{BlockHeight, SignedBlock};

    #[test_log::test(tokio::test)]
    async fn tcp_test() -> Result<()> {
        let test: TcpConnectionPool<
            DataAvailabilityServerCodec,
            DataAvailabilityServerRequest,
            DataAvailabilityServerEvent,
        > = TcpConnectionPool::listen("0.0.0.0:2345".to_string()).await?;

        let (sender, mut receiver) = test.run_in_background().await?;

        let mut client: TcpClient<
            DataAvailabilityClientCodec,
            DataAvailabilityServerRequest,
            DataAvailabilityServerEvent,
        > = TcpClient::connect("me".to_string(), &"0.0.0.0:2345".to_string()).await?;

        // Ping
        let _ = client.ping().await?;

        // Send data to server
        let _ = client
            .send(DataAvailabilityServerRequest(BlockHeight(2)))
            .await?;

        tokio::time::sleep(Duration::from_secs(1)).await;

        let d = receiver.try_recv().unwrap().data;

        assert_eq!(DataAvailabilityServerRequest(BlockHeight(2)), *d);
        assert!(receiver.try_recv().is_err());

        // From server to client
        _ = sender
            .send(TcpCommand::Broadcast(Box::new(
                DataAvailabilityServerEvent::SignedBlock(SignedBlock::default()),
            )))
            .await;

        assert_eq!(
            client.receiver.try_next().await.unwrap().unwrap(),
            TcpMessage::Data(DataAvailabilityServerEvent::SignedBlock(
                SignedBlock::default()
            ))
        );

        Ok(())
    }
}
