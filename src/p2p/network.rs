use crate::model::{Block, Transaction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
pub enum NetMessage {
    Version(Version),
    Verack,
    Ping,
    Pong,
    MempoolMessage(MempoolNetMessage),
    ConsensusMessage(ConsensusNetMessage),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ConsensusNetMessage {
    CommitBlock(Block),
}

impl NetMessage {
    pub fn to_binary(&self) -> Vec<u8> {
        bincode::serialize(&self).expect("Could not serialize NetMessage")
    }
}
