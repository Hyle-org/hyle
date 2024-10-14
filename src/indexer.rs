//! Index system for historical data.

mod api;
pub mod model;

use crate::{
    bus::{bus_client, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{Block, CommonRunContext, Hashable},
    utils::modules::Module,
};
use anyhow::{bail, Context, Error, Result};
use axum::{routing::get, Router};
use core::str;
use model::{TransactionStatus, TransactionType};
use sqlx::types::chrono::DateTime;
use sqlx::{postgres::PgPoolOptions, PgPool, Pool, Postgres};
use std::sync::Arc;
use tracing::{debug, error, info};

bus_client! {
#[derive(Debug)]
struct IndexerBusClient {
    receiver(ConsensusEvent),
}
}

pub type IndexerState = PgPool;

#[derive(Debug)]
pub struct Indexer {
    bus: IndexerBusClient,
    inner: IndexerState,
}

impl Module for Indexer {
    fn name() -> &'static str {
        "Indexer"
    }

    type Context = Arc<CommonRunContext>;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = IndexerBusClient::new_from_bus(ctx.bus.new_handle()).await;

        let inner = PgPoolOptions::new()
            .max_connections(20)
            .connect(&ctx.config.database_url)
            .await
            .context("Failed to connect to the database")?;

        info!("Checking for new DB migration...");
        sqlx::migrate!("./src/indexer/migrations")
            .run(&inner)
            .await?;

        let indexer = Indexer { bus, inner };

        let mut ctx_router = ctx
            .router
            .lock()
            .expect("Context router should be available");
        let router = ctx_router
            .take()
            .expect("Context router should be available")
            .nest("/v1/indexer", indexer.api());
        let _ = ctx_router.insert(router);

        Ok(indexer)
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl Indexer {
    pub fn share(&self) -> IndexerState {
        self.inner.clone()
    }

    pub async fn start(&mut self) -> Result<()> {
        handle_messages! {
            on_bus self.bus,
            listen<ConsensusEvent> cmd => {
                match self.handle_consensus_event(cmd).await{
                    Ok(_) => (),
                    Err(e) => error!("Error while handling consensus event: {:#}", e),
                }
            }
        }
    }

    pub fn api(&self) -> Router<()> {
        Router::new()
            // block
            .route("/blocks", get(api::get_blocks))
            .route("/block/last", get(api::get_last_block))
            .route("/block/height/:height", get(api::get_block))
            .route("/block/hash/:hash", get(api::get_block_by_hash))
            // transaction
            .route("/transactions", get(api::get_transactions))
            .route(
                "/transactions/block/:height",
                get(api::get_transactions_by_height),
            )
            .route(
                "/transactions/contract/:contract_name",
                get(api::get_transactions_with_contract_name),
            )
            .route(
                "/transaction/hash/:tx_hash",
                get(api::get_transaction_with_hash),
            )
            // blob
            .route(
                "/blobs/settled/contract/:contract_name",
                get(api::get_settled_blobs_by_contract_name),
            )
            .route(
                "/blobs/unsettled/contract/:contract_name",
                get(api::get_unsettled_blobs_by_contract_name),
            )
            .route("/blobs/hash/:tx_hash", get(api::get_blobs_by_tx_hash))
            .route("/blob/hash/:tx_hash/index/:blob_index", get(api::get_blob))
            // contract
            .route("/contract/:contract_name", get(api::get_contract))
            .route(
                "/state/contract/:contract_name/block/:height",
                get(api::get_contract_state_by_height),
            )
            .with_state(self.inner.clone())
    }

    async fn handle_consensus_event(&mut self, event: ConsensusEvent) -> Result<()> {
        match event {
            ConsensusEvent::CommitBlock { block } => self.handle_block(block).await,
        }
    }

    async fn handle_block(&mut self, block: Block) -> Result<(), Error> {
        info!("new block {} with {} txs", block.height, block.txs.len());

        let mut transaction = self.inner.begin().await?;

        let block_hash = &block.hash().inner;
        let block_parent_hash = &block.parent_hash.inner;
        let block_height = i64::try_from(block.height.0)
            .map_err(|_| anyhow::anyhow!("Block height is too large to fit into an i64"))?;

        let block_timestamp = match DateTime::from_timestamp(
            i64::try_from(block.timestamp)
                .map_err(|_| anyhow::anyhow!("Timestamp too large for i64"))?,
            0,
        ) {
            Some(date) => date,
            None => bail!("Block's timestamp is incorrect"),
        };

        // Insert the block into the blocks table
        sqlx::query(
            "INSERT INTO blocks (hash, parent_hash, height, timestamp) VALUES ($1, $2, $3, $4)",
        )
        .bind(block_hash)
        .bind(block_parent_hash)
        .bind(block_height)
        .bind(block_timestamp)
        .execute(&mut *transaction)
        .await?;

        for (tx_index, tx) in block.txs.iter().enumerate() {
            let tx_hash = &tx.hash().0;
            debug!("tx:{:?} hash {:?}", tx_hash, tx);

            let version = i32::try_from(tx.version)
                .map_err(|_| anyhow::anyhow!("Tx version is too large to fit into an i32"))?;
            let tx_index = i32::try_from(tx_index)
                .map_err(|_| anyhow::anyhow!("Tx index is too large to fit into an i64"))?;

            match tx.transaction_data {
                crate::model::TransactionData::Blob(ref tx) => {
                    // Insert the transaction into the transactions table
                    let tx_type = TransactionType::BlobTransaction;
                    let tx_status = TransactionStatus::Sequenced;
                    sqlx::query(
                        "INSERT INTO transactions (tx_hash, block_hash, tx_index, version, transaction_type, transaction_status)
                        VALUES ($1, $2, $3, $4, $5, $6)")
                    .bind(tx_hash)
                    .bind(block_hash)
                    .bind(tx_index)
                    .bind(version)
                    .bind(tx_type)
                    .bind(tx_status)
                    .execute(&mut *transaction)
                    .await?;

                    for (blob_index, blob) in tx.blobs.iter().enumerate() {
                        let blob_index = i32::try_from(blob_index).map_err(|_| {
                            anyhow::anyhow!("Blob index is too large to fit into an i32")
                        })?;
                        let identity = &tx.identity.0;
                        let contract_name = &blob.contract_name.0;
                        let blob = &blob.data.0;
                        sqlx::query(
                            "INSERT INTO blobs (tx_hash, blob_index, identity, contract_name, data)
                             VALUES ($1, $2, $3, $4, $5)",
                        )
                        .bind(tx_hash)
                        .bind(blob_index)
                        .bind(identity)
                        .bind(contract_name)
                        .bind(blob)
                        .execute(&mut *transaction)
                        .await?;
                    }
                }
                crate::model::TransactionData::Proof(ref tx) => {
                    // Insert the transaction into the transactions table
                    let tx_type = TransactionType::ProofTransaction;
                    let tx_status = TransactionStatus::Success;
                    sqlx::query(
                        "INSERT INTO transactions (tx_hash, block_hash, tx_index, version, transaction_type, transaction_status)
                        VALUES ($1, $2, $3, $4, $5, $6)")
                    .bind(tx_hash)
                    .bind(block_hash)
                    .bind(tx_index)
                    .bind(version)
                    .bind(tx_type)
                    .bind(tx_status)
                    .execute(&mut *transaction)
                    .await?;

                    let proof = &tx.proof;

                    sqlx::query("INSERT INTO proofs (tx_hash, proof) VALUES ($1, $2)")
                        .bind(tx_hash)
                        .bind(proof)
                        .execute(&mut *transaction)
                        .await?;

                    // Adding all blob_references
                    for blob_ref in tx.blobs_references.iter() {
                        let contract_name = &blob_ref.contract_name.0;
                        let blob_tx_hash = &blob_ref.blob_tx_hash.0;
                        let blob_index = i32::try_from(blob_ref.blob_index.0).map_err(|_| {
                            anyhow::anyhow!("Blob index is too large to fit into an i32")
                        })?;

                        sqlx::query(                            "INSERT INTO blob_references (tx_hash, contract_name, blob_tx_hash, blob_index)
                             VALUES ($1, $2, $3, $4)")
                        .bind(tx_hash)
                        .bind(contract_name)
                        .bind(blob_tx_hash)
                        .bind(blob_index)
                        .execute(&mut *transaction)
                        .await?;
                    }
                    // TODO: if verification is correct, change the transaction status for associated blod
                    // TODO: if verification is correct, add HyleOutput
                }
                crate::model::TransactionData::RegisterContract(ref tx) => {
                    // Insert the transaction into the transactions table
                    let tx_type = TransactionType::RegisterContractTransaction;
                    let tx_status = TransactionStatus::Success;
                    sqlx::query(
                        "INSERT INTO transactions (tx_hash, block_hash, tx_index, version, transaction_type, transaction_status)
                        VALUES ($1, $2, $3, $4, $5, $6)")
                    .bind(tx_hash)
                    .bind(block_hash)
                    .bind(tx_index)
                    .bind(version)
                    .bind(tx_type)
                    .bind(tx_status)
                    .execute(&mut *transaction)
                    .await?;
                    let owner = &tx.owner;
                    let verifier = &tx.verifier;
                    let program_id = &tx.program_id;
                    let state_digest = &tx.state_digest.0;
                    let contract_name = &tx.contract_name.0;

                    // Adding to Contract table
                    sqlx::query(
                        "INSERT INTO contracts (tx_hash, owner, verifier, program_id, state_digest, contract_name)
                        VALUES ($1, $2, $3, $4, $5, $6)")
                    .bind(tx_hash)
                    .bind(owner)
                    .bind(verifier)
                    .bind(program_id)
                    .bind(state_digest)
                    .bind(contract_name)
                    .execute(&mut *transaction)
                    .await?;

                    // Adding to ContractState table
                    sqlx::query(
                        "INSERT INTO contract_state (contract_name, block_hash, state_digest)
                        VALUES ($1, $2, $3)",
                    )
                    .bind(contract_name)
                    .bind(block_hash)
                    .bind(state_digest)
                    .execute(&mut *transaction)
                    .await?;
                }
                crate::model::TransactionData::Stake(ref _staker) => {
                    tracing::warn!("TODO: add staking to indexer db");
                }
            }
        }

        // Commit the transaction
        transaction.commit().await?;

        Ok(())
    }
}

impl std::ops::Deref for Indexer {
    type Target = Pool<Postgres>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod test {
    use axum_test::TestServer;

    use super::*;

    async fn setup_indexer(pool: PgPool) -> Result<TestServer> {
        let indexer = new_indexer(pool).await;
        let router = indexer.api();
        TestServer::new(router)
    }

    async fn new_indexer(pool: PgPool) -> Indexer {
        Indexer {
            bus: IndexerBusClient::new_from_bus(SharedMessageBus::default()).await,
            inner: pool,
        }
    }

    #[test_log::test(sqlx::test(
        migrations = "./src/indexer/migrations",
        fixtures(path = "../tests/fixtures", scripts("test_data"))
    ))]
    async fn test_indexer_api(pool: PgPool) -> Result<()> {
        let server = setup_indexer(pool).await?;

        // Blocks
        // Get all blocks
        let transactions_response = server.get("/blocks").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get the last block
        let transactions_response = server.get("/block/last").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get block by height
        let transactions_response = server.get("/block/height/1").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get block by hash
        let transactions_response = server
            .get("/block/hash/block1aaaaaaaaaaaaaaaaaaaaaaaaaa")
            .await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Transactions
        // Get all transactions
        let transactions_response = server.get("/transactions").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get all transactions by height
        let transactions_response = server.get("/transactions/block/2").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get an existing transaction by name
        let transactions_response = server.get("/transactions/contract/contract_1").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get an unknown transaction by name
        let transactions_response = server.get("/transactions/contract/unknown_contract").await;
        transactions_response.assert_status_not_found();

        // Get an existing transaction by hash
        let transactions_response = server
            .get("/transaction/hash/test_tx_hash_1aaaaaaaaaaaaaaaaaa")
            .await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get an unknown transaction by hash
        let unknown_tx = server.get("/transaction/hash/unknown").await;
        unknown_tx.assert_status_not_found();

        // Blobs
        // Get all settled blobs by contract name
        let transactions_response = server.get("/blobs/settled/contract/contract_1").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get all unsettled blobs by contract name
        let transactions_response = server.get("/blobs/unsettled/contract/contract_1").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get blobs by tx_hash
        let transactions_response = server
            .get("/blobs/hash/test_tx_hash_2aaaaaaaaaaaaaaaaaa")
            .await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get unknown blobs by tx_hash
        let transactions_response = server
            .get("/blobs/hash/test_tx_hash_1aaaaaaaaaaaaaaaaaa")
            .await;
        transactions_response.assert_status_not_found();

        // Get blob by tx_hash and index
        let transactions_response = server
            .get("/blob/hash/test_tx_hash_2aaaaaaaaaaaaaaaaaa/index/0")
            .await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get blob by tx_hash and unknown index
        let transactions_response = server
            .get("/blob/hash/test_tx_hash_2aaaaaaaaaaaaaaaaaa/index/1000")
            .await;
        transactions_response.assert_status_not_found();

        // Contracts
        // Get contract by name
        let transactions_response = server.get("/contract/contract_1").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get contract state by name and height
        let transactions_response = server.get("/state/contract/contract_1/block/1").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        Ok(())
    }
}
