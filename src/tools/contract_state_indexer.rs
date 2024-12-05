use anyhow::{anyhow, bail, Error, Result};
use axum::Router;
use futures::{SinkExt, StreamExt};
use hyle_contract_sdk::{BlobIndex, ContractName, TxHash};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::{net::TcpStream, sync::RwLock};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, info, warn};

use crate::{
    bus::BusMessage,
    model::{
        Blob, BlobTransaction, Block, BlockHeight, Hashable, RegisterContractTransaction,
        Transaction, TransactionData,
    },
    node_state::NodeState,
    utils::modules::Module,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProverEvent {
    NewTx(Transaction),
}
impl BusMessage for ProverEvent {}

pub struct Store<State> {
    states: BTreeMap<ContractName, State>,
    unsettled_blobs: BTreeMap<TxHash, BlobTransaction>,
    node_state: NodeState,
}

pub struct ContractStateIndexer<State> {
    da_stream: Framed<TcpStream, LengthDelimitedCodec>,
    store: Arc<RwLock<Store<State>>>,
    program_id: String,
    handler: Box<dyn Fn(&BlobTransaction, BlobIndex, State) -> Result<State> + Send + Sync>,
}

pub struct ContractStateIndexerCtx<State> {
    pub router: Arc<Mutex<Option<Router>>>,
    pub da_address: String,
    pub program_id: String,
    pub handler: Box<dyn Fn(&BlobTransaction, BlobIndex, State) -> Result<State> + Send + Sync>,
}

impl<State> Module for ContractStateIndexer<State>
where
    State: Serialize
        + TryFrom<hyle_contract_sdk::StateDigest, Error = Error>
        + Clone
        + Sync
        + Send
        + 'static,
{
    type Context = ContractStateIndexerCtx<State>;
    fn name() -> &'static str {
        "HyllarIndexer"
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        info!("Fetching current block height");
        let da_stream = connect_to(&ctx.da_address, BlockHeight(0)).await?;

        let store = Arc::new(RwLock::new(Store {
            states: BTreeMap::new(),
            unsettled_blobs: BTreeMap::new(),
            node_state: NodeState::default(),
        }));

        let api = api::api(Arc::clone(&store)).await;
        if let Ok(mut guard) = ctx.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/indexer/contract", api));
            }
        }

        Ok(ContractStateIndexer {
            da_stream,
            store,
            program_id: ctx.program_id,
            handler: ctx.handler,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

mod api {
    use axum::{extract::Path, routing::get, Router};

    use super::*;
    use crate::rest::AppError;
    use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};

    pub(super) async fn api<State>(store: Arc<RwLock<Store<State>>>) -> Router<()>
    where
        State: Clone + Sync + Send + Serialize + 'static,
    {
        Router::new()
            .route("/:name/state", get(get_state))
            .with_state(store)
    }

    pub async fn get_state<S: Serialize + Clone + 'static>(
        Path(name): Path<ContractName>,
        State(state): State<Arc<RwLock<Store<S>>>>,
    ) -> Result<impl IntoResponse, AppError> {
        let s = state.read().await;
        s.states.get(&name).cloned().map(Json).ok_or_else(|| {
            AppError(
                StatusCode::NOT_FOUND,
                anyhow!("Contract '{}' not found", name),
            )
        })
    }
}

impl<State> ContractStateIndexer<State>
where
    State: TryFrom<hyle_contract_sdk::StateDigest, Error = Error> + Clone,
{
    pub async fn start(&mut self) -> Result<(), Error> {
        loop {
            let frame = self.da_stream.next().await;
            if let Some(Ok(cmd)) = frame {
                let bytes = cmd;
                let block: Block =
                    bincode::decode_from_slice(&bytes, bincode::config::standard())?.0;
                if let Err(e) = self.handle_block(block).await {
                    error!("Error while handling block: {:#}", e);
                }
                SinkExt::<bytes::Bytes>::send(&mut self.da_stream, "ok".into()).await?;
            } else if frame.is_none() {
                bail!("DA stream closed");
            } else if let Some(Err(e)) = frame {
                bail!("Error while reading DA stream: {}", e);
            }
        }
    }

    async fn handle_block(&mut self, block: Block) -> Result<()> {
        info!(
            "📦 Handling block #{} with {} txs",
            block.height,
            block.txs.len()
        );
        let handled = self.store.write().await.node_state.handle_new_block(block);
        debug!("📦 Handled {:?}", handled);

        for c_tx in handled.new_contract_txs {
            if let TransactionData::RegisterContract(tx) = c_tx.transaction_data {
                self.handle_register_contract(tx).await?;
            }
        }

        for b_tx in handled.new_blob_txs {
            if let TransactionData::Blob(tx) = b_tx.transaction_data {
                self.handle_blob(tx).await?;
            }
        }

        for s_tx in handled.settled_blob_tx_hashes {
            self.settle_tx(s_tx).await?;
        }
        Ok(())
    }

    async fn handle_register_contract(&mut self, tx: RegisterContractTransaction) -> Result<()> {
        let program_id = hex::encode(tx.program_id.as_slice());
        if program_id != self.program_id {
            return Ok(());
        }
        info!("📝 Registering supported contract '{}'", tx.contract_name);
        let state = tx.state_digest.try_into()?;
        self.store
            .write()
            .await
            .states
            .insert(tx.contract_name.clone(), state);
        Ok(())
    }

    async fn handle_blob(&mut self, tx: BlobTransaction) -> Result<()> {
        let tx_hash = tx.hash();
        let mut found_supported_blob = false;

        for b in &tx.blobs {
            if self
                .store
                .read()
                .await
                .states
                .contains_key(&b.contract_name)
            {
                found_supported_blob = true;
                break;
            }
        }

        if found_supported_blob {
            info!("⚒️  Found supported blob in transaction: {}", tx_hash);
            self.store
                .write()
                .await
                .unsettled_blobs
                .insert(tx_hash.clone(), tx);
        }

        Ok(())
    }

    async fn settle_tx(&mut self, tx: TxHash) -> Result<()> {
        let mut store = self.store.write().await;
        let Some(tx) = store.unsettled_blobs.get(&tx).cloned() else {
            debug!("🔨 No supported blobs found in transaction: {}", tx);
            return Ok(());
        };

        debug!("🔨 Settling transaction: {}", tx.hash());

        for (index, Blob { contract_name, .. }) in tx.blobs.iter().enumerate() {
            if !store.states.contains_key(contract_name) {
                continue;
            }

            let state = store
                .states
                .get(contract_name)
                .cloned()
                .ok_or(anyhow!("No state found for {contract_name}"))?;

            let new_state = (self.handler)(&tx, BlobIndex(index as u32), state)?;

            info!("📈 Updated state for {contract_name}");

            *store.states.get_mut(contract_name).unwrap() = new_state;
        }
        Ok(())
    }
}

pub async fn connect_to(
    target: &str,
    height: BlockHeight,
) -> Result<Framed<TcpStream, LengthDelimitedCodec>> {
    info!(
        "Connecting to node for data availability stream on {}",
        &target
    );
    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();

    let stream = loop {
        debug!("Trying to connect to {}", target);
        match TcpStream::connect(&target).await {
            Ok(stream) => break stream,
            Err(e) => {
                if start.elapsed() >= timeout {
                    bail!("Failed to connect to {}: {}. Timeout reached.", target, e);
                }
                warn!(
                    "Failed to connect to {}: {}. Retrying in 1 second...",
                    target, e
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    };
    let addr = stream.local_addr()?;
    let mut da_stream = Framed::new(stream, LengthDelimitedCodec::new());
    info!(
        "Connected to data stream to {} on {}. Starting stream from height {}",
        &target, addr, height
    );
    // Send the start height
    let height = bincode::encode_to_vec(height.0, bincode::config::standard())?;
    da_stream.send(height.into()).await?;
    Ok(da_stream)
}
