mod blobs;
mod blocks;
mod contracts;
mod db;
pub mod model;
mod proofs;
mod store;
mod transactions;

use crate::{
    bus::SharedMessageBus,
    consensus::ConsensusEvent,
    model::{Block, BlockHeight, Hashable},
    utils::conf::SharedConf,
};
use anyhow::Result;
use core::str;
use db::Db;
use model::{Contract, Transaction};
use std::sync::Arc;
use tokio::{
    sync::Mutex,
    time::{sleep, Duration},
};
use tracing::{debug, error, info};

#[derive(Debug, Clone)]
pub struct Indexer {
    inner: Arc<Mutex<IndexerInner>>,
}

impl Indexer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(IndexerInner::new()?)),
        })
    }

    pub fn share(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    pub async fn start(&mut self, config: SharedConf, bus: SharedMessageBus) {
        let interval = config.storage.interval;
        let mut receiver = bus.receiver::<ConsensusEvent>().await;

        loop {
            sleep(Duration::from_secs(interval)).await;
            tokio::select! {
                Ok(event) = receiver.recv() => {
                    match event {
                        ConsensusEvent::CommitBlock{block, ..} => self.handle_block(block).await,
                    }
                }
            }
        }
    }

    async fn handle_block(&mut self, block: Block) {
        info!("new block {} with {} txs", block.height, block.txs.len());
        let mut guard = self.inner.lock().await;
        for (ti, tx) in block.txs.iter().enumerate() {
            let tx_hash = format!("{}", tx.hash());
            debug!("tx:{} hash {}", ti, tx_hash);
            match tx.transaction_data {
                crate::model::TransactionData::Blob(ref tx) => {
                    for (bi, blob) in tx.blobs.iter().enumerate() {
                        if let Err(e) =
                            guard
                                .db
                                .blobs
                                .store(block.height, ti, &tx_hash, bi, &tx.identity, blob)
                        {
                            error!("storing blob of tx {} in block {}: {}", ti, block.height, e);
                        }
                    }
                }
                crate::model::TransactionData::Proof(ref tx) => {
                    if let Err(e) = guard.db.proofs.store(
                        block.height,
                        ti,
                        &tx_hash,
                        &tx.blobs_references,
                        &tx.proof,
                    ) {
                        error!(
                            "storing proof of tx {} in block {}: {}",
                            ti, block.height, e
                        );
                    }
                }
                crate::model::TransactionData::RegisterContract(ref tx) => {
                    if let Err(e) = guard.db.contracts.store(tx) {
                        error!(
                            "storing contract {} of tx {} in block {}: {}",
                            tx.contract_name.0, ti, block.height, e
                        );
                    }
                }
            }
            if let Err(e) =
                guard
                    .db
                    .transactions
                    .store(block.height, ti, &tx_hash, &tx.transaction_data)
            {
                error!(
                    "storing contract of tx {} in block {}: {}",
                    ti, block.height, e
                );
            }
        }
        // store block
        if let Err(e) = guard.db.blocks.store(block) {
            error!("storing block: {}", e);
        }
    }
}

impl std::ops::Deref for Indexer {
    type Target = Arc<Mutex<IndexerInner>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Debug)]
pub struct IndexerInner {
    // txs: Vec<Transaction>,
    // contracts: Vec<Contract>,
    // blobs: Vec<Blob>,
    // proofs: Vec<Proof>,
    pub db: Db,
}

impl IndexerInner {
    pub fn new() -> Result<Self> {
        Ok(Self {
            // txs: Vec::new(),
            // contracts: Vec::new(),
            // blobs: Vec::new(),
            // proofs: Vec::new(),
            db: Db::new("indexer.db")?,
        })
    }

    // API:

    pub fn get_contract(&self, name: &str) -> Result<Option<Contract>> {
        self.db.contracts.retrieve(name)
    }

    pub fn get_block(&self, height: BlockHeight) -> Result<Option<Block>> {
        self.db.blocks.retrieve(height)
    }

    pub fn last_block(&self) -> Option<Block> {
        self.db.blocks.last.clone()
    }

    pub fn get_tx(&self, tx_hash: &str) -> Result<Option<Transaction>> {
        self.db.transactions.retrieve(tx_hash)
    }
}
