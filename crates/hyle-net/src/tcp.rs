use std::{
    collections::HashMap,
    net::Ipv4Addr,
    time::{SystemTime, UNIX_EPOCH},
};

use borsh::{BorshDeserialize, BorshSerialize};
use bytes::BytesMut;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use tokio::{
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};
use tokio_util::codec::{Decoder, Encoder, Framed, LengthDelimitedCodec};

use crate::net::{TcpListener, TcpStream};
use anyhow::{anyhow, bail, Context, Result};
use tracing::{debug, error, info, trace, warn};

pub fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize, PartialEq)]
pub enum TcpMessage<Data: Clone> {
    Ping,
    Data(Data),
}

// TODO: Add ConnectPeer, RemovePeer ?
#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
pub enum TcpCommand<Data: Clone> {
    Broadcast(Data),
    Send(String, Data),
}

// TODO: when useful, we can add NewPeer event, PeerDisconnected ...
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
            borsh::from_slice(&src_ldc[..]).context("Decode TcpServerMessage wrapper type")?;

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

pub struct TcpServer<Codec, Req: Clone, Res: Clone + std::fmt::Debug>
where
    Codec: Decoder<Item = Req> + Encoder<Res> + Default,
{
    tcp_listener: TcpListener,
    pool_sender: Sender<TcpEvent<Req>>,
    pool_receiver: Receiver<TcpEvent<Req>>,
    ping_sender: Sender<String>,
    ping_receiver: Receiver<String>,
    peers: HashMap<String, PeerStream<Codec, Req, Res>>,
}

impl<Codec, Req, Res> TcpServer<Codec, Req, Res>
where
    Codec: Decoder<Item = Req> + Encoder<Res> + Default + Send + 'static,
    <Codec as Decoder>::Error: std::fmt::Debug + Send,
    <Codec as Encoder<Res>>::Error: std::fmt::Debug + Send,
    Req: BorshDeserialize + Clone + Send + 'static + std::fmt::Debug,
    Res: BorshSerialize + Clone + Send + 'static + std::fmt::Debug,
{
    pub async fn start(port: u16, pool_name: &'static str) -> Result<Self> {
        let tcp_listener = TcpListener::bind(&(Ipv4Addr::UNSPECIFIED, port)).await?;
        let (pool_sender, pool_receiver) = tokio::sync::mpsc::channel(100);
        let (ping_sender, ping_receiver) = tokio::sync::mpsc::channel(100);
        debug!(
            "Starting TcpConnectionPool {}, listening for stream requests on {}",
            &pool_name, port
        );
        Ok(TcpServer::<Codec, Req, Res> {
            peers: HashMap::new(),
            tcp_listener,
            pool_sender,
            pool_receiver,
            ping_sender,
            ping_receiver,
        })
    }

    pub async fn listen_next(&mut self) -> Option<TcpEvent<Req>> {
        loop {
            tokio::select! {
                Ok((stream, addr)) = self.tcp_listener.accept() => {
                    _  = self.setup_peer(stream, &addr.to_string());
                }

                Some(peer_id) = self.ping_receiver.recv() => {
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.last_ping = get_current_timestamp();
                    }
                }
                message = self.pool_receiver.recv() => {
                    return message;
                }
            }
        }
    }

    pub async fn broadcast(&mut self, msg: Res) -> Result<()> {
        self.send_command(TcpCommand::Broadcast(msg)).await
    }

    pub async fn send(&mut self, to: String, msg: Res) -> Result<()> {
        self.send_command(TcpCommand::Send(to, msg)).await
    }

    pub async fn send_command(&mut self, msg: TcpCommand<Res>) -> Result<()> {
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
            TcpCommand::Send(to, data) => {
                // FIXME: Retry on error ?
                debug!("Sending data {:?} to {}", data, to);
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
        tcp_stream: TcpStream,
        // FIXME: Use something safer to identify a peer. For now its ok to use its ip
        peer_ip: &String,
    ) -> Result<()> {
        let (sender, mut receiver) =
            Framed::new(tcp_stream, TcpMessageCodec::<Codec>::default()).split();
        // Start a task to process pings from the peer.
        // We do the processing in the main select! loop to keep things synchronous.
        // This makes it easier to store data in the same struct without mutexing.
        let ping_sender = self.ping_sender.clone();
        let pool_sender = self.pool_sender.clone();
        let cloned_peer_ip = peer_ip.clone();
        let abort = tokio::spawn(async move {
            while let Some(msg) = receiver.next().await {
                debug!("Received message {:?}", &msg);
                match msg {
                    Ok(TcpMessage::Ping) => {
                        _ = ping_sender.send(cloned_peer_ip.clone()).await;
                    }
                    Ok(TcpMessage::Data(data)) => {
                        _ = pool_sender
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
        });

        tracing::debug!("Peer {} connected", peer_ip);
        // Store peer in the list.
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

    // TODO: clean method to stop everything
}

/// A peer we can send data to
#[derive(Debug)]
struct PeerStream<Codec, In, Out>
where
    In: Clone,
    Out: Clone + std::fmt::Debug,
    Codec: Decoder<Item = In> + Encoder<Out> + Default,
{
    /// Last timestamp we received a ping from the peer.
    last_ping: u64,
    /// Sender to stream data to the peer
    sender: SplitSink<Framed<TcpStream, TcpMessageCodec<Codec>>, TcpMessage<Out>>,
    /// Handle to abort the receiving side of the stream
    abort: JoinHandle<()>,
}

pub struct TcpClient<ClientCodec, Req, Res>
where
    ClientCodec: Decoder<Item = Res> + Encoder<Req> + Default,
    Req: Clone,
    Res: Clone + std::fmt::Debug,
{
    id: String,
    sender: SplitSink<Framed<TcpStream, TcpMessageCodec<ClientCodec>>, TcpMessage<Req>>,
    receiver: SplitStream<Framed<TcpStream, TcpMessageCodec<ClientCodec>>>,
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

    pub async fn close(mut self) -> Result<()> {
        self.sender.close().await?;
        Ok(())
    }
}

#[macro_export]
macro_rules! implem_tcp_codec {
    ($codec:ident, decode: $in:ty, encode: $out:ty) => {
        #[derive(Default, Debug)]
        pub struct $codec;

        impl tokio_util::codec::Encoder<$out> for $codec {
            type Error = anyhow::Error;

            fn encode(
                &mut self,
                event: $out,
                dst: &mut bytes::BytesMut,
            ) -> Result<(), Self::Error> {
                let bytes: Vec<u8> = borsh::to_vec(&event)?;
                bytes::BufMut::put_slice(dst, bytes.as_slice());
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

pub use implem_tcp_codec;

#[macro_export]
macro_rules! tcp_client_server {
    ($vis:vis $name:ident, request: $req:ty, response: $res:ty) => {
        paste::paste! {
        $vis mod [< codec_ $name:snake >] {
            #![allow(unused)]
            pub use super::$req;
            pub use super::$res;
            use anyhow::{Context, Result};
            $crate::tcp::implem_tcp_codec!{
                ClientCodec,
                decode: $res,
                encode: $req
            }
            $crate::tcp::implem_tcp_codec!{
                ServerCodec,
                decode: $req,
                encode: $res
            }

            pub type Client = $crate::tcp::TcpClient<ClientCodec, $req, $res>;
            pub type Server = $crate::tcp::TcpServer<ServerCodec, $req, $res>;

            pub async fn start_server(port: u16) -> Result<Server> {
                $crate::tcp::TcpServer::<ServerCodec, $req, $res>::start(port, stringify!($name)).await
            }
            pub async fn connect<Id: std::fmt::Display, A: $crate::net::ToSocketAddrs + std::fmt::Display>(id: Id, addr: A) -> Result<Client> {
                $crate::tcp::TcpClient::<ClientCodec, $req, $res>::connect(id, addr).await
            }
        }
        }
    };
}

pub use tcp_client_server;

#[cfg(test)]
pub mod tests {
    use std::time::Duration;

    use crate::tcp::TcpMessage;

    use anyhow::Result;
    use borsh::{BorshDeserialize, BorshSerialize};
    use futures::TryStreamExt;

    #[derive(BorshDeserialize, BorshSerialize, Clone, Debug, PartialEq, Eq)]
    pub struct DataAvailabilityRequest(pub usize);

    #[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
    pub enum DataAvailabilityEvent {
        SignedBlock(String),
    }

    tcp_client_server! {
        DataAvailability,
        request: DataAvailabilityRequest,
        response: DataAvailabilityEvent
    }

    #[tokio::test]
    async fn tcp_test() -> Result<()> {
        let mut server = codec_data_availability::start_server(2345).await?;

        let mut client = codec_data_availability::connect("me".to_string(), "0.0.0.0:2345").await?;

        // Ping
        client.ping().await?;

        // Send data to server
        client.send(DataAvailabilityRequest(2)).await?;

        tokio::time::sleep(Duration::from_secs(1)).await;

        let d = server.listen_next().await.unwrap().data;

        assert_eq!(DataAvailabilityRequest(2), *d);
        assert!(server.pool_receiver.try_recv().is_err());

        // From server to client
        _ = server
            .broadcast(DataAvailabilityEvent::SignedBlock("blabla".to_string()))
            .await;

        assert_eq!(
            client.receiver.try_next().await.unwrap().unwrap(),
            TcpMessage::Data(DataAvailabilityEvent::SignedBlock("blabla".to_string()))
        );

        Ok(())
    }
}
