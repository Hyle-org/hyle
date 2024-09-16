use crate::model::{Block, Transaction};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub struct Version {
    pub id: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetInput<T> {
    pub msg: T,
}

impl<T> NetInput<T> {
    pub fn new(msg: T) -> Self {
        Self { msg }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Broadcast {
    pub peer_id: u64,
    pub msg: NetMessage,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NetMessage {
    Version(Version),
    Verack,
    Ping,
    Pong,
    MempoolMessage(MempoolNetMessage),
    ConsensusMessage(ConsensusNetMessage),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum ConsensusNetMessage {
    CommitBlock(Block),
}

impl NetMessage {
    pub fn to_binary(&self) -> Vec<u8> {
        bincode::encode_to_vec(&self, bincode::config::standard())
            .expect("Could not serialize NetMessage")
    }
}
