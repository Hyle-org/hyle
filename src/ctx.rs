use std::fs;
use std::time::Duration;
use std::time::SystemTime;
use tokio::sync::mpsc::Receiver;

use serde::Deserialize;
use serde::Serialize;
use tracing::info;
use tracing::warn;

use crate::model::{Block, Transaction};

#[derive(Serialize, Deserialize)]
pub struct Ctx {
    mempool: Vec<Transaction>,
    blocks: Vec<Block>,
}

impl Ctx {
    pub fn add_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    pub fn handle_tx(&mut self, tx: Transaction) {
        info!("New tx: {:?}", tx);
        self.mempool.push(tx);

        let last_block = self.blocks.last().unwrap();
        let last = last_block.timestamp;

        if last.elapsed().unwrap() > Duration::from_secs(5) {
            let mempool = self.mempool.drain(0..).collect();
            self.add_block(Block {
                parent_hash: last_block.hash_block(),
                height: last_block.height + 1,
                timestamp: SystemTime::now(),
                txs: mempool,
            });
            info!("New block {:?}", self.blocks.last());

            self.save_on_disk();
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
                    Ok(ctx) => {
                        info!(
                            "Loaded {} blocks and {} tx in mempool from disk.",
                            ctx.blocks.len(),
                            ctx.mempool.len()
                        );
                        ctx
                    }
                    Err(error) => {
                        warn!("Could not deserialize file data.bin. Error :{}. Starting a fresh context.", error);
                        Ctx::default()
                    }
                }
            }
            Err(error) => {
                warn!(
                    "Could not read file data.bin. Error: {}. Starting with a fresh context.",
                    error
                );
                Ctx::default()
            }
        }
    }

    pub async fn start(&mut self, mut rx: Receiver<Transaction>) {
        while let Some(tx) = rx.recv().await {
            self.handle_tx(tx)
        }
    }
}

impl std::default::Default for Ctx {
    fn default() -> Self {
        Self {
            mempool: vec![],
            blocks: vec![Block::default()],
        }
    }
}
