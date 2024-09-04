use crate::model::{Block, Transaction};
use std::time::Duration;
use std::time::SystemTime;

use serde::Deserialize;
use serde::Serialize;
use tracing::info;

#[derive(Default, Serialize, Deserialize)]
pub struct Ctx {
    mempool: Vec<Transaction>,
    blocks: Vec<Block>,
}

impl Ctx {
    pub fn add_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    pub fn handle_tx(&mut self, data: &str) {
        self.mempool.push(Transaction {
            inner: data.to_string(),
        });

        let last_block = self.blocks.last().unwrap();
        let last = last_block.timestamp;

        if last.elapsed().unwrap() > Duration::from_secs(5) {
            self.blocks.push(Block {
                parent_hash: last_block.hash_block(),
                height: last_block.height + 1,
                timestamp: SystemTime::now(),
                txs: self.mempool.drain(0..).collect(),
            });
            info!("New block {:?}", self.blocks.last());
        } else {
            info!("New tx: {}", data);
        }
    }

    pub fn save_on_disk(&mut self) {
        let encoded: Vec<u8> = bincode::serialize(&self).unwrap();
    }
}
