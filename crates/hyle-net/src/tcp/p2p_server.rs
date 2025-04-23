use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_crypto::BlstCrypto;
use sdk::{SignedByValidator, ValidatorPublicKey};
use tokio::task::JoinSet;
use tokio_util::codec::{Decoder, Encoder};
use tracing::{info, trace, warn};

use crate::tcp::{tcp_client::TcpClient, Handshake, P2PTcpEvent};

use super::{tcp_server::TcpServer, NodeConnectionData, P2PTcpMessage, TcpEvent};

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
pub struct PeerInfo {
    // Timestamp of the lastest handshake
    timestamp: u128,
    // This is the socket_addr used in the tcp_server for the current peer
    socket_addr: String,
    // The address that will be used to reconnect to that peer
    #[allow(dead_code)]
    pub node_connection_data: NodeConnectionData,
}

type HandShakeJoinSet<Codec, Msg> =
    JoinSet<anyhow::Result<TcpClient<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>>>;

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
    node_p2p_public_address: String,
    node_da_public_address: String,
    tcp_server: TcpServer<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
    pub peers: HashMap<ValidatorPublicKey, PeerInfo>,
    handshake_clients_tasks: HandShakeJoinSet<Codec, Msg>,
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
        node_p2p_public_address: String,
        node_da_public_address: String,
        tcp_server: TcpServer<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
    ) -> Self {
        Self {
            crypto,
            node_id,
            node_p2p_public_address,
            node_da_public_address,
            tcp_server,
            peers: HashMap::new(),
            handshake_clients_tasks: JoinSet::new(),
        }
    }

    pub async fn listen_next(&mut self) -> Option<P2PTcpEvent<Codec, Msg>> {
        tokio::select! {
            tcp_event = self.tcp_server.listen_next() => {
                tcp_event.map(P2PTcpEvent::TcpEvent)
            },
            Some(joinset_result) = self.handshake_clients_tasks.join_next() => {
                if let Ok(task_result) = joinset_result {
                    if let Ok(tcp_client) = task_result {
                        Some(P2PTcpEvent::HandShakeTcpClient(tcp_client))
                    }
                    else {
                        warn!("Error during TcpClient connection for handshake");
                        None
                    }
                }
                else {
                    warn!("Error during joinset execution of handshake task");
                    None
                }
            },
        }
    }

    pub fn set_max_frame_length(&mut self, len: usize) {
        self.tcp_server.set_max_frame_length(len);
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
            P2PTcpEvent::HandShakeTcpClient(tcp_client) => {
                if let Err(e) = self.do_handshake(tcp_client).await {
                    warn!("Error during handshake: {:?}", e);
                    // TODO: Retry ?
                }
                Ok(None)
            }
        }
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
        for peer_info in self.peers.clone().values() {
            if peer_info.socket_addr == dest {
                self.start_handshake_task(
                    peer_info.node_connection_data.p2p_public_address.clone(),
                );
            }
        }
        None
    }

    fn handle_closed_event(&mut self, dest: String) {
        // TODO: investigate how to properly handle this case
        // The connection has been closed by peer. We do not try to reconnect to it. We remove the peer.
        for (peer_pub_key, peer_info) in self.peers.clone() {
            if peer_info.socket_addr == dest {
                self.peers.remove(&peer_pub_key);
                self.tcp_server.drop_peer_stream(dest.clone());
                break;
            }
        }
    }

    async fn handle_handshake(
        &mut self,
        dest: String,
        handshake: Handshake,
    ) -> anyhow::Result<Option<P2PServerEvent<Msg>>> {
        match handshake {
            Handshake::Hello((v, timestamp)) => {
                // Verify message signature
                BlstCrypto::verify(&v).context("Error verifying Hello message")?;

                info!("👋 Processing Hello handshake message {:?}", v.msg);
                match self.create_signed_node_connection_data() {
                    Ok(verack) => {
                        // Send Verack response
                        if let Err(e) = self
                            .tcp_server
                            .send(
                                dest.clone(),
                                P2PTcpMessage::Handshake(Handshake::Verack((verack, timestamp))),
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

                Ok(self.handle_peer_update(&v, timestamp, dest))
            }
            Handshake::Verack((v, timestamp)) => {
                // Verify message signature
                BlstCrypto::verify(&v).context("Error verifying Verack message")?;

                info!("👋 Processing Verack handshake message {:?}", v.msg);
                Ok(self.handle_peer_update(&v, timestamp, dest))
            }
        }
    }

    fn handle_peer_update(
        &mut self,
        v: &SignedByValidator<NodeConnectionData>,
        timestamp: u128,
        dest: String,
    ) -> Option<P2PServerEvent<Msg>> {
        let peer_pubkey = v.signature.validator.clone();

        if let Some(peer_info) = self.peers.get_mut(&peer_pubkey) {
            if peer_info.timestamp < timestamp {
                self.tcp_server
                    .drop_peer_stream(peer_info.socket_addr.clone());
                peer_info.timestamp = timestamp;
                peer_info.socket_addr = dest.clone();
            } else {
                self.tcp_server.drop_peer_stream(dest);
            }
            None
        } else {
            let peer_info = PeerInfo {
                timestamp,
                socket_addr: dest,
                node_connection_data: v.msg.clone(),
            };
            self.peers.insert(peer_pubkey.clone(), peer_info);
            tracing::info!("New peer connected : {:?}", peer_pubkey);
            Some(P2PServerEvent::NewPeer {
                name: v.msg.name.to_string(),
                pubkey: v.signature.validator.clone(),
                da_address: v.msg.da_public_address.clone(),
            })
        }
    }

    pub fn remove_peer(&mut self, peer_pubkey: &ValidatorPublicKey) {
        if let Some(peer_info) = self.peers.remove(peer_pubkey) {
            self.tcp_server
                .drop_peer_stream(peer_info.socket_addr.clone());
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

    pub fn start_handshake(&mut self, peer_ip: String) {
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

        self.start_handshake_task(peer_ip);
    }

    pub fn start_handshake_task(&mut self, peer_ip: String) {
        let handshake_task = TcpClient::connect("p2p_server_handshake", peer_ip.clone());
        self.handshake_clients_tasks.spawn(handshake_task);
    }

    async fn do_handshake(
        &mut self,
        tcp_client: TcpClient<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
    ) -> anyhow::Result<()> {
        let signed_node_connection_data = self.create_signed_node_connection_data()?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis();

        let socket_addr = tcp_client.socket_addr.to_string();
        self.tcp_server.setup_client(tcp_client)?;
        self.tcp_server
            .send(
                socket_addr,
                P2PTcpMessage::Handshake(Handshake::Hello((
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
        msg: Msg,
    ) -> anyhow::Result<()> {
        let peer_info = match self.peers.get(&validator_pub_key) {
            Some(info) => info,
            None => {
                warn!(
                    "Trying to send message to unknown Peer {}. Unable to proceed.",
                    validator_pub_key
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
            bail!(
                "Failed to send message to peer {}: {:?}",
                validator_pub_key,
                e
            );
        }
        Ok(())
    }

    pub async fn broadcast(&mut self, msg: Msg) -> HashMap<ValidatorPublicKey, anyhow::Error> {
        let peer_keys: HashSet<ValidatorPublicKey> = self.peers.keys().cloned().collect();
        self.broadcast_only_for(&peer_keys, msg).await
    }

    pub async fn broadcast_only_for(
        &mut self,
        only_for: &HashSet<ValidatorPublicKey>,
        msg: Msg,
    ) -> HashMap<ValidatorPublicKey, anyhow::Error> {
        let mut failed_sends = HashMap::new();
        // TODO: investigate if parallelizing this is better
        for validator_pub_key in only_for {
            if let Err(e) = self.send(validator_pub_key.clone(), msg.clone()).await {
                failed_sends.insert(validator_pub_key.clone(), e);
            }
        }
        failed_sends
    }
}

#[cfg(test)]
pub mod tests {
    use anyhow::Result;
    use borsh::{BorshDeserialize, BorshSerialize};
    use hyle_crypto::BlstCrypto;
    use tokio::net::TcpListener;

    use crate::p2p_server_mod;

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
            format!("127.0.0.1:{port1}"),
            "127.0.0.1:4321".into(), // send some dummy address for DA
        )
        .await?;
        let p2p_server2 = p2p_server_test::start_server(
            crypto2.into(),
            "node2".to_string(),
            port2,
            format!("127.0.0.1:{port2}"),
            "127.0.0.1:4321".into(), // send some dummy address for DA
        )
        .await?;

        Ok(((port1, p2p_server1), (port2, p2p_server2)))
    }

    async fn process_messages(
        p2p_server: &mut super::P2PServer<p2p_server_test::codec_tcp::ServerCodec, TestMessage>,
        n: u8,
    ) -> Result<()> {
        // Process messages for both servers to complete handshake
        for _ in 0..n {
            tokio::select! {
                event = p2p_server.listen_next() => {
                    p2p_server
                        .handle_p2p_tcp_event(event.unwrap())
                        .await?;
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    println!("Timeout waiting for message");
                    break;
                }
            }
        }
        Ok(())
    }

    #[test_log::test(tokio::test)]
    async fn p2p_server_concurrent_handshake_test() -> Result<()> {
        let ((port1, mut p2p_server1), (port2, mut p2p_server2)) = setup_p2p_server_pair().await?;

        // Initiate handshake from p2p_server1 to p2p_server2
        p2p_server1.start_handshake(format!("127.0.0.1:{port2}"));

        // Initiate handshake from p2p_server2 to p2p_server1
        p2p_server2.start_handshake(format!("127.0.0.1:{port1}"));

        // For TcpClient to connect
        process_messages(&mut p2p_server1, 1).await?;
        process_messages(&mut p2p_server2, 1).await?;

        // For handshake Hello message
        process_messages(&mut p2p_server1, 1).await?;
        process_messages(&mut p2p_server2, 1).await?;

        // For handshake Verack message
        process_messages(&mut p2p_server1, 1).await?;
        process_messages(&mut p2p_server2, 1).await?;

        // Verify that both servers have each other in their peers map
        assert_eq!(p2p_server1.peers.len(), 1);
        assert_eq!(p2p_server2.peers.len(), 1);

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
        p2p_server1.start_handshake(format!("127.0.0.1:{port2}"));

        // Server1 waits for TcpClient to reconnect
        process_messages(&mut p2p_server1, 1).await?;
        // Server2 receives Hello message
        process_messages(&mut p2p_server2, 1).await?;
        // Server1 receives Verack message
        process_messages(&mut p2p_server1, 1).await?;

        // Verify initial connection
        assert_eq!(p2p_server1.peers.len(), 1);
        let peer_key = p2p_server1.peers.keys().next().unwrap().clone();

        // Simulate disconnection by dropping peer from server2
        p2p_server2.remove_peer(p2p_server1.crypto.validator_pubkey());

        // Try to send a message - this should trigger reconnection
        let test_msg = TestMessage("test reconnection".to_string());
        p2p_server1.send(peer_key.clone(), test_msg).await?;

        // Verify that server1 received the error for the previous send
        process_messages(&mut p2p_server1, 1).await?;

        // Server1 waits for TcpClient to reconnect
        process_messages(&mut p2p_server1, 1).await?;

        // Verify that server2 receives new handshake after reconnection
        process_messages(&mut p2p_server2, 1).await?;

        // Verify that server1 received server2's Verack
        process_messages(&mut p2p_server1, 1).await?;

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
