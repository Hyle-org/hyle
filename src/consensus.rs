use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, default::Default, fs, time::Duration};
use tokio::{sync::broadcast::Sender, time::sleep};
use tracing::{info, warn};

use crate::{
    bus::{command_response::CmdRespClient, SharedMessageBus},
    handle_messages,
    mempool::{MempoolCommand, MempoolResponse},
    model::{get_current_timestamp, Block, Hashable, Transaction},
    p2p::network::{OutboundMessage, ReplicaRegistryNetMessage, Signed, SignedConsensusNetMessage},
    replica_registry::ReplicaRegistry,
    utils::{conf::SharedConf, crypto::BlstCrypto, logger::LogMe},
};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum ConsensusNetMessage {
    CommitBlock(Block),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    SaveOnDisk,
    GenerateNewBlock,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusEvent {
    CommitBlock { batch_id: String, block: Block },
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct Consensus {
    replicas: ReplicaRegistry,
    blocks: Vec<Block>,
    batch_id: u64,
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

        info!("New block {}", block.height);
        block
    }

    pub fn save_on_disk(&self) -> Result<()> {
        let mut writer = fs::File::create("data.bin").log_error("Create Ctx file")?;
        bincode::encode_into_std_write(self, &mut writer, bincode::config::standard())
            .log_error("Serializing Ctx chain")?;
        info!("Saved blockchain on disk with {} blocks", self.blocks.len());

        Ok(())
    }

    pub fn load_from_disk() -> Result<Self> {
        let mut reader = fs::File::open("data.bin").log_warn("Loading data from disk")?;
        let ctx: Consensus =
            bincode::decode_from_std_read(&mut reader, bincode::config::standard())
                .log_warn("Deserializing data from disk")?;
        info!("Loaded {} blocks from disk.", ctx.blocks.len());

        Ok(ctx)
    }

    fn handle_net_message(&mut self, msg: Signed<ConsensusNetMessage>) {
        match self.replicas.check_signed(&msg) {
            Ok(valid) => {
                if valid {
                    match msg.msg {
                        ConsensusNetMessage::CommitBlock(block) => {
                            info!("Got a commited block {:?}", block)
                        }
                    }
                } else {
                    warn!("Invalid signature for message {:?}", msg);
                }
            }
            Err(e) => warn!("Error while checking signed message: {}", e),
        }
    }

    async fn handle_command(
        &mut self,
        msg: ConsensusCommand,
        bus: &SharedMessageBus,
        crypto: &BlstCrypto,
        consensus_event_sender: &Sender<ConsensusEvent>,
        outbound_sender: &Sender<OutboundMessage>,
    ) -> Result<()> {
        match msg {
            ConsensusCommand::GenerateNewBlock => {
                self.batch_id += 1;
                if let Some(res) = bus
                    .request(MempoolCommand::CreatePendingBatch {
                        id: self.batch_id.to_string(),
                    })
                    .await?
                {
                    match res {
                        MempoolResponse::PendingBatch { id, txs } => {
                            info!("Received pending batch {} with {} txs", &id, &txs.len());
                            self.tx_batches.insert(id.clone(), txs);
                            let block = self.new_block();
                            // send to internal bus
                            _ = consensus_event_sender
                                .send(ConsensusEvent::CommitBlock {
                                    batch_id: id,
                                    block: block.clone(),
                                })
                                .context(
                                    "Failed to send ConsensusEvent::CommitBlock msg on the bus",
                                )?;
                            // send to network
                            _ = outbound_sender
                                .send(OutboundMessage::broadcast(Self::sign_net_message(crypto, ConsensusNetMessage::CommitBlock(block))?)).context("Failed to send ConsensusNetMessage::CommitBlock msg on the bus")?;
                        }
                    }
                }
                Ok(())
            }

            ConsensusCommand::SaveOnDisk => {
                _ = self.save_on_disk();
                Ok(())
            }
        }
    }

    pub async fn start(
        &mut self,
        bus: SharedMessageBus,
        config: SharedConf,
        crypto: BlstCrypto,
    ) -> Result<()> {
        let interval = config.storage.interval;
        let is_master = config.peers.is_empty();
        let outbound_sender = bus.sender::<OutboundMessage>().await;
        let consensus_event_sender = bus.sender::<ConsensusEvent>().await;
        let consensus_command_sender = bus.sender::<ConsensusCommand>().await;

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
        handle_messages! {
            listen<ConsensusCommand>(bus) = cmd => {
                _ = self.handle_command(cmd, &bus, &crypto, &consensus_event_sender, &outbound_sender).await;
            },
            listen<SignedConsensusNetMessage>(bus) = cmd => {
                self.handle_net_message(cmd);
            },
            listen<ReplicaRegistryNetMessage>(bus) = cmd => {
                self.replicas.handle_net_message(cmd);
            },
        }
    }

    fn sign_net_message(
        crypto: &BlstCrypto,
        msg: ConsensusNetMessage,
    ) -> Result<Signed<ConsensusNetMessage>> {
        crypto.sign(msg)
    }
}

impl Default for Consensus {
    fn default() -> Self {
        Self {
            replicas: ReplicaRegistry::default(),
            blocks: vec![Block::default()],
            batch_id: 0,
            tx_batches: HashMap::new(),
            current_block_batches: vec![],
        }
    }
}
