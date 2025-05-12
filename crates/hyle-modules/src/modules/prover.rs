use std::{fmt::Debug, path::PathBuf, sync::Arc};

use crate::bus::{BusClientSender, SharedMessageBus};
use crate::{log_error, module_bus_client, module_handle_messages, modules::Module};
use anyhow::{anyhow, Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use client_sdk::{
    helpers::ClientSdkProver, rest_client::NodeApiHttpClient,
    transaction_builder::TxExecutorHandler,
};
use sdk::{
    BlobIndex, BlobTransaction, Block, BlockHeight, Calldata, ContractName, Hashed, NodeStateEvent,
    ProofTransaction, TransactionData, TxContext, TxHash, HYLE_TESTNET_CHAIN_ID,
};
use tracing::{debug, error, info, warn};

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
    ctx: Arc<AutoProverCtx>,
    store: AutoProverStore<Contract>,
}

#[derive(Default, BorshSerialize, BorshDeserialize)]
pub struct AutoProverStore<Contract> {
    unsettled_txs: Vec<(BlobTransaction, TxContext)>,
    state_history: Vec<(TxHash, Contract)>,
    contract: Contract,
}

module_bus_client! {
#[derive(Debug)]
pub struct AutoProverBusClient<Contract: Send + Sync + Clone + 'static> {
    sender(AutoProverEvent<Contract>),
    receiver(NodeStateEvent),
}
}

pub struct AutoProverCtx {
    pub data_directory: PathBuf,
    pub start_height: BlockHeight,
    pub prover: Arc<dyn ClientSdkProver<Vec<Calldata>> + Send + Sync>,
    pub contract_name: ContractName,
    pub node: Arc<NodeApiHttpClient>,
}

impl AutoProverCtx {
    #[cfg(feature = "risc0")]
    pub fn risc0(
        data_directory: PathBuf,
        elf: &'static [u8],
        contract_name: ContractName,
        node: Arc<NodeApiHttpClient>,
    ) -> Self {
        Self {
            data_directory,
            start_height: BlockHeight(0),
            prover: Arc::new(client_sdk::helpers::risc0::Risc0Prover::new(elf)),
            contract_name,
            node,
        }
    }

    #[cfg(feature = "sp1")]
    pub fn sp1(
        data_directory: PathBuf,
        elf: &'static [u8],
        contract_name: ContractName,
        start_height: BlockHeight,
        node: Arc<NodeApiHttpClient>,
    ) -> Self {
        Self {
            data_directory,
            start_height,
            prover: Arc::new(client_sdk::helpers::sp1::SP1Prover::new(elf)),
            contract_name,
            node,
        }
    }
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
    Contract:
        TxExecutorHandler + BorshDeserialize + Default + Debug + Send + Sync + Clone + 'static,
{
    type Context = Arc<AutoProverCtx>;

    async fn build(bus: SharedMessageBus, ctx: Self::Context) -> Result<Self> {
        let bus = AutoProverBusClient::<Contract>::new_from_bus(bus.new_handle()).await;

        let file = ctx
            .data_directory
            .join(format!("autoprover_{}.bin", ctx.contract_name).as_str());

        let store = Self::load_from_disk_or_default::<AutoProverStore<Contract>>(file.as_path());

        Ok(AutoProver { bus, store, ctx })
    }

    async fn run(&mut self) -> Result<()> {
        module_handle_messages! {
            on_bus self.bus,
            listen<NodeStateEvent> event => {
                _ = log_error!(self.handle_node_state_event(event).await, "handle note state event")
            }

        };

        Ok(())
    }
}

impl<Contract> AutoProver<Contract>
where
    Contract: TxExecutorHandler + Default + Debug + Clone + Send + Sync + 'static,
{
    async fn handle_node_state_event(&mut self, event: NodeStateEvent) -> Result<()> {
        let NodeStateEvent::NewBlock(block) = event;
        self.handle_processed_block(*block).await?;

        Ok(())
    }

    async fn handle_processed_block(&mut self, block: Block) -> Result<()> {
        let mut blobs = vec![];
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
        self.prove_supported_blob(blobs)?;

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
            self.store.contract = Contract::default();
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
        let mut calldatas = vec![];
        let mut initial_commitment_metadata = None;
        let len = blobs.len();
        for (blob_index, tx, tx_ctx) in blobs {
            let old_tx = tx_ctx.block_height.0 < self.ctx.start_height.0;

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
                            node_client.send_tx_proof(&tx).await,
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
