use anyhow::{anyhow, Error, Result};
use bincode::{Decode, Encode};
use hyle_contract_sdk::{BlobIndex, ContractName, TxHash};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ops::Deref, path::PathBuf, sync::Arc};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use crate::{
    bus::BusMessage,
    data_availability::DataEvent,
    handle_messages,
    model::{
        Blob, BlobTransaction, Block, CommonRunContext, Hashable, RegisterContractTransaction,
        Transaction, TransactionData,
    },
    node_state::NodeState,
    utils::{conf::Conf, modules::Module},
};

use super::{contract_handlers::ContractHandler, indexer_bus_client::IndexerBusClient};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProverEvent {
    NewTx(Transaction),
}
impl BusMessage for ProverEvent {}

#[derive(Encode, Decode)]
pub struct Store<State> {
    pub state: Option<State>,
    pub contract_name: ContractName,
    pub unsettled_blobs: BTreeMap<TxHash, BlobTransaction>,
    pub node_state: NodeState,
}

impl<State> Default for Store<State> {
    fn default() -> Self {
        Store {
            state: None,
            contract_name: Default::default(),
            unsettled_blobs: BTreeMap::new(),
            node_state: NodeState::default(),
        }
    }
}

pub struct ContractStateIndexer<State> {
    bus: IndexerBusClient,
    store: Arc<RwLock<Store<State>>>,
    contract_name: ContractName,
    file: PathBuf,
    config: Arc<Conf>,
}

pub struct ContractStateIndexerCtx {
    pub common: Arc<CommonRunContext>,
    pub contract_name: ContractName,
}

impl<State> Module for ContractStateIndexer<State>
where
    State: Serialize
        + TryFrom<hyle_contract_sdk::StateDigest, Error = Error>
        + Clone
        + Sync
        + Send
        + ContractHandler
        + Encode
        + Decode
        + 'static,
{
    type Context = ContractStateIndexerCtx;
    fn name() -> &'static str {
        stringify!(ContractStateIndexer)
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = IndexerBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        let file = ctx
            .common
            .config
            .data_directory
            .join(format!("state_indexer_{}.bin", ctx.contract_name).as_str());

        let mut store = Self::load_from_disk_or_default::<Store<State>>(file.as_path());
        store.contract_name = ctx.contract_name.clone();
        let store = Arc::new(RwLock::new(store));

        let api = State::api(Arc::clone(&store)).await;
        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest(
                    format!("/v1/indexer/contract/{}", ctx.contract_name).as_str(),
                    api,
                ));
            }
        }
        let config = ctx.common.config.clone();

        Ok(ContractStateIndexer {
            bus,
            config,
            file,
            store,
            contract_name: ctx.contract_name,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl<State> ContractStateIndexer<State>
where
    State: Serialize
        + TryFrom<hyle_contract_sdk::StateDigest, Error = Error>
        + Clone
        + Sync
        + Send
        + ContractHandler
        + Encode
        + Decode
        + 'static,
{
    pub async fn start(&mut self) -> Result<(), Error> {
        handle_messages! {
        on_bus self.bus,
        listen<DataEvent> cmd => {
            if let Err(e) = self.handle_data_availability_event(cmd).await {
                error!(cn = %self.contract_name, "Error while handling data availability event: {:#}", e)
            }
        }
        }
    }

    /// Note: Each copy of the contract state indexer does the same handle_block on each data event
    /// coming from data availability. In a future refacto, data availability will stream handled blocks instead
    /// thus we could refacto this part too to avoid same processing in NodeState in each indexer
    async fn handle_data_availability_event(&mut self, event: DataEvent) -> Result<(), Error> {
        if let DataEvent::NewBlock(block) = event {
            self.handle_block(block).await?;
        }
        if let Err(e) = Self::save_on_disk::<Store<State>>(
            self.config.data_directory.as_path(),
            self.file.as_path(),
            self.store.read().await.deref(),
        ) {
            tracing::warn!(cn = %self.contract_name, "Failed to save consensus state on disk: {}", e);
        }

        Ok(())
    }

    async fn handle_block(&mut self, block: Block) -> Result<()> {
        info!(
            cn = %self.contract_name, "📦 Handling block #{} with {} txs",
            block.height,
            block.txs.len()
        );
        debug!(cn = %self.contract_name, "📦 Block: {:?}", block);
        let handled = self.store.write().await.node_state.handle_new_block(block);
        debug!(cn = %self.contract_name, "📦 Handled {:?}", handled);

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
        if tx.contract_name != self.contract_name {
            return Ok(());
        }
        info!(cn = %self.contract_name, "📝 Registering supported contract '{}'", tx.contract_name);
        let state = tx.state_digest.try_into()?;
        self.store.write().await.state = Some(state);
        Ok(())
    }

    async fn handle_blob(&mut self, tx: BlobTransaction) -> Result<()> {
        let tx_hash = tx.hash();
        let mut found_supported_blob = false;

        for b in &tx.blobs {
            if self.contract_name == b.contract_name {
                found_supported_blob = true;
                break;
            }
        }

        if found_supported_blob {
            info!(cn = %self.contract_name, "⚒️  Found supported blob in transaction: {}", tx_hash);
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
            debug!(cn = %self.contract_name, "🔨 No supported blobs found in transaction: {}", tx);
            return Ok(());
        };

        info!(cn = %self.contract_name, "🔨 Settling transaction: {}", tx.hash());

        for (index, Blob { contract_name, .. }) in tx.blobs.iter().enumerate() {
            if self.contract_name != *contract_name {
                continue;
            }

            let state = store
                .state
                .clone()
                .ok_or(anyhow!("No state found for {contract_name}"))?;

            let new_state = State::handle(&tx, BlobIndex(index as u32), state)?;

            info!(cn = %self.contract_name, "📈 Updated state for {contract_name}");

            store.state = Some(new_state);
        }
        Ok(())
    }
}
