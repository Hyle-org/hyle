use crate::{
    bus::SharedMessageBus,
    consensus::ConsensusEvent,
    model::{Block, BlockHeight, Hashable, Transaction},
    utils::{conf::SharedConf, logger::LogMe},
};
use anyhow::Result;
use bincode::{self, Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, sync::Arc};
use tokio::{
    sync::Mutex,
    time::{sleep, Duration},
};
use tracing::{error, info, warn};

type Position = usize;

#[derive(Debug, Serialize, Deserialize, Encode, Decode)]
struct TxRef(BlockHeight, Position);

#[derive(Debug, Serialize, Deserialize, Encode, Decode)]
pub struct IndexerInner {
    blocks: HashMap<BlockHeight, Block>,
    txs: HashMap<String, TxRef>,
    height: BlockHeight,
}

#[derive(Debug, Clone)]
pub struct Indexer {
    inner: Arc<Mutex<IndexerInner>>,
}

impl Indexer {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(IndexerInner {
                blocks: HashMap::new(),
                txs: HashMap::new(),
                height: BlockHeight(0),
            })),
        }
    }

    pub fn share(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    pub async fn start(&mut self, config: SharedConf, bus: SharedMessageBus) {
        let interval = config.storage.interval;
        let mut receiver = bus.receiver::<ConsensusEvent>().await;

        if let Err(e) = self.lock().await.load_from_disk() {
            warn!("Loading from disk: {}", e);
        }

        loop {
            sleep(Duration::from_secs(interval)).await;
            tokio::select! {
                Ok(event) = receiver.recv() => {
                    match event {
                        ConsensusEvent::CommitBlock{block, ..} => {
                            info!("new block {} with {} txs", block.height, block.txs.len());
                            let mut guard = self.inner.lock().await;
                            guard.height = block.height;
                            for (i, tx) in block.txs.iter().enumerate() {
                                let hash = format!("{}", tx.hash());
                                guard.txs.insert(hash, TxRef(block.height, i));
                            }
                            _ = guard.blocks.insert(block.height, block);
                        }
                    }
                }
            }

            if let Err(e) = self.lock().await.save_to_disk() {
                error!("Saving to disk: {}", e);
            }
        }
    }
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Deref for Indexer {
    type Target = Arc<Mutex<IndexerInner>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl IndexerInner {
    pub fn save_to_disk(&self) -> Result<()> {
        let mut writer = fs::File::create("indexer.bin").log_error("Create indexer file")?;
        bincode::encode_into_std_write(self, &mut writer, bincode::config::standard())
            .log_error("Serializing Ctx chain")?;
        info!("Saved {} blocks to disk", self.blocks.len());

        Ok(())
    }

    pub fn load_from_disk(&self) -> Result<Self> {
        let mut reader = fs::File::open("indexer.bin").log_warn("Loading indexer from disk")?;
        let ctx: IndexerInner =
            bincode::decode_from_std_read(&mut reader, bincode::config::standard())
                .log_warn("Deserializing data from disk")?;
        info!("Loaded {} blocks from disk.", ctx.blocks.len());

        Ok(ctx)
    }

    // API:

    pub fn get_block(&self, height: &BlockHeight) -> Option<Block> {
        self.blocks.get(height).cloned()
    }

    pub fn last_block(&self) -> Option<Block> {
        self.blocks.get(&self.height).cloned()
    }

    pub fn get_tx(&self, txhash: &str) -> Option<Transaction> {
        self.txs
            .get(txhash)
            .and_then(|TxRef(h, i)| self.blocks.get(h).and_then(|b| b.txs.get(*i)))
            .cloned()
    }

    pub fn _get_txs(&self) -> Vec<Transaction> {
        todo!()
    }
}
