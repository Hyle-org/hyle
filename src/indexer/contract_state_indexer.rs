use anyhow::{anyhow, Error, Result};
use hyle_contract_sdk::{BlobIndex, ContractName, TxHash};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
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
    utils::modules::Module,
};

use super::{contract_handlers::ContractHandler, indexer_bus_client::IndexerBusClient};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProverEvent {
    NewTx(Transaction),
}
impl BusMessage for ProverEvent {}

pub struct Store<State> {
    pub states: BTreeMap<ContractName, State>,
    pub unsettled_blobs: BTreeMap<TxHash, BlobTransaction>,
    pub node_state: NodeState,
}

pub struct ContractStateIndexer<State> {
    bus: IndexerBusClient,
    store: Arc<RwLock<Store<State>>>,
    program_id: String,
}

pub struct ContractStateIndexerCtx {
    pub common: Arc<CommonRunContext>,
    pub program_id: String,
}

impl<State> Module for ContractStateIndexer<State>
where
    State: Serialize
        + TryFrom<hyle_contract_sdk::StateDigest, Error = Error>
        + Clone
        + Sync
        + Send
        + ContractHandler
        + 'static,
{
    type Context = ContractStateIndexerCtx;
    fn name() -> &'static str {
        stringify!(ContractStateIndexer)
    }

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = IndexerBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        let store: Arc<RwLock<Store<State>>> = Arc::new(RwLock::new(Store {
            states: BTreeMap::new(),
            unsettled_blobs: BTreeMap::new(),
            node_state: NodeState::default(),
        }));

        let api = State::api(Arc::clone(&store)).await;
        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/indexer/contract", api));
            }
        }

        Ok(ContractStateIndexer {
            bus,
            store,
            program_id: ctx.program_id,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl<State> ContractStateIndexer<State>
where
    State: TryFrom<hyle_contract_sdk::StateDigest, Error = Error> + Clone + ContractHandler,
{
    pub async fn start(&mut self) -> Result<(), Error> {
        handle_messages! {
        on_bus self.bus,
        listen<DataEvent> cmd => {
            if let Err(e) = self.handle_data_availability_event(cmd).await {
                error!("Error while handling data availability event: {:#}", e)
            }
        }
        }
    }

    async fn handle_data_availability_event(&mut self, event: DataEvent) -> Result<(), Error> {
        if let DataEvent::NewBlock(block) = event {
            self.handle_block(block).await?;
        }
        Ok(())
    }

    async fn handle_block(&mut self, block: Block) -> Result<()> {
        info!(
            "📦 Handling block #{} with {} txs",
            block.height,
            block.txs.len()
        );
        debug!("📦 Block: {:?}", block);
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

        info!("🔨 Settling transaction: {}", tx.hash());

        for (index, Blob { contract_name, .. }) in tx.blobs.iter().enumerate() {
            if !store.states.contains_key(contract_name) {
                continue;
            }

            let state = store
                .states
                .get(contract_name)
                .cloned()
                .ok_or(anyhow!("No state found for {contract_name}"))?;

            let new_state = State::handle(&tx, BlobIndex(index as u32), state)?;

            info!("📈 Updated state for {contract_name}");

            *store.states.get_mut(contract_name).unwrap() = new_state;
        }
        Ok(())
    }
}
