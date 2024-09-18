use crate::model::{Block, Transaction};
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

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NetMessage {
    HandshakeMessage(HandshakeNetMessage),
    MempoolMessage(MempoolNetMessage),
    ConsensusMessage(ConsensusNetMessage),
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

impl From<MempoolNetMessage> for NetMessage {
    fn from(msg: MempoolNetMessage) -> Self {
        NetMessage::MempoolMessage(msg)
    }
}

impl From<ConsensusNetMessage> for NetMessage {
    fn from(msg: ConsensusNetMessage) -> Self {
        NetMessage::ConsensusMessage(msg)
    }
}

impl NetMessage {
    pub fn to_binary(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard())
            .expect("Could not serialize NetMessage")
    }
}
