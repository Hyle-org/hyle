use anyhow::{anyhow, Context, Error, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use client_sdk::contract_indexer::{ContractHandler, ContractStateStore};
use hyle_contract_sdk::{BlobIndex, ContractName, TxId};
use hyle_model::{RegisterContractEffect, TxContext};
use serde::{Deserialize, Serialize};
use std::{ops::Deref, path::PathBuf, sync::Arc};
use tokio::sync::RwLock;
use tracing::debug;

use crate::{
    bus::BusMessage,
    log_error,
    model::{Blob, BlobTransaction, Block, CommonRunContext, Hashed, Transaction, TransactionData},
    module_handle_messages,
    node_state::module::NodeStateEvent,
    utils::{conf::Conf, modules::Module},
};

use super::indexer_bus_client::IndexerBusClient;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ProverEvent {
    NewTx(Transaction),
}
impl BusMessage for ProverEvent {}

pub struct ContractStateIndexer<State> {
    bus: IndexerBusClient,
    store: Arc<RwLock<ContractStateStore<State>>>,
    contract_name: ContractName,
    file: PathBuf,
    #[allow(dead_code)]
    config: Arc<Conf>,
}

pub struct ContractStateIndexerCtx {
    pub common: Arc<CommonRunContext>,
    pub contract_name: ContractName,
}

impl<State> Module for ContractStateIndexer<State>
where
    State: Serialize
        + Clone
        + Sync
        + Send
        + std::fmt::Debug
        + ContractHandler
        + BorshSerialize
        + BorshDeserialize
        + 'static,
{
    type Context = ContractStateIndexerCtx;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = IndexerBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        let file = ctx
            .common
            .config
            .data_directory
            .join(format!("state_indexer_{}.bin", ctx.contract_name).as_str());

        let mut store =
            Self::load_from_disk_or_default::<ContractStateStore<State>>(file.as_path());
        store.contract_name = ctx.contract_name.clone();
        let store = Arc::new(RwLock::new(store));

        let (nested, mut api) = State::api(Arc::clone(&store)).await;
        if let Ok(mut o) = ctx.common.openapi.lock() {
            // Deduplicate operation ids
            for p in api.paths.paths.iter_mut() {
                p.1.get = p.1.get.take().map(|mut g| {
                    g.operation_id = g.operation_id.map(|o| format!("{}_{o}", ctx.contract_name));
                    g
                });
                p.1.post = p.1.post.take().map(|mut g| {
                    g.operation_id = g.operation_id.map(|o| format!("{}_{o}", ctx.contract_name));
                    g
                });
            }
            *o = o
                .clone()
                .nest(format!("/v1/indexer/contract/{}", ctx.contract_name), api);
        }

        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest(
                    format!("/v1/indexer/contract/{}", ctx.contract_name).as_str(),
                    nested,
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
        + Clone
        + Sync
        + Send
        + std::fmt::Debug
        + ContractHandler
        + BorshSerialize
        + BorshDeserialize
        + 'static,
{
    pub async fn start(&mut self) -> Result<(), Error> {
        module_handle_messages! {
            on_bus self.bus,
            listen<NodeStateEvent> event => {
                _ = log_error!(self.handle_node_state_event(event)
                    .await,
                    "Handling node state event")
            }
        };

        if let Err(e) = Self::save_on_disk::<ContractStateStore<State>>(
            self.file.as_path(),
            self.store.read().await.deref(),
        ) {
            tracing::warn!(cn = %self.contract_name, "Failed to save contract state indexer on disk: {}", e);
        }
        Ok(())
    }

    /// Note: Each copy of the contract state indexer does the same handle_block on each data event
    /// coming from node state.
    async fn handle_node_state_event(&mut self, event: NodeStateEvent) -> Result<(), Error> {
        let NodeStateEvent::NewBlock(block) = event;
        self.handle_processed_block(*block).await?;

        Ok(())
    }

    async fn handle_processed_block(&mut self, block: Block) -> Result<()> {
        let tx_context = block.get_context();
        for (_, contract) in block.registered_contracts {
            if self.contract_name == contract.contract_name {
                self.handle_register_contract(contract).await?;
            }
        }

        if !block.txs.is_empty() {
            debug!(handler = %self.contract_name, "🔨 Processing block: {}", block.block_height);
        }

        for (tx_id, tx) in block.txs {
            if let TransactionData::Blob(tx) = tx.transaction_data {
                self.handle_blob(tx_id, tx).await?;
            }
        }

        for s_tx in block.successful_txs {
            let dp_hash = block
                .dp_hashes
                .get(&s_tx)
                .context(format!(
                    "No parent data proposal hash present for successful tx {}",
                    s_tx.0
                ))?
                .clone();
            self.settle_tx(&TxId(dp_hash, s_tx), tx_context.clone())
                .await?;
        }
        Ok(())
    }

    async fn handle_blob(&mut self, tx_id: TxId, tx: BlobTransaction) -> Result<()> {
        let mut found_supported_blob = false;

        for b in &tx.blobs {
            if self.contract_name == b.contract_name {
                found_supported_blob = true;
                break;
            }
        }

        if found_supported_blob {
            debug!(cn = %self.contract_name, "⚒️  Found supported blob in transaction: {}", tx_id);
            self.store.write().await.unsettled_blobs.insert(tx_id, tx);
        }

        Ok(())
    }

    async fn handle_register_contract(&self, contract: RegisterContractEffect) -> Result<()> {
        let state = State::default();
        tracing::info!(cn = %self.contract_name, "📝 Registered suppored contract '{}' with initial state '{state:?}'", contract.contract_name);
        self.store.write().await.state = Some(state);
        Ok(())
    }

    async fn settle_tx(&mut self, tx: &TxId, tx_context: TxContext) -> Result<()> {
        let mut store = self.store.write().await;
        let Some(tx) = store.unsettled_blobs.remove(tx) else {
            debug!(cn = %self.contract_name, "🔨 No supported blobs found in transaction: {}", tx);
            return Ok(());
        };

        debug!(cn = %self.contract_name, "🔨 Settling transaction: {}", tx.hashed());

        for (index, Blob { contract_name, .. }) in tx.blobs.iter().enumerate() {
            if self.contract_name != *contract_name {
                continue;
            }

            let state = store
                .state
                .clone()
                .ok_or(anyhow!("No state found for {contract_name}"))?;

            let new_state = State::handle(&tx, BlobIndex(index), state, tx_context.clone())?;

            debug!(cn = %self.contract_name, "📈 Updated state for {contract_name}");

            store.state = Some(new_state);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hyle_contract_sdk::{BlobData, HyleContract, ProgramId, StateDigest};
    use hyle_model::{DataProposalHash, Digestable};
    use utoipa::openapi::OpenApi;

    use super::*;
    use crate::bus::metrics::BusMetrics;
    use crate::model::SignedBlock;
    use crate::node_state::NodeState;
    use crate::utils::conf::Conf;
    use crate::{bus::SharedMessageBus, model::CommonRunContext};
    use std::sync::Arc;

    #[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
    struct MockState(Vec<u8>);

    impl TryFrom<StateDigest> for MockState {
        type Error = Error;

        fn try_from(value: StateDigest) -> Result<Self> {
            Ok(MockState(value.0))
        }
    }

    impl Digestable for MockState {
        fn as_digest(&self) -> StateDigest {
            StateDigest(self.0.clone())
        }
    }

    impl HyleContract for MockState {
        fn execute(&mut self, _: &hyle_model::ContractInput) -> hyle_contract_sdk::RunResult {
            Err("not implemented".into())
        }
    }

    impl ContractHandler for MockState {
        fn handle(
            tx: &BlobTransaction,
            index: BlobIndex,
            mut state: Self,
            _tx_context: TxContext,
        ) -> Result<Self> {
            state.0 = tx.blobs.get(index.0).unwrap().data.0.clone();
            Ok(state)
        }

        async fn api(_store: Arc<RwLock<ContractStateStore<Self>>>) -> (axum::Router<()>, OpenApi) {
            (axum::Router::new(), OpenApi::default())
        }
    }

    async fn build_indexer(contract_name: ContractName) -> ContractStateIndexer<MockState> {
        let common = Arc::new(CommonRunContext {
            bus: SharedMessageBus::new(BusMetrics::global("global".to_string())),
            config: Arc::new(Conf::default()),
            router: Default::default(),
            openapi: Default::default(),
        });

        let ctx = ContractStateIndexerCtx {
            common: common.clone(),
            contract_name,
        };

        ContractStateIndexer::<MockState>::build(ctx).await.unwrap()
    }

    async fn register_contract(indexer: &mut ContractStateIndexer<MockState>) {
        let state_digest = StateDigest::default();
        let rce = RegisterContractEffect {
            contract_name: indexer.contract_name.clone(),
            state_digest,
            verifier: "test".into(),
            program_id: ProgramId(vec![]),
        };
        indexer.handle_register_contract(rce).await.unwrap();
    }

    #[test_log::test(tokio::test)]
    async fn test_handle_register_contract() {
        let contract_name = ContractName::from("test_contract");
        let mut indexer = build_indexer(contract_name.clone()).await;

        register_contract(&mut indexer).await;

        let store = indexer.store.read().await;
        assert!(store.state.is_some());
    }

    #[test_log::test(tokio::test)]
    async fn test_handle_blob() {
        let contract_name = ContractName::from("test_contract");
        let blob = Blob {
            contract_name: contract_name.clone(),
            data: BlobData(vec![1, 2, 3]),
        };
        let tx = BlobTransaction::new("test", vec![blob]);
        let tx_hash = tx.hashed();

        let mut indexer = build_indexer(contract_name.clone()).await;
        register_contract(&mut indexer).await;
        indexer
            .handle_blob(TxId(DataProposalHash::default(), tx_hash.clone()), tx)
            .await
            .unwrap();

        let store = indexer.store.read().await;
        assert!(store
            .unsettled_blobs
            .contains_key(&TxId(DataProposalHash::default(), tx_hash.clone())));
        assert!(store.state.clone().unwrap().0.is_empty());
    }

    #[test_log::test(tokio::test)]
    async fn test_settle_tx() {
        let contract_name = ContractName::from("test_contract");
        let blob = Blob {
            contract_name: contract_name.clone(),
            data: BlobData(vec![1, 2, 3]),
        };
        let tx = BlobTransaction::new("test", vec![blob]);
        let tx_id = TxId(DataProposalHash::default(), tx.hashed());

        let mut indexer = build_indexer(contract_name.clone()).await;
        register_contract(&mut indexer).await;
        {
            let mut store = indexer.store.write().await;
            store.unsettled_blobs.insert(tx_id.clone(), tx);
        }

        let tx_context = TxContext::default();

        indexer.settle_tx(&tx_id, tx_context).await.unwrap();

        let store = indexer.store.read().await;
        assert!(!store.unsettled_blobs.contains_key(&tx_id));
        assert_eq!(store.state.clone().unwrap().0, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_handle_node_state_event() {
        let contract_name = ContractName::from("test_contract");
        let mut indexer = build_indexer(contract_name.clone()).await;
        register_contract(&mut indexer).await;

        let mut node_state = NodeState::default();
        let block = node_state.handle_signed_block(&SignedBlock::default());

        let event = NodeStateEvent::NewBlock(Box::new(block));

        indexer.handle_node_state_event(event).await.unwrap();
        // Add assertions based on the expected state changes
    }
}
