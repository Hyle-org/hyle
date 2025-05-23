use std::{fmt::Debug, path::PathBuf, sync::Arc};

use crate::bus::{BusClientSender, SharedMessageBus};
use crate::{log_error, module_bus_client, module_handle_messages, modules::Module};
use anyhow::{anyhow, Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use client_sdk::rest_client::NodeApiClient;
use client_sdk::{helpers::ClientSdkProver, transaction_builder::TxExecutorHandler};
use sdk::{
    BlobIndex, BlobTransaction, Block, BlockHeight, Calldata, ContractName, Hashed, NodeStateEvent,
    ProofTransaction, TransactionData, TxContext, TxHash, HYLE_TESTNET_CHAIN_ID,
};
use tracing::{debug, error, info, trace, warn};

/// `AutoProver` is a module that handles the proving of transactions
/// It listens to the node state events and processes all blobs in the block's transactions
/// for a given contract.
/// It asynchronously generates 1 ProofTransaction to prove all concerned blobs in a block
/// If a passed BlobTransaction times out, or is settled as failed, all blobs that are "after"
/// the failed transaction in the block are re-executed, and prooved all at once, even if in
/// multiple blocks.
/// This module requires the ELF to support multiproof. i.e. it requires the ELF to read
/// a `Vec<Calldata>` as input.
pub struct AutoProver<Contract: Send + Sync + Clone + 'static> {
    bus: AutoProverBusClient<Contract>,
    ctx: Arc<AutoProverCtx<Contract>>,
    store: AutoProverStore<Contract>,
    proved_height: BlockHeight,
    catching_up: Option<BlockHeight>,
    catching_blobs: Vec<(BlobIndex, BlobTransaction, TxContext)>,
}

#[derive(Default, BorshSerialize, BorshDeserialize)]
pub struct AutoProverStore<Contract> {
    unsettled_txs: Vec<(BlobTransaction, TxContext)>,
    state_history: Vec<(TxHash, Contract)>,
    contract: Contract,
    start_proving_at: BlockHeight,
}

module_bus_client! {
#[derive(Debug)]
pub struct AutoProverBusClient<Contract: Send + Sync + Clone + 'static> {
    sender(AutoProverEvent<Contract>),
    receiver(NodeStateEvent),
}
}

pub struct AutoProverCtx<Contract> {
    pub data_directory: PathBuf,
    pub start_height: BlockHeight,
    pub prover: Arc<dyn ClientSdkProver<Vec<Calldata>> + Send + Sync>,
    pub contract_name: ContractName,
    pub node: Arc<dyn NodeApiClient + Send + Sync>,
    pub default_state: Contract,
}

#[derive(Debug, Clone)]
pub enum AutoProverEvent<Contract> {
    /// Event sent when a blob is executed as failed
    /// proof will be generated & sent to the node
    FailedTx(TxHash, String),
    /// Event sent when a blob is executed as success
    /// proof will be generated & sent to the node
    SuccessTx(TxHash, Contract),
}

impl<Contract> Module for AutoProver<Contract>
where
    Contract: TxExecutorHandler
        + BorshSerialize
        + BorshDeserialize
        + Debug
        + Send
        + Sync
        + Clone
        + 'static,
{
    type Context = Arc<AutoProverCtx<Contract>>;

    async fn build(bus: SharedMessageBus, ctx: Self::Context) -> Result<Self> {
        let bus = AutoProverBusClient::<Contract>::new_from_bus(bus.new_handle()).await;

        let file = ctx
            .data_directory
            .join(format!("autoprover_{}.bin", ctx.contract_name).as_str());

        let store = match Self::load_from_disk::<AutoProverStore<Contract>>(file.as_path()) {
            Some(store) => store,
            None => AutoProverStore::<Contract> {
                contract: ctx.default_state.clone(),
                unsettled_txs: vec![],
                state_history: vec![],
                start_proving_at: ctx.start_height,
            },
        };

        let current_block = ctx.node.get_block_height().await?;
        let catching_up = if store.start_proving_at.0 < current_block.0 {
            Some(current_block)
        } else {
            None
        };

        Ok(AutoProver {
            bus,
            store,
            proved_height: ctx.start_height,
            ctx,
            catching_up,
            catching_blobs: vec![],
        })
    }

    async fn run(&mut self) -> Result<()> {
        module_handle_messages! {
            on_bus self.bus,
            listen<NodeStateEvent> event => {
                _ = log_error!(self.handle_node_state_event(event).await, "handle note state event")
            }
        };

        self.store.start_proving_at = self.proved_height;

        let _ = log_error!(
            Self::save_on_disk::<AutoProverStore<Contract>>(
                self.ctx
                    .data_directory
                    .join(format!("autoprover_{}.bin", self.ctx.contract_name))
                    .as_path(),
                &self.store,
            ),
            "Saving prover"
        );

        Ok(())
    }
}

impl<Contract> AutoProver<Contract>
where
    Contract: TxExecutorHandler + Debug + Clone + Send + Sync + 'static,
{
    async fn handle_node_state_event(&mut self, event: NodeStateEvent) -> Result<()> {
        let NodeStateEvent::NewBlock(block) = event;
        self.handle_processed_block(*block).await?;

        Ok(())
    }

    async fn handle_processed_block(&mut self, block: Block) -> Result<()> {
        let mut blobs = vec![];
        trace!(
            cn =% self.ctx.contract_name,
            block_height =% block.block_height,
            "Processing block {}",
            block.block_height
        );
        for (_, tx) in block.txs {
            if let TransactionData::Blob(tx) = tx.transaction_data {
                if tx
                    .blobs
                    .iter()
                    .all(|b| b.contract_name != self.ctx.contract_name)
                {
                    continue;
                }
                let tx_ctx = TxContext {
                    block_height: block.block_height,
                    block_hash: block.hash.clone(),
                    timestamp: block.block_timestamp.clone(),
                    lane_id: block
                        .lane_ids
                        .get(&tx.hashed())
                        .ok_or_else(|| anyhow!("Missing lane id in block for {}", tx.hashed()))?
                        .clone(),
                    chain_id: HYLE_TESTNET_CHAIN_ID,
                };
                blobs.extend(self.handle_blob(tx, tx_ctx));
            }
        }
        if !blobs.is_empty() {
            debug!(
                cn =% self.ctx.contract_name,
                "Found {} txs in block {} with provable blobs",
                blobs.len(),
                block.block_height
            );
            if let Some(catching_up) = self.catching_up {
                self.catching_blobs.extend(blobs);
                if block.block_height.0 < catching_up.0 {
                    debug!(
                        "Catching up block {}/{}. Proving delayed.",
                        block.block_height, catching_up
                    );
                } else {
                    debug!(
                        "Catching up block {}/{}. Proving now.",
                        block.block_height, catching_up
                    );
                    self.catching_up = None;
                    let blobs = self.catching_blobs.drain(..).collect();
                    self.prove_supported_blob(blobs)?;
                }
            } else {
                self.prove_supported_blob(blobs)?;
            }
        }

        if block.block_height.0 > self.proved_height.0 {
            self.proved_height = block.block_height;
        }

        for tx in block.successful_txs {
            self.settle_tx_success(&tx)?;
        }

        for tx in block.timed_out_txs {
            self.settle_tx_failed(&tx)?;
        }

        for tx in block.failed_txs {
            self.settle_tx_failed(&tx)?;
        }

        Ok(())
    }

    fn handle_blob(
        &mut self,
        tx: BlobTransaction,
        tx_ctx: TxContext,
    ) -> Vec<(BlobIndex, BlobTransaction, TxContext)> {
        let mut blobs = vec![];
        for (index, blob) in tx.blobs.iter().enumerate() {
            if blob.contract_name == self.ctx.contract_name {
                blobs.push((index.into(), tx.clone(), tx_ctx.clone()));
            }
        }
        self.store.unsettled_txs.push((tx, tx_ctx));
        blobs
    }

    fn settle_tx_success(&mut self, tx: &TxHash) -> Result<()> {
        let pos = self.store.state_history.iter().position(|(h, _)| h == tx);
        if let Some(pos) = pos {
            self.store.state_history = self.store.state_history.split_off(pos);
        }
        self.settle_tx(tx);
        Ok(())
    }

    fn settle_tx_failed(&mut self, tx: &TxHash) -> Result<()> {
        if let Some(pos) = self.settle_tx(tx) {
            self.handle_all_next_blobs(pos, tx)?;
            self.store.state_history.retain(|(h, _)| h != tx);
        }
        Ok(())
    }

    fn settle_tx(&mut self, hash: &TxHash) -> Option<usize> {
        let tx = self
            .store
            .unsettled_txs
            .iter()
            .position(|(t, _)| t.hashed() == *hash);
        if let Some(pos) = tx {
            self.store.unsettled_txs.remove(pos);
            return Some(pos);
        }
        None
    }

    fn handle_all_next_blobs(&mut self, idx: usize, failed_tx: &TxHash) -> Result<()> {
        let prev_state = self
            .store
            .state_history
            .iter()
            .enumerate()
            .find(|(_, (h, _))| h == failed_tx)
            .and_then(|(i, _)| {
                if i > 0 {
                    self.store.state_history.get(i - 1)
                } else {
                    None
                }
            });
        if let Some((_, contract)) = prev_state {
            debug!(cn =% self.ctx.contract_name, tx_hash =% failed_tx, "Reverting to previous state: {:?}", contract);
            self.store.contract = contract.clone();
        } else {
            warn!(cn =% self.ctx.contract_name, tx_hash =% failed_tx, "Reverting to default state");
            self.store.contract = self.ctx.default_state.clone();
        }
        let mut blobs = vec![];
        for (tx, ctx) in self.store.unsettled_txs.clone().iter().skip(idx) {
            for (index, blob) in tx.blobs.iter().enumerate() {
                if blob.contract_name == self.ctx.contract_name {
                    debug!(
                        cn =% self.ctx.contract_name,
                        "Re-execute blob for tx {} after a previous tx failure",
                        tx.hashed()
                    );
                    self.store.state_history.retain(|(h, _)| h != &tx.hashed());
                    blobs.push((index.into(), tx.clone(), ctx.clone()));
                }
            }
        }
        self.prove_supported_blob(blobs)
    }

    fn prove_supported_blob(
        &mut self,
        blobs: Vec<(BlobIndex, BlobTransaction, TxContext)>,
    ) -> Result<()> {
        debug!(
            cn =% self.ctx.contract_name,
            "Proving {} blobs",
            blobs.len()
        );
        let mut calldatas = vec![];
        let mut initial_commitment_metadata = None;
        let len = blobs.len();
        for (blob_index, tx, tx_ctx) in blobs {
            let old_tx = tx_ctx.block_height.0 < self.store.start_proving_at.0;

            let blob = tx.blobs.get(blob_index.0).ok_or_else(|| {
                anyhow!("Failed to get blob {} from tx {}", blob_index, tx.hashed())
            })?;
            let blobs = tx.blobs.clone();
            let tx_hash = tx.hashed();

            let state = self
                .store
                .contract
                .build_commitment_metadata(blob)
                .map_err(|e| anyhow!(e))
                .context("Failed to build commitment metadata")?;

            let commitment_metadata = state;

            if initial_commitment_metadata.is_none() {
                initial_commitment_metadata = Some(commitment_metadata.clone());
            } else {
                initial_commitment_metadata = Some(
                    self.store
                        .contract
                        .merge_commitment_metadata(
                            initial_commitment_metadata.unwrap(),
                            commitment_metadata,
                        )
                        .map_err(|e| anyhow!(e))
                        .context("Merging commitment_metadata")?,
                );
            }

            let calldata = Calldata {
                identity: tx.identity.clone(),
                tx_hash: tx_hash.clone(),
                private_input: vec![],
                blobs: blobs.clone().into(),
                index: blob_index,
                tx_ctx: Some(tx_ctx.clone()),
                tx_blob_count: blobs.len(),
            };

            match self
                .store
                .contract
                .handle(&calldata)
                .map_err(|e| anyhow!(e))
            {
                Err(e) => {
                    info!(
                        cn =% self.ctx.contract_name,
                        tx_hash =% tx.hashed(),
                        "Error while executing contract: {e}"
                    );
                    if !old_tx {
                        self.bus
                            .send(AutoProverEvent::FailedTx(tx_hash.clone(), e.to_string()))?;
                    }
                }
                Ok(msg) => {
                    info!(
                        cn =% self.ctx.contract_name,
                        tx_hash =% tx.hashed(),
                        "Executed contract: {}",
                        String::from_utf8_lossy(&msg.program_outputs)
                    );
                    if !old_tx {
                        self.bus.send(AutoProverEvent::SuccessTx(
                            tx_hash.clone(),
                            self.store.contract.clone(),
                        ))?;
                    }
                }
            }

            self.store
                .state_history
                .push((tx_hash.clone(), self.store.contract.clone()));

            if old_tx {
                debug!(
                    cn =% self.ctx.contract_name,
                    tx_hash =% tx.hashed(),
                    tx_height =% tx_ctx.block_height,
                    tx_height_proved =% self.store.start_proving_at,
                    "Skipping old tx",
                );
                continue;
            }

            calldatas.push(calldata);
        }

        if calldatas.is_empty() {
            return Ok(());
        }

        let Some(commitment_metadata) = initial_commitment_metadata else {
            return Ok(());
        };

        let node_client = self.ctx.node.clone();
        let prover = self.ctx.prover.clone();
        let contract_name = self.ctx.contract_name.clone();
        tokio::task::spawn(async move {
            let mut retries = 0;
            const MAX_RETRIES: u32 = 30;

            loop {
                match prover
                    .prove(commitment_metadata.clone(), calldatas.clone())
                    .await
                {
                    Ok(proof) => {
                        let tx = ProofTransaction {
                            contract_name: contract_name.clone(),
                            proof,
                        };
                        let _ = log_error!(
                            node_client.send_tx_proof(tx).await,
                            "failed to send proof to node"
                        );
                        info!("✅ Proved {len} txs");
                        break;
                    }
                    Err(e) => {
                        let should_retry =
                            e.to_string().contains("SessionCreateErr") && retries < MAX_RETRIES;
                        if should_retry {
                            warn!(
                                "Session creation error, retrying ({}/{})",
                                retries, MAX_RETRIES
                            );
                            retries += 1;
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            warn!(
                                "Session creation error, retrying ({}/{})",
                                retries, MAX_RETRIES
                            );
                            continue;
                        }
                        error!("Error proving tx: {:?}", e);
                        break;
                    }
                };
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        bus::metrics::BusMetrics,
        node_state::{test::new_node_state, NodeState},
    };

    use super::*;
    use client_sdk::helpers::test::TxExecutorTestProver;
    use client_sdk::rest_client::test::NodeApiMockClient;
    use sdk::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    #[derive(Debug, Clone, Default, BorshSerialize, BorshDeserialize)]
    struct TestContract {
        value: u32,
    }

    impl ZkContract for TestContract {
        fn execute(&mut self, calldata: &Calldata) -> sdk::RunResult {
            let (action, execution_ctx) = sdk::utils::parse_raw_calldata::<u32>(calldata)?;
            tracing::info!(
                "Executing contract (val = {}) with action: {:?}",
                self.value,
                action
            );
            self.value += action;
            Ok(("ok".to_string().into_bytes(), execution_ctx, vec![]))
        }

        fn commit(&self) -> sdk::StateCommitment {
            sdk::StateCommitment(
                borsh::to_vec(self)
                    .map_err(|e| anyhow!(e))
                    .context("Failed to commit state")
                    .unwrap(),
            )
        }
    }

    impl TxExecutorHandler for TestContract {
        fn build_commitment_metadata(&self, _blob: &Blob) -> Result<Vec<u8>> {
            borsh::to_vec(self).map_err(Into::into)
        }

        fn handle(&mut self, calldata: &Calldata) -> Result<sdk::HyleOutput> {
            let initial_state = self.commit();
            let mut res = self.execute(calldata);
            let next_state = self.commit();
            Ok(sdk::utils::as_hyle_output(
                initial_state,
                next_state,
                calldata,
                &mut res,
            ))
        }

        fn construct_state(
            _register_blob: &RegisterContractEffect,
            _metadata: &Option<Vec<u8>>,
        ) -> Result<Self> {
            Ok(Self::default())
        }
    }

    async fn setup() -> Result<(NodeState, AutoProver<TestContract>, Arc<NodeApiMockClient>)> {
        let mut node_state = new_node_state().await;
        let register = RegisterContractEffect {
            verifier: "test".into(),
            program_id: ProgramId(vec![]),
            state_commitment: TestContract::default().commit(),
            contract_name: "test".into(),
            timeout_window: Some(TimeoutWindow::Timeout(BlockHeight(5))),
        };
        node_state.handle_register_contract_effect(&register);

        let temp_dir = tempdir()?;
        let data_dir = temp_dir.path().to_path_buf();
        let api_client = Arc::new(NodeApiMockClient::new());

        let ctx = Arc::new(AutoProverCtx {
            data_directory: data_dir,
            start_height: BlockHeight(0),
            prover: Arc::new(TxExecutorTestProver::<TestContract>::new()),
            contract_name: ContractName("test".into()),
            node: api_client.clone(),
            default_state: TestContract::default(),
        });

        let bus = SharedMessageBus::new(BusMetrics::global("default".to_string()));
        let auto_prover = AutoProver::<TestContract>::build(bus.new_handle(), ctx).await?;

        Ok((node_state, auto_prover, api_client))
    }

    async fn get_txs(api_client: &Arc<NodeApiMockClient>) -> Vec<Transaction> {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut gard = api_client.pending_proofs.lock().unwrap();
        let txs = gard.drain(..).collect::<Vec<ProofTransaction>>();
        txs.into_iter()
            .map(|t| {
                let hyle_outputs = borsh::from_slice::<Vec<HyleOutput>>(&t.proof.0)
                    .context("parsing test proof")
                    .unwrap();
                for hyle_output in &hyle_outputs {
                    debug!(
                        "Initial state: {:?}, Next state: {:?}",
                        hyle_output.initial_state, hyle_output.next_state
                    );
                }

                let proven_blobs = hyle_outputs
                    .into_iter()
                    .map(|hyle_output| {
                        let blob_tx_hash = hyle_output.tx_hash.clone();
                        BlobProofOutput {
                            hyle_output,
                            program_id: ProgramId(vec![]),
                            blob_tx_hash,
                            original_proof_hash: t.proof.hashed(),
                        }
                    })
                    .collect();
                VerifiedProofTransaction {
                    contract_name: t.contract_name.clone(),
                    proven_blobs,
                    proof_hash: t.proof.hashed(),
                    proof_size: t.estimate_size(),
                    proof: Some(t.proof),
                    is_recursive: false,
                }
                .into()
            })
            .collect()
    }

    fn new_blob_tx(val: u32) -> Transaction {
        // random id to have a different tx hash
        let id: usize = rand::random();
        BlobTransaction::new(
            format!("{id}@test"),
            vec![Blob {
                contract_name: "test".into(),
                data: BlobData(borsh::to_vec(&val).unwrap()),
            }],
        )
        .into()
    }

    fn read_contract_state(node_state: &NodeState) -> TestContract {
        let state = node_state
            .contracts
            .get(&"test".into())
            .unwrap()
            .state
            .clone();

        borsh::from_slice::<TestContract>(&state.0).expect("Failed to decode contract state")
    }

    #[test_log::test(tokio::test)]
    async fn test_auto_prover_basic() -> Result<()> {
        let (mut node_state, mut auto_prover, api_client) = setup().await?;

        tracing::info!("✨ Block 1");
        let block_1 = node_state.craft_block_and_handle(1, vec![new_blob_tx(1)]);

        auto_prover.handle_processed_block(block_1).await?;

        let proofs = get_txs(&api_client).await;
        assert_eq!(proofs.len(), 1);

        tracing::info!("✨ Block 2");
        node_state.craft_block_and_handle(2, proofs);

        assert_eq!(read_contract_state(&node_state).value, 1);

        tracing::info!("✨ Block 3");
        let block_3 = node_state.craft_block_and_handle(
            3,
            vec![
                new_blob_tx(3), /* this one will timeout */
                new_blob_tx(3), /* this one will timeout */
                new_blob_tx(3),
            ],
        );
        auto_prover.handle_processed_block(block_3).await?;

        // Proofs 3 won't be sent, to trigger a timeout
        let proofs_3 = get_txs(&api_client).await;
        assert_eq!(proofs_3.len(), 1);

        tracing::info!("✨ Block 4");
        let block_4 = node_state
            .craft_block_and_handle(4, vec![new_blob_tx(4), new_blob_tx(4), new_blob_tx(4)]);
        auto_prover.handle_processed_block(block_4).await?;
        let proofs_4 = get_txs(&api_client).await;
        assert_eq!(proofs_4.len(), 1);

        for i in 5..15 {
            tracing::info!("✨ Block {i}");
            let block = node_state.craft_block_and_handle(i, vec![]);
            auto_prover.handle_processed_block(block).await?;
        }

        let proofs = get_txs(&api_client).await;
        assert_eq!(proofs.len(), 2);

        let _block_11 = node_state.craft_block_and_handle(16, proofs);
        assert_eq!(read_contract_state(&node_state).value, 16);

        Ok(())
    }
}
