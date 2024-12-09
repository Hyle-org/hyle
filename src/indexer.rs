//! Index system for historical data.

mod api;
pub mod contract_handlers;
pub mod contract_state_indexer;
pub mod da_listener;
pub mod model;

use crate::{
    bus::BusClientSender,
    data_availability::DataEvent,
    handle_messages,
    model::{
        BlobTransaction, Block, BlockHash, BlockHeight, CommonRunContext, ContractName, Hashable,
    },
    module_handle_messages,
    node_state::NodeState,
    utils::modules::{module_bus_client, Module},
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
use model::{BlobWithStatus, TransactionStatus, TransactionType, TransactionWithBlobs, TxHashDb};
use sqlx::types::chrono::DateTime;
use sqlx::Row;
use sqlx::{postgres::PgPoolOptions, PgPool, Pool, Postgres};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info};

module_bus_client! {
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
    state: IndexerApiState,
    new_sub_receiver: tokio::sync::mpsc::Receiver<(ContractName, WebSocket)>,
    subscribers: Subscribers,
    node_state: NodeState,
}

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./src/indexer/migrations");

impl Module for Indexer {
    type Context = Arc<CommonRunContext>;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = IndexerBusClient::new_from_bus(ctx.bus.new_handle()).await;

        let pool = PgPoolOptions::new()
            .max_connections(20)
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect(&ctx.config.database_url)
            .await
            .context("Failed to connect to the database")?;

        let node_state = Self::load_from_disk_or_default::<NodeState>(
            ctx.config
                .data_directory
                .join("indexer_node_state.bin")
                .as_path(),
        );

        info!("Checking for new DB migration...");
        let _ =
            tokio::time::timeout(tokio::time::Duration::from_secs(60), MIGRATOR.run(&pool)).await?;

        let (new_sub_sender, new_sub_receiver) = tokio::sync::mpsc::channel(100);

        let subscribers = HashMap::new();

        let indexer = Indexer {
            bus,
            state: IndexerApiState {
                db: pool,
                new_sub_sender,
            },
            new_sub_receiver,
            subscribers,
            node_state,
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
        module_handle_messages! {
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

                tokio::task::Builder::new()
                    .name("indexer-recv")
                    .spawn(async move {
                        while let Ok(transaction) = rx.recv().await {
                            if let Ok(json) = serde_json::to_string(&transaction) {
                                if socket.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            } else {
                                error!("Failed to serialize transaction to JSON");
                            }
                        }
                    })?;
            }
        }
        Ok(())
    }

    pub async fn get_last_block(&self) -> Result<Option<BlockHeight>> {
        let rows = sqlx::query("SELECT max(height) as max FROM blocks")
            .fetch_one(&self.state.db)
            .await?;
        Ok(rows
            .try_get("max")
            .map(|m: i64| Some(BlockHeight(m as u64)))
            .unwrap_or(None))
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
            .route("/blobs/hash/:tx_hash", get(api::get_blobs_by_tx_hash))
            .route("/blob/hash/:tx_hash/index/:blob_index", get(api::get_blob))
            // contract
            .route("/contracts", get(api::list_contracts))
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

    async fn handle_data_availability_event(&mut self, event: DataEvent) -> Result<(), Error> {
        match event {
            DataEvent::NewBlock(block) => self.handle_block(block).await,
            DataEvent::CatchupDone(_) => Ok(()),
        }
    }

    async fn handle_block(&mut self, block: Block) -> Result<(), Error> {
        info!("new block {} with {} txs", block.height, block.txs.len());

        let mut transaction = self.state.db.begin().await?;

        // Insert the block into the blocks table
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

        sqlx::query(
            "INSERT INTO blocks (hash, parent_hash, height, timestamp) VALUES ($1, $2, $3, $4)",
        )
        .bind(block_hash)
        .bind(block_parent_hash)
        .bind(block_height)
        .bind(block_timestamp)
        .execute(&mut *transaction)
        .await?;

        let handled_block_output = self.node_state.handle_new_block(block);

        // Handling Register Contract Transactions
        for tx in handled_block_output.new_contract_txs {
            let tx_hash: &TxHashDb = &tx.hash().into();
            let version = i32::try_from(tx.version)
                .map_err(|_| anyhow::anyhow!("Tx version is too large to fit into an i32"))?;

            let register_contract_tx = tx.transaction_data.register_contract()?;

            // Insert the transaction into the transactions table
            let tx_type = TransactionType::RegisterContractTransaction;
            let tx_status = TransactionStatus::Success;
            sqlx::query(
                "INSERT INTO transactions (tx_hash, block_hash, version, transaction_type, transaction_status)
                VALUES ($1, $2, $3, $4, $5)")
            .bind(tx_hash)
            .bind(block_hash)
            .bind(version)
            .bind(tx_type)
            .bind(tx_status)
            .execute(&mut *transaction)
            .await?;
            let owner = &register_contract_tx.owner;
            let verifier = &register_contract_tx.verifier;
            let program_id = &register_contract_tx.program_id;
            let state_digest = &register_contract_tx.state_digest.0;
            let contract_name = &register_contract_tx.contract_name.0;

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

        // Handling Blob Transactions
        for tx in handled_block_output.new_blob_txs {
            let tx_hash: &TxHashDb = &tx.hash().into();
            let version = i32::try_from(tx.version)
                .map_err(|_| anyhow::anyhow!("Tx version is too large to fit into an i32"))?;
            let blob_tx = tx.transaction_data.blob()?;

            // Send the transaction to all websocket subscribers
            self.send_blob_transaction_to_websocket_subscribers(
                &blob_tx, tx_hash, block_hash, &version,
            );

            // Insert the transaction into the transactions table
            let tx_type = TransactionType::BlobTransaction;
            let tx_status = TransactionStatus::Sequenced;
            sqlx::query(
                "INSERT INTO transactions (tx_hash, block_hash, version, transaction_type, transaction_status)
                VALUES ($1, $2, $3, $4, $5)")
            .bind(tx_hash)
            .bind(block_hash)
            .bind(version)
            .bind(tx_type)
            .bind(tx_status)
            .execute(&mut *transaction)
            .await?;

            for (blob_index, blob) in blob_tx.blobs.iter().enumerate() {
                let blob_index = i32::try_from(blob_index)
                    .map_err(|_| anyhow::anyhow!("Blob index is too large to fit into an i32"))?;
                let identity = &blob_tx.identity.0;
                let contract_name = &blob.contract_name.0;
                let blob = &blob.data.0;
                sqlx::query(
                    "INSERT INTO blobs (tx_hash, blob_index, identity, contract_name, data, verified)
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(tx_hash)
                .bind(blob_index)
                .bind(identity)
                .bind(contract_name)
                .bind(blob)
                .bind(false)
                .execute(&mut *transaction)
                .await?;
            }
        }

        // Handling Proof Transactions
        for tx in handled_block_output.new_verified_proof_txs {
            let tx_hash: &TxHashDb = &tx.hash().into();
            let version = i32::try_from(tx.version)
                .map_err(|_| anyhow::anyhow!("Tx version is too large to fit into an i32"))?;

            let verified_proof_tx = tx.transaction_data.verified_proof()?;

            // Insert the transaction into the transactions table
            let tx_type = TransactionType::ProofTransaction;
            let tx_status = TransactionStatus::Success;
            sqlx::query(
                "INSERT INTO transactions (tx_hash, block_hash, version, transaction_type, transaction_status)
                VALUES ($1, $2, $3, $4, $5)")
            .bind(tx_hash)
            .bind(block_hash)
            .bind(version)
            .bind(tx_type)
            .bind(tx_status)
            .execute(&mut *transaction)
            .await?;

            let proof = &verified_proof_tx.proof_transaction.proof.to_bytes()?;
            let serialized_hyle_output = serde_json::to_string(&verified_proof_tx.hyle_output)?;
            let blob_index: i32 = i32::try_from(verified_proof_tx.hyle_output.index.0)
                .map_err(|_| anyhow::anyhow!("Proof blob index is too large to fit into an i32"))?;
            let blob_tx_hash: &TxHashDb = &verified_proof_tx.hyle_output.tx_hash.into();
            let contract_name = &verified_proof_tx.proof_transaction.contract_name.0;
            sqlx::query(
                "INSERT INTO proofs (tx_hash, blob_tx_hash, blob_index, contract_name, proof, hyle_output) VALUES ($1, $2, $3, $4, $5, $6::jsonb)",
            )
            .bind(tx_hash)
            .bind(blob_tx_hash)
            .bind(blob_index)
            .bind(contract_name)
            .bind(proof)
            .bind(serialized_hyle_output)
            .execute(&mut *transaction)
            .await?;
        }

        // Handling failed transactions
        for tx in handled_block_output.failed_txs {
            let tx_hash: &TxHashDb = &tx.hash().into();
            let version = i32::try_from(tx.version)
                .map_err(|_| anyhow::anyhow!("Tx version is too large to fit into an i32"))?;

            // Insert the transaction into the transactions table
            let tx_type = TransactionType::get_type_from_transaction(&tx);
            let tx_status = TransactionStatus::Failure;
            sqlx::query(
                "INSERT INTO transactions (tx_hash, block_hash, version, transaction_type, transaction_status)
                VALUES ($1, $2, $3, $4, $5)")
            .bind(tx_hash)
            .bind(block_hash)
            .bind(version)
            .bind(tx_type)
            .bind(tx_status)
            .execute(&mut *transaction)
            .await?;
        }

        // Handling new stakers
        for _staker in handled_block_output.stakers {
            // TODO: add new table with stakers at a given height
        }

        // Handling timed out blob transactions
        for timed_out_tx_hash in handled_block_output.timed_out_tx_hashes {
            let tx_hash: &TxHashDb = &timed_out_tx_hash.into();
            sqlx::query("UPDATE transactions SET transaction_status = $1 WHERE tx_hash = $2")
                .bind(TransactionStatus::TimedOut)
                .bind(tx_hash)
                .execute(&mut *transaction)
                .await?;
        }

        // Handling verified blob
        for (blob_tx_hash, blob_index) in handled_block_output.verified_blobs {
            let blob_tx_hash: &TxHashDb = &blob_tx_hash.into();
            let blob_index = i32::try_from(blob_index.0)
                .map_err(|_| anyhow::anyhow!("Blob index is too large to fit into an i32"))?;

            sqlx::query("UPDATE blobs SET verified = true WHERE tx_hash = $1 AND blob_index = $2")
                .bind(blob_tx_hash)
                .bind(blob_index)
                .execute(&mut *transaction)
                .await?;
        }

        // Handling settled blob transactions
        for settled_blob_tx_hash in handled_block_output.settled_blob_tx_hashes {
            let tx_hash: &TxHashDb = &settled_blob_tx_hash.into();
            sqlx::query("UPDATE transactions SET transaction_status = $1 WHERE tx_hash = $2")
                .bind(TransactionStatus::Success)
                .bind(tx_hash)
                .execute(&mut *transaction)
                .await?;
        }

        // Handling updated contract state
        for (contract_name, state_digest) in handled_block_output.updated_states {
            let contract_name = &contract_name.0;
            let state_digest = &state_digest.0;
            sqlx::query(
                "UPDATE contract_state SET state_digest = $1 WHERE contract_name = $2 AND block_hash = $3",
            )
            .bind(state_digest.clone())
            .bind(contract_name.clone())
            .bind(block_hash)
            .execute(&mut *transaction)
            .await?;

            sqlx::query("UPDATE contracts SET state_digest = $1 WHERE contract_name = $2")
                .bind(state_digest)
                .bind(contract_name)
                .execute(&mut *transaction)
                .await?;
        }

        // Commit the transaction
        transaction.commit().await?;

        Ok(())
    }

    fn send_blob_transaction_to_websocket_subscribers(
        &self,
        tx: &BlobTransaction,
        tx_hash: &TxHashDb,
        block_hash: &BlockHash,
        version: &i32,
    ) {
        for (contrat_name, senders) in self.subscribers.iter() {
            if tx
                .blobs
                .iter()
                .any(|blob| &blob.contract_name == contrat_name)
            {
                let enriched_tx = TransactionWithBlobs {
                    tx_hash: tx_hash.clone(),
                    block_hash: block_hash.clone(),
                    version: *version,
                    transaction_type: TransactionType::BlobTransaction,
                    transaction_status: TransactionStatus::Sequenced,
                    identity: tx.identity.0.clone(),
                    blobs: tx
                        .blobs
                        .iter()
                        .map(|blob| BlobWithStatus {
                            contract_name: blob.contract_name.0.clone(),
                            data: blob.data.0.clone(),
                            proof_outputs: vec![],
                        })
                        .collect(),
                };
                senders.iter().for_each(|sender| {
                    let _ = sender.send(enriched_tx.clone());
                });
            }
        }
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
    use assert_json_diff::assert_json_include;
    use axum_test::TestServer;
    use hyle_contract_sdk::{BlobIndex, HyleOutput, Identity, StateDigest, TxHash};
    use model::{BlockDb, ContractDb};
    use serde_json::json;
    use std::{
        future::IntoFuture,
        net::{Ipv4Addr, SocketAddr},
    };

    use crate::{
        bus::SharedMessageBus,
        model::{
            Blob, BlobData, BlockHeight, ProofData, ProofTransaction, RegisterContractTransaction,
            Transaction, TransactionData, VerifiedProofTransaction,
        },
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
            new_sub_receiver,
            subscribers: HashMap::new(),
            node_state: NodeState::default(),
        }
    }

    fn new_register_tx(contract_name: ContractName, state_digest: StateDigest) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::RegisterContract(RegisterContractTransaction {
                owner: "test".to_string(),
                verifier: "test".to_string(),
                program_id: vec![],
                state_digest,
                contract_name,
            }),
        }
    }

    fn new_blob_tx(
        first_contract_name: ContractName,
        second_contract_name: ContractName,
    ) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::Blob(BlobTransaction {
                identity: Identity("test.c1".to_owned()),
                blobs: vec![
                    Blob {
                        contract_name: first_contract_name,
                        data: BlobData(vec![1, 2, 3]),
                    },
                    Blob {
                        contract_name: second_contract_name,
                        data: BlobData(vec![1, 2, 3]),
                    },
                ],
            }),
        }
    }

    fn new_proof_tx(
        contract_name: ContractName,
        blob_index: BlobIndex,
        blob_tx_hash: TxHash,
        initial_state: StateDigest,
        next_state: StateDigest,
        blobs: Vec<u8>,
    ) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::VerifiedProof(VerifiedProofTransaction {
                proof_transaction: ProofTransaction {
                    blob_tx_hash: blob_tx_hash.clone(),
                    contract_name: contract_name.clone(),
                    proof: ProofData::Bytes(initial_state.0.clone()),
                },
                hyle_output: HyleOutput {
                    version: 1,
                    initial_state,
                    next_state,
                    identity: Identity("test.c1".to_owned()),
                    tx_hash: blob_tx_hash,
                    index: blob_index,
                    blobs,
                    success: true,
                    program_outputs: vec![],
                },
            }),
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_indexer_handle_block_flow() -> Result<()> {
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

        let mut indexer = new_indexer(db).await;
        let server = setup_test_server(&indexer).await?;

        let initial_state = StateDigest(vec![1, 2, 3]);
        let next_state = StateDigest(vec![4, 5, 6]);
        let first_contract_name = ContractName("c1".to_owned());
        let second_contract_name = ContractName("c2".to_owned());

        let register_tx_1 = new_register_tx(first_contract_name.clone(), initial_state.clone());
        let register_tx_2 = new_register_tx(second_contract_name.clone(), initial_state.clone());

        let blob_transaction =
            new_blob_tx(first_contract_name.clone(), second_contract_name.clone());
        let blob_transaction_hash = blob_transaction.hash();

        let proof_tx_1 = new_proof_tx(
            first_contract_name.clone(),
            BlobIndex(0),
            blob_transaction_hash.clone(),
            initial_state.clone(),
            next_state.clone(),
            vec![99, 49, 1, 2, 3, 99, 50, 1, 2, 3],
        );

        let proof_tx_2 = new_proof_tx(
            second_contract_name.clone(),
            BlobIndex(1),
            blob_transaction_hash.clone(),
            initial_state.clone(),
            next_state.clone(),
            vec![99, 49, 1, 2, 3, 99, 50, 1, 2, 3],
        );

        let other_blob_transaction =
            new_blob_tx(second_contract_name.clone(), first_contract_name.clone());
        let other_blob_transaction_hash = other_blob_transaction.hash();
        // Send two proofs for the same blob
        let proof_tx_3 = new_proof_tx(
            first_contract_name.clone(),
            BlobIndex(1),
            other_blob_transaction_hash.clone(),
            StateDigest(vec![7, 7, 7]),
            StateDigest(vec![9, 9, 9]),
            vec![99, 50, 1, 2, 3, 99, 49, 1, 2, 3],
        );
        let proof_tx_4 = new_proof_tx(
            first_contract_name.clone(),
            BlobIndex(1),
            other_blob_transaction_hash.clone(),
            StateDigest(vec![8, 8]),
            StateDigest(vec![9, 9]),
            vec![99, 50, 1, 2, 3, 99, 49, 1, 2, 3],
        );

        let txs = vec![
            register_tx_1,
            register_tx_2,
            blob_transaction,
            proof_tx_1,
            proof_tx_2,
            other_blob_transaction,
            proof_tx_3,
            proof_tx_4,
        ];

        let block = Block {
            parent_hash: BlockHash::new(""),
            height: BlockHeight(1),
            timestamp: 1,
            new_bonded_validators: vec![],
            txs,
        };

        indexer
            .handle_block(block)
            .await
            .expect("Failed to handle block");

        let transactions_response = server.get("/contract/c1").await;
        transactions_response.assert_status_ok();
        let json_response = transactions_response.json::<ContractDb>();
        assert_eq!(json_response.state_digest, next_state.0);

        let transactions_response = server.get("/contract/c2").await;
        transactions_response.assert_status_ok();
        let json_response = transactions_response.json::<ContractDb>();
        assert_eq!(json_response.state_digest, next_state.0);

        let blob_transactions_response = server.get("/blob_transactions/contract/c1").await;
        blob_transactions_response.assert_status_ok();
        assert_json_include!(
            actual: blob_transactions_response.json::<serde_json::Value>(),
            expected: json!([
                {
                    "blobs": [{
                        "contract_name": "c1",
                        "data": [1,2,3],
                        "proof_outputs": [{}]
                    }],
                    "tx_hash": blob_transaction_hash.to_string(),
                },
                {
                    "blobs": [{
                        "contract_name": "c1",
                        "data": [1,2,3],
                        "proof_outputs": [
                            {
                                "initial_state": [7,7,7],
                            },
                            {
                                "initial_state": [8,8],
                            }
                        ]
                    }],
                    "transaction_status": "Sequenced",
                    "tx_hash": other_blob_transaction_hash.to_string(),
                }
            ])
        );

        let blob_transactions_response = server.get("/blob_transactions/contract/c2").await;
        blob_transactions_response.assert_status_ok();
        assert_json_include!(
            actual: blob_transactions_response.json::<serde_json::Value>(),
            expected: json!([
                {
                    "blobs": [{
                        "contract_name": "c2",
                        "data": [1,2,3],
                        "proof_outputs": [{}]
                    }],
                    "tx_hash": blob_transaction_hash.to_string(),
                },
                {
                    "blobs": [{
                        "contract_name": "c2",
                        "data": [1,2,3],
                        "proof_outputs": []
                    }],
                    "tx_hash": other_blob_transaction_hash.to_string(),
                }
            ])
        );

        Ok(())
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

        // Test pagination
        let transactions_response = server.get("/blocks?nb_results=1").await;
        transactions_response.assert_status_ok();
        assert_eq!(transactions_response.json::<Vec<BlockDb>>().len(), 1);
        assert_eq!(
            transactions_response
                .json::<Vec<BlockDb>>()
                .first()
                .unwrap()
                .height,
            2
        );
        let transactions_response = server.get("/blocks?nb_results=1&start_block=1").await;
        transactions_response.assert_status_ok();
        assert_eq!(transactions_response.json::<Vec<BlockDb>>().len(), 1);
        assert_eq!(
            transactions_response
                .json::<Vec<BlockDb>>()
                .first()
                .unwrap()
                .height,
            1
        );
        // Test negative end of blocks
        let transactions_response = server.get("/blocks?nb_results=10&start_block=4").await;
        transactions_response.assert_status_ok();

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
