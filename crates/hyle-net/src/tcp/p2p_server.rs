use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context};
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_crypto::BlstCrypto;
use sdk::{hyle_model_utils::TimestampMs, SignedByValidator, ValidatorPublicKey};
use tokio::{task::JoinSet, time::Interval};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{debug, error, info, trace, warn};

use crate::{
    clock::TimestampMsClock,
    tcp::{tcp_client::TcpClient, Handshake, P2PTcpEvent},
};

use super::{tcp_server::TcpServer, Canal, NodeConnectionData, P2PTcpMessage, TcpEvent};

#[derive(Debug)]
pub enum P2PServerEvent<Msg: Clone> {
    NewPeer {
        name: String,
        pubkey: ValidatorPublicKey,
        da_address: String,
    },
    P2PMessage {
        msg: Msg,
    },
}

#[derive(Clone, Debug)]
pub struct PeerSocket {
    // Timestamp of the lastest handshake
    timestamp: TimestampMs,
    // This is the socket_addr used in the tcp_server for the current peer
    socket_addr: String,
}

#[derive(Clone, Debug)]
pub struct PeerInfo {
    // Hashmap containing a sockets for all canals of this peer
    pub canals: HashMap<Canal, PeerSocket>,
    // The address that will be used to reconnect to that peer
    #[allow(dead_code)]
    pub node_connection_data: NodeConnectionData,
}

type HandShakeJoinSet<Codec, Msg> = JoinSet<(
    String,
    anyhow::Result<TcpClient<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>>,
    Canal,
)>;

#[derive(Debug)]
pub enum HandshakeOngoing {
    TcpClientStartedAt(TimestampMs),
    HandshakeStartedAt(TimestampMs),
}

/// P2PServer is a wrapper around TcpServer that manages peer connections
/// Its role is to process a full handshake with a peer, in order to get its public key.
/// Once handshake is done, the peer is added to the list of peers.
/// The connection is kept alive, hance restarted if it disconnects.
pub struct P2PServer<Codec, Msg>
where
    Codec: Decoder<Item = P2PTcpMessage<Msg>> + Encoder<P2PTcpMessage<Msg>> + Default,
    Msg: BorshSerialize + Clone + std::fmt::Debug,
    P2PTcpMessage<Msg>: Clone + std::fmt::Debug,
{
    crypto: Arc<BlstCrypto>,
    node_id: String,
    // Hashmap containing the last attempts to connect
    pub connecting: HashMap<String, HandshakeOngoing>,
    node_p2p_public_address: String,
    node_da_public_address: String,
    max_frame_length: Option<usize>,
    pub tcp_server: TcpServer<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
    pub peers: HashMap<ValidatorPublicKey, PeerInfo>,
    handshake_clients_tasks: HandShakeJoinSet<Codec, Msg>,
    peers_ping_ticker: Interval,
}

impl<Codec, Msg> P2PServer<Codec, Msg>
where
    Codec:
        Decoder<Item = P2PTcpMessage<Msg>> + Encoder<P2PTcpMessage<Msg>> + Default + Send + 'static,
    <Codec as Decoder>::Error: std::fmt::Debug + Send,
    <Codec as Encoder<P2PTcpMessage<Msg>>>::Error: std::fmt::Debug + Send,
    Msg: BorshSerialize + Clone + Send + 'static + std::fmt::Debug,
    P2PTcpMessage<Msg>:
        BorshDeserialize + BorshSerialize + Clone + Send + 'static + std::fmt::Debug,
{
    pub fn new(
        crypto: Arc<BlstCrypto>,
        node_id: String,
        max_frame_length: Option<usize>,
        node_p2p_public_address: String,
        node_da_public_address: String,
        tcp_server: TcpServer<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
    ) -> Self {
        Self {
            crypto,
            node_id,
            connecting: HashMap::default(),
            max_frame_length,
            node_p2p_public_address,
            node_da_public_address,
            tcp_server,
            peers: HashMap::new(),
            handshake_clients_tasks: JoinSet::new(),
            peers_ping_ticker: tokio::time::interval(std::time::Duration::from_secs(2)),
        }
    }

    pub async fn listen_next(&mut self) -> P2PTcpEvent<Codec, Msg> {
        loop {
            tokio::select! {
                Some(tcp_event) = self.tcp_server.listen_next() => {
                    return P2PTcpEvent::TcpEvent(tcp_event);
                },
                Some(joinset_result) = self.handshake_clients_tasks.join_next() => {
                    if let Ok(task_result) = joinset_result {
                        if let (public_addr, Ok(tcp_client), canal) = task_result {
                            return P2PTcpEvent::HandShakeTcpClient(public_addr, tcp_client, canal);
                        }
                        else {
                            warn!("Error during TcpClient connection for handshake");
                            continue
                        }
                    }
                    else {
                        warn!("Error during joinset execution of handshake task");
                        continue
                    }
                },
                _ = self.peers_ping_ticker.tick() => {
                    return P2PTcpEvent::PingPeers;
                }
            }
        }
    }

    /// Handle a P2PTCPEvent. This is done as separate function to easily handle async tasks
    pub async fn handle_p2p_tcp_event(
        &mut self,
        p2p_tcp_event: P2PTcpEvent<Codec, Msg>,
    ) -> anyhow::Result<Option<P2PServerEvent<Msg>>> {
        match p2p_tcp_event {
            P2PTcpEvent::TcpEvent(tcp_event) => match tcp_event {
                TcpEvent::Message {
                    dest,
                    data: P2PTcpMessage::Handshake(handshake),
                } => self.handle_handshake(dest, handshake).await,
                TcpEvent::Message {
                    dest: _,
                    data: P2PTcpMessage::Data(msg),
                } => Ok(Some(P2PServerEvent::P2PMessage { msg })),
                TcpEvent::Error { dest, error } => {
                    self.handle_error_event(dest, error).await;
                    Ok(None)
                }
                TcpEvent::Closed { dest } => {
                    self.handle_closed_event(dest);
                    Ok(None)
                }
            },
            P2PTcpEvent::HandShakeTcpClient(public_addr, tcp_client, canal) => {
                if let Err(e) = self.do_handshake(public_addr, tcp_client, canal).await {
                    warn!("Error during handshake: {:?}", e);
                    // TODO: Retry ?
                }
                Ok(None)
            }
            P2PTcpEvent::PingPeers => {
                for peer_socket in self.peers.values().flat_map(|v| v.canals.values()) {
                    if let Err(e) = self.tcp_server.ping(peer_socket.socket_addr.clone()).await {
                        warn!("Error pinging peer {}: {:?}", peer_socket.socket_addr, e);
                    }
                }
                Ok(None)
            }
        }
    }

    pub fn find_socket_addr(&self, canal: &Canal, vid: &ValidatorPublicKey) -> Option<&String> {
        self.peers
            .get(vid)
            .and_then(|p| p.canals.get(canal).map(|socket| &socket.socket_addr))
    }

    pub fn get_socket_mut(
        &mut self,
        canal: &Canal,
        vid: &ValidatorPublicKey,
    ) -> Option<&mut PeerSocket> {
        self.peers
            .get_mut(vid)
            .and_then(|p| p.canals.get_mut(canal))
    }

    pub fn get_peer_by_socket_addr(
        &self,
        dest: &String,
    ) -> Option<(&Canal, &PeerInfo, &PeerSocket)> {
        self.peers.values().find_map(|peer_info| {
            peer_info
                .canals
                .iter()
                .find(|(_canal, peer_socket)| &peer_socket.socket_addr == dest)
                .map(|(canal, peer_socket)| (canal, peer_info, peer_socket))
        })
    }

    async fn handle_error_event(
        &mut self,
        dest: String,
        _error: String,
    ) -> Option<P2PServerEvent<Msg>> {
        warn!("Error with peer connection: {:?}", _error);
        // There was an error with the connection with the peer. We try to reconnect.

        // TODO: An error can happen when a message was no *sent* correctly. Investigate how to handle that specific case
        // TODO: match the error type to decide what to do
        self.tcp_server.drop_peer_stream(dest.clone());
        if let Some((canal, info, _)) = self.get_peer_by_socket_addr(&dest) {
            self.start_handshake_task(
                info.node_connection_data.p2p_public_address.clone(),
                canal.clone(),
            )
        }
        None
    }

    fn handle_closed_event(&mut self, dest: String) {
        // TODO: investigate how to properly handle this case
        // The connection has been closed by peer. We do not try to reconnect to it. We remove the peer.
        self.tcp_server.drop_peer_stream(dest.clone());
        if let Some((canal, info, _)) = self.get_peer_by_socket_addr(&dest) {
            self.start_handshake_task(
                info.node_connection_data.p2p_public_address.clone(),
                canal.clone(),
            )
        }
    }

    async fn handle_handshake(
        &mut self,
        dest: String,
        handshake: Handshake,
    ) -> anyhow::Result<Option<P2PServerEvent<Msg>>> {
        match handshake {
            Handshake::Hello((canal, v, timestamp)) => {
                // Verify message signature
                BlstCrypto::verify(&v).context("Error verifying Hello message")?;

                info!(
                    "👋 [{}] Processing Hello handshake message {:?}",
                    canal, v.msg
                );
                match self.create_signed_node_connection_data() {
                    Ok(verack) => {
                        // Send Verack response
                        if let Err(e) = self
                            .tcp_server
                            .send(
                                dest.clone(),
                                P2PTcpMessage::Handshake(Handshake::Verack((
                                    canal.clone(),
                                    verack,
                                    timestamp.clone(),
                                ))),
                            )
                            .await
                        {
                            bail!("Error sending Verack message to {dest}: {:?}", e);
                        }
                    }
                    Err(e) => {
                        bail!("Error creating signed node connection data: {:?}", e);
                    }
                }

                Ok(self.handle_peer_update(canal, &v, timestamp, dest))
            }
            Handshake::Verack((canal, v, timestamp)) => {
                // Verify message signature
                BlstCrypto::verify(&v).context("Error verifying Verack message")?;

                info!(
                    "👋 [{}] Processing Verack handshake message {:?}",
                    canal, v.msg
                );
                Ok(self.handle_peer_update(canal, &v, timestamp, dest))
            }
        }
    }

    fn handle_peer_update(
        &mut self,
        canal: Canal,
        v: &SignedByValidator<NodeConnectionData>,
        timestamp: TimestampMs,
        dest: String,
    ) -> Option<P2PServerEvent<Msg>> {
        let peer_pubkey = v.signature.validator.clone();

        // error!("Peers connecting {:?}", self.connecting);
        // error!("Peers all {:?}", self.peers);

        if let Some(peer_socket) = self.get_socket_mut(&canal, &peer_pubkey) {
            let peer_addr_to_drop = if peer_socket.timestamp < timestamp {
                debug!(
                    "Local peer {}/{} ({}): dropping socket {} in favor of more recent one {}",
                    v.msg.p2p_public_address, canal, peer_pubkey, peer_socket.socket_addr, dest
                );
                let socket_addr = peer_socket.socket_addr.clone();
                peer_socket.timestamp = timestamp;
                peer_socket.socket_addr = dest.clone();
                socket_addr.clone()
            } else {
                debug!(
                    "Local peer {}/{} ({}): keeping socket {} and discard too old {}",
                    v.msg.p2p_public_address, canal, peer_pubkey, peer_socket.socket_addr, dest
                );
                dest
            };
            self.tcp_server.drop_peer_stream(peer_addr_to_drop);
            None
        } else {
            // If the validator exists, but not this canal, we create it
            if let Some(validator) = self.peers.get_mut(&peer_pubkey) {
                debug!(
                    "Local peer {}/{} ({}): creating canal for existing peer on socket {}",
                    v.msg.p2p_public_address, canal, peer_pubkey, dest
                );
                validator.canals.insert(
                    canal.clone(),
                    PeerSocket {
                        timestamp,
                        socket_addr: dest,
                    },
                );
            }
            // If the validator was never created before
            else {
                debug!(
                    "Local peer {}/{} ({}): creating new peer and canal on socket {}",
                    v.msg.p2p_public_address, canal, peer_pubkey, dest
                );
                let peer_info = PeerInfo {
                    canals: HashMap::from_iter(vec![(
                        canal.clone(),
                        PeerSocket {
                            timestamp,
                            socket_addr: dest,
                        },
                    )]),
                    node_connection_data: v.msg.clone(),
                };

                self.peers.insert(peer_pubkey.clone(), peer_info);
            }
            tracing::info!("New peer connected on canal {}: {}", canal, peer_pubkey);
            Some(P2PServerEvent::NewPeer {
                name: v.msg.name.to_string(),
                pubkey: v.signature.validator.clone(),
                da_address: v.msg.da_public_address.clone(),
            })
        }
    }

    pub fn remove_peer(&mut self, peer_pubkey: &ValidatorPublicKey, canal: Canal) {
        if let Some(peer_info) = self.peers.get_mut(peer_pubkey) {
            if let Some(removed) = peer_info.canals.remove(&canal) {
                self.tcp_server
                    .drop_peer_stream(removed.socket_addr.clone());
            }
        }
    }

    fn create_signed_node_connection_data(
        &self,
    ) -> anyhow::Result<SignedByValidator<NodeConnectionData>> {
        let node_connection_data = NodeConnectionData {
            version: 1,
            name: self.node_id.clone(),
            p2p_public_address: self.node_p2p_public_address.clone(),
            da_public_address: self.node_da_public_address.clone(),
        };
        self.crypto.sign(node_connection_data)
    }

    fn start_handshake_from_existing_peer(
        &mut self,
        pubkey: &ValidatorPublicKey,
        canal: Canal,
    ) -> anyhow::Result<()> {
        tracing::error!("Attempt to reconnect to {}/{}", pubkey, canal);
        let peer = self
            .peers
            .get_mut(pubkey)
            .context(format!("Peer not found {}", pubkey))?;
        tracing::error!(
            "Attempt to reconnect to {}/{}",
            peer.node_connection_data.p2p_public_address,
            canal
        );

        let now = TimestampMsClock::now();

        // A connection is already started for this public address ? If it is too old, let 's try retry one
        // If it is recent, let's wait for it to finish
        if let Some(ongoing) = self
            .connecting
            .get(&peer.node_connection_data.p2p_public_address)
        {
            match ongoing {
                HandshakeOngoing::TcpClientStartedAt(last_connect_attempt) => {
                    if now.clone() - last_connect_attempt.clone() < Duration::from_secs(2) {
                        {
                            return Ok(());
                        }
                    }
                }
                HandshakeOngoing::HandshakeStartedAt(last_handshake_at) => {
                    if now.clone() - last_handshake_at.clone() < Duration::from_secs(2) {
                        {
                            return Ok(());
                        }
                    }
                }
            }
        }

        let peer_address = peer.node_connection_data.p2p_public_address.clone();

        tracing::error!("Reconnecting to {}/{}", peer_address, canal);

        self.connecting.insert(
            peer_address.clone(),
            HandshakeOngoing::TcpClientStartedAt(now),
        );
        self.start_handshake_task(peer_address, canal);
        Ok(())
    }

    pub fn start_handshake(&mut self, peer_ip: String, canal: Canal) {
        if peer_ip == self.node_p2p_public_address {
            trace!("Trying to connect to self");
            return;
        }

        for peer in self.peers.values() {
            if peer_ip == peer.node_connection_data.p2p_public_address {
                warn!("Peer {} already connected", peer_ip);
                return;
            }
        }

        self.start_handshake_task(peer_ip, canal);
    }

    /// Create a tcp connection
    pub fn start_handshake_task(&mut self, peer_ip: String, canal: Canal) {
        let mfl = self.max_frame_length;
        self.handshake_clients_tasks.spawn(async move {
            let handshake_task =
                TcpClient::connect_with_opts("p2p_server_handshake", mfl, peer_ip.clone());

            let result = handshake_task.await;
            (peer_ip, result, canal)
        });
    }

    async fn do_handshake(
        &mut self,
        public_addr: String,
        tcp_client: TcpClient<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
        canal: Canal,
    ) -> anyhow::Result<()> {
        let signed_node_connection_data = self.create_signed_node_connection_data()?;
        let timestamp = TimestampMsClock::now();

        error!(
            "Doing handshake on {}({})/{}",
            public_addr,
            tcp_client.socket_addr.to_string(),
            canal
        );

        self.connecting.insert(
            public_addr.clone(),
            HandshakeOngoing::HandshakeStartedAt(timestamp.clone()),
        );

        let addr = format!("{}/{}", public_addr, canal);
        self.tcp_server.setup_client(addr.clone(), tcp_client);
        self.tcp_server
            .send(
                addr,
                P2PTcpMessage::Handshake(Handshake::Hello((
                    canal,
                    signed_node_connection_data.clone(),
                    timestamp,
                ))),
            )
            .await?;

        Ok(())
    }

    pub async fn send(
        &mut self,
        validator_pub_key: ValidatorPublicKey,
        canal: Canal,
        msg: Msg,
    ) -> anyhow::Result<()> {
        let peer_info = match self
            .peers
            .get(&validator_pub_key)
            .and_then(|peer| peer.canals.get(&canal))
        {
            Some(info) => info,
            None => {
                warn!(
                    "Trying to send message to unknown Peer {}/{}. Unable to proceed.",
                    validator_pub_key, canal
                );
                return Ok(());
            }
        };

        if let Err(e) = self
            .tcp_server
            .send(
                peer_info.socket_addr.clone(),
                P2PTcpMessage::Data(msg.clone()),
            )
            .await
        {
            self.start_handshake_from_existing_peer(&validator_pub_key, canal)
                .context(format!(
                    "Re-handshaking after message sending error with peer {}",
                    validator_pub_key
                ))?;
            bail!(
                "Failed to send message to peer {}: {:?}",
                validator_pub_key,
                e
            );
        }
        Ok(())
    }

    pub async fn broadcast(
        &mut self,
        msg: Msg,
        canal: Canal,
    ) -> HashMap<ValidatorPublicKey, anyhow::Error> {
        let peer_addr_to_pubkey: HashMap<String, (Canal, ValidatorPublicKey)> = self
            .peers
            .iter()
            .flat_map(|(pubkey, peer)| {
                peer.canals
                    .iter()
                    .filter(|(_canal, _socket)| _canal == &&canal)
                    .map(|(_canal, socket)| {
                        (socket.socket_addr.clone(), (_canal.clone(), pubkey.clone()))
                    })
            })
            .collect();

        let res = self
            .tcp_server
            .send_parallel(
                peer_addr_to_pubkey.keys().cloned().collect(),
                P2PTcpMessage::Data(msg),
            )
            .await;

        HashMap::from_iter(res.into_iter().filter_map(|(k, v)| {
            peer_addr_to_pubkey.get(&k).map(|(_canal, pubkey)| {
                error!("Error sending message to {} during broadcast: {}", k, v);
                if let Err(e) = self.start_handshake_from_existing_peer(pubkey, _canal.clone()) {
                    warn!("Problem when triggering re-handshake after message sending error with peer {}/{}: {}", pubkey, _canal, e);
                }
                (pubkey.clone(), v)
            })
        }))
    }

    pub async fn broadcast_only_for(
        &mut self,
        only_for: &HashSet<ValidatorPublicKey>,
        canal: Canal,
        msg: Msg,
    ) -> HashMap<ValidatorPublicKey, anyhow::Error> {
        let peer_addr_to_pubkey: HashMap<String, (Canal, ValidatorPublicKey)> = self
            .peers
            .iter()
            .flat_map(|(pubkey, peer)| {
                peer.canals
                    .iter()
                    .filter(|(_canal, _socket)| _canal == &&canal && only_for.contains(pubkey))
                    .map(|(_canal, socket)| {
                        (socket.socket_addr.clone(), (_canal.clone(), pubkey.clone()))
                    })
            })
            .collect();

        let res = self
            .tcp_server
            .send_parallel(
                peer_addr_to_pubkey.keys().cloned().collect(),
                P2PTcpMessage::Data(msg),
            )
            .await;

        HashMap::from_iter(res.into_iter().filter_map(|(k, v)| {
            peer_addr_to_pubkey.get(&k).map(|(canal, pubkey)| {
                error!("Error sending message to {} during broadcast: {}", k, v);
                if let Err(e) = self.start_handshake_from_existing_peer(pubkey, canal.clone()) {
                    warn!("Problem when triggering re-handshake after message sending error with peer {}/{:?}: {}", pubkey, canal, e);
                }
                (pubkey.clone(), v)
            })
        }))
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashSet;

    use anyhow::Result;
    use borsh::{BorshDeserialize, BorshSerialize};
    use hyle_crypto::BlstCrypto;
    use tokio::net::TcpListener;

    use crate::{
        p2p_server_mod,
        tcp::{Canal, Handshake, P2PTcpEvent, P2PTcpMessage, TcpEvent},
    };

    pub async fn find_available_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        addr.port()
    }
    // Simple message type for testing
    #[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
    pub struct TestMessage(String);

    p2p_server_mod! {
        pub test,
        message: crate::tcp::p2p_server::tests::TestMessage
    }

    macro_rules! receive_and_handle_event {
        ($server:expr, $pattern:pat, $error_msg:expr) => {{
            let event = receive_event($server, $error_msg).await?;
            assert!(
                matches!(event, $pattern),
                "Expected {:?}, got {:?}",
                stringify!($pattern),
                event,
            );
            $server.handle_p2p_tcp_event(event).await?;
        }};
    }

    async fn setup_p2p_server_pair() -> Result<(
        (
            u16,
            super::P2PServer<p2p_server_test::codec_tcp::ServerCodec, TestMessage>,
        ),
        (
            u16,
            super::P2PServer<p2p_server_test::codec_tcp::ServerCodec, TestMessage>,
        ),
    )> {
        let crypto1 = BlstCrypto::new_random().unwrap();
        let crypto2 = BlstCrypto::new_random().unwrap();

        let port1 = find_available_port().await;
        let port2 = find_available_port().await;

        tracing::info!("Starting P2P server1 on port {port1}");
        tracing::info!("Starting P2P server2 on port {port2}");
        let p2p_server1 = p2p_server_test::start_server(
            crypto1.into(),
            "node1".to_string(),
            port1,
            None,
            format!("127.0.0.1:{port1}"),
            "127.0.0.1:4321".into(), // send some dummy address for DA
        )
        .await?;
        let p2p_server2 = p2p_server_test::start_server(
            crypto2.into(),
            "node2".to_string(),
            port2,
            None,
            format!("127.0.0.1:{port2}"),
            "127.0.0.1:4321".into(), // send some dummy address for DA
        )
        .await?;

        Ok(((port1, p2p_server1), (port2, p2p_server2)))
    }

    async fn receive_event(
        p2p_server: &mut super::P2PServer<p2p_server_test::codec_tcp::ServerCodec, TestMessage>,
        error_msg: &str,
    ) -> Result<P2PTcpEvent<p2p_server_test::codec_tcp::ServerCodec, TestMessage>> {
        loop {
            let timeout_duration = std::time::Duration::from_millis(50);

            match tokio::time::timeout(timeout_duration, p2p_server.listen_next()).await {
                Ok(event) => match event {
                    P2PTcpEvent::PingPeers => {
                        continue;
                    }
                    _ => return Ok(event),
                },
                Err(_) => {
                    anyhow::bail!("Timed out while waiting for message: {error_msg}");
                }
            }
        }
    }

    #[test_log::test(tokio::test)]
    async fn p2p_server_concurrent_handshake_test() -> Result<()> {
        let ((port1, mut p2p_server1), (port2, mut p2p_server2)) = setup_p2p_server_pair().await?;

        // Initiate handshake from p2p_server1 to p2p_server2
        p2p_server1.start_handshake(format!("127.0.0.1:{port2}"), Canal::new("A"));

        // Initiate handshake from p2p_server2 to p2p_server1
        p2p_server2.start_handshake(format!("127.0.0.1:{port1}"), Canal::new("A"));

        // For TcpClient to connect
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::HandShakeTcpClient(_, _, _),
            "Expected HandShake TCP Client connection"
        );
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::HandShakeTcpClient(_, _, _),
            "Expected HandShake TCP Client connection"
        );

        // For handshake Hello message
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Hello(_))
            }),
            "Expected HandShake Hello message"
        );
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Hello(_))
            }),
            "Expected HandShake Hello message"
        );

        // For handshake Verack message
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Verack(_))
            }),
            "Expected HandShake Verack"
        );
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Verack(_))
            }),
            "Expected HandShake Verack"
        );

        // Verify that both servers have each other in their peers map
        assert_eq!(p2p_server1.peers.len(), 1);
        assert_eq!(p2p_server2.peers.len(), 1);

        assert_eq!(p2p_server1.peers.values().last().unwrap().canals.len(), 1);
        assert_eq!(p2p_server2.peers.values().last().unwrap().canals.len(), 1);

        // Both peers should have each other's ValidatorPublicKey in their maps
        let p1_peer_key = p2p_server1.peers.keys().next().unwrap();
        let p2_peer_key = p2p_server2.peers.keys().next().unwrap();
        assert_eq!(p1_peer_key.0, p2p_server2.crypto.validator_pubkey().0);
        assert_eq!(p2_peer_key.0, p2p_server1.crypto.validator_pubkey().0);

        Ok(())
    }
    #[test_log::test(tokio::test)]
    async fn p2p_server_multi_canal_handshake() -> Result<()> {
        let ((port1, mut p2p_server1), (port2, mut p2p_server2)) = setup_p2p_server_pair().await?;

        // Initiate handshake from p2p_server1 to p2p_server2 on canal A
        p2p_server1.start_handshake(format!("127.0.0.1:{port2}"), Canal::new("A"));

        // Initiate handshake from p2p_server2 to p2p_server1 on canal A
        p2p_server2.start_handshake(format!("127.0.0.1:{port1}"), Canal::new("A"));

        // Initiate handshake from p2p_server1 to p2p_server2 on canal B
        p2p_server1.start_handshake(format!("127.0.0.1:{port2}"), Canal::new("B"));

        // Initiate handshake from p2p_server2 to p2p_server1 on canal B
        p2p_server2.start_handshake(format!("127.0.0.1:{port1}"), Canal::new("B"));

        // For TcpClient to connect
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::HandShakeTcpClient(_, _, _),
            "Expected HandShake TCP Client connection"
        );
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::HandShakeTcpClient(_, _, _),
            "Expected HandShake TCP Client connection"
        );
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::HandShakeTcpClient(_, _, _),
            "Expected HandShake TCP Client connection"
        );
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::HandShakeTcpClient(_, _, _),
            "Expected HandShake TCP Client connection"
        );

        // For handshake Hello message
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Hello(_))
            }),
            "Expected HandShake Hello message canal A"
        );
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Hello(_))
            }),
            "Expected HandShake Hello message canal B"
        );
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Hello(_))
            }),
            "Expected HandShake Hello message canal A"
        );
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Hello(_))
            }),
            "Expected HandShake Hello message canal B"
        );

        // For handshake Verack message
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Verack(_))
            }),
            "Expected HandShake Verack"
        );
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Verack(_))
            }),
            "Expected HandShake Verack"
        );

        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Verack(_))
            }),
            "Expected HandShake Verack"
        );
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Verack(_))
            }),
            "Expected HandShake Verack"
        );
        // Verify that both servers have each other in their peers map
        assert_eq!(p2p_server1.peers.len(), 1);
        assert_eq!(p2p_server2.peers.len(), 1);

        assert_eq!(p2p_server1.peers.values().last().unwrap().canals.len(), 2);
        assert_eq!(p2p_server2.peers.values().last().unwrap().canals.len(), 2);

        assert_eq!(
            HashSet::<String>::from_iter(
                p2p_server1
                    .peers
                    .values()
                    .flat_map(|v| v.canals.values().map(|v2| v2.socket_addr.clone()))
                    .collect::<Vec<_>>()
            ),
            HashSet::from_iter(p2p_server1.tcp_server.connected_clients())
        );
        assert_eq!(
            HashSet::<String>::from_iter(
                p2p_server2
                    .peers
                    .values()
                    .flat_map(|v| v.canals.values().map(|v2| v2.socket_addr.clone()))
                    .collect::<Vec<_>>()
            ),
            HashSet::from_iter(p2p_server2.tcp_server.connected_clients())
        );

        // Both peers should have each other's ValidatorPublicKey in their maps
        let p1_peer_key = p2p_server1.peers.keys().next().unwrap();
        let p2_peer_key = p2p_server2.peers.keys().next().unwrap();
        assert_eq!(p1_peer_key.0, p2p_server2.crypto.validator_pubkey().0);
        assert_eq!(p2_peer_key.0, p2p_server1.crypto.validator_pubkey().0);

        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn p2p_server_reconnection_test() -> Result<()> {
        let ((_, mut p2p_server1), (port2, mut p2p_server2)) = setup_p2p_server_pair().await?;

        // Initial connection
        p2p_server1.start_handshake(format!("127.0.0.1:{port2}"), Canal::new("A"));

        // Server1 waits for TcpClient to connect
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::HandShakeTcpClient(_, _, _),
            "Expected HandShake TCP Client connection"
        );
        // Server2 receives Hello message
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Hello(_))
            }),
            "Expected HandShake Hello message"
        );
        // Server1 receives Verack message
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Verack(_))
            }),
            "Expected HandShake Verack"
        );

        // Verify initial connection
        assert_eq!(p2p_server1.peers.len(), 1);

        // Simulate disconnection by dropping peer from server2
        p2p_server2.remove_peer(p2p_server1.crypto.validator_pubkey(), Canal::new("A"));

        // Server1 receives Closed message
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Closed { dest: _ }),
            "Expected Tcp Error message"
        );

        // Server1 waits for TcpClient to reconnect
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::HandShakeTcpClient(_, _, _),
            "Expected HandShake TCP Client connection"
        );
        // Server2 receives Hello message
        receive_and_handle_event!(
            &mut p2p_server2,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Hello(_))
            }),
            "Expected HandShake Hello message"
        );
        // Server1 receives Verack message
        receive_and_handle_event!(
            &mut p2p_server1,
            P2PTcpEvent::TcpEvent(TcpEvent::Message {
                dest: _,
                data: P2PTcpMessage::Handshake(Handshake::Verack(_))
            }),
            "Expected HandShake Verack"
        );

        assert_eq!(
            p2p_server1.peers.len(),
            1,
            "Server1 should connected to only server2"
        );
        assert_eq!(
            p2p_server2.peers.len(),
            1,
            "Server2 should have reconnected to server1"
        );

        Ok(())
    }
}
