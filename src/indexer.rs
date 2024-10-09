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
use anyhow::{Context, Result};
use axum::{routing::get, Router};
use core::str;
use sqlx::{postgres::PgPoolOptions, PgPool, Pool, Postgres};
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use tracing::{debug, info};

pub fn u64_to_str(u: u64, buf: &mut [u8]) -> &str {
    let mut cursor = Cursor::new(&mut buf[..]);
    _ = write!(cursor, "{}", u);
    let len = cursor.position() as usize;
    str::from_utf8(&buf[..len]).unwrap()
}

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

        // FIXME: is it the right place to run migration ?
        debug!("Checking for new DB migration...");
        sqlx::migrate!().run(&inner).await?;

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
                self.handle_consensus_event(cmd).await;
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
            // proof
            // .route("/proof/last", get(api::get_last_proof))
            // .route("/proof/:block_height/:tx_index", get(api::get_proof))
            // .route("/proof/:tx_hash", get(api::get_proof_with_hash))
            // contract
            .route("/contract/:contract_name", get(api::get_contract))
            .route(
                "/state/contract/:contract_name/block/:height",
                get(api::get_contract_state_by_height),
            )
            .with_state(self.inner.clone())
    }

    async fn handle_consensus_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitBlock { block, .. } => self.handle_block(block).await,
        }
    }

    async fn handle_block(&mut self, block: Block) {
        info!("new block {} with {} txs", block.height, block.txs.len());
        for tx in block.txs.iter() {
            let tx_hash = format!("{}", tx.hash());
            debug!("tx:{:?} hash {}", tx, tx_hash);
            // match tx.transaction_data {
            //     crate::model::TransactionData::Blob(ref tx) => {
            //         for (bi, blob) in tx.blobs.iter().enumerate() {
            //             if let Err(e) = self.inner.write().await.blobs.put(
            //                 block.height,
            //                 ti,
            //                 bi,
            //                 &tx_hash,
            //                 &tx.identity,
            //                 blob,
            //             ) {
            //                 error!("storing blob of tx {} in block {}: {}", ti, block.height, e);
            //             }
            //         }
            //     }
            //     crate::model::TransactionData::Proof(ref tx) => {
            //         if let Err(e) =
            //             self.inner
            //                 .write()
            //                 .await
            //                 .proofs
            //                 .put(block.height, ti, &tx_hash, tx)
            //         {
            //             error!(
            //                 "storing proof of tx {} in block {}: {}",
            //                 ti, block.height, e
            //             );
            //         }
            //     }
            //     crate::model::TransactionData::RegisterContract(ref tx) => {
            //         if let Err(e) =
            //             self.inner
            //                 .write()
            //                 .await
            //                 .contracts
            //                 .put(block.height, ti, &tx_hash, tx)
            //         {
            //             error!(
            //                 "storing contract {} of tx {} in block {}: {}",
            //                 tx.contract_name.0, ti, block.height, e
            //             );
            //         }
            //     }
            // }
            // if let Err(e) = self.inner.write().await.transactions.put(
            //     block.height,
            //     ti,
            //     &tx_hash,
            //     &tx.transaction_data,
            // ) {
            //     error!(
            //         "storing contract of tx {} in block {}: {}",
            //         ti, block.height, e
            //     );
            // }
        }
        // store block
        // if let Err(e) = self.inner.write().await.blocks.put(block) {
        //     error!("storing block: {}", e);
        // }
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
        migrations = "./migrations",
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
        let transactions_response = server.get("/block/hash/block1").await;
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
        let transactions_response = server.get("/transaction/hash/test_tx_hash_1").await;
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
        let transactions_response = server.get("/blobs/hash/test_tx_hash_2").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get unknown blobs by tx_hash
        let transactions_response = server.get("/blobs/hash/test_tx_hash_1").await;
        transactions_response.assert_status_not_found();

        // Get blob by tx_hash and index
        let transactions_response = server.get("/blob/hash/test_tx_hash_2/index/0").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get blob by tx_hash and unknown index
        let transactions_response = server.get("/blob/hash/test_tx_hash_2/index/1000").await;
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
