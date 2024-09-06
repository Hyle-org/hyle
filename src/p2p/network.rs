use serde::{Deserialize, Serialize};

use crate::model::Transaction;

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
    NewTransaction(Transaction),
}

impl NetMessage {
    pub fn as_binary(&self) -> Vec<u8> {
        bincode::serialize(&self).expect("Could not serialize NetMessage")
    }
}
