use rand::{distributions::Alphanumeric, Rng};
use tokio::time::Instant; // 0.8

#[derive(Debug)]
pub struct Transaction {
    pub inner: String,
}

#[derive(Debug)]
pub struct Block {
    pub parent_hash: String,
    pub height: usize,
    pub timestamp: Instant,
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
            timestamp: Instant::now(),
            txs: vec![],
        }
    }
}
