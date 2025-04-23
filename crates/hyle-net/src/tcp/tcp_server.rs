use std::{
    collections::HashMap,
    io::{Error, ErrorKind},
    net::{Ipv4Addr, SocketAddr},
};

use anyhow::Context;
use borsh::{BorshDeserialize, BorshSerialize};
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_util::codec::{Decoder, Encoder, Framed};

use crate::{
    net::{TcpListener, TcpStream},
    tcp::{get_current_timestamp, TcpMessage, TcpMessageCodec},
};
use tracing::{debug, error, trace, warn};

use super::{tcp_client::TcpClient, SocketStream, TcpEvent};

pub struct TcpServer<Codec, Req: Clone + std::fmt::Debug, Res: Clone + std::fmt::Debug>
where
    Codec: Decoder<Item = Req> + Encoder<Res> + Default,
{
    tcp_listener: TcpListener,
    max_frame_length: Option<usize>,
    pool_sender: Sender<TcpEvent<Req>>,
    pool_receiver: Receiver<TcpEvent<Req>>,
    ping_sender: Sender<String>,
    ping_receiver: Receiver<String>,
    sockets: HashMap<String, SocketStream<Codec, Req, Res>>,
}

impl<Codec, Req, Res> TcpServer<Codec, Req, Res>
where
    Codec: Decoder<Item = Req> + Encoder<Res> + Default + Send + 'static,
    <Codec as Decoder>::Error: std::fmt::Debug + Send,
    <Codec as Encoder<Res>>::Error: std::fmt::Debug + Send,
    Req: BorshDeserialize + Clone + Send + 'static + std::fmt::Debug,
    Res: BorshSerialize + Clone + Send + 'static + std::fmt::Debug,
{
    pub async fn start(port: u16, pool_name: String) -> anyhow::Result<Self> {
        Self::start_with_opts(port, None, pool_name).await
    }

    pub async fn start_with_opts(
        port: u16,
        max_frame_length: Option<usize>,
        pool_name: String,
    ) -> anyhow::Result<Self> {
        let tcp_listener = TcpListener::bind(&(Ipv4Addr::UNSPECIFIED, port)).await?;
        let (pool_sender, pool_receiver) = tokio::sync::mpsc::channel(100);
        let (ping_sender, ping_receiver) = tokio::sync::mpsc::channel(100);
        debug!(
            "Starting TcpConnectionPool {}, listening for stream requests on {} with max_frame_len: {:?}",
            &pool_name, port, max_frame_length
        );
        Ok(TcpServer::<Codec, Req, Res> {
            sockets: HashMap::new(),
            max_frame_length,
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
                Ok((stream, socket_addr)) = self.tcp_listener.accept() => {
                    let codec = match self.max_frame_length {
                        Some(len) => TcpMessageCodec::<Codec>::new(len),
                        None => TcpMessageCodec::<Codec>::default()
                    };

                    let (sender, receiver) = Framed::new(stream, codec).split();

                    _  = self.setup_stream(sender, receiver, &socket_addr.to_string());
                }

                Some(socket_addr) = self.ping_receiver.recv() => {
                    if let Some(socket) = self.sockets.get_mut(&socket_addr) {
                        socket.last_ping = get_current_timestamp();
                    }
                }
                message = self.pool_receiver.recv() => {
                    return message;
                }
            }
        }
    }

    /// Local_addr of the underlying tcp_listener
    pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
        self.tcp_listener
            .local_addr()
            .context("Getting local_addr from TcpListener in TcpServer")
    }

    /// Adresses of currently connected clients (no health check)
    pub fn connected_clients(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.sockets.keys().cloned().collect::<Vec<String>>())
    }

    pub async fn broadcast(&mut self, msg: Res) -> HashMap<String, Error> {
        let mut failed_sockets = HashMap::new();
        debug!("Broadcasting msg {:?} to all", msg);

        // TODO: investigate if parallelizing this is better
        for socket_addr in self.sockets.keys().cloned().collect::<Vec<_>>() {
            if let Err(e) = self.send(socket_addr.clone(), msg.clone()).await {
                failed_sockets.insert(socket_addr.clone(), e);
            }
        }

        failed_sockets
    }

    pub async fn send(&mut self, socket_addr: String, msg: Res) -> Result<(), Error> {
        debug!("Sending msg {:?} to {}", msg, socket_addr);
        if let Some(stream) = self.sockets.get_mut(&socket_addr) {
            stream.sender.send(TcpMessage::Data(msg)).await
        } else {
            Err(Error::new(
                ErrorKind::NotFound,
                format!(
                    "Failed to retrieve socket address {} for sending a message",
                    &socket_addr
                ),
            ))
        }
    }

    fn setup_stream(
        &mut self,
        sender: SplitSink<Framed<TcpStream, TcpMessageCodec<Codec>>, TcpMessage<Res>>,
        mut receiver: SplitStream<Framed<TcpStream, TcpMessageCodec<Codec>>>,
        socket_addr: &String,
    ) -> anyhow::Result<()> {
        // Start a task to process pings from the peer.
        // We do the processing in the main select! loop to keep things synchronous.
        // This makes it easier to store data in the same struct without mutexing.
        let ping_sender = self.ping_sender.clone();
        let pool_sender = self.pool_sender.clone();
        let cloned_socket_addr = socket_addr.clone();

        // This task is responsible for reception of ping and message.
        // If an error occurs and is not an InvalidData error, we assume the task is to be aborted.
        // If the stream is closed, we also assume the task is to be aborted.
        let abort = tokio::spawn(async move {
            loop {
                match receiver.next().await {
                    Some(Ok(TcpMessage::Ping)) => {
                        _ = ping_sender.send(cloned_socket_addr.clone()).await;
                    }
                    Some(Ok(TcpMessage::Data(data))) => {
                        debug!(
                            "Received data from socket {}: {:?}",
                            cloned_socket_addr, data
                        );
                        _ = pool_sender
                            .send(TcpEvent::Message {
                                dest: cloned_socket_addr.clone(),
                                data,
                            })
                            .await;
                    }
                    Some(Err(err)) => {
                        if err.kind() == ErrorKind::InvalidData {
                            error!("Received invalid data in socket {cloned_socket_addr} event loop: {err}",);
                        } else {
                            // If the error is not invalid data, we can assume the socket is closed.
                            warn!(
                                "Closing socket {} after error: {:?}",
                                cloned_socket_addr,
                                err.kind()
                            );
                            // Send an event indicating the connection is closed due to an error
                            _ = pool_sender
                                .send(TcpEvent::Error {
                                    dest: cloned_socket_addr.clone(),
                                    error: err.to_string(),
                                })
                                .await;
                            break;
                        }
                    }
                    None => {
                        // If we reach here, the stream has been closed.
                        warn!("Socket {} closed", cloned_socket_addr);
                        // Send an event indicating the connection is closed
                        _ = pool_sender
                            .send(TcpEvent::Closed {
                                dest: cloned_socket_addr.clone(),
                            })
                            .await;
                        break;
                    }
                }
            }
        });
        tracing::debug!("Socket {} connected", socket_addr);
        // Store socket in the list.
        self.sockets.insert(
            socket_addr.to_string(),
            SocketStream {
                last_ping: get_current_timestamp(),
                sender,
                abort,
            },
        );

        Ok(())
    }

    pub fn setup_client(&mut self, tcp_client: TcpClient<Codec, Res, Req>) -> anyhow::Result<()> {
        let socket_addr = tcp_client.socket_addr.to_string();
        let (sender, receiver) = tcp_client.split();
        self.setup_stream(sender, receiver, &socket_addr)
    }

    pub fn drop_peer_stream(&mut self, peer_ip: String) {
        if let Some(peer_stream) = self.sockets.remove(&peer_ip) {
            peer_stream.abort.abort();
            trace!("Peer {} dropped & disconnected", peer_ip);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::time::Duration;

    use crate::{
        tcp::{TcpEvent, TcpMessage},
        tcp_client_server,
    };

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
        request: crate::tcp::tcp_server::tests::DataAvailabilityRequest,
        response: crate::tcp::tcp_server::tests::DataAvailabilityEvent
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

        let d = match server.listen_next().await.unwrap() {
            TcpEvent::Message { data, .. } => data,
            _ => panic!("Expected a Message event"),
        };

        assert_eq!(DataAvailabilityRequest(2), d);
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
