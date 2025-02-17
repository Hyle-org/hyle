use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use bytes::{BufMut, BytesMut};
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
use tokio_util::codec::{Decoder, Encoder, Framed};

use anyhow::{anyhow, Context, Result};
use tracing::{debug, error, info};

use crate::{
    data_availability::codec::{
        DataAvailabilityClientCodec, DataAvailabilityServerCodec, DataAvailabilityServerEvent,
        DataAvailabilityServerRequest,
    },
    utils::logger::LogMe,
};

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub enum TcpMessage<Data: Clone> {
    Ping,
    Data(Data),
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub enum TcpOutgoingMessage<Data: Clone> {
    Broadcast(Data),
    Send(String, Data),
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub struct TcpIncomingMessage<Data: Clone> {
    dest: String,
    data: Data,
}

// A Generic Codec to unwrap/wrap with TcpMessage<T>
#[derive(Debug)]
pub struct TcpMessageCodec<T> {
    _marker: std::marker::PhantomData<T>,
}

impl<T> Default for TcpMessageCodec<T> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
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

        let msg: TcpMessage<Decodable> =
            borsh::from_slice(&mut &src[..]).context("Decode TcpServerMessage wrapper type")?;

        src.clear();
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

        dst.put_slice(serialized.as_slice());
        Ok(())
    }
}

pub struct TcpConnectionPool<Codec, In: Clone, Out: Clone>
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
    ) -> Result<(
        Sender<Box<TcpOutgoingMessage<Out>>>,
        Receiver<Box<TcpIncomingMessage<In>>>,
    )> {
        let (out_sender, out_receiver) = tokio::sync::mpsc::channel(100);
        let (in_sender, in_receiver) = tokio::sync::mpsc::channel(100);

        tokio::task::Builder::new()
            .name("tcp-connection-pool-loop")
            .spawn(async move {
                _ = self
                    .run(out_receiver, in_sender)
                    .await
                    .log_error("Running connection pool loop");
            })?;

        Ok((out_sender, in_receiver))
    }

    async fn run(
        &mut self,
        mut pool_recv: Receiver<Box<TcpOutgoingMessage<Out>>>,
        pool_sender: Sender<Box<TcpIncomingMessage<In>>>,
    ) -> Result<()> {
        let (ping_sender, mut ping_receiver) = tokio::sync::mpsc::channel(100);
        loop {
            // ;)
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

    async fn send(&mut self, msg: Box<TcpOutgoingMessage<Out>>) -> Result<()> {
        match *msg {
            TcpOutgoingMessage::Broadcast(data) => {
                let mut to_remove = Vec::new();
                for (peer_id, peer) in self.peers.iter_mut() {
                    let last_ping = peer.last_ping;
                    if last_ping + 60 * 5 < get_current_timestamp() {
                        info!("peer {} timed out", &peer_id);
                        peer.abort.abort();
                        to_remove.push(peer_id.clone());
                    } else {
                        debug!("streaming event {:?} to peer {}", &data, &peer_id);
                        match peer.sender.send(TcpMessage::Data(data.clone())).await {
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
            TcpOutgoingMessage::Send(to, data) => {
                // Retry on error
                let peer_stream = self
                    .peers
                    .get_mut(&to)
                    .context(format!("Getting peer {} to send a message", &to))?;
                peer_stream
                    .sender
                    .send(TcpMessage::Data(data))
                    .await
                    .map_err(|_| anyhow!("Sending message to peer {}", &to))?;
            }
        }

        Ok(())
    }

    fn setup_peer(
        &mut self,
        ping_sender: Sender<String>,
        sender_received_messages: Sender<Box<TcpIncomingMessage<In>>>,
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
                info!("Starting receiving messages");
                while let Some(msg) = receiver.next().await {
                    info!("Received message {:?}", &msg);
                    match msg {
                        Ok(message) => match message {
                            TcpMessage::Ping => {
                                _ = ping_sender.send(cloned_peer_ip.clone()).await;
                            }
                            TcpMessage::Data(data) => {
                                _ = sender_received_messages
                                    .send(Box::new(TcpIncomingMessage {
                                        dest: cloned_peer_ip.clone(),
                                        data,
                                    }))
                                    .await;
                            }
                        },
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
    Out: Clone,
    Codec: Decoder<Item = In> + Encoder<Out> + Default,
{
    /// Last timestamp we received a ping from the peer.
    last_ping: u64,
    /// Sender to stream blocks to the peer
    sender: SplitSink<Framed<TcpStream, TcpMessageCodec<Codec>>, TcpMessage<Out>>,
    /// Handle to abort the receiving side of the stream
    abort: JoinHandle<()>,
}

struct TcpClient<ClientCodec, In, Out>
where
    ClientCodec: Decoder<Item = Out> + Encoder<In> + Default,
    In: Clone,
    Out: Clone,
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
    Out: BorshDeserialize + Clone + Send + 'static,
    In: BorshSerialize + Clone + Send + 'static,
{
    pub async fn connect(id: String, addr: &String) -> Result<TcpClient<ClientCodec, In, Out>> {
        let tcp_stream = TcpStream::connect(addr).await?;
        let (sender, receiver) =
            Framed::new(tcp_stream, TcpMessageCodec::<ClientCodec>::default()).split();

        Ok(TcpClient {
            id,
            sender,
            receiver,
        })
    }

    pub async fn send(&mut self, msg: In) -> Result<()> {
        self.sender.send(TcpMessage::Data(msg)).await?;

        Ok(())
    }
    pub async fn ping(&mut self) -> Result<()> {
        self.sender.send(TcpMessage::Ping).await?;

        Ok(())
    }
}

#[tokio::test]
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

    let _ = client.ping().await?;
    let _ = client.send(DataAvailabilityServerRequest::Ping).await?;

    let d = receiver.try_recv().unwrap().data;

    assert_eq!(DataAvailabilityServerRequest::Ping, d);

    Ok(())
}
