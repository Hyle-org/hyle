//! Index system for historical data.

mod api;
pub mod model;

use crate::{
    bus::{bus_client, SharedMessageBus},
    data_availability::DataEvent,
    handle_messages,
    model::{Block, CommonRunContext, ContractName, Hashable},
    utils::modules::Module,
};
use anyhow::{bail, Context, Error, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use core::str;
use futures::{SinkExt, StreamExt};
use model::{TransactionStatus, TransactionType, TransactionWithBlobs, TxHashDb};
use sqlx::types::chrono::DateTime;
use sqlx::{postgres::PgPoolOptions, PgPool, Pool, Postgres};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    net::TcpStream,
    sync::{broadcast, mpsc},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, info};

bus_client! {
#[derive(Debug)]
struct IndexerBusClient {
    receiver(DataEvent),
}
}

// TODO: generalize for all tx types
type Subscribers = HashMap<ContractName, Vec<broadcast::Sender<TransactionWithBlobs>>>;

#[derive(Debug, Clone)]
pub struct IndexerApiState {
    db: PgPool,
    new_sub_sender: mpsc::Sender<(ContractName, WebSocket)>,
}

#[derive(Debug)]
pub struct Indexer {
    bus: IndexerBusClient,
    da_stream: Option<Framed<TcpStream, LengthDelimitedCodec>>,
    state: IndexerApiState,
    new_sub_receiver: tokio::sync::mpsc::Receiver<(ContractName, WebSocket)>,
    subscribers: Subscribers,
}

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./src/indexer/migrations");

impl Module for Indexer {
    fn name() -> &'static str {
        "Indexer"
    }

    type Context = Arc<CommonRunContext>;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = IndexerBusClient::new_from_bus(ctx.bus.new_handle()).await;

        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect(&ctx.config.database_url)
            .await
            .context("Failed to connect to the database")?;

        info!("Checking for new DB migration...");
        MIGRATOR.run(&pool).await?;

        let (new_sub_sender, new_sub_receiver) = tokio::sync::mpsc::channel(100);

        let subscribers = HashMap::new();

        let indexer = Indexer {
            bus,
            state: IndexerApiState {
                db: pool,
                new_sub_sender,
            },
            da_stream: None,
            new_sub_receiver,
            subscribers,
        };

        if let Ok(mut guard) = ctx.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/indexer", indexer.api()));
                return Ok(indexer);
            }
        }
        anyhow::bail!("context router should be available");
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl Indexer {
    pub async fn start(&mut self) -> Result<()> {
        if self.da_stream.is_none() {
            handle_messages! {
                on_bus self.bus,
                listen<DataEvent> cmd => {
                    if let Err(e) = self.handle_data_availability_event(cmd).await {
                        error!("Error while handling data availability event: {:#}", e)
                    }
                }

                Some((contract_name, mut socket)) = self.new_sub_receiver.recv() => {

                    let (tx, mut rx) = broadcast::channel(100);
                    // Append tx to the list of subscribers for contract_name
                    self.subscribers.entry(contract_name)
                        .or_default()
                        .push(tx);

                    tokio::spawn(async move {
                        while let Ok(transaction) = rx.recv().await {
                            if let Ok(json) = serde_json::to_string(&transaction) {
                                if socket.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            } else {
                                error!("Failed to serialize transaction to JSON");
                            }
                        }
                    });
                }

            }
        } else {
            handle_messages! {
                on_bus self.bus,
                cmd = self.da_stream.as_mut().expect("da_stream must exist").next() => {
                    if let Some(Ok(cmd)) = cmd {
                        let bytes = cmd;
                        let block: Block = bincode::decode_from_slice(&bytes, bincode::config::standard())?.0;
                        if let Err(e) = self.handle_block(block).await {
                            error!("Error while handling block: {:#}", e);
                        }
                        SinkExt::<bytes::Bytes>::send(self.da_stream.as_mut().expect("da_stream must exist"), "ok".into()).await?;
                    } else if cmd.is_none() {
                        self.da_stream = None;
                        // TODO: retry
                        return Err(anyhow::anyhow!("DA stream closed"));
                    } else if let Some(Err(e)) = cmd {
                        self.da_stream = None;
                        // TODO: retry
                        return Err(anyhow::anyhow!("Error while reading DA stream: {}", e));
                    }
                }

                Some((contract_name, mut socket)) = self.new_sub_receiver.recv() => {

                    let (tx, mut rx) = broadcast::channel(100);
                    // Append tx to the list of subscribers for contract_name
                    self.subscribers.entry(contract_name)
                        .or_default()
                        .push(tx);

                    tokio::spawn(async move {
                        while let Ok(transaction) = rx.recv().await {
                            if let Ok(json) = serde_json::to_string(&transaction) {
                                if socket.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            } else {
                                error!("Failed to serialize transaction to JSON");
                            }
                        }
                    });
                }
            }
        }
    }

    pub async fn connect_to(&mut self, target: &str) -> Result<()> {
        info!(
            "Connecting to node for data availability stream on {}",
            &target
        );
        let stream = TcpStream::connect(&target).await?;
        let addr = stream.local_addr()?;
        self.da_stream = Some(Framed::new(stream, LengthDelimitedCodec::new()));
        info!("Connected to data stream to {} on {}", &target, addr);
        // Send the start height
        let height_as_bytes: bytes::BytesMut = u64::to_be_bytes(0)[..].into();
        self.da_stream
            .as_mut()
            .expect("da_stream must exist")
            .send(height_as_bytes.into())
            .await?;
        Ok(())
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
                get(api::get_transactions_by_contract),
            )
            .route(
                "/transaction/hash/:tx_hash",
                get(api::get_transaction_with_hash),
            )
            .route(
                "/blob_transactions/contract/:contract_name",
                get(api::get_blob_transactions_by_contract),
            )
            .route(
                "/blob_transactions/contract/:contract_name/ws",
                get(Self::get_blob_transactions_by_contract_ws_handler),
            )
            // blob
            .route(
                "/blobs/contract/:contract_name",
                get(api::get_blobs_by_contract),
            )
            .route(
                "/blobs/settled/contract/:contract_name",
                get(api::get_settled_blobs_by_contract),
            )
            .route(
                "/blobs/unsettled/contract/:contract_name",
                get(api::get_unsettled_blobs_by_contract),
            )
            .route("/blobs/hash/:tx_hash", get(api::get_blobs_by_tx_hash))
            .route("/blob/hash/:tx_hash/index/:blob_index", get(api::get_blob))
            // contract
            .route("/contract/:contract_name", get(api::get_contract))
            .route(
                "/state/contract/:contract_name/block/:height",
                get(api::get_contract_state_by_height),
            )
            .with_state(self.state.clone())
    }

    async fn get_blob_transactions_by_contract_ws_handler(
        ws: WebSocketUpgrade,
        Path(contract_name): Path<String>,
        State(state): State<IndexerApiState>,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| {
            Self::get_blob_transactions_by_contract_ws(socket, contract_name, state.new_sub_sender)
        })
    }

    async fn get_blob_transactions_by_contract_ws(
        socket: WebSocket,
        contract_name: String,
        new_sub_sender: mpsc::Sender<(ContractName, WebSocket)>,
    ) {
        // TODO: properly handle errors and ws messages
        _ = new_sub_sender
            .send((ContractName(contract_name), socket))
            .await;
    }

    async fn handle_data_availability_event(&mut self, event: DataEvent) -> Result<()> {
        match event {
            DataEvent::NewBlock(block) => self.handle_block(block).await,
        }
    }

    async fn handle_block(&mut self, block: Block) -> Result<(), Error> {
        info!("new block {} with {} txs", block.height, block.txs.len());

        let mut transaction = self.state.db.begin().await?;

        let block_hash = &block.hash();
        let block_parent_hash = &block.parent_hash;
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
            let tx_hash: &TxHashDb = &tx.hash().into();
            debug!("tx hash {:?}", tx_hash);

            let version = i32::try_from(tx.version)
                .map_err(|_| anyhow::anyhow!("Tx version is too large to fit into an i32"))?;
            let tx_index = i32::try_from(tx_index)
                .map_err(|_| anyhow::anyhow!("Tx index is too large to fit into an i64"))?;

            match tx.transaction_data {
                crate::model::TransactionData::Blob(ref tx) => {
                    // Send the transaction to all websocket subscribers
                    for (contrat_name, senders) in self.subscribers.iter() {
                        if tx
                            .blobs
                            .iter()
                            .any(|blob| &blob.contract_name == contrat_name)
                        {
                            let enriched_tx = TransactionWithBlobs {
                                tx_hash: tx_hash.clone(),
                                block_hash: block_hash.clone(),
                                tx_index,
                                version,
                                transaction_type: TransactionType::BlobTransaction,
                                transaction_status: TransactionStatus::Sequenced,
                                identity: tx.identity.0.clone(),
                                blobs: tx.blobs.clone(),
                            };
                            senders.iter().for_each(|sender| {
                                let _ = sender.send(enriched_tx.clone());
                            });
                        }
                    }
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

                    let proof = &tx.proof.to_bytes()?;

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
        &self.state.db
    }
}

#[cfg(test)]
mod test {
    use axum_test::TestServer;
    use std::{
        future::IntoFuture,
        net::{Ipv4Addr, SocketAddr},
    };

    use super::*;

    use sqlx::postgres::PgPoolOptions;
    use testcontainers_modules::{postgres::Postgres, testcontainers::runners::AsyncRunner};

    async fn setup_test_server(indexer: &Indexer) -> Result<TestServer> {
        let router = indexer.api();
        TestServer::new(router)
    }

    async fn new_indexer(pool: PgPool) -> Indexer {
        let (new_sub_sender, new_sub_receiver) = tokio::sync::mpsc::channel(100);

        Indexer {
            bus: IndexerBusClient::new_from_bus(SharedMessageBus::default()).await,
            state: IndexerApiState {
                db: pool,
                new_sub_sender,
            },
            da_stream: None,
            new_sub_receiver,
            subscribers: HashMap::new(),
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_indexer_api() -> Result<()> {
        let container = Postgres::default().start().await.unwrap();
        let db = PgPoolOptions::new()
            .max_connections(5)
            .connect(&format!(
                "postgresql://postgres:postgres@localhost:{}/postgres",
                container.get_host_port_ipv4(5432).await.unwrap()
            ))
            .await
            .unwrap();
        MIGRATOR.run(&db).await.unwrap();
        sqlx::raw_sql(include_str!("../tests/fixtures/test_data.sql"))
            .execute(&db)
            .await?;

        let mut indexer = new_indexer(db).await;
        let server = setup_test_server(&indexer).await?;

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
            .get("/block/hash/block1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
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
            .get("/transaction/hash/test_tx_hash_1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get an unknown transaction by hash
        let unknown_tx = server.get("/transaction/hash/1111111111111111111111111111111111111111111111111111111111111111").await;
        unknown_tx.assert_status_not_found();

        // Blobs
        // Get all transactions for a specific contract name
        let transactions_response = server.get("/blob_transactions/contract/contract_1").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get all blobs for a specific contract name
        let transactions_response = server.get("/blobs/contract/contract_1").await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

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
            .get("/blobs/hash/test_tx_hash_2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get unknown blobs by tx_hash
        let transactions_response = server
            .get("/blobs/hash/test_tx_hash_1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .await;
        transactions_response.assert_status_not_found();

        // Get blob by tx_hash and index
        let transactions_response = server
            .get("/blob/hash/test_tx_hash_2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/index/0")
            .await;
        transactions_response.assert_status_ok();
        assert!(!transactions_response.text().is_empty());

        // Get blob by tx_hash and unknown index
        let transactions_response = server
            .get("/blob/hash/test_tx_hash_2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/index/1000")
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

        // Websocket
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(axum::serve(listener, indexer.api()).into_future());

        let _ = tokio_tungstenite::connect_async(format!(
            "ws://{addr}/blob_transactions/contract/contract_1/ws"
        ))
        .await
        .unwrap();

        if let Some(tx) = indexer.new_sub_receiver.recv().await {
            let (contract_name, _) = tx;
            assert_eq!(contract_name, ContractName("contract_1".to_string()));
        }

        Ok(())
    }
}
