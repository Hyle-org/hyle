use std::{
    collections::{HashMap, HashSet},
    io::ErrorKind,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_crypto::BlstCrypto;
use sdk::{SignedByValidator, ValidatorPublicKey};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{debug, info, trace, warn};

use crate::tcp::{tcp_client::TcpClient, Handshake};

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
    node_p2p_public_adress: String,
    node_da_public_adress: String,
    tcp_server: TcpServer<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
    pub peers: HashMap<ValidatorPublicKey, PeerInfo>,
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
        node_p2p_public_adress: String,
        node_da_public_adress: String,
        tcp_server: TcpServer<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
    ) -> Self {
        Self {
            crypto,
            node_id,
            node_p2p_public_adress,
            node_da_public_adress,
            tcp_server,
            peers: HashMap::new(),
        }
    }

    pub async fn listen_next(&mut self) -> Option<TcpEvent<P2PTcpMessage<Msg>>> {
        self.tcp_server.listen_next().await
    }

    /// Handle a TCP event. This is done as separate function to easily handle async tasks
    pub async fn handle_tcp_event(
        &mut self,
        tcp_event: TcpEvent<P2PTcpMessage<Msg>>,
    ) -> anyhow::Result<Option<P2PServerEvent<Msg>>> {
        debug!("Received TCP event: {:?}", tcp_event);
        match tcp_event {
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
        }
    }

    async fn handle_error_event(&mut self, dest: String, _error: String) {
        // There was an error with the connection with the peer. We try to reconnect.

        // TODO: An error can happen when a message was no *sent* correctly. Investigate how to handle that specific case
        // TODO: match the error type to decide what to do
        for (peer_pubkey, peer_info) in self.peers.clone() {
            if peer_info.socket_addr == dest {
                if let Err(e) = self
                    .send_hello_message(peer_info.node_connection_data.p2p_public_adress)
                    .await
                {
                    tracing::error!(
                        "Could not reconnect to peer {}: {}. Removing peer from list",
                        peer_info.socket_addr,
                        e
                    );
                    self.peers.remove(&peer_pubkey);
                }
                break;
            }
        }
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
                da_address: v.msg.da_public_adress.clone(),
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
            p2p_public_adress: self.node_p2p_public_adress.clone(),
            da_public_adress: self.node_da_public_adress.clone(),
        };
        self.crypto.sign(node_connection_data)
    }

    pub async fn send_hello_message(&mut self, peer_ip: String) -> anyhow::Result<()> {
        let signed_node_connection_data = self.create_signed_node_connection_data()?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis();

        let tcp_client: TcpClient<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>> =
            TcpClient::connect("p2p_server", peer_ip.clone()).await?;
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
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }

    pub async fn start_handshake(&mut self, peer_ip: String) -> anyhow::Result<()> {
        if peer_ip == self.node_p2p_public_adress {
            trace!("Trying to connect to self");
            return Ok(());
        }

        for peer in self.peers.values() {
            if peer_ip == peer.node_connection_data.p2p_public_adress {
                warn!("Peer {} already connected", peer_ip);
                return Ok(());
            }
        }
        self.send_hello_message(peer_ip).await
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

        let validator_ip = peer_info.socket_addr.clone();
        let peer_connection_addr = peer_info.node_connection_data.p2p_public_adress.clone();

        if let Err(e) = self
            .tcp_server
            .send(validator_ip.clone(), P2PTcpMessage::Data(msg.clone()))
            .await
        {
            match e.kind() {
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset => {
                    // The connection is broken, we need to reconnect
                    warn!(
                        "Connection to peer {} at {} is broken. Attempting to reconnect...",
                        validator_pub_key, validator_ip
                    );
                    self.send_hello_message(peer_connection_addr.clone())
                        .await
                        .context(format!("Failed to reconnect to peer {}", validator_pub_key))?;
                }
                _ => {
                    bail!(
                        "Error sending message to peer {}: {:?}",
                        validator_pub_key,
                        e
                    );
                }
            }
        }

        Ok(())
    }

    pub async fn broadcast(&mut self, msg: Msg) -> anyhow::Result<()> {
        let peer_keys: HashSet<ValidatorPublicKey> = self.peers.keys().cloned().collect();
        self.broadcast_only_for(&peer_keys, msg).await
    }

    pub async fn broadcast_only_for(
        &mut self,
        only_for: &HashSet<ValidatorPublicKey>,
        msg: Msg,
    ) -> anyhow::Result<()> {
        // TODO: investigate if parallelizing this is better
        for validator_pub_key in only_for {
            if let Err(e) = self.send(validator_pub_key.clone(), msg.clone()).await {
                warn!(
                    "Failed to send message to peer {}: {:?}",
                    validator_pub_key, e
                );
            }
        }
        Ok(())
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

        let p2p_server1 = p2p_server_test::start_server(
            crypto1,
            "node1".to_string(),
            port1,
            format!("127.0.0.1:{port1}"),
            "127.0.0.1:4321".into(), // send some dummy address for DA
        )
        .await?;
        let p2p_server2 = p2p_server_test::start_server(
            crypto2,
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
                        .handle_tcp_event(event.unwrap())
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
        p2p_server1
            .start_handshake(format!("127.0.0.1:{port2}"))
            .await?;

        // Initiate handshake from p2p_server2 to p2p_server1
        p2p_server2
            .start_handshake(format!("127.0.0.1:{port1}"))
            .await?;

        // Process messages for both servers to complete handshake
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
        p2p_server1
            .start_handshake(format!("127.0.0.1:{port2}"))
            .await?;

        // Process initial handshake
        // Server2 receives Hello message
        process_messages(&mut p2p_server2, 1).await?;
        // Server1 receives Verack message
        process_messages(&mut p2p_server1, 1).await?;

        // Verify initial connection
        assert_eq!(p2p_server1.peers.len(), 1);
        let peer_key = p2p_server1.peers.keys().next().unwrap().clone();

        // Simulate disconnection by dropping peer from server2
        p2p_server2.remove_peer(p2p_server1.crypto.validator_pubkey());
        // let peer_info = p2p_server2.peers.values().next().unwrap();
        // p2p_server2
        //     .tcp_server
        //     .drop_peer_stream(peer_info.socket_addr.clone());

        // Verify that server1 received the connection closed event
        // process_messages(&mut p2p_server1, 1).await?;

        // Try to send a message - this should trigger reconnection
        let test_msg = TestMessage("test reconnection".to_string());
        p2p_server1.send(peer_key.clone(), test_msg).await?;

        // Verify that server1 received the error for the previous send
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
