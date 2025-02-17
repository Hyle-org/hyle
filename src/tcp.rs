use std::{collections::HashMap, default, marker::PhantomData};

use borsh::{BorshDeserialize, BorshSerialize};
use bytes::{BufMut, BytesMut};
use futures::{stream::SplitSink, SinkExt, StreamExt};
use hyle_model::{utils::get_current_timestamp, SignedBlock};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};
use tokio_util::codec::{Decoder, Encoder, Framed};

use anyhow::{anyhow, Context, Result};
use tracing::{error, info};

use crate::{
    data_availability::codec::{
        DataAvailabilityServerCodec, DataAvailabilityServerEvent, DataAvailabilityServerRequest,
    },
    utils::logger::LogMe,
};

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub struct TcpMessage<Data: Clone> {
    peer_origin: String,
    peer_dest: Option<String>,
    data: Option<Data>,
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

struct Test;
impl Test {
    async fn test() -> Result<()> {
        let test: TcpConnectionPool<
            DataAvailabilityServerCodec,
            DataAvailabilityServerRequest,
            DataAvailabilityServerEvent,
        > = TcpConnectionPool::listen("addr".to_string()).await?;

        let (sender, receiver) = test.run_in_background().await?;
        Ok(())
    }
}

impl<Codec, In, Out> TcpConnectionPool<Codec, In, Out>
where
    Codec: Decoder<Item = In> + Encoder<Out> + Default + Send + 'static,
    <Codec as Decoder>::Error: std::fmt::Debug + Send,
    <Codec as Encoder<Out>>::Error: std::fmt::Debug + Send,
    In: BorshDeserialize + Clone + Send + 'static,
    Out: BorshSerialize + Clone + Send + 'static,
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
    ) -> Result<(Sender<Box<TcpMessage<Out>>>, Receiver<Box<TcpMessage<In>>>)> {
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
        mut pool_recv: Receiver<Box<TcpMessage<Out>>>,
        pool_sender: Sender<Box<TcpMessage<In>>>,
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

    async fn send(&mut self, msg: Box<TcpMessage<Out>>) -> Result<()> {
        if let Some(dest) = msg.peer_dest.clone() {
            let peer_stream = self
                .peers
                .get_mut(&dest)
                .context("Getting peer to send a message")?;
            peer_stream
                .sender
                .send(*msg)
                .await
                .map_err(|_| anyhow!("Sending message to peer {}", &dest))?;
        } else {
            // Broadcast
            for (_peer_id, peer_stream) in self.peers.iter_mut() {
                if let Err(_) =
                    SinkExt::<TcpMessage<Out>>::send(&mut peer_stream.sender, (*msg).clone()).await
                {
                    error!("Sending message to peer {} when broadcasting", _peer_id);
                }
            }
        }

        Ok(())
    }

    fn setup_peer(
        &mut self,
        ping_sender: Sender<String>,
        sender_received_messages: Sender<Box<TcpMessage<In>>>,
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
                    match msg {
                        Ok(message) => {
                            // Empty message is a ping
                            if message.data.is_none() {
                                _ = ping_sender.send(cloned_peer_ip.clone()).await;
                            } else {
                                _ = sender_received_messages.send(Box::new(message)).await;
                            }
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
