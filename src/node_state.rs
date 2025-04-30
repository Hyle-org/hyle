//! State required for participation in consensus by the node.

use crate::model::contract_registration::validate_contract_registration_metadata;
use crate::model::contract_registration::{
    validate_contract_name_registration, validate_state_commitment_size,
};
use crate::model::verifiers::NativeVerifiers;
use crate::model::*;
use anyhow::{bail, Context, Error, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_contract_sdk::{BlobIndex, HyleOutput, TxHash};
use hyle_tld::handle_blob_for_hyle_tld;
use metrics::NodeStateMetrics;
use ordered_tx_map::OrderedTxMap;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use timeouts::Timeouts;
use tracing::{debug, error, info, trace};

mod api;
mod hyle_tld;
pub mod metrics;
pub mod module;
mod ordered_tx_map;
mod timeouts;

struct SettledTxOutput {
    // Original blob transaction, now settled.
    pub tx: UnsettledBlobTransaction,
    /// Result of the settlement
    pub result: Result<SettlementResult, ()>,
}

#[derive(Debug, Clone)]
// Similar to OnchainEffect but slightly more adapted to nodestate settlement
enum SideEffect {
    Register(Contract),
    // Pass a full Contract because it's simpler in the settlement logic
    UpdateState(Contract),
    Delete(ContractName),
}

impl SideEffect {
    fn apply(&mut self, other_effect: SideEffect) {
        tracing::trace!("Applying side effect: {:?} -> {:?}", self, other_effect);
        match (self, other_effect) {
            (SideEffect::Register(reg), SideEffect::UpdateState(contract)) => {
                reg.state = contract.state
            }
            (SideEffect::Delete(_), SideEffect::UpdateState(_)) => {}
            (me, other) => *me = other,
        }
    }
}

#[derive(Debug, Clone)]
struct SettlementResult {
    contract_changes: BTreeMap<ContractName, SideEffect>,
    blob_proof_output_indices: Vec<usize>,
}

/// How a new blob TX should be handled by the node.
#[derive(Debug)]
enum BlobTxHandled {
    /// The node should try to settle the TX right away.
    ShouldSettle(TxHash),
    /// The TX is a duplicate of another unsettled TX and should be ignored/
    Duplicate,
    /// No special handling.
    Ok,
}

#[derive(Debug, Clone)]
pub struct NodeState {
    pub metrics: NodeStateMetrics,
    pub store: NodeStateStore,
}

impl NodeState {
    pub fn create(node_id: String, module_name: &'static str) -> Self {
        NodeState {
            metrics: NodeStateMetrics::global(node_id, module_name),
            store: NodeStateStore::default(),
        }
    }
}

impl std::ops::Deref for NodeState {
    type Target = NodeStateStore;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl std::ops::DerefMut for NodeState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.store
    }
}

/// NodeState manages the flattened, up-to-date state of the chain.
/// It processes raw transactions and outputs more structured data for indexers.
/// See also: NodeStateModule for the actual module implementation.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
pub struct NodeStateStore {
    timeouts: Timeouts,
    pub current_height: BlockHeight,
    // This field is public for testing purposes
    pub contracts: HashMap<ContractName, Contract>,
    unsettled_transactions: OrderedTxMap,
}

// TODO: we should register the 'hyle' TLD in the genesis block.
impl Default for NodeStateStore {
    fn default() -> Self {
        let mut ret = Self {
            timeouts: Timeouts::default(),
            current_height: BlockHeight(0),
            contracts: HashMap::new(),
            unsettled_transactions: OrderedTxMap::default(),
        };
        // Insert a default hyle-TLD contract
        ret.contracts.insert(
            "hyle".into(),
            Contract {
                name: "hyle".into(),
                program_id: ProgramId(vec![]),
                state: StateCommitment(vec![0]),
                verifier: Verifier("hyle".to_owned()),
                timeout_window: TimeoutWindow::NoTimeout,
            },
        );
        ret
    }
}

impl NodeState {
    pub fn handle_signed_block(&mut self, signed_block: &SignedBlock) -> Block {
        let next_block = self.current_height + 1 == signed_block.height();
        let initial_block = self.current_height.0 == 0 && signed_block.height().0 == 0;
        if !next_block && !initial_block {
            error!(
                "Handling signed block of height {} while current height is {}",
                signed_block.height(),
                self.current_height
            );
        }
        debug!("Handling signed block: {:?}", signed_block.height());

        self.current_height = signed_block.height();

        let mut block_under_construction = Block {
            parent_hash: signed_block.parent_hash().clone(),
            hash: signed_block.hashed(),
            block_height: signed_block.height(),
            block_timestamp: signed_block.consensus_proposal.timestamp.clone(),
            txs: vec![], // To avoid a double borrow, we'll add the transactions later
            failed_txs: vec![],
            blob_proof_outputs: vec![],
            successful_txs: vec![],
            verified_blobs: vec![],
            staking_actions: vec![],
            new_bounded_validators: signed_block
                .consensus_proposal
                .staking_actions
                .iter()
                .filter_map(|v| match v {
                    ConsensusStakingAction::Bond { candidate } => Some(candidate.pubkey.clone()),
                    _ => None,
                })
                .collect(),
            timed_out_txs: vec![], // Added below as it needs the block
            registered_contracts: vec![],
            deleted_contracts: vec![],
            updated_states: BTreeMap::new(),
            transactions_events: BTreeMap::new(),
            dp_parent_hashes: BTreeMap::new(),
            lane_ids: BTreeMap::new(),
        };

        self.clear_timeouts(&mut block_under_construction);

        let mut next_unsettled_txs = BTreeSet::new();
        // Handle all transactions
        for (lane_id, tx_id, tx) in signed_block.iter_txs_with_id() {
            // TODO: make this more efficient
            debug!("TX {} on lane {}", tx_id.1, lane_id);
            block_under_construction
                .lane_ids
                .insert(tx_id.1.clone(), lane_id.clone());
            block_under_construction
                .dp_parent_hashes
                .insert(tx_id.1.clone(), tx_id.0.clone());

            match &tx.transaction_data {
                TransactionData::Blob(blob_transaction) => {
                    match self.handle_blob_tx(
                        tx_id.0.clone(),
                        blob_transaction,
                        TxContext {
                            lane_id,
                            block_hash: block_under_construction.hash.clone(),
                            block_height: block_under_construction.block_height,
                            timestamp: signed_block.consensus_proposal.timestamp.clone(),
                            chain_id: HYLE_TESTNET_CHAIN_ID,
                        },
                    ) {
                        Ok(BlobTxHandled::ShouldSettle(tx_hash)) => {
                            let mut blob_tx_to_try_and_settle = BTreeSet::new();
                            blob_tx_to_try_and_settle.insert(tx_hash);
                            // In case of a BlobTransaction with only native verifies, we need to trigger the
                            // settlement here as we will never get a ProofTransaction
                            next_unsettled_txs = self.settle_txs_until_done(
                                &mut block_under_construction,
                                blob_tx_to_try_and_settle,
                            );
                        }
                        Ok(BlobTxHandled::Duplicate) => {
                            // literally do nothing.
                        }
                        Ok(BlobTxHandled::Ok) => {
                            block_under_construction
                                .transactions_events
                                .entry(tx_id.1.clone())
                                .or_default()
                                .push(TransactionStateEvent::Sequenced);
                        }
                        Err(e) => {
                            let err = format!("Failed to handle blob transaction: {:?}", e);
                            error!("{err}");
                            block_under_construction
                                .transactions_events
                                .entry(tx_id.1.clone())
                                .or_default()
                                .push(TransactionStateEvent::Error(err));
                            block_under_construction.failed_txs.push(tx_id.1.clone());
                        }
                    }
                }
                TransactionData::Proof(_) => {
                    error!("Unverified recursive proof transaction should not be in a block");
                }
                TransactionData::VerifiedProof(proof_tx) => {
                    // First, store the proofs and check if we can settle the transaction
                    // NB: if some of the blob proof outputs are bad, we just ignore those
                    // but we don't actually fail the transaction.
                    debug!(
                        "Handling verified proof transaction with {} proven blobs for {} (hash: {})",
                        proof_tx.proven_blobs.len(),
                        proof_tx.contract_name,
                        &tx_id
                    );
                    let blob_tx_to_try_and_settle = proof_tx
                        .proven_blobs
                        .iter()
                        .filter_map(|blob_proof_data| {
                            match self.handle_blob_proof(
                                tx_id.1.clone(),
                                blob_proof_data,
                                &mut block_under_construction,
                            ) {
                                Ok(maybe_tx_hash) => maybe_tx_hash,
                                Err(err) => {
                                    let err = format!(
                                        "Failed to handle blob #{} in verified proof transaction {}: {err:#}",
                                        blob_proof_data.hyle_output.index, &tx_id);
                                    info!("{err}");
                                    block_under_construction
                                        .transactions_events
                                        .entry(tx_id.1.clone())
                                        .or_default()
                                        .push(TransactionStateEvent::Error(err));
                                    None
                                }
                            }})
                            .collect::<BTreeSet<_>>();
                    // Then try to settle transactions when we can.
                    next_unsettled_txs = self.settle_txs_until_done(
                        &mut block_under_construction,
                        blob_tx_to_try_and_settle,
                    );
                }
            }
            // For each transaction that could not be settled, if it is the next one to be settled, set its timeout
            for unsettled_tx in next_unsettled_txs.iter() {
                if self.unsettled_transactions.is_next_to_settle(unsettled_tx) {
                    if let Some(block_height) = self
                        .unsettled_transactions
                        .get(unsettled_tx)
                        .map(|ut| ut.tx_context.block_height)
                    {
                        // Get the contract's timeout window
                        #[allow(clippy::unwrap_used, reason = "must exist because of above checks")]
                        let timeout_window = self
                            .unsettled_transactions
                            .get(unsettled_tx)
                            .map(|tx| self.get_tx_timeout_window(tx.blobs.iter().map(|b| &b.blob)))
                            .unwrap();
                        if let TimeoutWindow::Timeout(timeout_window) = timeout_window {
                            // Update timeouts
                            self.timeouts
                                .set(unsettled_tx.clone(), block_height, timeout_window);
                        }
                    }
                }
            }
            next_unsettled_txs.clear();
            block_under_construction.txs.push((tx_id, tx.clone()));
        }

        self.metrics.record_contracts(self.contracts.len() as u64);
        self.metrics
            .record_unsettled_transactions(self.unsettled_transactions.len() as u64);
        self.metrics.add_processed_block();
        self.metrics.record_current_height(self.current_height.0);

        debug!("Done handling signed block: {:?}", signed_block.height());

        block_under_construction
    }

    #[cfg(test)]
    pub fn handle_register_contract_effect(&mut self, tx: &RegisterContractEffect) {
        info!("📝 Registering contract {}", tx.contract_name);
        self.contracts.insert(
            tx.contract_name.clone(),
            Contract {
                name: tx.contract_name.clone(),
                program_id: tx.program_id.clone(),
                state: tx.state_commitment.clone(),
                verifier: tx.verifier.clone(),
                timeout_window: tx.timeout_window.clone().unwrap_or_default(),
            },
        );
    }

    fn get_tx_timeout_window<'a, T: IntoIterator<Item = &'a Blob>>(
        &self,
        blobs: T,
    ) -> TimeoutWindow {
        let mut timeout = TimeoutWindow::NoTimeout;
        for blob in blobs {
            if let Some(contract_timeout) = self
                .contracts
                .get(&blob.contract_name)
                .map(|c| c.timeout_window.clone())
            {
                timeout = match (timeout, contract_timeout) {
                    (TimeoutWindow::NoTimeout, contract_timeout) => contract_timeout,
                    (TimeoutWindow::Timeout(a), TimeoutWindow::Timeout(b)) => {
                        TimeoutWindow::Timeout(a.min(b))
                    }
                    _ => TimeoutWindow::NoTimeout,
                }
            }
        }
        timeout
    }

    fn handle_blob_tx(
        &mut self,
        parent_dp_hash: DataProposalHash,
        tx: &BlobTransaction,
        tx_context: TxContext,
    ) -> Result<BlobTxHandled, Error> {
        let tx_hash = tx.hashed();
        debug!("Handle blob tx: {:?} (hash: {})", tx, tx_hash);

        tx.validate_identity()?;

        if tx.blobs.is_empty() {
            bail!("Blob Transaction must have at least one blob");
        }

        let (blob_tx_hash, blobs_hash) = (tx.hashed(), tx.blobs_hash());

        let mut should_try_and_settle = true;

        let blobs: Vec<UnsettledBlobMetadata> = tx
            .blobs
            .iter()
            .enumerate()
            .map(|(index, blob)| {
                tracing::trace!("Handling blob - {:?}", blob);
                if let Some(Ok(verifier)) = self
                    .contracts
                    .get(&blob.contract_name)
                    .map(|b| TryInto::<NativeVerifiers>::try_into(&b.verifier))
                {
                    let hyle_output = hyle_verifiers::native::verify(
                        blob_tx_hash.clone(),
                        BlobIndex(index),
                        &tx.blobs,
                        verifier,
                    );
                    tracing::trace!("Native verifier in blob tx - {:?}", hyle_output);
                    return UnsettledBlobMetadata {
                        blob: blob.clone(),
                        possible_proofs: vec![(verifier.into(), hyle_output)],
                    };
                } else if blob.contract_name.0 == "hyle" {
                    // 'hyle' is a special case -> See settlement logic.
                } else {
                    should_try_and_settle = false;
                }
                UnsettledBlobMetadata {
                    blob: blob.clone(),
                    possible_proofs: vec![],
                }
            })
            .collect();

        // If we're behind other pending transactions, we can't settle yet.
        match self.unsettled_transactions.add(UnsettledBlobTransaction {
            identity: tx.identity.clone(),
            parent_dp_hash,
            hash: tx_hash.clone(),
            tx_context,
            blobs_hash,
            blobs,
        }) {
            Some(should_settle) => should_try_and_settle = should_settle && should_try_and_settle,
            None => {
                return Ok(BlobTxHandled::Duplicate);
            }
        }

        if self.unsettled_transactions.is_next_to_settle(&blob_tx_hash) {
            let block_height = self.current_height;
            // Update timeouts
            let timeout_window = self.get_tx_timeout_window(&tx.blobs);
            if let TimeoutWindow::Timeout(timeout_window) = timeout_window {
                self.timeouts
                    .set(blob_tx_hash.clone(), block_height, timeout_window);
            }
        }

        if should_try_and_settle {
            Ok(BlobTxHandled::ShouldSettle(tx_hash))
        } else {
            Ok(BlobTxHandled::Ok)
        }
    }

    fn handle_blob_proof(
        &mut self,
        proof_tx_hash: TxHash,
        blob_proof_data: &BlobProofOutput,
        block_under_construction: &mut Block,
    ) -> Result<Option<TxHash>, Error> {
        let blob_tx_hash = blob_proof_data.blob_tx_hash.clone();
        // Find the blob being proven and whether we should try to settle the TX.
        let Some((unsettled_tx, should_settle_tx)) = self
            .unsettled_transactions
            .get_for_settlement(&blob_tx_hash)
        else {
            bail!("BlobTx {} not found", &blob_tx_hash);
        };

        block_under_construction.dp_parent_hashes.insert(
            unsettled_tx.hash.clone(),
            unsettled_tx.parent_dp_hash.clone(),
        );
        block_under_construction.lane_ids.insert(
            unsettled_tx.hash.clone(),
            unsettled_tx.tx_context.lane_id.clone(),
        );

        // TODO: add diverse verifications ? (without the inital state checks!).
        // TODO: success to false is valid outcome and can be settled.
        Self::verify_hyle_output(unsettled_tx, &blob_proof_data.hyle_output)?;

        let Some(blob) = unsettled_tx
            .blobs
            .get_mut(blob_proof_data.hyle_output.index.0)
        else {
            bail!(
                "blob at index {} not found in blob TX {}",
                blob_proof_data.hyle_output.index.0,
                &blob_tx_hash
            );
        };

        // If we arrived here, HyleOutput provided is OK and can now be saved
        debug!(
            "Saving a hyle_output for BlobTx {} index {}",
            blob_proof_data.hyle_output.tx_hash.0, blob_proof_data.hyle_output.index
        );

        let program_output = std::str::from_utf8(&blob_proof_data.hyle_output.program_outputs)
            .map(|s| s.to_string())
            .unwrap_or(hex::encode(&blob_proof_data.hyle_output.program_outputs));
        block_under_construction
            .transactions_events
            .entry(blob_tx_hash.clone())
            .or_default()
            .push(TransactionStateEvent::NewProof {
                blob_index: blob_proof_data.hyle_output.index,
                proof_tx_hash: proof_tx_hash.clone(),
                program_output,
            });

        blob.possible_proofs.push((
            blob_proof_data.program_id.clone(),
            blob_proof_data.hyle_output.clone(),
        ));

        let unsettled_tx_hash = unsettled_tx.hash.clone();

        block_under_construction
            .blob_proof_outputs
            .push(HandledBlobProofOutput {
                proof_tx_hash,
                blob_tx_hash: unsettled_tx_hash.clone(),
                blob_index: blob_proof_data.hyle_output.index,
                blob_proof_output_index: blob.possible_proofs.len() - 1,
                #[allow(clippy::indexing_slicing, reason = "Guaranteed to exist by the above")]
                contract_name: unsettled_tx.blobs[blob_proof_data.hyle_output.index.0]
                    .blob
                    .contract_name
                    .clone(),
                hyle_output: blob_proof_data.hyle_output.clone(),
            });

        Ok(match should_settle_tx {
            true => Some(unsettled_tx_hash),
            false => None,
        })
    }

    /// Settle all transactions that are ready to settle.
    /// Returns the list of new TXs next to be settled
    fn settle_txs_until_done(
        &mut self,
        block_under_construction: &mut Block,
        mut blob_tx_to_try_and_settle: BTreeSet<TxHash>,
    ) -> BTreeSet<TxHash> {
        let mut unsettlable_txs = BTreeSet::default();
        loop {
            // TODO: investigate most performant order;
            let Some(bth) = blob_tx_to_try_and_settle.pop_first() else {
                break;
            };

            let events = block_under_construction
                .transactions_events
                .entry(bth.clone())
                .or_default();

            let dp_parent_hash = block_under_construction
                .dp_parent_hashes
                .entry(bth.clone())
                .or_default();

            let lane_id = block_under_construction
                .lane_ids
                .entry(bth.clone())
                .or_default();

            match self.try_to_settle_blob_tx(&bth, events, dp_parent_hash, lane_id) {
                Ok(SettledTxOutput {
                    tx: settled_tx,
                    result,
                }) => {
                    // Settle the TX and add any new TXs to try and settle next.
                    if let Some(mut txs) =
                        self.on_settled_blob_tx(block_under_construction, bth, settled_tx, result)
                    {
                        blob_tx_to_try_and_settle.append(&mut txs)
                    }
                }
                Err(e) => {
                    unsettlable_txs.insert(bth.clone());
                    let e = format!("Failed to settle: {}", e);
                    debug!(tx_hash = %bth, "{e}");
                    events.push(TransactionStateEvent::SettleEvent(e));
                }
            }
        }
        unsettlable_txs
    }

    fn try_to_settle_blob_tx(
        &mut self,
        unsettled_tx_hash: &TxHash,
        events: &mut Vec<TransactionStateEvent>,
        dp_parent_hash: &mut DataProposalHash,
        lane_id: &mut LaneId,
    ) -> Result<SettledTxOutput, Error> {
        trace!("Trying to settle blob tx: {:?}", unsettled_tx_hash);

        let unsettled_tx =
            self.unsettled_transactions
                .get(unsettled_tx_hash)
                .ok_or(anyhow::anyhow!(
                    "Unsettled transaction not found in the state: {:?}",
                    unsettled_tx_hash
                ))?;

        // Insert dp hash of the tx, whether its a success or not
        *dp_parent_hash = unsettled_tx.parent_dp_hash.clone();
        *lane_id = unsettled_tx.tx_context.lane_id.clone();

        // Sanity check: if some of the blob contracts are not registered, we can't proceed
        if !unsettled_tx.blobs.iter().all(|blob_metadata| {
            tracing::trace!("Checking contract: {:?}", blob_metadata.blob.contract_name);
            self.contracts
                .contains_key(&blob_metadata.blob.contract_name)
        }) {
            bail!("Cannot settle TX: some blob contracts are not registered");
        }

        let updated_contracts = BTreeMap::new();

        let result = match Self::settle_blobs_recursively(
            &self.contracts,
            updated_contracts,
            unsettled_tx.blobs.iter(),
            vec![],
            events,
            &self.metrics,
        ) {
            Some(res) => res,
            None => {
                bail!("Tx: {} is not ready to settle.", unsettled_tx.hash);
            }
        };

        // We are OK to settle now.

        #[allow(clippy::unwrap_used, reason = "must exist because of above checks")]
        let unsettled_tx = self
            .unsettled_transactions
            .remove(unsettled_tx_hash)
            .unwrap();

        Ok(SettledTxOutput {
            tx: unsettled_tx,
            result,
        })
    }

    fn settle_blobs_recursively<'a>(
        contracts: &HashMap<ContractName, Contract>,
        mut contract_changes: BTreeMap<ContractName, SideEffect>,
        mut blob_iter: impl Iterator<Item = &'a UnsettledBlobMetadata> + Clone,
        mut blob_proof_output_indices: Vec<usize>,
        events: &mut Vec<TransactionStateEvent>,
        _metrics: &NodeStateMetrics,
    ) -> Option<Result<SettlementResult, ()>> {
        // Recursion end-case: we succesfully settled all prior blobs, so success.
        let Some(current_blob) = blob_iter.next() else {
            tracing::trace!("Settlement - Done");
            return Some(Ok(SettlementResult {
                contract_changes,
                blob_proof_output_indices,
            }));
        };

        let contract_name = &current_blob.blob.contract_name;
        blob_proof_output_indices.push(0);

        #[allow(
            clippy::unwrap_used,
            reason = "all contract names are validated to exist above"
        )]
        // Super special case - the hyle contract has "synthetic proofs".
        // We need to check the current state of 'current_contracts' to check validity,
        // so we really can't do this before we've settled the earlier blobs.
        if contract_name.0 == "hyle" {
            tracing::trace!("Settlement - processing for Hyle");
            return match handle_blob_for_hyle_tld(
                contracts,
                &mut contract_changes,
                &current_blob.blob,
            ) {
                Ok(()) => {
                    tracing::trace!("Settlement - OK side effect");
                    Self::settle_blobs_recursively(
                        contracts,
                        contract_changes,
                        blob_iter.clone(),
                        blob_proof_output_indices.clone(),
                        events,
                        _metrics,
                    )
                }
                Err(err) => {
                    // We have a valid proof of failure, we short-circuit.
                    let msg = format!("Could not settle blob proof output for 'hyle': {:?}", err);
                    debug!("{msg}");
                    events.push(TransactionStateEvent::SettleEvent(msg));
                    Some(Err(()))
                }
            };
        }

        // Regular case: go through each proof for this blob. If they settle, carry on recursively.
        for (i, proof_metadata) in current_blob.possible_proofs.iter().enumerate() {
            #[allow(clippy::unwrap_used, reason = "pushed above so last must exist")]
            let blob_index = blob_proof_output_indices.last_mut().unwrap();
            *blob_index = i;

            // TODO: ideally make this CoW
            let mut current_contracts = contract_changes.clone();
            if let Err(msg) = Self::process_proof(
                contracts,
                &mut current_contracts,
                contract_name,
                proof_metadata,
            ) {
                // Not a valid proof, log it and try the next one.
                let msg = format!(
                    "Could not settle blob proof output #{} for contract '{}': {}",
                    i, contract_name, msg
                );
                debug!("{msg}");
                events.push(TransactionStateEvent::SettleEvent(msg));
                continue;
            }
            if !proof_metadata.1.success {
                // We have a valid proof of failure, we short-circuit.
                let msg = format!(
                    "Proven failure for blob {} - {:?}",
                    i,
                    String::from_utf8(proof_metadata.1.program_outputs.clone())
                );
                debug!("{msg}");
                events.push(TransactionStateEvent::SettleEvent(msg));
                return Some(Err(()));
            }

            tracing::trace!("Settlement - OK blob");
            match Self::settle_blobs_recursively(
                contracts,
                current_contracts,
                blob_iter.clone(),
                blob_proof_output_indices.clone(),
                events,
                _metrics,
            ) {
                // If this proof settles, early return, otherwise try the next one (with continue for explicitness)
                Some(res) => return Some(res),
                _ => continue,
            }
        }
        // If we end up here we didn't manage to settle all blobs, so the TX isn't ready yet.
        None
    }

    /// Handle a settled blob transaction.
    /// Handles the multiple side-effects of settling.
    /// This returns the list of new TXs to try and settle next,
    /// i.e. the "next" TXs for each contract.
    fn on_settled_blob_tx(
        &mut self,
        block_under_construction: &mut Block,
        bth: TxHash,
        settled_tx: UnsettledBlobTransaction,
        tx_result: Result<SettlementResult, ()>,
    ) -> Option<BTreeSet<TxHash>> {
        // Transaction was settled, update our state.

        // If it's a failed settlement, mark it so and move on.
        if tx_result.is_err() {
            block_under_construction
                .transactions_events
                .entry(bth.clone())
                .or_default()
                .push(TransactionStateEvent::SettledAsFailed);
            info!("⛈️ Settled tx {} has failed", &bth);

            block_under_construction.failed_txs.push(bth);
            return None;
        }

        // Otherwise process the side-effects.
        #[allow(clippy::unwrap_used, reason = "must exist because of above checks")]
        let SettlementResult {
            contract_changes: contracts_changes,
            blob_proof_output_indices,
        } = tx_result.unwrap();

        block_under_construction
            .transactions_events
            .entry(bth.clone())
            .or_default()
            .push(TransactionStateEvent::Settled);
        self.metrics.add_settled_transactions(1);
        info!("✨ Settled tx {}", &bth);

        // Keep track of which blob proof output we used to settle the TX for each blob.
        // Also note all the TXs that we might want to try and settle next
        let next_txs_to_try_and_settle = settled_tx
            .blobs
            .iter()
            .enumerate()
            .filter_map(|(i, blob_metadata)| {
                block_under_construction.verified_blobs.push((
                    bth.clone(),
                    hyle_contract_sdk::BlobIndex(i),
                    blob_proof_output_indices.get(i).cloned(),
                ));

                self.unsettled_transactions
                    .get_next_unsettled_tx(&blob_metadata.blob.contract_name)
                    .cloned()
            })
            .collect::<BTreeSet<_>>();

        // Take note of staking
        for blob_metadata in settled_tx.blobs.into_iter() {
            let blob = blob_metadata.blob;
            // Keep track of all stakers
            if blob.contract_name.0 == "staking" {
                if let Ok(structured_blob) = StructuredBlob::try_from(blob) {
                    let staking_action: StakingAction = structured_blob.data.parameters;

                    block_under_construction
                        .staking_actions
                        .push((settled_tx.identity.clone(), staking_action));
                } else {
                    error!("Failed to parse StakingAction");
                }
            }
        }

        // Update contract states
        for (_, side_effect) in contracts_changes.into_iter() {
            match side_effect {
                SideEffect::Delete(contract_name) => {
                    debug!("✏️ Delete {} contract", contract_name);
                    self.contracts.remove(&contract_name);

                    block_under_construction
                        .deleted_contracts
                        .push((bth.clone(), contract_name));
                }
                SideEffect::Register(contract) => {
                    let has_contract = self.contracts.contains_key(&contract.name);
                    if has_contract {
                        debug!(
                            "✍️  Modify '{}', contract state: {:?}",
                            &contract.name, contract.state
                        );
                    } else {
                        info!("📝 Registering contract {}", contract.name);
                        debug!(
                            "📝 Register '{}', contract state: {:?}",
                            &contract.name, contract.state
                        );
                    }
                    self.contracts
                        .insert(contract.name.clone(), contract.clone());

                    block_under_construction.registered_contracts.push((
                        bth.clone(),
                        RegisterContractEffect {
                            contract_name: contract.name.clone(),
                            program_id: contract.program_id.clone(),
                            state_commitment: contract.state.clone(),
                            verifier: contract.verifier.clone(),
                            timeout_window: Some(contract.timeout_window.clone()),
                        },
                    ));

                    // TODO: would be nice to have a drain-like API here.
                    block_under_construction
                        .updated_states
                        .insert(contract.name, contract.state);
                }
                // clippy lint set here because setting it on expressions is experimental
                #[allow(clippy::unwrap_used, reason = "we check existence before get_mut")]
                SideEffect::UpdateState(contract) => {
                    if !self.contracts.contains_key(&contract.name) {
                        // We presume this was because it's been deleted so everything is OK.
                        debug!(
                            "🪦 Updating contract {} cannot happen - no longer exists",
                            &contract.name
                        );
                        continue;
                    }
                    debug!(
                        "✍️ Update {} contract state: {:?}",
                        &contract.name, contract.state
                    );
                    self.contracts.get_mut(&contract.name).unwrap().state = contract.state.clone();

                    // TODO: would be nice to have a drain-like API here.
                    block_under_construction
                        .updated_states
                        .insert(contract.name, contract.state);
                }
            }
        }

        // Keep track of settled txs
        block_under_construction.successful_txs.push(bth);

        Some(next_txs_to_try_and_settle)
    }

    // Called when processing a verified proof TX - checks the proof is potentially valid for settlement.
    // This is an "internally coherent" check - you can't rely on any node_state data as
    // the state might be different when settling.
    fn verify_hyle_output(
        unsettled_tx: &UnsettledBlobTransaction,
        hyle_output: &HyleOutput,
    ) -> Result<(), Error> {
        // Identity verification
        if unsettled_tx.identity != hyle_output.identity {
            bail!(
                "Proof identity '{}' does not correspond to BlobTx identity '{}'.",
                hyle_output.identity,
                unsettled_tx.identity
            )
        }

        // Verify Tx hash matches
        if hyle_output.tx_hash != unsettled_tx.hash {
            bail!(
                "Proof tx_hash '{}' does not correspond to BlobTx hash '{}'.",
                hyle_output.tx_hash,
                unsettled_tx.hash
            )
        }

        if let Some(tx_ctx) = &hyle_output.tx_ctx {
            if *tx_ctx != unsettled_tx.tx_context {
                bail!(
                    "Proof tx_context '{:?}' does not correspond to BlobTx tx_context '{:?}'.",
                    tx_ctx,
                    unsettled_tx.tx_context
                )
            }
        }

        // blob_hash verification
        let extracted_blobs_hash = (&hyle_output.blobs).into();
        if !unsettled_tx.blobs_hash.includes_all(&extracted_blobs_hash) {
            bail!(
                "Proof blobs hash '{}' do not correspond to BlobTx blobs hash '{}'.",
                extracted_blobs_hash,
                unsettled_tx.blobs_hash
            )
        }

        // Get the specific blob we're verifying
        let blob = &unsettled_tx
            .blobs
            .get(hyle_output.index.0)
            .context("Blob index out of bounds")?
            .blob;

        // Verify that each side effect has a matching register/delete contract action in this specific blob
        if let Ok(data) = StructuredBlobData::<RegisterContractAction>::try_from(blob.data.clone())
        {
            let Some(eff) = hyle_output.onchain_effects.first() else {
                bail!(
                    "Proof for RegisterContractAction blob #{} does not have any onchain effects",
                    hyle_output.index
                )
            };
            if let OnchainEffect::RegisterContract(effect) = eff {
                if effect != &data.parameters {
                    bail!(
                        "Proof for RegisterContractAction blob #{} does not match the onchain effect",
                        hyle_output.index
                    )
                }
            } else {
                bail!(
                    "Proof for RegisterContractAction blob #{} does not have a register onchain effect",
                    hyle_output.index
                )
            }
        } else if let Ok(data) =
            StructuredBlobData::<DeleteContractAction>::try_from(blob.data.clone())
        {
            let Some(eff) = hyle_output.onchain_effects.first() else {
                bail!(
                    "Proof for DeleteContractAction blob #{} does not have any onchain effects",
                    hyle_output.index
                )
            };
            if let OnchainEffect::DeleteContract(effect) = eff {
                if effect != &data.parameters.contract_name {
                    bail!(
                        "Proof for DeleteContractAction blob #{} does not match the onchain effect",
                        hyle_output.index
                    )
                }
            } else {
                bail!(
                    "Proof for DeleteContractAction blob #{} does not have a delete onchain effect",
                    hyle_output.index
                )
            }
        }

        Ok(())
    }

    // Helper for process_proof
    fn get_contract<'a>(
        contracts: &'a HashMap<ContractName, Contract>,
        contract_changes: &'a BTreeMap<ContractName, SideEffect>,
        contract_name: &ContractName,
    ) -> Result<&'a Contract, Error> {
        let Some(contract) = contract_changes
            .get(contract_name)
            .and_then(|c| match c {
                SideEffect::Register(c) => Some(c),
                SideEffect::UpdateState(c) => Some(c),
                _ => None,
            })
            .or(contracts.get(contract_name))
        else {
            // Contract not found (presumably no longer exists), we can't settle this TX.
            bail!(
                "Cannot settle blob, contract '{}' no longer exists",
                contract_name
            );
        };
        Ok(contract)
    }

    // Called when trying to actually settle a blob TX - processes a proof for settlement.
    // verify_hyle_output has already been called at this point.
    fn process_proof(
        contracts: &HashMap<ContractName, Contract>,
        contract_changes: &mut BTreeMap<ContractName, SideEffect>,
        contract_name: &ContractName,
        proof_metadata: &(ProgramId, HyleOutput),
    ) -> Result<()> {
        validate_state_commitment_size(&proof_metadata.1.next_state)?;

        let contract = Self::get_contract(contracts, contract_changes, contract_name)?.clone();

        tracing::trace!(
            "Processing proof for contract {} with state {:?}",
            contract.name,
            contract.state
        );
        if proof_metadata.1.initial_state != contract.state {
            bail!(
                "Initial state mismatch: {:?}, expected {:?}",
                proof_metadata.1.initial_state,
                contract.state
            )
        }

        if proof_metadata.0 != contract.program_id {
            bail!(
                "Program ID mismatch: {:?}, expected {:?}",
                proof_metadata.0,
                contract.program_id
            )
        }

        for state_read in &proof_metadata.1.state_reads {
            let other_contract = Self::get_contract(contracts, contract_changes, &state_read.0)?;
            if state_read.1 != other_contract.state {
                bail!(
                    "State read {:?} does not match other contract state {:?}",
                    state_read,
                    other_contract.state
                )
            }
        }

        for effect in &proof_metadata.1.onchain_effects {
            match effect {
                OnchainEffect::RegisterContract(effect) => {
                    validate_contract_registration_metadata(
                        &contract.name,
                        &effect.contract_name,
                        &effect.verifier,
                        &effect.program_id,
                        &effect.state_commitment,
                    )?;
                    contract_changes.insert(
                        effect.contract_name.clone(),
                        SideEffect::Register(Contract {
                            name: effect.contract_name.clone(),
                            program_id: effect.program_id.clone(),
                            state: effect.state_commitment.clone(),
                            verifier: effect.verifier.clone(),
                            timeout_window: effect
                                .timeout_window
                                .clone()
                                .unwrap_or(contract.timeout_window.clone()),
                        }),
                    );
                }
                OnchainEffect::DeleteContract(cn) => {
                    // TODO - check the contract exists ?
                    validate_contract_name_registration(&contract.name, cn)?;
                    contract_changes.insert(cn.clone(), SideEffect::Delete(cn.clone()));
                }
            }
        }

        // Apply the generic state updates
        let contract_name = contract.name.clone();
        let update = SideEffect::UpdateState(Contract {
            state: proof_metadata.1.next_state.clone(),
            ..contract
        });
        contract_changes
            .entry(contract_name)
            .and_modify(|c| c.apply(update.clone()))
            .or_insert(update);

        Ok(())
    }

    /// Clear timeouts for transactions that have timed out.
    /// This happens in four steps:
    ///    1. Retrieve the transactions that have timed out
    ///    2. For each contract involved in these transactions, retrieve the next transaction to settle
    ///    3. Try to settle_until_done all descendant transactions
    ///    4. Among the remaining descendants, set a timeout for them
    fn clear_timeouts(&mut self, block_under_construction: &mut Block) {
        let mut txs_at_timeout = self.timeouts.drop(&block_under_construction.block_height);
        txs_at_timeout.retain(|tx| {
            if let Some(mut tx) = self.unsettled_transactions.remove(tx) {
                info!("⏰ Blob tx timed out: {}", &tx.hash);
                self.metrics.add_triggered_timeouts();
                let hash = tx.hash.clone();
                let parent_hash = tx.parent_dp_hash.clone();
                let lane_id = tx.tx_context.lane_id.clone();
                block_under_construction
                    .transactions_events
                    .entry(hash.clone())
                    .or_default()
                    .push(TransactionStateEvent::TimedOut);
                block_under_construction
                    .dp_parent_hashes
                    .insert(hash.clone(), parent_hash);
                block_under_construction.lane_ids.insert(hash, lane_id);

                // Attempt to settle following transactions
                let mut blob_tx_to_try_and_settle = BTreeSet::new();
                tx.blobs.drain(..).for_each(|b| {
                    if let Some(tx) = self
                        .unsettled_transactions
                        .get_next_unsettled_tx(&b.blob.contract_name)
                    {
                        blob_tx_to_try_and_settle.insert(tx.clone());
                    }
                });
                // Then try to settle transactions when we can.
                let next_unsettled_txs =
                    self.settle_txs_until_done(block_under_construction, blob_tx_to_try_and_settle);

                // For each transaction that could not be settled, if it is the next one to be settled, reset its timeout
                for unsettled_tx in next_unsettled_txs {
                    if self.unsettled_transactions.is_next_to_settle(&unsettled_tx) {
                        let block_height = self.current_height;
                        #[allow(clippy::unwrap_used, reason = "must exist because of above checks")]
                        let tx = self.unsettled_transactions.get(&unsettled_tx).unwrap();
                        // Get the contract's timeout window
                        let timeout_window =
                            self.get_tx_timeout_window(tx.blobs.iter().map(|b| &b.blob));
                        if let TimeoutWindow::Timeout(timeout_window) = timeout_window {
                            // Set the timeout for the transaction
                            self.timeouts
                                .set(unsettled_tx.clone(), block_height, timeout_window);
                        }
                    }
                }

                true
            } else {
                false
            }
        });

        block_under_construction.timed_out_txs = txs_at_timeout;
    }
}

#[cfg(test)]
pub mod test {
    use core::panic;

    mod native_verifiers;

    use super::*;
    use assertables::assert_err;
    use hyle_net::clock::TimestampMsClock;
    use utils::TimestampMs;

    async fn new_node_state() -> NodeState {
        NodeState {
            metrics: NodeStateMetrics::global("test".to_string(), "test"),
            store: NodeStateStore::default(),
        }
    }

    fn new_blob(contract: &str) -> Blob {
        Blob {
            contract_name: ContractName::new(contract),
            data: BlobData(vec![0, 1, 2, 3]),
        }
    }

    pub fn make_register_contract_tx(name: ContractName) -> BlobTransaction {
        BlobTransaction::new(
            "hyle@hyle",
            vec![RegisterContractAction {
                verifier: "test".into(),
                program_id: ProgramId(vec![]),
                state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                contract_name: name,
                timeout_window: None,
            }
            .as_blob("hyle".into(), None, None)],
        )
    }

    pub fn make_register_contract_effect(contract_name: ContractName) -> RegisterContractEffect {
        RegisterContractEffect {
            verifier: "test".into(),
            program_id: ProgramId(vec![]),
            state_commitment: StateCommitment(vec![0, 1, 2, 3]),
            contract_name,
            timeout_window: None,
        }
    }

    pub fn new_proof_tx(
        contract: &ContractName,
        hyle_output: &HyleOutput,
        blob_tx_hash: &TxHash,
    ) -> VerifiedProofTransaction {
        let proof = ProofTransaction {
            contract_name: contract.clone(),
            proof: ProofData(borsh::to_vec(&vec![hyle_output.clone()]).unwrap()),
        };
        VerifiedProofTransaction {
            contract_name: contract.clone(),
            proven_blobs: vec![BlobProofOutput {
                hyle_output: hyle_output.clone(),
                program_id: ProgramId(vec![]),
                blob_tx_hash: blob_tx_hash.clone(),
                original_proof_hash: proof.proof.hashed(),
            }],
            proof_hash: proof.proof.hashed(),
            proof_size: proof.estimate_size(),
            proof: Some(proof.proof),
            is_recursive: false,
        }
    }

    pub fn make_hyle_output(blob_tx: BlobTransaction, blob_index: BlobIndex) -> HyleOutput {
        HyleOutput {
            version: 1,
            identity: blob_tx.identity.clone(),
            index: blob_index,
            blobs: blob_tx.blobs.clone().into(),
            tx_blob_count: blob_tx.blobs.len(),
            initial_state: StateCommitment(vec![0, 1, 2, 3]),
            next_state: StateCommitment(vec![4, 5, 6]),
            success: true,
            tx_hash: blob_tx.hashed(),
            tx_ctx: None,
            state_reads: vec![],
            onchain_effects: vec![],
            program_outputs: vec![],
        }
    }

    pub fn make_hyle_output_with_state(
        blob_tx: BlobTransaction,
        blob_index: BlobIndex,
        initial_state: &[u8],
        next_state: &[u8],
    ) -> HyleOutput {
        HyleOutput {
            version: 1,
            identity: blob_tx.identity.clone(),
            index: blob_index,
            blobs: blob_tx.blobs.clone().into(),
            tx_blob_count: blob_tx.blobs.len(),
            initial_state: StateCommitment(initial_state.to_vec()),
            next_state: StateCommitment(next_state.to_vec()),
            success: true,
            tx_hash: blob_tx.hashed(),
            tx_ctx: None,
            state_reads: vec![],
            onchain_effects: vec![],
            program_outputs: vec![],
        }
    }

    fn craft_signed_block(height: u64, txs: Vec<Transaction>) -> SignedBlock {
        SignedBlock {
            certificate: AggregateSignature::default(),
            consensus_proposal: ConsensusProposal {
                slot: height,
                ..ConsensusProposal::default()
            },
            data_proposals: vec![(LaneId::default(), vec![DataProposal::new(None, txs)])],
        }
    }

    fn bogus_tx_context() -> TxContext {
        TxContext {
            lane_id: LaneId::default(),
            block_hash: ConsensusProposalHash("0xfedbeef".to_owned()),
            block_height: BlockHeight(133),
            timestamp: TimestampMsClock::now(),
            chain_id: HYLE_TESTNET_CHAIN_ID,
        }
    }

    #[test_log::test(tokio::test)]
    async fn happy_path_with_tx_context() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let register_c1 = make_register_contract_effect(c1.clone());
        state.handle_register_contract_effect(&register_c1);

        let identity = Identity::new("test@c1");
        let blob_tx = BlobTransaction::new(identity.clone(), vec![new_blob("c1")]);

        let blob_tx_id = blob_tx.hashed();

        let ctx = bogus_tx_context();
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx, ctx.clone())
            .unwrap();

        let mut hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        hyle_output.tx_ctx = Some(ctx.clone());
        let verified_proof = new_proof_tx(&c1, &hyle_output, &blob_tx_id);
        // Modify something so it would fail.
        let mut ctx = ctx.clone();
        ctx.timestamp = TimestampMs(1234);
        hyle_output.tx_ctx = Some(ctx);
        let verified_proof_bad = new_proof_tx(&c1, &hyle_output, &blob_tx_id);

        let block = state.handle_signed_block(&craft_signed_block(
            1,
            vec![verified_proof_bad.into(), verified_proof.into()],
        ));
        assert_eq!(block.blob_proof_outputs.len(), 1);
        // We don't actually fail proof txs with blobs that fail
        assert_eq!(block.failed_txs.len(), 0);
        assert_eq!(block.successful_txs.len(), 1);

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
    }

    #[test_log::test(tokio::test)]
    async fn blob_tx_without_blobs() {
        let mut state = new_node_state().await;
        let identity = Identity::new("test@c1");

        let blob_tx = BlobTransaction::new(identity.clone(), vec![]);

        assert_err!(state.handle_blob_tx(
            DataProposalHash::default(),
            &blob_tx,
            bogus_tx_context()
        ));
    }

    #[test_log::test(tokio::test)]
    async fn blob_tx_with_incorrect_identity() {
        let mut state = new_node_state().await;
        let identity = Identity::new("incorrect_id");

        let blob_tx = BlobTransaction::new(identity.clone(), vec![new_blob("test")]);

        assert_err!(state.handle_blob_tx(
            DataProposalHash::default(),
            &blob_tx,
            bogus_tx_context()
        ));
    }

    #[test_log::test(tokio::test)]
    async fn two_proof_for_one_blob_tx() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let c2 = ContractName::new("c2");
        let identity = Identity::new("test@c1");

        let register_c1 = make_register_contract_effect(c1.clone());
        let register_c2 = make_register_contract_effect(c2.clone());

        let blob_tx =
            BlobTransaction::new(identity.clone(), vec![new_blob(&c1.0), new_blob(&c2.0)]);

        let blob_tx_hash = blob_tx.hashed();

        state.handle_register_contract_effect(&register_c1);
        state.handle_register_contract_effect(&register_c2);
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx, bogus_tx_context())
            .unwrap();

        let hyle_output_c1 = make_hyle_output(blob_tx.clone(), BlobIndex(0));

        let verified_proof_c1 = new_proof_tx(&c1, &hyle_output_c1, &blob_tx_hash);

        let hyle_output_c2 = make_hyle_output(blob_tx.clone(), BlobIndex(1));

        let verified_proof_c2 = new_proof_tx(&c2, &hyle_output_c2, &blob_tx_hash);

        state.handle_signed_block(&craft_signed_block(
            10,
            vec![verified_proof_c1.into(), verified_proof_c2.into()],
        ));

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
    }

    #[test_log::test(tokio::test)]
    async fn wrong_blob_index_for_contract() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let c2 = ContractName::new("c2");

        let register_c1 = make_register_contract_effect(c1.clone());
        let register_c2 = make_register_contract_effect(c2.clone());

        let blob_tx_1 = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![new_blob(&c1.0), new_blob(&c2.0)],
        );
        let blob_tx_hash_1 = blob_tx_1.hashed();

        state.handle_register_contract_effect(&register_c1);
        state.handle_register_contract_effect(&register_c2);
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx_1, bogus_tx_context())
            .unwrap();

        let hyle_output_c1 = make_hyle_output(blob_tx_1.clone(), BlobIndex(1)); // Wrong index

        let verified_proof_c1 = new_proof_tx(&c1, &hyle_output_c1, &blob_tx_hash_1);

        state.handle_signed_block(&craft_signed_block(10, vec![verified_proof_c1.into()]));

        // Check that we did not settle
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![0, 1, 2, 3]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![0, 1, 2, 3]);
    }

    #[test_log::test(tokio::test)]
    async fn two_proof_for_same_blob() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let c2 = ContractName::new("c2");

        let register_c1 = make_register_contract_effect(c1.clone());
        let register_c2 = make_register_contract_effect(c2.clone());

        let blob_tx = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![new_blob(&c1.0), new_blob(&c2.0)],
        );
        let blob_tx_hash = blob_tx.hashed();

        state.handle_register_contract_effect(&register_c1);
        state.handle_register_contract_effect(&register_c2);
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx, bogus_tx_context())
            .unwrap();

        let hyle_output_c1 = make_hyle_output(blob_tx.clone(), BlobIndex(0));

        let verified_proof_c1 = new_proof_tx(&c1, &hyle_output_c1, &blob_tx_hash);

        state.handle_signed_block(&craft_signed_block(
            10,
            vec![verified_proof_c1.clone().into(), verified_proof_c1.into()],
        ));

        assert_eq!(
            state
                .unsettled_transactions
                .get(&blob_tx_hash)
                .unwrap()
                .blobs
                .first()
                .unwrap()
                .possible_proofs
                .len(),
            2
        );
        // Check that we did not settled
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![0, 1, 2, 3]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![0, 1, 2, 3]);
    }

    #[test_log::test(tokio::test)]
    async fn two_proof_with_some_invalid_blob_proof_output() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");

        let register_c1 = make_register_contract_effect(c1.clone());

        let blob_tx = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![new_blob(&c1.0), new_blob(&c1.0)],
        );

        let blob_tx_hash = blob_tx.hashed();

        state.handle_register_contract_effect(&register_c1);
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx, bogus_tx_context())
            .unwrap();

        let hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        let verified_proof = new_proof_tx(&c1, &hyle_output, &blob_tx_hash);
        let invalid_output = make_hyle_output(blob_tx.clone(), BlobIndex(4));
        let mut invalid_verified_proof = new_proof_tx(&c1, &invalid_output, &blob_tx_hash);

        invalid_verified_proof
            .proven_blobs
            .insert(0, verified_proof.proven_blobs.first().unwrap().clone());

        let block =
            state.handle_signed_block(&craft_signed_block(5, vec![invalid_verified_proof.into()]));

        // We don't fail.
        assert_eq!(block.failed_txs.len(), 0);
        // We only store one of the two.
        assert_eq!(block.blob_proof_outputs.len(), 1);
    }

    #[test_log::test(tokio::test)]
    async fn settle_with_multiple_state_reads() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let c2 = ContractName::new("c2");

        state.handle_signed_block(&craft_signed_block(
            10,
            vec![
                make_register_contract_tx(c1.clone()).into(),
                make_register_contract_tx(c2.clone()).into(),
            ],
        ));

        let blob_tx = BlobTransaction::new(Identity::new("test@c1"), vec![new_blob(&c1.0)]);
        let mut ho = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        // Add an incorrect state read
        ho.state_reads
            .push((c2.clone(), StateCommitment(vec![9, 8, 7])));

        let effects = state.handle_signed_block(&craft_signed_block(
            11,
            vec![
                blob_tx.clone().into(),
                new_proof_tx(&c1, &ho, &blob_tx.hashed()).into(),
            ],
        ));

        assert!(effects
            .transactions_events
            .get(&blob_tx.hashed())
            .unwrap()
            .iter()
            .any(|e| {
                let TransactionStateEvent::SettleEvent(errmsg) = e else {
                    return false;
                };
                errmsg.contains("does not match other contract state")
            }));

        let mut ho = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        // Now correct state reads (some redundant ones to validate that this works)
        ho.state_reads
            .push((c2.clone(), state.contracts.get(&c2).unwrap().state.clone()));
        ho.state_reads
            .push((c2.clone(), state.contracts.get(&c2).unwrap().state.clone()));
        ho.state_reads
            .push((c1.clone(), state.contracts.get(&c1).unwrap().state.clone()));

        let effects = state.handle_signed_block(&craft_signed_block(
            12,
            vec![new_proof_tx(&c1, &ho, &blob_tx.hashed()).into()],
        ));
        assert_eq!(effects.blob_proof_outputs.len(), 1);
        assert_eq!(effects.successful_txs.len(), 1);
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
    }

    #[test_log::test(tokio::test)]
    async fn change_same_contract_state_multiple_times_in_same_tx() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");

        let register_c1 = make_register_contract_effect(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);
        let third_blob = new_blob(&c1.0);

        let blob_tx = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![first_blob, second_blob, third_blob],
        );
        let blob_tx_hash = blob_tx.hashed();

        state.handle_register_contract_effect(&register_c1);
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx, bogus_tx_context())
            .unwrap();

        let first_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));

        let verified_first_proof = new_proof_tx(&c1, &first_hyle_output, &blob_tx_hash);

        let mut second_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(1));
        second_hyle_output.initial_state = first_hyle_output.next_state.clone();
        second_hyle_output.next_state = StateCommitment(vec![7, 8, 9]);

        let verified_second_proof = new_proof_tx(&c1, &second_hyle_output, &blob_tx_hash);

        let mut third_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(2));
        third_hyle_output.initial_state = second_hyle_output.next_state.clone();
        third_hyle_output.next_state = StateCommitment(vec![10, 11, 12]);

        let verified_third_proof = new_proof_tx(&c1, &third_hyle_output, &blob_tx_hash);

        state.handle_signed_block(&craft_signed_block(
            10,
            vec![
                verified_first_proof.into(),
                verified_second_proof.into(),
                verified_third_proof.into(),
            ],
        ));

        // Check that we did settled with the last state
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![10, 11, 12]);
    }

    #[test_log::test(tokio::test)]
    async fn dead_end_in_proving_settles_still() {
        let mut state = new_node_state().await;

        let c1 = ContractName::new("c1");
        let register_c1 = make_register_contract_effect(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);
        let third_blob = new_blob(&c1.0);
        let blob_tx = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![first_blob, second_blob, third_blob],
        );

        let blob_tx_hash = blob_tx.hashed();

        state.handle_register_contract_effect(&register_c1);
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx, bogus_tx_context())
            .unwrap();

        // The test is that we send a proof for the first blob, then a proof the second blob with next_state B,
        // then a proof for the second blob with next_state C, then a proof for the third blob with initial_state C,
        // and it should settle, ignoring the initial 'dead end'.

        let first_proof_tx = new_proof_tx(
            &c1,
            &make_hyle_output_with_state(blob_tx.clone(), BlobIndex(0), &[0, 1, 2, 3], &[2]),
            &blob_tx_hash,
        );

        let second_proof_tx_b = new_proof_tx(
            &c1,
            &make_hyle_output_with_state(blob_tx.clone(), BlobIndex(1), &[2], &[3]),
            &blob_tx_hash,
        );

        let second_proof_tx_c = new_proof_tx(
            &c1,
            &make_hyle_output_with_state(blob_tx.clone(), BlobIndex(1), &[2], &[4]),
            &blob_tx_hash,
        );

        let third_proof_tx = new_proof_tx(
            &c1,
            &make_hyle_output_with_state(blob_tx.clone(), BlobIndex(2), &[4], &[5]),
            &blob_tx_hash,
        );

        let block = state.handle_signed_block(&craft_signed_block(
            4,
            vec![
                first_proof_tx.into(),
                second_proof_tx_b.into(),
                second_proof_tx_c.into(),
                third_proof_tx.into(),
            ],
        ));

        assert_eq!(
            block
                .verified_blobs
                .iter()
                .map(|(_, _, idx)| idx.unwrap())
                .collect::<Vec<_>>(),
            vec![0, 1, 0]
        );
        // Check that we did settled with the last state
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![5]);
    }

    #[test_log::test(tokio::test)]
    async fn duplicate_proof_with_inconsistent_state_should_never_settle() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");

        let register_c1 = make_register_contract_effect(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);

        let blob_tx = BlobTransaction::new(Identity::new("test@c1"), vec![first_blob, second_blob]);

        let blob_tx_hash = blob_tx.hashed();

        state.handle_register_contract_effect(&register_c1);
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx, bogus_tx_context())
            .unwrap();

        // Create legitimate proof for Blob1
        let first_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        let verified_first_proof = new_proof_tx(&c1, &first_hyle_output, &blob_tx_hash);

        // Create hacky proof for Blob1
        let mut another_first_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        another_first_hyle_output.initial_state = first_hyle_output.next_state.clone();
        another_first_hyle_output.next_state = first_hyle_output.initial_state.clone();

        let another_verified_first_proof =
            new_proof_tx(&c1, &another_first_hyle_output, &blob_tx_hash);

        let mut second_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(1));
        second_hyle_output.initial_state = another_first_hyle_output.next_state.clone();
        second_hyle_output.next_state = StateCommitment(vec![7, 8, 9]);

        let verified_second_proof = new_proof_tx(&c1, &second_hyle_output, &blob_tx_hash);

        state.handle_signed_block(&craft_signed_block(
            10,
            vec![
                verified_first_proof.into(),
                another_verified_first_proof.into(),
                verified_second_proof.into(),
            ],
        ));

        // Check that we did not settled
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![0, 1, 2, 3]);
    }

    #[test_log::test(tokio::test)]
    async fn duplicate_proof_with_inconsistent_state_should_never_settle_another() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");

        let register_c1 = make_register_contract_effect(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);
        let third_blob = new_blob(&c1.0);

        let blob_tx = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![first_blob, second_blob, third_blob],
        );

        let blob_tx_hash = blob_tx.hashed();

        state.handle_register_contract_effect(&register_c1);
        state
            .handle_blob_tx(DataProposalHash::default(), &blob_tx, bogus_tx_context())
            .unwrap();

        // Create legitimate proof for Blob1
        let first_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        let verified_first_proof = new_proof_tx(&c1, &first_hyle_output, &blob_tx_hash);

        let mut second_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(1));
        second_hyle_output.initial_state = first_hyle_output.next_state.clone();
        second_hyle_output.next_state = StateCommitment(vec![7, 8, 9]);

        let verified_second_proof = new_proof_tx(&c1, &second_hyle_output, &blob_tx_hash);

        let mut third_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(2));
        third_hyle_output.initial_state = first_hyle_output.next_state.clone();
        third_hyle_output.next_state = StateCommitment(vec![10, 11, 12]);

        let verified_third_proof = new_proof_tx(&c1, &third_hyle_output, &blob_tx_hash);

        state.handle_signed_block(&craft_signed_block(
            10,
            vec![
                verified_first_proof.into(),
                verified_second_proof.into(),
                verified_third_proof.into(),
            ],
        ));

        // Check that we did not settled
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![0, 1, 2, 3]);
    }

    #[test_log::test(tokio::test)]
    async fn test_auto_settle_next_txs_after_settle() {
        let mut state = new_node_state().await;

        let c1 = ContractName::new("c1");
        let c2 = ContractName::new("c2");
        let register_c1 = make_register_contract_tx(c1.clone());
        let register_c2 = make_register_contract_tx(c2.clone());

        // Add four transactions - A blocks B/C, B blocks D.
        // Send proofs for B, C, D before A.
        let tx_a = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![new_blob(&c1.0), new_blob(&c2.0)],
        );
        let tx_b = BlobTransaction::new(Identity::new("test@c1"), vec![new_blob(&c1.0)]);
        let tx_c = BlobTransaction::new(Identity::new("test@c2"), vec![new_blob(&c2.0)]);
        let tx_d = BlobTransaction::new(Identity::new("test2@c1"), vec![new_blob(&c1.0)]);

        let tx_a_hash = tx_a.hashed();
        let hyle_output =
            make_hyle_output_with_state(tx_a.clone(), BlobIndex(0), &[0, 1, 2, 3], &[12]);
        let tx_a_proof_1 = new_proof_tx(&c1, &hyle_output, &tx_a_hash);
        let hyle_output =
            make_hyle_output_with_state(tx_a.clone(), BlobIndex(1), &[0, 1, 2, 3], &[22]);
        let tx_a_proof_2 = new_proof_tx(&c2, &hyle_output, &tx_a_hash);

        let tx_b_hash = tx_b.hashed();
        let hyle_output = make_hyle_output_with_state(tx_b.clone(), BlobIndex(0), &[12], &[13]);
        let tx_b_proof = new_proof_tx(&c1, &hyle_output, &tx_b_hash);

        let tx_c_hash = tx_c.hashed();
        let hyle_output = make_hyle_output_with_state(tx_c.clone(), BlobIndex(0), &[22], &[23]);
        let tx_c_proof = new_proof_tx(&c1, &hyle_output, &tx_c_hash);

        let tx_d_hash = tx_d.hashed();
        let hyle_output = make_hyle_output_with_state(tx_d.clone(), BlobIndex(0), &[13], &[14]);
        let tx_d_proof = new_proof_tx(&c1, &hyle_output, &tx_d_hash);

        state.handle_signed_block(&craft_signed_block(
            104,
            vec![
                register_c1.into(),
                register_c2.into(),
                tx_a.into(),
                tx_b.into(),
                tx_b_proof.into(),
                tx_d.into(),
                tx_d_proof.into(),
            ],
        ));

        state.handle_signed_block(&craft_signed_block(
            108,
            vec![tx_c.into(), tx_c_proof.into()],
        ));

        // Now settle the first, which should auto-settle the pending ones, then the ones waiting for these.
        assert_eq!(
            state
                .handle_signed_block(&craft_signed_block(
                    110,
                    vec![tx_a_proof_1.into(), tx_a_proof_2.into(),]
                ))
                .successful_txs,
            vec![tx_a_hash, tx_b_hash, tx_d_hash, tx_c_hash]
        );
    }
    #[test_log::test(tokio::test)]
    async fn test_tx_timeout_simple() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let register_c1 = make_register_contract_tx(c1.clone());

        // First basic test - Time out a TX.
        let blob_tx = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![new_blob(&c1.0), new_blob(&c1.0)],
        );

        let txs = vec![register_c1.into(), blob_tx.clone().into()];

        let blob_tx_hash = blob_tx.hashed();

        state.handle_signed_block(&craft_signed_block(3, txs));

        // This should trigger the timeout
        let timed_out_tx_hashes = state
            .handle_signed_block(&craft_signed_block(103, vec![]))
            .timed_out_txs;

        // Check that the transaction has timed out
        assert!(timed_out_tx_hashes.contains(&blob_tx_hash));
        assert!(state.unsettled_transactions.get(&blob_tx_hash).is_none());
    }

    #[test_log::test(tokio::test)]
    async fn test_tx_no_timeout_once_settled() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let register_c1 = make_register_contract_tx(c1.clone());

        // Add a new transaction and settle it.
        let blob_tx = BlobTransaction::new(Identity::new("test@c1"), vec![new_blob(&c1.0)]);

        let crafted_block = craft_signed_block(
            104,
            vec![register_c1.clone().into(), blob_tx.clone().into()],
        );

        let blob_tx_hash = blob_tx.hashed();

        state.handle_signed_block(&crafted_block);

        assert_eq!(
            timeouts::tests::get(&state.timeouts, &blob_tx_hash),
            Some(BlockHeight(204))
        );

        let first_hyle_output = make_hyle_output(blob_tx, BlobIndex(0));
        let verified_first_proof = new_proof_tx(&c1, &first_hyle_output, &blob_tx_hash);

        // Settle TX
        assert_eq!(
            state
                .handle_signed_block(&craft_signed_block(105, vec![verified_first_proof.into(),],))
                .successful_txs,
            vec![blob_tx_hash.clone()]
        );

        assert!(state.unsettled_transactions.get(&blob_tx_hash).is_none());
        // The TX remains in the map
        assert_eq!(
            timeouts::tests::get(&state.timeouts, &blob_tx_hash),
            Some(BlockHeight(204))
        );

        // Time out
        let timed_out_tx_hashes = state
            .handle_signed_block(&craft_signed_block(204, vec![]))
            .timed_out_txs;

        // Check that the transaction remains settled and cleared from the timeout map
        assert!(!timed_out_tx_hashes.contains(&blob_tx_hash));
        assert!(state.unsettled_transactions.get(&blob_tx_hash).is_none());
        assert_eq!(timeouts::tests::get(&state.timeouts, &blob_tx_hash), None);
    }

    #[test_log::test(tokio::test)]
    async fn test_tx_on_timeout_settle_next_txs() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let c2 = ContractName::new("c2");
        let register_c1 = make_register_contract_tx(c1.clone());
        let register_c2 = make_register_contract_tx(c2.clone());

        // Add Three transactions - the first blocks the next two, but the next two are ready to settle.
        let blocking_tx = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![new_blob(&c1.0), new_blob(&c2.0)],
        );
        let blocking_tx_hash = blocking_tx.hashed();

        let ready_same_block =
            BlobTransaction::new(Identity::new("test@c1"), vec![new_blob(&c1.0)]);
        let ready_later_block =
            BlobTransaction::new(Identity::new("test@c2"), vec![new_blob(&c2.0)]);
        let ready_same_block_hash = ready_same_block.hashed();
        let ready_later_block_hash = ready_later_block.hashed();
        let hyle_output = make_hyle_output(ready_same_block.clone(), BlobIndex(0));
        let ready_same_block_verified_proof =
            new_proof_tx(&c1, &hyle_output, &ready_same_block_hash);

        let hyle_output = make_hyle_output(ready_later_block.clone(), BlobIndex(0));
        let ready_later_block_verified_proof =
            new_proof_tx(&c2, &hyle_output, &ready_later_block_hash);

        let crafted_block = craft_signed_block(
            104,
            vec![
                register_c1.into(),
                register_c2.into(),
                blocking_tx.into(),
                ready_same_block.into(),
                ready_same_block_verified_proof.into(),
            ],
        );

        state.handle_signed_block(&crafted_block);

        let later_crafted_block = craft_signed_block(
            108,
            vec![
                ready_later_block.into(),
                ready_later_block_verified_proof.into(),
            ],
        );

        state.handle_signed_block(&later_crafted_block);

        // Time out
        let block = state.handle_signed_block(&craft_signed_block(204, vec![]));

        // Only the blocking TX should be timed out
        assert_eq!(block.timed_out_txs, vec![blocking_tx_hash]);

        // The others have been settled
        [ready_same_block_hash, ready_later_block_hash]
            .iter()
            .for_each(|tx_hash| {
                assert!(!block.timed_out_txs.contains(tx_hash));
                assert!(state.unsettled_transactions.get(tx_hash).is_none());
                assert!(block.successful_txs.contains(tx_hash));
            });
    }

    #[test_log::test(tokio::test)]
    async fn test_tx_reset_timeout_on_tx_settlement() {
        // Create four transactions that are inter dependent
        // Tx1 --> Tx2 (ready to be settled)
        //     |-> Tx3 -> Tx4

        // We want to test that when Tx1 times out:
        // - Tx2 gets settled
        // - Tx3's timeout is reset
        // - Tx4 is neither resetted nor timedout.

        // We then want to test that when Tx3 settles:
        // - Tx4's timeout is set

        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let c2 = ContractName::new("c2");
        let register_c1 = make_register_contract_tx(c1.clone());
        let register_c2 = make_register_contract_tx(c2.clone());

        const TIMEOUT_WINDOW: BlockHeight = BlockHeight(100);

        // Add Three transactions - the first blocks the next two, and the next two are NOT ready to settle.
        let tx1 = BlobTransaction::new(
            Identity::new("test@c1"),
            vec![new_blob(&c1.0), new_blob(&c2.0)],
        );
        let tx2 = BlobTransaction::new(Identity::new("test@c1"), vec![new_blob(&c1.0)]);
        let tx3 = BlobTransaction::new(Identity::new("test@c2"), vec![new_blob(&c2.0)]);
        let tx4 = BlobTransaction::new(Identity::new("test2@c2"), vec![new_blob(&c2.0)]);
        let tx1_hash = tx1.hashed();
        let tx2_hash = tx2.hashed();
        let tx3_hash = tx3.hashed();
        let tx4_hash = tx4.hashed();

        let hyle_output = make_hyle_output(tx2.clone(), BlobIndex(0));
        let tx2_verified_proof = new_proof_tx(&c1, &hyle_output, &tx2_hash);
        let hyle_output = make_hyle_output(tx3.clone(), BlobIndex(0));
        let tx3_verified_proof = new_proof_tx(&c2, &hyle_output, &tx3_hash);

        state.handle_signed_block(&craft_signed_block(
            104,
            vec![
                register_c1.into(),
                register_c2.into(),
                tx1.into(),
                tx2.into(),
                tx2_verified_proof.into(),
                tx3.into(),
                tx4.into(),
            ],
        ));

        // Assert timeout only contains tx1
        assert_eq!(
            timeouts::tests::get(&state.timeouts, &tx1_hash),
            Some(104 + TIMEOUT_WINDOW)
        );
        assert_eq!(timeouts::tests::get(&state.timeouts, &tx2_hash), None);
        assert_eq!(timeouts::tests::get(&state.timeouts, &tx3_hash), None);
        assert_eq!(timeouts::tests::get(&state.timeouts, &tx4_hash), None);

        // Time out
        let block = state.handle_signed_block(&craft_signed_block(204, vec![]));

        // Assert that only tx1 has timed out
        assert_eq!(block.timed_out_txs, vec![tx1_hash.clone()]);
        assert_eq!(timeouts::tests::get(&state.timeouts, &tx1_hash), None);

        // Assert that tx2 has settled
        assert_eq!(state.unsettled_transactions.get(&tx2_hash), None);
        assert_eq!(timeouts::tests::get(&state.timeouts, &tx2_hash), None);

        // Assert that tx3 timeout is reset
        assert_eq!(
            timeouts::tests::get(&state.timeouts, &tx3_hash),
            Some(204 + TIMEOUT_WINDOW)
        );

        // Assert that tx4 has no timeout
        assert_eq!(timeouts::tests::get(&state.timeouts, &tx4_hash), None);

        // Tx3 settles
        state.handle_signed_block(&craft_signed_block(250, vec![tx3_verified_proof.into()]));

        // Assert that tx3 has settled.
        assert_eq!(state.unsettled_transactions.get(&tx3_hash), None);
        assert_eq!(timeouts::tests::get(&state.timeouts, &tx1_hash), None);
        assert_eq!(timeouts::tests::get(&state.timeouts, &tx2_hash), None);

        // Assert that tx4 timeout is set with remaining timeout window
        assert_eq!(
            timeouts::tests::get(&state.timeouts, &tx4_hash),
            Some(104 + TIMEOUT_WINDOW)
        );
    }

    #[test_log::test(tokio::test)]
    async fn test_duplicate_tx_timeout() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let register_c1 = make_register_contract_tx(c1.clone());

        // First register the contract
        state.handle_signed_block(&craft_signed_block(1, vec![register_c1.into()]));

        // Create a transaction
        let blob_tx = BlobTransaction::new(Identity::new("test@c1"), vec![new_blob(&c1.0)]);
        let blob_tx_hash = blob_tx.hashed();

        // Submit the same transaction multiple times in different blocks
        state.handle_signed_block(&craft_signed_block(2, vec![blob_tx.clone().into()]));

        // Sanity check for timeout
        assert_eq!(
            timeouts::tests::get(&state.timeouts, &blob_tx_hash),
            Some(BlockHeight(2) + BlockHeight(100))
        );

        state.handle_signed_block(&craft_signed_block(3, vec![blob_tx.clone().into()]));
        let block = state.handle_signed_block(&craft_signed_block(4, vec![blob_tx.clone().into()]));

        assert!(block.failed_txs.is_empty());
        assert!(block.successful_txs.is_empty());

        // Verify only one instance of the transaction is tracked
        assert_eq!(state.unsettled_transactions.len(), 1);
        assert!(state.unsettled_transactions.get(&blob_tx_hash).is_some());

        // Check the timeout is still the same
        assert_eq!(
            timeouts::tests::get(&state.timeouts, &blob_tx_hash),
            Some(BlockHeight(2) + BlockHeight(100)) // Timeout should be based on first appearance
        );

        // Time out the transaction
        let block = state.handle_signed_block(&craft_signed_block(102, vec![]));

        // Verify the transaction was timed out
        assert_eq!(block.timed_out_txs, vec![blob_tx_hash.clone()]);
        assert!(state.unsettled_transactions.get(&blob_tx_hash).is_none());
        assert_eq!(timeouts::tests::get(&state.timeouts, &blob_tx_hash), None);

        // Submit the same transaction again after timeout
        state.handle_signed_block(&craft_signed_block(103, vec![blob_tx.clone().into()]));

        // Verify it's treated as a new transaction
        assert_eq!(state.unsettled_transactions.len(), 1);
        assert!(state.unsettled_transactions.get(&blob_tx_hash).is_some());
        assert_eq!(
            timeouts::tests::get(&state.timeouts, &blob_tx_hash),
            Some(BlockHeight(103) + BlockHeight(100))
        );
    }

    mod contract_registration {
        use std::collections::HashSet;

        use super::*;

        pub fn make_register_tx(
            sender: Identity,
            tld: ContractName,
            name: ContractName,
        ) -> BlobTransaction {
            BlobTransaction::new(
                sender,
                vec![RegisterContractAction {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                    contract_name: name,
                    timeout_window: None,
                }
                .as_blob(tld, None, None)],
            )
        }

        #[test_log::test(tokio::test)]
        async fn test_register_contract_simple_hyle() {
            let mut state = new_node_state().await;

            let register_c1 = make_register_tx("hyle@hyle".into(), "hyle".into(), "c1".into());
            let register_c2 = make_register_tx("hyle@hyle".into(), "hyle".into(), "c2.hyle".into());
            let register_c3 = make_register_tx("hyle@hyle".into(), "hyle".into(), "c3".into());

            let block_1 = craft_signed_block(1, vec![register_c1.clone().into()]);
            state.handle_signed_block(&block_1);

            state.handle_signed_block(&craft_signed_block(
                2,
                vec![register_c2.into(), register_c3.into()],
            ));

            assert_eq!(
                state.contracts.keys().collect::<HashSet<_>>(),
                HashSet::from_iter(vec![
                    &"hyle".into(),
                    &"c1".into(),
                    &"c2.hyle".into(),
                    &"c3".into()
                ])
            );

            let block =
                state.handle_signed_block(&craft_signed_block(3, vec![register_c1.clone().into()]));

            assert_eq!(block.failed_txs, vec![register_c1.hashed()]);
            assert_eq!(state.contracts.len(), 4);
        }

        #[test_log::test(tokio::test)]
        async fn test_register_contract_failure() {
            let mut state = new_node_state().await;

            let register_1 =
                make_register_tx("hyle@hyle".into(), "hyle".into(), "c1.hyle.lol".into());
            let register_2 =
                make_register_tx("other@hyle".into(), "hyle".into(), "c2.hyle.hyle".into());
            let register_3 = make_register_tx("hyle@hyle".into(), "hyle".into(), "c3.other".into());
            let register_4 = make_register_tx("hyle@hyle".into(), "hyle".into(), ".hyle".into());
            let register_5 = BlobTransaction::new(
                "hyle@hyle",
                vec![Blob {
                    contract_name: "hyle".into(),
                    data: BlobData(vec![0, 1, 2, 3]),
                }],
            );
            let register_good =
                make_register_tx("hyle@hyle".into(), "hyle".into(), "c1.hyle".into());

            let signed_block = craft_signed_block(
                1,
                vec![
                    register_1.clone().into(),
                    register_2.clone().into(),
                    register_3.clone().into(),
                    register_4.clone().into(),
                    register_5.clone().into(),
                    register_good.clone().into(),
                ],
            );

            let block = state.handle_signed_block(&signed_block);

            assert_eq!(state.contracts.len(), 2);
            assert_eq!(block.successful_txs, vec![register_good.hashed()]);
            assert_eq!(
                block.failed_txs,
                vec![
                    register_1.hashed(),
                    register_2.hashed(),
                    register_3.hashed(),
                    register_4.hashed(),
                    register_5.hashed(),
                ]
            );
        }

        #[test_log::test(tokio::test)]
        async fn test_register_contract_proof_mismatch() {
            let mut state = new_node_state().await;

            // Create a valid registration transaction
            let register_parent_tx =
                make_register_tx("hyle@hyle".into(), "hyle".into(), "test.hyle".into());
            let register_parent_tx_hash = register_parent_tx.hashed();
            let register_tx = make_register_tx(
                "hyle@test.hyle".into(),
                "test.hyle".into(),
                "sub.test.hyle".into(),
            );
            let tx_hash = register_tx.hashed();

            // Create a proof with mismatched registration effect
            let mut output = make_hyle_output(register_tx.clone(), BlobIndex(0));
            output
                .onchain_effects
                .push(OnchainEffect::RegisterContract(RegisterContractEffect {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_commitment: StateCommitment(vec![9, 9, 9, 9]), // Different state_commitment than in the blob action
                    contract_name: "sub.test.hyle".into(),
                    timeout_window: None,
                }));

            let proof_tx = new_proof_tx(&"test.hyle".into(), &output, &tx_hash);

            // Submit both transactions
            let block = state.handle_signed_block(&craft_signed_block(
                1,
                vec![
                    register_parent_tx.into(),
                    register_tx.into(),
                    proof_tx.into(),
                ],
            ));

            // The transaction should fail because the proof's registration effect doesn't match the blob action
            tracing::warn!("{:?}", state.contracts);
            assert_eq!(state.contracts.len(), 2); // sub.test.hyle shouldn't exist
            assert_eq!(block.successful_txs, vec![register_parent_tx_hash]); // No successful transactions
        }

        #[test_log::test(tokio::test)]
        async fn test_register_contract_composition() {
            let mut state = new_node_state().await;
            let register = make_register_tx("hyle@hyle".into(), "hyle".into(), "hydentity".into());
            let block =
                state.handle_signed_block(&craft_signed_block(1, vec![register.clone().into()]));

            check_block_is_ok(&block);

            assert_eq!(state.contracts.len(), 2);

            let compositing_register_willfail = BlobTransaction::new(
                "test@hydentity",
                vec![
                    RegisterContractAction {
                        verifier: "test".into(),
                        program_id: ProgramId(vec![]),
                        state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                        contract_name: "c1".into(),
                        timeout_window: None,
                    }
                    .as_blob("hyle".into(), None, None),
                    Blob {
                        contract_name: "hydentity".into(),
                        data: BlobData(vec![0, 1, 2, 3]),
                    },
                ],
            );
            // Try to register the same contract validly later.
            // Change identity to change blob tx hash
            let compositing_register_good = BlobTransaction::new(
                "test2@hydentity",
                compositing_register_willfail.blobs.clone(),
            );

            let crafted_block = craft_signed_block(
                102,
                vec![
                    compositing_register_willfail.clone().into(),
                    compositing_register_good.clone().into(),
                ],
            );

            let block = state.handle_signed_block(&crafted_block);
            assert_eq!(state.contracts.len(), 2);

            check_block_is_ok(&block);

            let proof_tx = new_proof_tx(
                &"hyle".into(),
                &make_hyle_output(compositing_register_good.clone(), BlobIndex(1)),
                &compositing_register_good.hashed(),
            );

            let block = state.handle_signed_block(&craft_signed_block(103, vec![proof_tx.into()]));

            check_block_is_ok(&block);

            assert_eq!(state.contracts.len(), 2);

            // Send a third one that will fail early on settlement of the second because duplication
            // (and thus test the early-failure settlement path)

            let third_tx = BlobTransaction::new(
                "test3@hydentity",
                compositing_register_willfail.blobs.clone(),
            );
            let proof_tx = new_proof_tx(
                &"hyle".into(),
                &make_hyle_output(third_tx.clone(), BlobIndex(1)),
                &third_tx.hashed(),
            );

            let block =
                state.handle_signed_block(&craft_signed_block(104, vec![third_tx.clone().into()]));

            check_block_is_ok(&block);

            assert_eq!(state.contracts.len(), 2);

            let block =
                state.handle_signed_block(&craft_signed_block(105, vec![proof_tx.clone().into()]));

            check_block_is_ok(&block);

            let block = state.handle_signed_block(&craft_signed_block(202, vec![]));

            check_block_is_ok(&block);

            assert_eq!(
                block.timed_out_txs,
                vec![compositing_register_willfail.hashed()]
            );
            assert_eq!(state.contracts.len(), 3);
        }

        fn check_block_is_ok(block: &Block) {
            let dp_hashes: Vec<TxHash> = block.dp_parent_hashes.clone().into_keys().collect();

            for tx_hash in block.successful_txs.iter() {
                assert!(dp_hashes.contains(tx_hash));
            }

            for tx_hash in block.failed_txs.iter() {
                assert!(dp_hashes.contains(tx_hash));
            }

            for tx_hash in block.timed_out_txs.iter() {
                assert!(dp_hashes.contains(tx_hash));
            }

            for (tx_hash, _) in block.transactions_events.iter() {
                assert!(dp_hashes.contains(tx_hash));
            }
        }

        pub fn make_delete_tx(
            sender: Identity,
            tld: ContractName,
            contract_name: ContractName,
        ) -> BlobTransaction {
            BlobTransaction::new(
                sender,
                vec![DeleteContractAction { contract_name }.as_blob(tld, None, None)],
            )
        }

        #[test_log::test(tokio::test)]
        async fn test_register_contract_and_delete_hyle() {
            let mut state = new_node_state().await;

            let register_c1 = make_register_tx("hyle@hyle".into(), "hyle".into(), "c1".into());
            let register_c2 = make_register_tx("hyle@hyle".into(), "hyle".into(), "c2.hyle".into());
            // This technically doesn't matter as it's actually the proof that does the work
            let register_sub_c2 = make_register_tx(
                "toto@c2.hyle".into(),
                "c2.hyle".into(),
                "sub.c2.hyle".into(),
            );

            let mut output = make_hyle_output(register_sub_c2.clone(), BlobIndex(0));
            output
                .onchain_effects
                .push(OnchainEffect::RegisterContract(RegisterContractEffect {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                    contract_name: "sub.c2.hyle".into(),
                    timeout_window: None,
                }));
            let sub_c2_proof = new_proof_tx(&"c2.hyle".into(), &output, &register_sub_c2.hashed());

            let block = state.handle_signed_block(&craft_signed_block(
                1,
                vec![
                    register_c1.into(),
                    register_c2.into(),
                    register_sub_c2.into(),
                    sub_c2_proof.into(),
                ],
            ));
            assert_eq!(
                block
                    .registered_contracts
                    .iter()
                    .map(|(_, rce)| rce.contract_name.0.clone())
                    .collect::<Vec<_>>(),
                vec!["c1", "c2.hyle", "sub.c2.hyle"]
            );
            assert_eq!(state.contracts.len(), 4);

            // Now delete them.
            let self_delete_tx = make_delete_tx("c1@c1".into(), "c1".into(), "c1".into());
            let delete_sub_tx = make_delete_tx(
                "toto@c2.hyle".into(),
                "c2.hyle".into(),
                "sub.c2.hyle".into(),
            );
            let delete_tx = make_delete_tx("hyle@hyle".into(), "hyle".into(), "c2.hyle".into());

            let mut output = make_hyle_output(self_delete_tx.clone(), BlobIndex(0));
            output
                .onchain_effects
                .push(OnchainEffect::DeleteContract("c1".into()));
            let delete_self_proof =
                new_proof_tx(&"c1.hyle".into(), &output, &self_delete_tx.hashed());

            let mut output =
                make_hyle_output_with_state(delete_sub_tx.clone(), BlobIndex(0), &[4, 5, 6], &[1]);
            output
                .onchain_effects
                .push(OnchainEffect::DeleteContract("sub.c2.hyle".into()));
            let delete_sub_proof =
                new_proof_tx(&"c2.hyle".into(), &output, &delete_sub_tx.hashed());

            let block = state.handle_signed_block(&craft_signed_block(
                2,
                vec![
                    self_delete_tx.into(),
                    delete_sub_tx.into(),
                    delete_self_proof.into(),
                    delete_sub_proof.into(),
                    delete_tx.into(),
                ],
            ));

            assert_eq!(
                block
                    .deleted_contracts
                    .iter()
                    .map(|(_, dce)| dce.0.clone())
                    .collect::<Vec<_>>(),
                vec!["c1", "sub.c2.hyle", "c2.hyle"]
            );
            assert_eq!(state.contracts.len(), 1);
        }

        #[test_log::test(tokio::test)]
        async fn test_hyle_sub_delete() {
            let mut state = new_node_state().await;

            let register_c2 = make_register_tx("hyle@hyle".into(), "hyle".into(), "c2.hyle".into());
            // This technically doesn't matter as it's actually the proof that does the work
            let register_sub_c2 = make_register_tx(
                "toto@c2.hyle".into(),
                "c2.hyle".into(),
                "sub.c2.hyle".into(),
            );

            let mut output = make_hyle_output(register_sub_c2.clone(), BlobIndex(0));
            output
                .onchain_effects
                .push(OnchainEffect::RegisterContract(RegisterContractEffect {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                    contract_name: "sub.c2.hyle".into(),
                    timeout_window: None,
                }));
            let sub_c2_proof = new_proof_tx(&"c2.hyle".into(), &output, &register_sub_c2.hashed());

            state.handle_signed_block(&craft_signed_block(
                1,
                vec![
                    register_c2.into(),
                    register_sub_c2.into(),
                    sub_c2_proof.into(),
                ],
            ));
            assert_eq!(state.contracts.len(), 3);

            // Now delete the intermediate contract first, then delete the sub-contract via hyle
            let delete_tx = make_delete_tx("hyle@hyle".into(), "hyle".into(), "c2.hyle".into());
            let delete_sub_tx =
                make_delete_tx("hyle@hyle".into(), "hyle".into(), "sub.c2.hyle".into());

            let block = state.handle_signed_block(&craft_signed_block(
                2,
                vec![delete_tx.into(), delete_sub_tx.into()],
            ));

            assert_eq!(
                block
                    .deleted_contracts
                    .iter()
                    .map(|(_, dce)| dce.0.clone())
                    .collect::<Vec<_>>(),
                vec!["c2.hyle", "sub.c2.hyle"]
            );
            assert_eq!(state.contracts.len(), 1);
        }

        #[test_log::test(tokio::test)]
        async fn test_register_update_delete_combinations_hyle() {
            let register_tx = make_register_tx("hyle@hyle".into(), "hyle".into(), "c.hyle".into());
            let delete_tx = make_delete_tx("hyle@hyle".into(), "hyle".into(), "c.hyle".into());
            let delete_self_tx =
                make_delete_tx("hyle@c.hyle".into(), "c.hyle".into(), "c.hyle".into());
            let update_tx =
                make_register_tx("test@c.hyle".into(), "c.hyle".into(), "c.hyle".into());

            let mut output = make_hyle_output(update_tx.clone(), BlobIndex(0));
            output
                .onchain_effects
                .push(OnchainEffect::RegisterContract(RegisterContractEffect {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                    contract_name: "c.hyle".into(),
                    timeout_window: None,
                }));
            let proof_update = new_proof_tx(&"c.hyle".into(), &output, &update_tx.hashed());

            let mut output =
                make_hyle_output_with_state(delete_self_tx.clone(), BlobIndex(0), &[4, 5, 6], &[1]);
            output
                .onchain_effects
                .push(OnchainEffect::DeleteContract("c.hyle".into()));
            let proof_delete = new_proof_tx(&"c.hyle".into(), &output, &delete_self_tx.hashed());

            async fn test_combination(
                proofs: Option<&[&VerifiedProofTransaction]>,
                txs: &[&BlobTransaction],
                expected_ct: usize,
                expected_txs: usize,
            ) {
                let mut state = new_node_state().await;
                let mut txs = txs
                    .iter()
                    .map(|tx| (*tx).clone().into())
                    .collect::<Vec<_>>();
                if let Some(proofs) = proofs {
                    txs.extend(proofs.iter().map(|p| (*p).clone().into()));
                }
                let block = state.handle_signed_block(&craft_signed_block(1, txs));

                assert_eq!(state.contracts.len(), expected_ct);
                assert_eq!(block.successful_txs.len(), expected_txs);
                info!("done");
            }

            // Test all combinations
            test_combination(None, &[&register_tx], 2, 1).await;
            test_combination(None, &[&delete_tx], 1, 0).await;
            test_combination(None, &[&register_tx, &delete_tx], 1, 2).await;
            test_combination(Some(&[&proof_update]), &[&register_tx, &update_tx], 2, 2).await;
            test_combination(
                Some(&[&proof_update]),
                &[&register_tx, &update_tx, &delete_tx],
                1,
                3,
            )
            .await;
            test_combination(
                Some(&[&proof_update, &proof_delete]),
                &[&register_tx, &update_tx, &delete_self_tx],
                1,
                3,
            )
            .await;
        }
        #[test_log::test(tokio::test)]
        async fn test_custom_timeout_then_upgrade_with_none() {
            let mut state = new_node_state().await;

            let c1 = ContractName::new("c1");
            let register_c1 = make_register_contract_tx(c1.clone());

            // Register the contract
            state.handle_signed_block(&craft_signed_block(1, vec![register_c1.into()]));

            let custom_timeout = BlockHeight(150);

            // Upgrade the contract with a custom timeout
            {
                let action = RegisterContractAction {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_commitment: StateCommitment(vec![0, 1, 2, 3]),
                    contract_name: c1.clone(),
                    timeout_window: Some(TimeoutWindow::Timeout(custom_timeout)),
                };
                let upgrade_with_timeout = BlobTransaction::new(
                    Identity::new("test@c1"),
                    vec![action.clone().as_blob("c1".into(), None, None)],
                );

                let upgrade_with_timeout_hash = upgrade_with_timeout.hashed();
                state.handle_signed_block(&craft_signed_block(
                    2,
                    vec![upgrade_with_timeout.clone().into()],
                ));

                // Verify the timeout is set correctly - this is the old timeout
                assert_eq!(
                    timeouts::tests::get(&state.timeouts, &upgrade_with_timeout_hash),
                    Some(BlockHeight(2) + BlockHeight(100))
                );

                // Settle it
                let mut hyle_output = make_hyle_output(upgrade_with_timeout, BlobIndex(0));
                hyle_output
                    .onchain_effects
                    .push(OnchainEffect::RegisterContract(action));
                let upgrade_with_timeout_proof =
                    new_proof_tx(&c1, &hyle_output, &upgrade_with_timeout_hash);
                state.handle_signed_block(&craft_signed_block(
                    3,
                    vec![upgrade_with_timeout_proof.into()],
                ));
            }

            // Upgrade the contract again with a None timeout
            {
                let action = RegisterContractAction {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_commitment: StateCommitment(vec![4, 5, 6]),
                    contract_name: c1.clone(),
                    timeout_window: None,
                };
                let upgrade_with_none = BlobTransaction::new(
                    Identity::new("test@c1"),
                    vec![action.clone().as_blob("c1".into(), None, None)],
                );

                let upgrade_with_none_hash = upgrade_with_none.hashed();
                state.handle_signed_block(&craft_signed_block(
                    4,
                    vec![upgrade_with_none.clone().into()],
                ));

                // Verify the timeout is the custom timeout
                assert_eq!(
                    timeouts::tests::get(&state.timeouts, &upgrade_with_none_hash),
                    Some(BlockHeight(4) + custom_timeout)
                );
                // Settle it
                let mut hyle_output = make_hyle_output_with_state(
                    upgrade_with_none,
                    BlobIndex(0),
                    &[4, 5, 6],
                    &[4, 5, 6],
                );
                hyle_output
                    .onchain_effects
                    .push(OnchainEffect::RegisterContract(action));
                let upgrade_with_none_proof =
                    new_proof_tx(&c1, &hyle_output, &upgrade_with_none_hash);
                state.handle_signed_block(&craft_signed_block(
                    5,
                    vec![upgrade_with_none_proof.into()],
                ));
            }

            // Upgrade the contract again with another custom timeout
            let another_custom_timeout = BlockHeight(200);
            {
                let action = RegisterContractAction {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_commitment: StateCommitment(vec![4, 5, 6]),
                    contract_name: c1.clone(),
                    timeout_window: Some(TimeoutWindow::Timeout(another_custom_timeout)),
                };
                let upgrade_with_another_timeout = BlobTransaction::new(
                    Identity::new("test@c1"),
                    vec![action.clone().as_blob("c1".into(), None, None)],
                );

                let upgrade_with_another_timeout_hash = upgrade_with_another_timeout.hashed();
                state.handle_signed_block(&craft_signed_block(
                    6,
                    vec![upgrade_with_another_timeout.clone().into()],
                ));

                // Verify the timeout is still the OG custom timeout
                assert_eq!(
                    timeouts::tests::get(&state.timeouts, &upgrade_with_another_timeout_hash),
                    Some(BlockHeight(6) + custom_timeout)
                );
                // Settle it
                let mut hyle_output = make_hyle_output_with_state(
                    upgrade_with_another_timeout,
                    BlobIndex(0),
                    &[4, 5, 6],
                    &[4, 5, 6],
                );
                hyle_output
                    .onchain_effects
                    .push(OnchainEffect::RegisterContract(action));
                let upgrade_with_another_timeout_proof =
                    new_proof_tx(&c1, &hyle_output, &upgrade_with_another_timeout_hash);
                state.handle_signed_block(&craft_signed_block(
                    7,
                    vec![upgrade_with_another_timeout_proof.into()],
                ));
            }

            // Send a final transaction with no timeout and Check it uses the new timeout
            let final_tx = BlobTransaction::new(
                Identity::new("test@c1"),
                vec![Blob {
                    contract_name: c1.clone(),
                    data: BlobData(vec![0, 1, 2, 3]),
                }],
            );

            let final_tx_hash = final_tx.hashed();
            state.handle_signed_block(&craft_signed_block(8, vec![final_tx.into()]));

            // Verify the timeout remains the same as the last custom timeout
            assert_eq!(
                timeouts::tests::get(&state.timeouts, &final_tx_hash),
                Some(BlockHeight(8) + another_custom_timeout)
            );
        }
    }
}
