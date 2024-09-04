use std::time::SystemTime;

use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Transaction {
    pub inner: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Block {
    pub parent_hash: String,
    pub height: usize,
    pub timestamp: SystemTime,
    pub txs: Vec<Transaction>,
}

impl Block {
    pub fn hash_block(&self) -> String {
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(7)
            .map(char::from)
            .collect()
    }
}

impl std::default::Default for Block {
    fn default() -> Self {
        Block {
            parent_hash: "000".to_string(),
            height: 0,
            timestamp: SystemTime::now(),
            txs: vec![],
        }
    }
}
