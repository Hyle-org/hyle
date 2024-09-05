use std::fs;
use tokio::sync::mpsc::Receiver;

use crate::logger::LogMe;
use crate::model::get_current_timestamp;
use crate::model::{Block, Transaction};
use serde::Deserialize;
use serde::Serialize;
use tracing::info;

#[derive(Debug)]
pub enum CtxCommand {
    AddTransaction(Transaction),
    SaveOnDisk,
    GenerateNewBlock,
}

#[derive(Serialize, Deserialize)]
pub struct Ctx {
    mempool: Vec<Transaction>,
    blocks: Vec<Block>,
}

impl Ctx {
    fn add_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    fn handle_tx(&mut self, tx: Transaction) {
        info!("New tx: {:?}", tx);
        self.mempool.push(tx);
    }

    fn new_block(&mut self) {
        let last_block = self.blocks.last().unwrap();

        let mempool = self.mempool.drain(0..).collect();
        self.add_block(Block {
            parent_hash: last_block.hash_block(),
            height: last_block.height + 1,
            timestamp: get_current_timestamp(),
            txs: mempool,
        });
        info!("New block {:?}", self.blocks.last());
    }

    pub fn save_on_disk(&mut self) -> anyhow::Result<()> {
        let encoded = bincode::serialize(&self).log_error("Serializing Ctx chain")?;
        fs::write("data.bin", encoded).log_error("Write Ctx file")?;
        info!(
            "Saved blockchain on disk with {} blocks and {} tx in mempool.",
            self.blocks.len(),
            self.mempool.len()
        );
        Ok(())
    }

    pub fn load_from_disk() -> anyhow::Result<Self> {
        let read_v = fs::read("data.bin").log_warn("Loading data from disk")?;
        let ctx = bincode::deserialize::<Self>(&read_v).log_warn("Deserializing data from disk")?;
        info!(
            "Loaded {} blocks and {} tx in mempool from disk.",
            ctx.blocks.len(),
            ctx.mempool.len()
        );

        Ok(ctx)
    }

    pub async fn start(&mut self, mut rx: Receiver<CtxCommand>) -> anyhow::Result<()> {
        while let Some(msg) = rx.recv().await {
            match msg {
                CtxCommand::AddTransaction(tx) => self.handle_tx(tx),
                CtxCommand::GenerateNewBlock => self.new_block(),
                CtxCommand::SaveOnDisk => {
                    self.save_on_disk()?;
                }
            }
        }
        Ok(())
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
