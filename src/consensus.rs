use std::collections::HashMap;
use std::fs;
use std::time::Duration;
use tokio::select;
use tokio::sync::mpsc::{Receiver, UnboundedReceiver, UnboundedSender};

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tracing::info;

use crate::mempool::MempoolCommand;
use crate::mempool::MempoolResponse;
use crate::model::get_current_timestamp;
use crate::model::{Block, Hashable, Transaction};
use crate::utils::conf::Conf;
use crate::utils::logger::LogMe;

#[derive(Debug)]
pub enum ConsensusCommand {
    SaveOnDisk,
    GenerateNewBlock,
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

    fn new_block(&mut self) {
        let last_block = self.blocks.last().unwrap();

        let mut all_txs = vec![];

        // prepare accumulated txs to feed the block
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

        // Commit block
        self.add_block(block);

        // Once commited we clean the state for the next block
        for cbb in self.current_block_batches.iter() {
            self.tx_batches.remove(cbb);
        }
        _ = self.current_block_batches.drain(0..);

        info!("New block {:?}", self.blocks.last());
    }

    pub fn save_on_disk(&mut self) -> anyhow::Result<()> {
        let encoded = bincode::serialize(&self).log_error("Serializing Ctx chain")?;
        fs::write("data.bin", encoded).log_error("Write Ctx file")?;
        info!("Saved blockchain on disk with {} blocks", self.blocks.len(),);
        Ok(())
    }

    pub fn load_from_disk() -> anyhow::Result<Self> {
        let read_v = fs::read("data.bin").log_warn("Loading data from disk")?;
        let ctx = bincode::deserialize::<Self>(&read_v).log_warn("Deserializing data from disk")?;
        info!("Loaded {} blocks from disk.", ctx.blocks.len());

        Ok(ctx)
    }

    pub async fn start(
        &mut self,
        consensus_command_sender: Sender<ConsensusCommand>,
        mut consensus_command_receiver: Receiver<ConsensusCommand>,
        mempool_command_sender: UnboundedSender<MempoolCommand>,
        mut mempool_response_receiver: UnboundedReceiver<MempoolResponse>,
        config: &Conf,
    ) -> anyhow::Result<()> {
        let interval = config.storage.interval;

        let is_master = config.peers.is_empty();

        if is_master {
            info!(
                "No peers configured, starting as master generating blocks every {} seconds",
                interval
            );

            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(interval)).await;

                    consensus_command_sender
                        .send(ConsensusCommand::GenerateNewBlock)
                        .await
                        .log_error("Cannot send message over channel");
                    consensus_command_sender
                        .send(ConsensusCommand::SaveOnDisk)
                        .await
                        .log_error("Cannot send message over channel");
                }
            });
        }

        let mut batch_id = 0;

        loop {
            select! {
                Some(msg) = consensus_command_receiver.recv() => {
                    match msg {
                        ConsensusCommand::GenerateNewBlock => {
                            batch_id += 1;
                            mempool_command_sender.send(MempoolCommand::CreatePendingBatch { id: batch_id.to_string() });
                            self.new_block()
                        },
                        ConsensusCommand::SaveOnDisk => {
                            let _ = self.save_on_disk();
                        }
                    }
                }
                Some(mempool_response) = mempool_response_receiver.recv() => {
                    match mempool_response {
                        MempoolResponse::PendingBatch { id, txs } => {
                            info!("Received pending batch {} with {} txs", &id, &txs.len());
                            self.tx_batches.insert(id, txs);
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