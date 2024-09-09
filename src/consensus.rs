use std::fs;
use std::time::Duration;
use tokio::sync::mpsc::{Receiver, UnboundedSender};

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tracing::info;

use crate::conf::Conf;
use crate::logger::LogMe;
use crate::model::get_current_timestamp;
use crate::model::{Block, Hashable, Transaction};

#[derive(Debug)]
pub enum ConsensusCommand {
    AddTransaction(Transaction),
    SaveOnDisk,
    GenerateNewBlock,
}

#[derive(Serialize, Deserialize)]
pub struct Consensus {
    mempool: Vec<Transaction>,
    blocks: Vec<Block>,
}

impl Consensus {
    fn add_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    async fn handle_tx(&mut self, tx: Transaction, sender: &UnboundedSender<Transaction>) {
        info!("New tx: {:?}", tx);
        _ = sender
            .send(tx.clone())
            .log_error("broadcasting tx to mempool nodes");
        self.mempool.push(tx);

        let last_block = self.blocks.last().unwrap();
        let last = last_block.timestamp;

        if get_current_timestamp() - last > 5 {
            let mempool = self.mempool.drain(0..).collect();
            let b = Block {
                parent_hash: last_block.hash(),
                height: last_block.height + 1,
                timestamp: get_current_timestamp(),
                txs: mempool,
            };
            info!("parent block hash: {}", b.parent_hash);
            info!("New block {:?}", &b);
            self.add_block(b);
        }
    }

    fn new_block(&mut self) {
        let last_block = self.blocks.last().unwrap();

        let mempool = self.mempool.drain(0..).collect();
        self.add_block(Block {
            parent_hash: last_block.hash(),
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

    pub async fn start(
        &mut self,
        tx: Sender<ConsensusCommand>,
        mut rx: Receiver<ConsensusCommand>,
        sender: UnboundedSender<Transaction>,
        config: &Conf,
    ) -> anyhow::Result<()> {
        let interval = config.storage.interval;

        if config.peers.is_empty() {
            info!(
                "No peers configured, starting as master generating blocks every {} seconds",
                interval
            );

            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(interval)).await;

                    tx.send(ConsensusCommand::GenerateNewBlock)
                        .await
                        .expect("Cannot send message over channel");
                    tx.send(ConsensusCommand::SaveOnDisk)
                        .await
                        .expect("Cannot send message over channel");
                }
            });
        }

        while let Some(msg) = rx.recv().await {
            match msg {
                // TODO: when the bus is ready, this Event AddTransaction should arrive directly
                // from peer to mempool whithout passing through Consensus
                ConsensusCommand::AddTransaction(tx) => self.handle_tx(tx, &sender).await,
                ConsensusCommand::GenerateNewBlock => self.new_block(),
                ConsensusCommand::SaveOnDisk => {
                    let _ = self.save_on_disk();
                }
            }
        }
        Ok(())
    }
}

impl std::default::Default for Consensus {
    fn default() -> Self {
        Self {
            mempool: vec![],
            blocks: vec![Block::default()],
        }
    }
}
