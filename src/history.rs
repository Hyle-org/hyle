//! Lightweight archival system for past states. Optional.

mod api;
mod blobs;
mod blocks;
mod contracts;
mod db;
pub mod model;
mod proofs;
mod transactions;

use crate::{
    bus::{bus_client, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{Block, Hashable, SharedRunContext},
    rest,
    utils::modules::Module,
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
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use transactions::Transactions;

pub fn u64_to_str(u: u64, buf: &mut [u8]) -> &str {
    let mut cursor = Cursor::new(&mut buf[..]);
    _ = write!(cursor, "{}", u);
    let len = cursor.position() as usize;
    str::from_utf8(&buf[..len]).unwrap()
}

bus_client! {
#[derive(Debug)]
struct HistoryBusClient {
    receiver(ConsensusEvent),
}
}

pub type HistoryState = Arc<RwLock<HistoryInner>>;

#[derive(Debug)]
pub struct History {
    bus: HistoryBusClient,
    inner: HistoryState,
}

impl Module for History {
    fn name() -> &'static str {
        "History"
    }

    type Context = SharedRunContext;

    async fn build(ctx: &Self::Context) -> Result<Self> {
        let bus = HistoryBusClient::new_from_bus(ctx.bus.new_handle()).await;

        let db = sled::Config::new()
            .use_compression(true)
            .compression_factor(15)
            .path(ctx.config.history_db_path())
            .open()
            .context("opening the database")?;

        let inner = HistoryInner::new(db)?;

        Ok(History {
            bus,
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    fn run(&mut self, _ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl History {
    pub fn share(&self) -> HistoryState {
        self.inner.clone()
    }

    pub async fn start(&mut self) -> Result<()> {
        handle_messages! {
            on_bus self.bus,
            listen<ConsensusEvent> cmd => {
                self.handle_consensus_event(cmd).await;
            }
        }
    }

    pub fn api() -> Router<rest::RouterState> {
        Router::new()
            // block
            .route("/blocks", get(api::get_blocks))
            .route("/block/last", get(api::get_last_block))
            .route("/block/:height", get(api::get_block))
            // transaction
            .route("/transactions", get(api::get_transactions))
            .route("/transaction/last", get(api::get_last_transaction))
            .route("/transaction/:height/:tx_index", get(api::get_transaction))
            .route("/transaction/:tx_hash", get(api::get_transaction_with_hash))
            // blob
            .route("/blobs", get(api::get_blobs))
            .route("/blobs/last", get(api::get_last_blob))
            .route(
                "/blob/:block_height/:tx_index/:blob_index",
                get(api::get_blob),
            )
            .route("/blobs/:tx_hash/:blob_index", get(api::get_blob_with_hash))
            // proof
            .route("/proofs", get(api::get_proofs))
            .route("/proof/last", get(api::get_last_proof))
            .route("/proof/:block_height/:tx_index", get(api::get_proof))
            .route("/proof/:tx_hash", get(api::get_proof_with_hash))
            // contract
            .route("/contracts", get(api::get_contracts))
            .route("/contract/:name", get(api::get_contract))
    }

    async fn handle_consensus_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitBlock { block, .. } => self.handle_block(block).await,
        }
    }

    async fn handle_block(&mut self, block: Block) {
        info!("new block {} with {} txs", block.height, block.txs.len());
        for (ti, tx) in block.txs.iter().enumerate() {
            let tx_hash = format!("{}", tx.hash());
            debug!("tx:{} hash {}", ti, tx_hash);
            match tx.transaction_data {
                crate::model::TransactionData::Blob(ref tx) => {
                    for (bi, blob) in tx.blobs.iter().enumerate() {
                        if let Err(e) = self.inner.write().await.blobs.put(
                            block.height,
                            ti,
                            bi,
                            &tx_hash,
                            &tx.identity,
                            blob,
                        ) {
                            error!("storing blob of tx {} in block {}: {}", ti, block.height, e);
                        }
                    }
                }
                crate::model::TransactionData::Proof(ref tx) => {
                    if let Err(e) =
                        self.inner
                            .write()
                            .await
                            .proofs
                            .put(block.height, ti, &tx_hash, tx)
                    {
                        error!(
                            "storing proof of tx {} in block {}: {}",
                            ti, block.height, e
                        );
                    }
                }
                crate::model::TransactionData::RegisterContract(ref tx) => {
                    if let Err(e) =
                        self.inner
                            .write()
                            .await
                            .contracts
                            .put(block.height, ti, &tx_hash, tx)
                    {
                        error!(
                            "storing contract {} of tx {} in block {}: {}",
                            tx.contract_name.0, ti, block.height, e
                        );
                    }
                }
                crate::model::TransactionData::Stake(_) => {
                    warn!("Temporary transaction type 'Stake' not stored in history.")
                }
            }
            if let Err(e) = self.inner.write().await.transactions.put(
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
        if let Err(e) = self.inner.write().await.blocks.put(block) {
            error!("storing block: {}", e);
        }
    }
}

impl std::ops::Deref for History {
    type Target = RwLock<HistoryInner>;

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
    pub fn new(db: sled::Db) -> Result<Self> {
        Ok(Self {
            blocks: Blocks::new(&db)?,
            blobs: Blobs::new(&db)?,
            proofs: Proofs::new(&db)?,
            contracts: Contracts::new(&db)?,
            transactions: Transactions::new(&db)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        history::{blobs::Blobs, contracts::Contracts, proofs::Proofs, transactions::Transactions},
        model::{
            Blob, BlobData, BlobIndex, BlobReference, BlobTransaction, Block, BlockHash,
            BlockHeight, ContractName, Identity, ProofTransaction, RegisterContractTransaction,
            StateDigest, Transaction, TransactionData, TxHash,
        },
    };

    use super::blocks::Blocks;
    use anyhow::Result;

    #[test]
    fn test_blocks() -> Result<()> {
        let tmpdir = tempdir::TempDir::new("history-tests")?;
        let db = sled::open(tmpdir.path().join("history"))?;
        let mut blocks = Blocks::new(&db)?;
        assert!(
            blocks.len() == 1,
            "blocks should contain genesis block after creation"
        );
        let block = Block {
            parent_hash: BlockHash {
                inner: vec![0, 1, 2, 3],
            },
            height: BlockHeight(1),
            timestamp: 42,
            txs: vec![Transaction {
                version: 1,
                transaction_data: TransactionData::Blob(BlobTransaction {
                    identity: Identity("tx_id".to_string()),
                    blobs: vec![Blob {
                        contract_name: ContractName("c1".to_string()),
                        data: BlobData(vec![4, 5, 6]),
                    }],
                }),
                inner: "tx".to_string(),
            }],
        };
        blocks.put(block.clone())?;
        assert!(blocks.last().height == block.height);
        let last = blocks.get(BlockHeight(1))?;
        assert!(last.is_some());
        assert!(last.unwrap().height == BlockHeight(1));
        Ok(())
    }

    #[test]
    fn test_transactions() -> Result<()> {
        let tmpdir = tempdir::TempDir::new("history-tests")?;
        let db = sled::open(tmpdir.path().join("history"))?;
        let mut transactions = Transactions::new(&db)?;
        assert!(transactions.len() == 0);
        let transaction = TransactionData::Blob(BlobTransaction {
            identity: Identity("tx_id".to_string()),
            blobs: vec![Blob {
                contract_name: ContractName("c1".to_string()),
                data: BlobData(vec![4, 5, 6]),
            }],
        });
        let tx_hash = "hash123".to_string();
        transactions.put(BlockHeight(2), 3, &tx_hash, &transaction)?;
        let last = transactions
            .get(BlockHeight(2), 3)?
            .expect("last transaction");
        assert!(last.block_height == BlockHeight(2));
        assert!(last.tx_index == 3);

        let unknown = transactions.get(BlockHeight(8), 42)?;
        assert!(unknown.is_none());

        let last = transactions.last()?.expect("last transaction");
        assert!(last.block_height == BlockHeight(2));
        assert!(last.tx_index == 3);

        let last = transactions
            .get_with_hash(&tx_hash)?
            .expect("transaction with hash");
        assert!(last.block_height == BlockHeight(2));
        assert_eq!(last.tx_hash, tx_hash);
        Ok(())
    }

    #[test]
    fn test_blobs() -> Result<()> {
        let tmpdir = tempdir::TempDir::new("history-tests")?;
        let db = sled::open(tmpdir.path().join("history"))?;
        let mut blobs = Blobs::new(&db)?;
        assert!(blobs.len() == 0);
        let blob = Blob {
            contract_name: ContractName("c1".to_string()),
            data: BlobData(vec![4, 5, 6]),
        };
        let tx_identity = Identity("tx_id".to_string());
        let tx_hash = "hash123".to_string();
        blobs.put(BlockHeight(2), 3, 4, &tx_hash, &tx_identity, &blob)?;

        let last = blobs.get(BlockHeight(2), 3, 4)?.expect("last blob");
        assert!(last.block_height == BlockHeight(2));
        assert!(last.tx_index == 3);

        let last = blobs.last()?.expect("last blob");
        assert!(last.block_height == BlockHeight(2));
        assert!(last.tx_index == 3);
        assert!(last.blob_index == 4);

        let unknown = blobs.get(BlockHeight(8), 42, 6)?;
        assert!(unknown.is_none());

        let last = blobs.get_with_hash(&tx_hash, 4)?.expect("blob with hash");
        assert!(last.block_height == BlockHeight(2));
        assert_eq!(last.tx_hash, tx_hash);
        Ok(())
    }

    #[test]
    fn test_contracts() -> Result<()> {
        let tmpdir = tempdir::TempDir::new("history-tests")?;
        let db = sled::open(tmpdir.path().join("history"))?;
        let mut contracts = Contracts::new(&db)?;
        assert!(contracts.len() == 0);
        let contract_name = "c1".to_string();
        let tx_hash = "hash123".to_string();
        let contract = RegisterContractTransaction {
            contract_name: ContractName(contract_name.clone()),
            owner: "owner".to_string(),
            program_id: vec![7, 8, 9],
            verifier: "verifier".to_string(),
            state_digest: StateDigest(vec![1, 3, 5]),
        };

        contracts.put(BlockHeight(2), 3, &tx_hash, &contract)?;

        let contract = contracts.get(&contract_name).expect("contract with name");
        assert!(contract.is_some());
        let contract = contract.unwrap();
        assert_eq!(contract.block_height, BlockHeight(2));
        assert_eq!(contract.tx_index, 3);

        let contract_name = "c2".to_string();
        let contract = RegisterContractTransaction {
            contract_name: ContractName(contract_name.clone()),
            owner: "owner".to_string(),
            program_id: vec![2, 4, 6],
            verifier: "verifier".to_string(),
            state_digest: StateDigest(vec![5, 7, 8]),
        };
        contracts.put(BlockHeight(4), 6, &tx_hash, &contract)?;

        let unknown = contracts.get("c42")?;
        assert!(unknown.is_none());

        let mut found = 0;
        for (i, item) in contracts.all().enumerate() {
            let elem = item?;
            match i {
                0 => {
                    assert_eq!(elem.key(), Some("c1"));
                    let c = elem.value()?;
                    assert_eq!(c.block_height, BlockHeight(2));
                    assert_eq!(c.tx_index, 3);
                    found += 1;
                }
                1 => {
                    assert_eq!(elem.key(), Some("c2"));
                    let c = elem.value()?;
                    assert_eq!(c.block_height, BlockHeight(4));
                    assert_eq!(c.tx_index, 6);
                    found += 1;
                }
                _ => unreachable!(),
            }
        }
        assert!(found == 2);

        Ok(())
    }

    #[test]
    fn test_proofs() -> Result<()> {
        let tmpdir = tempdir::TempDir::new("history-tests")?;
        let db = sled::open(tmpdir.path().join("history"))?;
        let mut proofs = Proofs::new(&db)?;
        assert!(proofs.len() == 0);
        let contract_name = "c1".to_string();
        let proof = ProofTransaction {
            proof: vec![2, 4, 6],
            blobs_references: vec![BlobReference {
                contract_name: ContractName(contract_name.clone()),
                blob_tx_hash: TxHash(vec![1, 7, 9]),
                blob_index: BlobIndex(1),
            }],
        };
        let tx_hash = "hash123".to_string();
        proofs.put(BlockHeight(2), 3, &tx_hash, &proof)?;

        let last = proofs.get(BlockHeight(2), 3)?.expect("last proof");
        assert!(last.block_height == BlockHeight(2));
        assert!(last.tx_index == 3);

        let last = proofs.last()?.expect("last proof");
        assert!(last.block_height == BlockHeight(2));
        assert!(last.tx_index == 3);

        let unknown = proofs.get(BlockHeight(8), 42)?;
        assert!(unknown.is_none());

        let last = proofs.get_with_hash(&tx_hash)?.expect("proof with hash");
        assert!(last.block_height == BlockHeight(2));
        assert_eq!(last.tx_hash, tx_hash);
        Ok(())
    }
}
