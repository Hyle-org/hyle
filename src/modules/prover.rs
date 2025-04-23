use std::{fmt::Debug, sync::Arc};

use crate::{
    bus::{BusClientSender, BusMessage},
    log_error,
    model::CommonRunContext,
    module_handle_messages,
    node_state::module::NodeStateEvent,
    utils::modules::{module_bus_client, Module},
};
use anyhow::{anyhow, Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use client_sdk::{
    helpers::risc0::Risc0Prover, rest_client::NodeApiHttpClient,
    transaction_builder::TxExecutorHandler,
};
use hyle_model::{
    BlobIndex, BlobTransaction, Block, BlockHeight, Calldata, ContractName, Hashed,
    ProofTransaction, TransactionData, TxContext, TxHash, HYLE_TESTNET_CHAIN_ID,
};
use tracing::{debug, error, info};

/// `AutoProver` is a module that handles the proving of transactions
/// It listens to the node state events and processes all blobs in the block's transactions
/// for a given contract.
/// It asynchronously generates 1 ProofTransaction to prove all concerned blobs in a block
/// If a passed BlobTransaction times out, or is settled as failed, all blobs that are "after"
/// the failed transaction in the block are re-executed, and prooved all at once, even if in
/// multiple blocks.
/// This module requires the ELF to support multiproof. i.e. it requires the ELF to read
/// a `Vec<Calldata>` as input.
pub struct AutoProver<Contract> {
    bus: AutoProverBusClient,
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
pub struct AutoProverBusClient {
    sender(ProverEvent),
    receiver(NodeStateEvent),
}
}

pub struct AutoProverCtx {
    pub common: Arc<CommonRunContext>,
    pub start_height: BlockHeight,
    pub elf: &'static [u8],
    pub contract_name: ContractName,
    pub node: Arc<NodeApiHttpClient>,
}

#[derive(Debug, Clone)]
pub enum ProverEvent {
    /// Event sent when a blob is executed as failed
    /// proof will be generated & sent to the node
    FailedTx(TxHash, String),
    /// Event sent when a blob is executed as success
    /// proof will be generated & sent to the node
    SuccessTx(TxHash),
}

impl BusMessage for ProverEvent {}

impl<Contract> Module for AutoProver<Contract>
where
    Contract: TxExecutorHandler + BorshDeserialize + Default + Debug + Send + Clone + 'static,
{
    type Context = Arc<AutoProverCtx>;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = AutoProverBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        let file = ctx
            .common
            .config
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
    Contract: TxExecutorHandler + Default + Debug + Clone,
{
    async fn handle_node_state_event(&mut self, event: NodeStateEvent) -> Result<()> {
        let NodeStateEvent::NewBlock(block) = event;
        self.handle_processed_block(*block).await?;

        Ok(())
    }

    async fn handle_processed_block(&mut self, block: Block) -> Result<()> {
        debug!("🔧 Processing block: {:?}", block.block_height);
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
            self.settle_tx_success(tx)?;
        }

        for tx in block.timed_out_txs {
            self.settle_tx_failed(tx)?;
        }

        for tx in block.failed_txs {
            self.settle_tx_failed(tx)?;
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

    fn settle_tx_success(&mut self, tx: TxHash) -> Result<()> {
        let pos = self.store.state_history.iter().position(|(h, _)| h == &tx);
        if let Some(pos) = pos {
            self.store.state_history = self.store.state_history.split_off(pos);
        }
        self.settle_tx(tx)?;
        Ok(())
    }

    fn settle_tx_failed(&mut self, tx: TxHash) -> Result<()> {
        self.handle_all_next_blobs(tx.clone())?;
        self.store.state_history.retain(|(h, _)| h != &tx);
        self.settle_tx(tx)
    }

    fn settle_tx(&mut self, tx: TxHash) -> Result<()> {
        let tx = self
            .store
            .unsettled_txs
            .iter()
            .position(|(t, _)| t.hashed() == tx);
        if let Some(pos) = tx {
            self.store.unsettled_txs.remove(pos);
        }
        Ok(())
    }

    fn handle_all_next_blobs(&mut self, failed_tx: TxHash) -> Result<()> {
        let idx = self
            .store
            .unsettled_txs
            .iter()
            .position(|(t, _)| t.hashed() == failed_tx);
        let prev_state = self
            .store
            .state_history
            .iter()
            .enumerate()
            .find(|(_, (h, _))| h == &failed_tx)
            .and_then(|(i, _)| {
                if i > 0 {
                    self.store.state_history.get(i - 1)
                } else {
                    None
                }
            });
        if let Some((_, contract)) = prev_state {
            debug!("Reverting to previous state: {:?}", contract);
            self.store.contract = contract.clone();
        } else {
            self.store.contract = Contract::default();
        }
        let mut blobs = vec![];
        for (tx, ctx) in self
            .store
            .unsettled_txs
            .clone()
            .iter()
            .skip(idx.unwrap_or(0) + 1)
        {
            for (index, blob) in tx.blobs.iter().enumerate() {
                if blob.contract_name == self.ctx.contract_name {
                    debug!(
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
                    info!("{} Error while executing contract: {e}", tx.hashed());
                    if !old_tx {
                        self.bus
                            .send(ProverEvent::FailedTx(tx_hash.clone(), e.to_string()))?;
                    }
                }
                Ok(msg) => {
                    info!(
                        "{} Executed contract: {}",
                        tx.hashed(),
                        String::from_utf8_lossy(&msg.program_outputs)
                    );
                    if !old_tx {
                        self.bus.send(ProverEvent::SuccessTx(tx_hash.clone()))?;
                    }
                }
            }

            self.store
                .state_history
                .push((tx_hash.clone(), self.store.contract.clone()));

            if old_tx {
                return Ok(());
            }

            calldatas.push(calldata);
        }

        let Some(commitment_metadata) = initial_commitment_metadata else {
            return Ok(());
        };

        let node_client = self.ctx.node.clone();
        let prover = Risc0Prover::new(self.ctx.elf);
        let contract_name = self.ctx.contract_name.clone();
        tokio::task::spawn(async move {
            match prover.prove(commitment_metadata, calldatas).await {
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
                }
                Err(e) => {
                    error!("Error proving tx: {:?}", e);
                }
            };
        });
        Ok(())
    }
}
