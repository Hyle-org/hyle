use crate::model::{Block, Transaction};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Version {
    pub id: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum NetMessage {
    Version(Version),
    Verack,
    Ping,
    Pong,
    // TODO: To replace with an ApiMessage equivalent
    NewTransaction(Transaction),
    MempoolMessage(MempoolNetMessage),
    ConsensusMessage(ConsensusNetMessage),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum MempoolNetMessage {
    NewTx(Transaction),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ConsensusNetMessage {
    CommitBlock(Block),
}

impl NetMessage {
    pub fn to_binary(&self) -> Vec<u8> {
        bincode::serialize(&self).expect("Could not serialize NetMessage")
    }
}
