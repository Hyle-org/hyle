use std::fs;
use std::time::Duration;
use std::time::SystemTime;

use serde::Deserialize;
use serde::Serialize;
use tracing::info;
use tracing::warn;

use crate::model::{Block, Transaction};

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

            self.save_on_disk();
        } else {
            info!("New tx: {}", data);
        }
    }

    pub fn save_on_disk(&mut self) {
        let encoded = bincode::serialize(&self).expect("Could not serialize chain");
        fs::write("data.bin", encoded).expect("could not write file");
        info!(
            "Saved blockchain on disk with {} blocks and {} tx in mempool.",
            self.blocks.len(),
            self.mempool.len()
        );
    }

    pub fn load_from_disk() -> Self {
        match fs::read("data.bin") {
            Ok(read_v) => {
                match bincode::deserialize::<Self>(&read_v) {
                    Ok(decoded_v) => {
                        info!(
                            "Loaded {} blocks and {} tx in mempool from disk.",
                            decoded_v.blocks.len(),
                            decoded_v.mempool.len()
                        );
                        decoded_v
                    }
                    Err(error) => {
                        warn!("Could not deserialize file data.bin. Error :{}. Starting a fresh context.", error);
                        let mut ctx = Self::default();
                        let genesis = Block::default();
                        ctx.add_block(genesis);
                        ctx
                    }
                }
            }
            Err(error) => {
                warn!(
                    "Could not read wile data.bin. Error: {}. Starting with a fresh context.",
                    error
                );
                let mut ctx = Self::default();
                let genesis = Block::default();
                ctx.add_block(genesis);
                ctx
            }
        }
    }
}
