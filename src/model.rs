// use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

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
        let mut hasher = Sha3_256::new();
        hasher.update(self.parent_hash.as_bytes());
        _ = write!(hasher, "{}", self.height);
        _ = write!(hasher, "{}", self.timestamp);
        for tx in self.txs.iter() {
            // FIXME:
            hasher.update(tx.inner.as_bytes());
        }
        hex::encode(hasher.finalize())
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
