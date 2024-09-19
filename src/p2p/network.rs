use std::fmt;

use crate::replica_registry::{Replica, ReplicaId};
use crate::{consensus::ConsensusNetMessage, mempool::MempoolNetMessage};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct Version {
    pub id: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutboundMessage {
    SendMessage { peer_id: u64, msg: NetMessage },
    BroadcastMessage(NetMessage),
}

impl OutboundMessage {
    pub fn broadcast<T: Into<NetMessage>>(msg: T) -> Self {
        OutboundMessage::BroadcastMessage(msg.into())
    }
    pub fn send<T: Into<NetMessage>>(peer_id: u64, msg: T) -> Self {
        OutboundMessage::SendMessage {
            peer_id,
            msg: msg.into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Encode, Decode, Default)]
pub struct Signature(pub Vec<u8>);

impl std::fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Signature")
            .field(&hex::encode(&self.0))
            .finish()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct Signed<T: Encode> {
    pub msg: T,
    pub signature: Signature,
    pub replica_id: ReplicaId,
}

pub type SignedMempoolNetMessage = Signed<MempoolNetMessage>;
pub type SignedConsensusNetMessage = Signed<ConsensusNetMessage>;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NetMessage {
    HandshakeMessage(HandshakeNetMessage),
    MempoolMessage(SignedMempoolNetMessage),
    ConsensusMessage(SignedConsensusNetMessage),
    ReplicaRegistryMessage(ReplicaRegistryNetMessage),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum ReplicaRegistryNetMessage {
    NewReplica(Replica),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum HandshakeNetMessage {
    Version(Version),
    Verack,
    Ping,
    Pong,
}

impl From<HandshakeNetMessage> for NetMessage {
    fn from(msg: HandshakeNetMessage) -> Self {
        NetMessage::HandshakeMessage(msg)
    }
}

impl From<ReplicaRegistryNetMessage> for NetMessage {
    fn from(msg: ReplicaRegistryNetMessage) -> Self {
        NetMessage::ReplicaRegistryMessage(msg)
    }
}

impl From<Signed<MempoolNetMessage>> for NetMessage {
    fn from(msg: Signed<MempoolNetMessage>) -> Self {
        NetMessage::MempoolMessage(msg)
    }
}

impl From<Signed<ConsensusNetMessage>> for NetMessage {
    fn from(msg: Signed<ConsensusNetMessage>) -> Self {
        NetMessage::ConsensusMessage(msg)
    }
}

impl NetMessage {
    pub fn to_binary(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard())
            .expect("Could not serialize NetMessage")
    }
}
