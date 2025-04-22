use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context};
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_crypto::BlstCrypto;
use sdk::{SignedByValidator, ValidatorPublicKey};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{info, trace, warn};

use crate::tcp::{tcp_client::TcpClient, Handshake};

use super::{tcp_server::TcpServer, NodeConnectionData, P2PTcpMessage, TcpEvent};

#[derive(Debug)]
pub enum P2PServerEvent {
    NewPeer {
        name: String,
        pubkey: ValidatorPublicKey,
        da_address: String,
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
    node_hostname: String,
    node_da_port: u16,
    node_p2p_port: u16,
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
        node_hostname: String,
        node_da_port: u16,
        node_p2p_port: u16,
        tcp_server: TcpServer<Codec, P2PTcpMessage<Msg>, P2PTcpMessage<Msg>>,
    ) -> Self {
        Self {
            crypto,
            node_id,
            node_hostname,
            node_da_port,
            node_p2p_port,
            tcp_server,
            peers: HashMap::new(),
        }
    }

    pub async fn listen_next(&mut self) -> Option<TcpEvent<P2PTcpMessage<Msg>>> {
        self.tcp_server.listen_next().await
    }

    pub async fn handle_handshake(
        &mut self,
        dest: String,
        handshake: Handshake,
    ) -> anyhow::Result<Option<P2PServerEvent>> {
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
    ) -> Option<P2PServerEvent> {
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
                da_address: format!("{}:{}", v.msg.hostname, v.msg.da_port),
            })
        }
    }

    fn create_signed_node_connection_data(
        &self,
    ) -> anyhow::Result<SignedByValidator<NodeConnectionData>> {
        let node_connection_data = NodeConnectionData {
            version: 1,
            name: self.node_id.clone(),
            p2p_port: self.node_p2p_port,
            da_port: self.node_da_port,
            hostname: self.node_hostname.clone(),
        };
        self.crypto.sign(node_connection_data)
    }

    pub async fn start_handshake(&mut self, peer_ip: String) -> anyhow::Result<()> {
        if peer_ip == format!("{}:{}", self.node_hostname, self.node_p2p_port) {
            trace!("Trying to connect to self");
            return Ok(());
        }

        for peer in self.peers.values() {
            if peer_ip
                == format!(
                    "{}:{}",
                    peer.node_connection_data.hostname, peer.node_connection_data.p2p_port
                )
            {
                warn!("Peer {} already connected", peer_ip);
                return Ok(());
            }
        }

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
            .await?;

        Ok(())
    }

    pub async fn send(
        &mut self,
        validator_pub_key: ValidatorPublicKey,
        msg: Msg,
    ) -> anyhow::Result<()> {
        if let Some(peer_info) = self.peers.get(&validator_pub_key) {
            let validator_ip = peer_info.socket_addr.clone();
            // We found the peer, we can send the message
            if let Err(e) = self
                .tcp_server
                .send(validator_ip.clone(), P2PTcpMessage::Data(msg.clone()))
                .await
            {
                bail!(
                    "Failed to send message to peer {} at {}: {:?}. Attempting to reconnect...",
                    validator_pub_key,
                    validator_ip,
                    e
                );
                // TODO: retry ? And probably redo a handshake
            }
            return Ok(());
        }

        // If we don't know the peer, log a warning
        warn!(
            "Trying to send message to unknown Peer {}. Unable to proceed.",
            validator_pub_key
        );

        Ok(())
    }

    pub async fn broadcast(&mut self, msg: Msg) -> anyhow::Result<()> {
        let failed_peers = self.tcp_server.broadcast(P2PTcpMessage::Data(msg)).await;
        if failed_peers.is_empty() {
            return Ok(());
        } else {
            // TODO: retry for each peer that failed ? And probably redo a handshake
        }
        Ok(())
    }

    pub async fn broadcast_only_for(
        &mut self,
        only_for: &HashSet<ValidatorPublicKey>,
        msg: Msg,
    ) -> anyhow::Result<()> {
        for validator_pub_key in only_for {
            if let Some(peer_info) = self.peers.get(validator_pub_key) {
                if let Err(e) = self
                    .tcp_server
                    .send(
                        peer_info.socket_addr.clone(),
                        P2PTcpMessage::Data(msg.clone()),
                    )
                    .await
                {
                    warn!(
                        "Failed to send message to peer {} at {}: {:?}",
                        validator_pub_key, peer_info.socket_addr, e
                    );
                }
            } else {
                warn!(
                    "ValidatorPublicKey {} not found in peers. Skipping.",
                    validator_pub_key
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

    use crate::tcp::P2PTcpMessage;

    use crate::p2p_server_mod;

    pub async fn find_available_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        addr.port()
    }
    // Simple message type for testing
    #[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
    pub struct TestMessage(String);

    crate::tcp_client_server! {
        pub TestP2pTcpMessage,
        request: crate::tcp::P2PTcpMessage<super::TestMessage>,
        response: crate::tcp::P2PTcpMessage<super::TestMessage>
    }

    #[test_log::test(tokio::test)]
    async fn p2p_server_concurrent_handshake_test() -> Result<()> {
        let crypto1 = BlstCrypto::new_random().unwrap();
        let crypto2 = BlstCrypto::new_random().unwrap();

        let port1 = find_available_port().await;
        let port2 = find_available_port().await;

        p2p_server_mod! {
            pub test,
            message: crate::tcp::p2p_server::tests::TestMessage
        };

        let mut p2p_server1 = p2p_server_test::start_server(
            crypto1,
            "node1".to_string(),
            "127.0.0.1".to_string(),
            0,
            port1,
        )
        .await?;
        let mut p2p_server2 = p2p_server_test::start_server(
            crypto2,
            "node2".to_string(),
            "127.0.0.1".to_string(),
            0,
            port2,
        )
        .await?;

        // Initiate handshake from p2p_server1 to p2p_server2
        p2p_server1
            .start_handshake(format!("127.0.0.1:{port2}"))
            .await?;

        // Initiate handshake from p2p_server2 to p2p_server1
        p2p_server2
            .start_handshake(format!("127.0.0.1:{port1}"))
            .await?;

        // Process messages for both servers to complete handshake
        for _ in 0..2 {
            tokio::select! {
                event = p2p_server1.listen_next() => {
                    if let Some(event) = event {
                        if let P2PTcpMessage::Handshake(handshake) = event.data {
                            p2p_server1
                                .handle_handshake(event.dest.clone(), handshake)
                                .await?;
                        }
                    }
                }
                event = p2p_server2.listen_next() => {
                    if let Some(event) = event {
                        if let P2PTcpMessage::Handshake(handshake) = event.data {
                            p2p_server2
                                .handle_handshake(event.dest.clone(), handshake)
                                .await?;
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    println!("Timeout waiting for handshake messages");
                    break;
                }
            }
        }

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
}
