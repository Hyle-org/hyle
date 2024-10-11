//! Minimal block storage layer for data availability.

mod blocks;

use crate::{
    bus::{bus_client, BusMessage, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{Block, BlockHash, BlockHeight, Hashable, SharedRunContext, ValidatorPublicKey},
    p2p::network::{NetMessage, OutboundMessage, PeerEvent},
    utils::{logger::LogMe, modules::Module},
};
use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use blocks::Blocks;
use core::str;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    io::{Cursor, Write},
};
use tracing::{debug, error, info};

pub fn u64_to_str(u: u64, buf: &mut [u8]) -> &str {
    let mut cursor = Cursor::new(&mut buf[..]);
    _ = write!(cursor, "{}", u);
    let len = cursor.position() as usize;
    str::from_utf8(&buf[..len]).unwrap()
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataNetMessage {
    QueryBlock {
        respond_to: ValidatorPublicKey,
        hash: BlockHash,
    },
    QueryLastBlock {
        respond_to: ValidatorPublicKey,
    },
    QueryBlockResponse {
        block: Block,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataEvent {
    NewBlock(Block),
}

impl BusMessage for DataNetMessage {}
impl BusMessage for DataEvent {}

impl From<DataNetMessage> for NetMessage {
    fn from(msg: DataNetMessage) -> Self {
        NetMessage::DataMessage(msg)
    }
}

bus_client! {
#[derive(Debug)]
struct DABusClient {
    sender(OutboundMessage),
    sender(DataEvent),
    receiver(ConsensusEvent),
    receiver(DataNetMessage),
    receiver(PeerEvent),
}
}

#[derive(Debug)]
pub struct DataAvailability {
    bus: DABusClient,
    pub blocks: Blocks,
    buffered_blocks: BTreeSet<Block>,
    self_pubkey: ValidatorPublicKey,
    asked_last_block: bool,
}

impl Module for DataAvailability {
    fn name() -> &'static str {
        "DataAvailability"
    }

    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = DABusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        let db = sled::Config::new()
            .use_compression(true)
            .compression_factor(15)
            .path(
                ctx.common
                    .config
                    .data_directory
                    .join("data_availability.db"),
            )
            .open()
            .context("opening the database")?;

        let buffered_blocks = BTreeSet::new();
        let self_pubkey = ctx.node.crypto.validator_pubkey().clone();

        Ok(DataAvailability {
            bus,
            blocks: Blocks::new(&db)?,
            buffered_blocks,
            self_pubkey,
            asked_last_block: false,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DataAvailability {
    pub async fn start(&mut self) -> Result<()> {
        handle_messages! {
            on_bus self.bus,
            listen<ConsensusEvent> cmd => {
                self.handle_consensus_event(cmd).await;
            }
            listen<DataNetMessage> msg => {
                _ = self.handle_data_message(msg)
                    .log_error("NodeState: Error while handling data message");
            }
            listen<PeerEvent> msg => {
                match msg {
                    PeerEvent::NewPeer { pubkey, .. } => {
                        if !self.asked_last_block {
                            info!("📡  Asking for last block from new peer");
                            self.query_last_block(pubkey);
                            self.asked_last_block = true;
                        }
                    }
                }
            }
        }
    }

    fn handle_data_message(&mut self, msg: DataNetMessage) -> Result<()> {
        match msg {
            DataNetMessage::QueryBlock { respond_to, hash } => {
                self.blocks.get(hash).map(|block| {
                    if let Some(block) = block {
                        _ = self.bus.send(OutboundMessage::send(
                            respond_to,
                            DataNetMessage::QueryBlockResponse { block },
                        ));
                    }
                })?;
            }
            DataNetMessage::QueryBlockResponse { block } => {
                debug!(
                    block_hash = %block.hash(),
                    block_height = %block.height,
                    "⬇️  Received block data");
                self.handle_block(block);
            }
            DataNetMessage::QueryLastBlock { respond_to } => {
                if let Some(block) = self.blocks.last() {
                    _ = self.bus.send(OutboundMessage::send(
                        respond_to,
                        DataNetMessage::QueryBlockResponse {
                            block: block.clone(),
                        },
                    ));
                }
            }
        }
        Ok(())
    }

    async fn handle_consensus_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitBlock { block, .. } => {
                info!(
                    block_hash = %block.hash(),
                    block_height = %block.height,
                    "🔒  Block committed");
                self.handle_block(block);
            }
        }
    }

    fn handle_block(&mut self, block: Block) {
        // if new block is not the next block in the chain, buffer
        if self.blocks.last().is_some() {
            if self
                .blocks
                .get(block.parent_hash.clone())
                .unwrap_or(None)
                .is_none()
            {
                debug!(
                    "Parent block '{}' not found for block hash='{}' height {}",
                    block.parent_hash,
                    block.hash(),
                    block.height
                );
                self.query_block(block.parent_hash.clone());
                self.buffered_blocks.insert(block);
                return;
            }
        // if genesis block is missing, buffer
        } else if block.height != BlockHeight(0) {
            debug!(
                "Received block with height {} but genesis block is missing",
                block.height
            );
            self.query_block(block.parent_hash.clone());
            self.buffered_blocks.insert(block);
            return;
        }

        info!(
            "new block {} with {} txs, last hash = {:?}",
            block.height,
            block.txs.len(),
            self.blocks.last_block_hash()
        );
        // store block
        let block_hash = block.hash();
        self.add_block(block);
        self.pop_buffer(block_hash);
    }

    fn pop_buffer(&mut self, mut last_block_hash: BlockHash) {
        while let Some(first_buffered) = self.buffered_blocks.first() {
            if first_buffered.parent_hash != last_block_hash {
                break;
            }

            let first_buffered = self.buffered_blocks.pop_first().unwrap();
            let first_buffered_hash = first_buffered.hash();

            self.add_block(first_buffered);
            last_block_hash = first_buffered_hash;
        }
    }

    fn add_block(&mut self, block: Block) {
        if let Err(e) = self.blocks.put(block.clone()) {
            error!("storing block: {}", e);
        } else {
            _ = self
                .bus
                .send(DataEvent::NewBlock(block))
                .log_error("Error sending DataEvent");
        }
    }

    fn query_block(&mut self, hash: BlockHash) {
        _ = self
            .bus
            .send(OutboundMessage::broadcast(DataNetMessage::QueryBlock {
                respond_to: self.self_pubkey.clone(),
                hash,
            }));
    }

    fn query_last_block(&mut self, peer: ValidatorPublicKey) {
        _ = self.bus.send(OutboundMessage::send(
            peer,
            DataNetMessage::QueryLastBlock {
                respond_to: self.self_pubkey.clone(),
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{
        Blob, BlobData, BlobTransaction, Block, BlockHash, BlockHeight, ContractName, Hashable,
        Identity, Transaction, TransactionData,
    };

    use super::blocks::Blocks;
    use anyhow::Result;

    #[test]
    fn test_blocks() -> Result<()> {
        let tmpdir = tempdir::TempDir::new("history-tests")?;
        let db = sled::open(tmpdir.path().join("history"))?;
        let mut blocks = Blocks::new(&db)?;
        let block = Block {
            parent_hash: BlockHash {
                inner: vec![0, 1, 2, 3],
            },
            height: BlockHeight(1),
            timestamp: 42,
            new_bonded_validators: vec![],
            txs: vec![Transaction {
                version: 1,
                transaction_data: TransactionData::Blob(BlobTransaction {
                    identity: Identity("tx_id".to_string()),
                    blobs: vec![Blob {
                        contract_name: ContractName("c1".to_string()),
                        data: BlobData(vec![4, 5, 6]),
                    }],
                }),
            }],
        };
        blocks.put(block.clone())?;
        assert!(blocks.last().unwrap().height == block.height);
        let last = blocks.get(block.hash())?;
        assert!(last.is_some());
        assert!(last.unwrap().height == BlockHeight(1));
        Ok(())
    }
}
