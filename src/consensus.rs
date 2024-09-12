use std::collections::HashMap;
use std::fs;
use std::time::Duration;
use tokio::select;

use serde::Deserialize;
use serde::Serialize;
use tokio::time::sleep;
use tracing::info;

use crate::bus::SharedMessageBus;
use crate::mempool::MempoolCommand;
use crate::mempool::MempoolResponse;
use crate::model::get_current_timestamp;
use crate::model::{Block, Hashable, Transaction};
use crate::utils::conf::Conf;
use crate::utils::logger::LogMe;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    SaveOnDisk,
    GenerateNewBlock,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusEvent {
    NewBlock(Block),
}

#[derive(Serialize, Deserialize)]
pub struct Consensus {
    blocks: Vec<Block>,
    // Accumulated batches from mempool
    tx_batches: HashMap<String, Vec<Transaction>>,
    // Current proposed block
    current_block_batches: Vec<String>,
    // Once a block commits, we store it there (or not ?)
    // committed_block_batches: HashMap<String, HashMap<String, Vec<Transaction>>>,
}

impl Consensus {
    fn add_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    fn new_block(&mut self) -> Block {
        let last_block = self.blocks.last().unwrap();

        let mut all_txs = vec![];

        // prepare accumulated txs to feed the block
        // a previous batch can be there because of a previous block that failed to commit
        for (batch_id, txs) in self.tx_batches.iter() {
            all_txs.extend(txs.clone());
            self.current_block_batches.push(batch_id.clone());
        }

        // Start Consensus with following block
        let block = Block {
            parent_hash: last_block.hash(),
            height: last_block.height + 1,
            timestamp: get_current_timestamp(),
            txs: all_txs,
        };

        // Waiting for commit... TODO split this task

        // Commit block/if commit fails,
        // block won't be added, next block will try to add the txs
        self.add_block(block.clone());

        // Once commited we clean the state for the next block
        for cbb in self.current_block_batches.iter() {
            self.tx_batches.remove(cbb);
        }
        _ = self.current_block_batches.drain(0..);

        info!("New block {:?}", block);
        block
    }

    pub fn save_on_disk(&mut self) -> anyhow::Result<()> {
        let encoded = bincode::serialize(&self).log_error("Serializing Ctx chain")?;
        fs::write("data.bin", encoded).log_error("Write Ctx file")?;
        info!("Saved blockchain on disk with {} blocks", self.blocks.len());
        Ok(())
    }

    pub fn load_from_disk() -> anyhow::Result<Self> {
        let read_v = fs::read("data.bin").log_warn("Loading data from disk")?;
        let ctx = bincode::deserialize::<Self>(&read_v).log_warn("Deserializing data from disk")?;
        info!("Loaded {} blocks from disk.", ctx.blocks.len());

        Ok(ctx)
    }

    pub async fn start(&mut self, bus: SharedMessageBus, config: &Conf) -> anyhow::Result<()> {
        let interval = config.storage.interval;

        let consensus_events_sender = bus.sender::<ConsensusEvent>().await;
        let consensus_command_sender = bus.sender::<ConsensusCommand>().await;
        let mut consensus_command_receiver = bus.receiver::<ConsensusCommand>().await;
        let mempool_command_sender = bus.sender::<MempoolCommand>().await;
        let mut mempool_response_receiver = bus.receiver::<MempoolResponse>().await;

        let is_master = config.peers.is_empty();

        if is_master {
            info!(
                "No peers configured, starting as master generating blocks every {} seconds",
                interval
            );

            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(interval)).await;

                    _ = consensus_command_sender
                        .send(ConsensusCommand::GenerateNewBlock)
                        .log_error("Cannot send message over channel");
                    _ = consensus_command_sender
                        .send(ConsensusCommand::SaveOnDisk)
                        .log_error("Cannot send message over channel");
                }
            });
        }

        let mut batch_id = 0;

        loop {
            select! {
                Ok(msg) = consensus_command_receiver.recv() => {
                    match msg {
                        ConsensusCommand::GenerateNewBlock => {
                            batch_id += 1;
                            _ = mempool_command_sender
                                .send(MempoolCommand::CreatePendingBatch { id: batch_id.to_string() })
                                .log_error("Creating a new block");
                        },
                        ConsensusCommand::SaveOnDisk => {
                            let _ = self.save_on_disk();
                        }
                    }
                }
                Ok(mempool_response) = mempool_response_receiver.recv() => {
                    match mempool_response {
                        MempoolResponse::PendingBatch { id, txs } => {
                            info!("Received pending batch {} with {} txs", &id, &txs.len());
                            self.tx_batches.insert(id, txs);
                            let block = self.new_block();
                            _ = consensus_events_sender.send(ConsensusEvent::NewBlock(block)).log_error("error sending new block");
                        }
                    }
                }
            }
        }
    }
}

impl std::default::Default for Consensus {
    fn default() -> Self {
        Self {
            blocks: vec![Block::default()],
            tx_batches: HashMap::new(),
            current_block_batches: vec![],
        }
    }
}
