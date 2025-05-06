//! State required for participation in consensus by the node.

use anyhow::{bail, Context, Error, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use contract_registration::validate_contract_registration_metadata;
use contract_registration::{validate_contract_name_registration, validate_state_commitment_size};
use hyle_tld::handle_blob_for_hyle_tld;
use metrics::NodeStateMetrics;
use ordered_tx_map::OrderedTxMap;
use sdk::verifiers::NativeVerifiers;
use sdk::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use timeouts::Timeouts;
use tracing::{debug, error, info, trace};

mod api;
pub mod contract_registration;
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
    pub fn handle_signed_block(&mut self, signed_block: &SignedBlock) -> Result<Block> {
        let next_block = self.current_height + 1 == signed_block.height();
        let initial_block = self.current_height.0 == 0 && signed_block.height().0 == 0;
        if !next_block && !initial_block {
            bail!(
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
                            debug!(
                                "Blob transaction: {:?} is already in the unsettled map, ignoring.",
                                tx_id
                            );
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

        Ok(block_under_construction)
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
                    BlobIndex(i),
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
    mod contract_registration_tests;
    mod node_state_tests;

    use super::*;
    use hyle_net::clock::TimestampMsClock;

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

    impl NodeState {
        // Convenience method to handle a signed block in tests.
        pub fn force_handle_block(&mut self, block: &SignedBlock) -> Block {
            if block.consensus_proposal.slot <= self.current_height.0
                || block.consensus_proposal.slot == 0
            {
                panic!("Invalid block height");
            }
            self.current_height = BlockHeight(block.consensus_proposal.slot - 1);
            self.handle_signed_block(block).unwrap()
        }

        pub fn craft_block_and_handle(&mut self, height: u64, txs: Vec<Transaction>) -> Block {
            let block = craft_signed_block(height, txs);
            self.force_handle_block(&block)
        }

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
}
