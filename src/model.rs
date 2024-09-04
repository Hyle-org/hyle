use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Transaction {
    pub inner: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Block {
    pub parent_hash: String,
    pub height: usize,
    pub timestamp: u64,
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
            timestamp: get_current_timestamp(),
            txs: vec![],
        }
    }
}

pub fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}
