use crate::model::{Block, Transaction};
use tokio::time::{Duration, Instant};
use tracing::info;

#[derive(Default)]
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

        if Instant::now() - last > Duration::from_secs(5) {
            self.blocks.push(Block {
                parent_hash: last_block.hash_block(),
                height: last_block.height + 1,
                timestamp: Instant::now(),
                txs: self.mempool.drain(0..).collect(),
            });
            info!("New block {:?}", self.blocks.last());
        } else {
            info!("New tx: {}", data);
        }
    }
}
