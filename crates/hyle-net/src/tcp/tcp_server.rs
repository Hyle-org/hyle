use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
};

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
use anyhow::{Context, Result};
use tracing::{debug, error, info};

use super::{tcp_client::TcpClient, SocketStream, TcpCommand, TcpEvent};

pub struct TcpServer<Codec, Req: Clone + std::fmt::Debug, Res: Clone + std::fmt::Debug>
where
    Codec: Decoder<Item = Req> + Encoder<Res> + Default,
{
    tcp_listener: TcpListener,
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
    pub async fn start(port: u16, pool_name: String) -> Result<Self> {
        let tcp_listener = TcpListener::bind(&(Ipv4Addr::UNSPECIFIED, port)).await?;
        let (pool_sender, pool_receiver) = tokio::sync::mpsc::channel(100);
        let (ping_sender, ping_receiver) = tokio::sync::mpsc::channel(100);
        debug!(
            "Starting TcpConnectionPool {}, listening for stream requests on {}",
            &pool_name, port
        );
        Ok(TcpServer::<Codec, Req, Res> {
            sockets: HashMap::new(),
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
                    let (sender, receiver) =
                        Framed::new(stream, TcpMessageCodec::<Codec>::default()).split();

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

    pub async fn broadcast(&mut self, msg: Res) -> HashSet<String> {
        // Broadcast can not fail
        self.send_command(TcpCommand::Broadcast(msg)).await.unwrap()
    }

    pub async fn send(&mut self, socket_addr: String, msg: Res) -> Result<HashSet<String>> {
        self.send_command(TcpCommand::Send(socket_addr, msg)).await
    }

    pub async fn send_command(&mut self, msg: TcpCommand<Res>) -> Result<HashSet<String>> {
        let mut failed_sockets = HashSet::new();
        match msg {
            TcpCommand::Broadcast(data) => {
                debug!("Broadcasting data {:?} to all", data);
                for (socket_addr, stream) in self.sockets.iter_mut() {
                    let last_ping = stream.last_ping;
                    if last_ping + 60 * 5 < get_current_timestamp() {
                        info!("peer {} timed out", &socket_addr);
                        stream.abort.abort();
                        failed_sockets.insert(socket_addr.clone());
                    } else {
                        debug!("streaming event to peer {}", &socket_addr);
                        match stream.sender.send(TcpMessage::Data(data.clone())).await {
                            Ok(_) => {}
                            Err(e) => {
                                debug!(
                                    "Couldn't send new block to peer {}, stopping streaming  : {:?}",
                                    &socket_addr, e
                                );
                                stream.abort.abort();
                                failed_sockets.insert(socket_addr.clone());
                            }
                        }
                    }
                }
            }
            TcpCommand::Send(socket_addr, data) => {
                debug!("Sending data {:?} to {}", data, socket_addr);
                let stream = self.sockets.get_mut(&socket_addr).context(format!(
                    "Failed to retrieve peer {} for sending a message",
                    &socket_addr
                ))?;

                if (stream.sender.send(TcpMessage::Data(data)).await).is_err() {
                    stream.abort.abort();
                    failed_sockets.insert(socket_addr.clone());
                }
            }
        }

        for peer in failed_sockets.iter() {
            self.sockets.remove(peer);
        }

        Ok(failed_sockets)
    }

    fn setup_stream(
        &mut self,
        sender: SplitSink<Framed<TcpStream, TcpMessageCodec<Codec>>, TcpMessage<Res>>,
        mut receiver: SplitStream<Framed<TcpStream, TcpMessageCodec<Codec>>>,
        socket_addr: &String,
    ) -> Result<()> {
        // Start a task to process pings from the peer.
        // We do the processing in the main select! loop to keep things synchronous.
        // This makes it easier to store data in the same struct without mutexing.
        let ping_sender = self.ping_sender.clone();
        let pool_sender = self.pool_sender.clone();
        let cloned_socket_addr = socket_addr.clone();
        let abort = tokio::spawn(async move {
            while let Some(msg) = receiver.next().await {
                debug!("Received message {:?}", &msg);
                match msg {
                    Ok(TcpMessage::Ping) => {
                        _ = ping_sender.send(cloned_socket_addr.clone()).await;
                    }
                    Ok(TcpMessage::Data(data)) => {
                        _ = pool_sender
                            .send(TcpEvent {
                                dest: cloned_socket_addr.clone(),
                                data,
                            })
                            .await;
                    }
                    Err(_) => {
                        error!("Decoding message in peer event loop");
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

    pub fn setup_client(&mut self, tcp_client: TcpClient<Codec, Res, Req>) -> Result<()> {
        let socket_addr = tcp_client.socket_addr.to_string();
        let (sender, receiver) = tcp_client.split();
        self.setup_stream(sender, receiver, &socket_addr)
    }

    pub fn drop_peer_stream(&mut self, peer_ip: String) {
        if let Some(peer_stream) = self.sockets.remove(&peer_ip) {
            peer_stream.abort.abort();
            info!("Peer {} dropped & disconnected", peer_ip);
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::time::Duration;

    use crate::{tcp::TcpMessage, tcp_client_server};

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

        let d = server.listen_next().await.unwrap().data;

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
