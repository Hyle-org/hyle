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
    SendMessage {
        replica_id: ReplicaId,
        msg: NetMessage,
    },
    BroadcastMessage(NetMessage),
}

impl OutboundMessage {
    pub fn broadcast<T: Into<NetMessage>>(msg: T) -> Self {
        OutboundMessage::BroadcastMessage(msg.into())
    }
    pub fn send<T: Into<NetMessage>>(replica_id: ReplicaId, msg: T) -> Self {
        OutboundMessage::SendMessage {
            replica_id,
            msg: msg.into(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Default)]
pub struct Signature(pub Vec<u8>);

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct Signed<T: Encode> {
    pub msg: T,
    pub signature: Signature,
    pub replica_id: ReplicaId,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NetMessage {
    HandshakeMessage(HandshakeNetMessage),
    MempoolMessage(Signed<MempoolNetMessage>),
    ConsensusMessage(Signed<ConsensusNetMessage>),
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
