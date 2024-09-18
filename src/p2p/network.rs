use crate::{
    model::{Block, Transaction},
    replica_registry::{Replica, ReplicaId},
};
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

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Default)]
pub struct Signature(pub Vec<u8>);

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct Signed<T: Encode> {
    pub msg: T,
    pub signature: Signature,
    pub replica_id: ReplicaId,
}
<<<<<<< HEAD

pub type SignedMempoolNetMessage = Signed<MempoolNetMessage>;
pub type SignedConsensusNetMessage = Signed<ConsensusNetMessage>;
=======

impl<T: Wrappable<T> + Encode> Signed<T> {
    pub fn wrap(self) -> NetMessage {
        T::wrap(self)
    }
}

pub trait Wrappable<T: Encode> {
    fn wrap(msg: Signed<T>) -> NetMessage;
}
>>>>>>> a8e9709 (✨ Add ReplicaRegistry)

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NetMessage {
    HandshakeMessage(HandshakeNetMessage),
<<<<<<< HEAD
    MempoolMessage(SignedMempoolNetMessage),
    ConsensusMessage(SignedConsensusNetMessage),
=======
    ReplicaRegistryMessage(ReplicaRegistryNetMessage),
    MempoolMessage(Signed<MempoolNetMessage>),
    ConsensusMessage(Signed<ConsensusNetMessage>),
>>>>>>> a8e9709 (✨ Add ReplicaRegistry)
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

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum ConsensusNetMessage {
    CommitBlock(Block),
}

impl From<HandshakeNetMessage> for NetMessage {
    fn from(msg: HandshakeNetMessage) -> Self {
        NetMessage::HandshakeMessage(msg)
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
