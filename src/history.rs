mod api;
mod blobs;
mod blocks;
pub use blocks::BlocksFilter;
mod contracts;
pub use contracts::ContractsFilter;
pub mod model;
mod proofs;
mod store;
mod transactions;
pub use transactions::TransactionsFilter;

use crate::{
    bus::SharedMessageBus,
    consensus::ConsensusEvent,
    model::{Block, Hashable},
    rest,
    utils::conf::SharedConf,
};
use anyhow::{Context, Result};
use axum::{routing::get, Router};
use blobs::Blobs;
use blocks::Blocks;
use contracts::Contracts;
use core::str;
use proofs::Proofs;
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use tokio::{
    sync::Mutex,
    time::{sleep, Duration},
};
use tracing::{debug, error, info};
use transactions::Transactions;

pub fn u64_to_str(u: u64, buf: &mut [u8]) -> &str {
    let mut cursor = Cursor::new(&mut buf[..]);
    _ = write!(cursor, "{}", u);
    let len = cursor.position() as usize;
    str::from_utf8(&buf[..len]).unwrap()
}

#[derive(Debug)]
pub struct History {
    inner: Arc<Mutex<HistoryInner>>,
}

impl History {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(HistoryInner::new()?)),
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

    pub fn api() -> Router<rest::RouterState> {
        Router::new()
            // block
            .route("/block/last", get(api::get_last_block))
            .route("/block/:height", get(api::get_block))
            .route("/blocks", get(api::get_blocks))
            // proof
            .route("/proof/:tx_hash", get(api::get_proof))
            // blob
            .route("/blobs/:tx_hash", get(api::get_blobs))
            .route("/blob/:tx_hash/:blob_index", get(api::get_blob))
            // transaction
            .route("/transactions", get(api::get_transactions))
            .route("/transaction/:height/:tx_index", get(api::get_transaction2))
            .route("/transaction/:tx_hash", get(api::get_transaction))
            // contract
            .route("/contracts", get(api::get_contracts))
            .route("/contracts/:name", get(api::get_contracts2))
            .route("/contract/:name/:tx_hash", get(api::get_contract2))
    }

    async fn handle_block(&mut self, block: Block) {
        info!("new block {} with {} txs", block.height, block.txs.len());
        for (ti, tx) in block.txs.iter().enumerate() {
            let tx_hash = format!("{}", tx.hash());
            debug!("tx:{} hash {}", ti, tx_hash);
            match tx.transaction_data {
                crate::model::TransactionData::Blob(ref tx) => {
                    for (bi, blob) in tx.blobs.iter().enumerate() {
                        if let Err(e) = self.inner.lock().await.blobs.put(
                            block.height,
                            ti,
                            &tx_hash,
                            bi,
                            &tx.identity,
                            blob,
                        ) {
                            error!("storing blob of tx {} in block {}: {}", ti, block.height, e);
                        }
                    }
                }
                crate::model::TransactionData::Proof(ref tx) => {
                    if let Err(e) = self.inner.lock().await.proofs.put(
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
                    if let Err(e) = self.inner.lock().await.contracts.put(&tx_hash, tx) {
                        error!(
                            "storing contract {} of tx {} in block {}: {}",
                            tx.contract_name.0, ti, block.height, e
                        );
                    }
                }
            }
            if let Err(e) = self.inner.lock().await.transactions.put(
                block.height,
                ti,
                &tx_hash,
                &tx.transaction_data,
            ) {
                error!(
                    "storing contract of tx {} in block {}: {}",
                    ti, block.height, e
                );
            }
        }
        // store block
        if let Err(e) = self.inner.lock().await.blocks.put(block) {
            error!("storing block: {}", e);
        }
    }
}

impl std::ops::Deref for History {
    type Target = Mutex<HistoryInner>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Debug)]
pub struct HistoryInner {
    pub blocks: Blocks,
    pub blobs: Blobs,
    pub proofs: Proofs,
    pub contracts: Contracts,
    pub transactions: Transactions,
}

impl HistoryInner {
    pub fn new() -> Result<Self> {
        let db = sled::Config::new()
            .use_compression(true)
            .compression_factor(15)
            .path("history.db")
            .open()
            .context("opening the database")?;
        Ok(Self {
            blocks: Blocks::new(&db)?,
            blobs: Blobs::new(&db)?,
            proofs: Proofs::new(&db)?,
            contracts: Contracts::new(&db)?,
            transactions: Transactions::new(&db)?,
        })
    }
}