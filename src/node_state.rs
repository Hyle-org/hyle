//! State required for participation in consensus by the node.

use crate::mempool::verifiers;
use crate::model::verifiers::NativeVerifiers;
use crate::model::*;
use anyhow::{bail, Error, Result};
use bincode::{Decode, Encode};
use contract_registration::validate_contract_registration;
use hyle_contract_sdk::{utils::parse_structured_blob, BlobIndex, HyleOutput, TxHash};
use ordered_tx_map::OrderedTxMap;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};
use timeouts::Timeouts;
use tracing::{debug, error, info, trace};

mod api;
pub mod module;
mod ordered_tx_map;
mod timeouts;

pub struct SettledTxOutput {
    // Original blob transaction, now settled.
    pub tx: UnsettledBlobTransaction,
    /// This is the index of the blob proof output used in the blob settlement, for each blob.
    pub blob_proof_output_indices: Vec<usize>,
    /// New data for contracts modified by the settled TX.
    pub updated_contracts: BTreeMap<ContractName, Contract>,
    /// Whether the transaction is settled as a success or a failure.
    pub success: bool,
}

/// NodeState manages the flattened, up-to-date state of the chain.
/// It processes raw transactions and outputs more structured data for indexers.
/// See also: NodeStateModule for the actual module implementation.
#[derive(Encode, Decode, Debug, Clone)]
pub struct NodeState {
    timeouts: Timeouts,
    current_height: BlockHeight,
    // This field is public for testing purposes
    pub contracts: HashMap<ContractName, Contract>,
    unsettled_transactions: OrderedTxMap,
}

// TODO: we should register the 'hyle' TLD in the genesis block.
impl Default for NodeState {
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
                state: StateDigest(vec![0]),
                verifier: Verifier("hyle".to_owned()),
            },
        );
        ret
    }
}

impl NodeState {
    pub fn handle_signed_block(&mut self, signed_block: &SignedBlock) -> Block {
        self.current_height = signed_block.height();

        let mut block_under_construction = Block {
            parent_hash: signed_block.parent_hash().clone(),
            hash: signed_block.hash(),
            block_height: signed_block.height(),
            block_timestamp: signed_block.consensus_proposal.timestamp,
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
            updated_states: BTreeMap::new(),
        };

        // We'll need to remember some data to validate transactions proofs.
        let tx_context = Arc::new(TxContext {
            block_hash: block_under_construction.hash.clone(),
            block_height: block_under_construction.block_height,
            timestamp: signed_block.consensus_proposal.timestamp.into(),
            chain_id: HYLE_TESTNET_CHAIN_ID,
        });

        self.clear_timeouts(&mut block_under_construction);

        let txs = signed_block.txs();
        // Handle all transactions
        for tx in txs.iter() {
            match &tx.transaction_data {
                TransactionData::Blob(blob_transaction) => {
                    match self.handle_blob_tx(blob_transaction, tx_context.clone()) {
                        Ok(Some(tx_hash)) => {
                            let mut blob_tx_to_try_and_settle = BTreeSet::new();
                            blob_tx_to_try_and_settle.insert(tx_hash);
                            // In case of a BlobTransaction with only native verifies, we need to trigger the
                            // settlement here as we will never get a ProofTransaction
                            self.settle_txs_until_done(
                                &mut block_under_construction,
                                blob_tx_to_try_and_settle,
                            );
                        }
                        Ok(None) => {}
                        Err(e) => {
                            error!("Failed to handle blob transaction: {:?}", e);
                            block_under_construction.failed_txs.push(tx.hash());
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
                    let blob_tx_to_try_and_settle = proof_tx
                        .proven_blobs
                        .iter()
                        .filter_map(|blob_proof_data| {
                            match self.handle_blob_proof(
                                proof_tx.hash(),
                                &mut block_under_construction.blob_proof_outputs,
                                blob_proof_data,
                            ) {
                                Ok(maybe_tx_hash) => maybe_tx_hash,
                                Err(err) => {
                                    info!(
                                        "Failed to handle blob #{} in verified proof transaction {:?}: {err}",
                                        blob_proof_data.hyle_output.index, proof_tx.hash(),
                                    );
                                    None
                                }
                            }
                        })
                        .collect::<BTreeSet<_>>();
                    // Then try to settle transactions when we can.
                    self.settle_txs_until_done(
                        &mut block_under_construction,
                        blob_tx_to_try_and_settle,
                    );
                }
            }
        }
        block_under_construction.txs = txs;
        block_under_construction
    }

    pub fn handle_register_contract_effect(&mut self, tx: &RegisterContractEffect) {
        info!("📝 Registering contract {}", tx.contract_name);
        self.contracts.insert(
            tx.contract_name.clone(),
            Contract {
                name: tx.contract_name.clone(),
                program_id: tx.program_id.clone(),
                state: tx.state_digest.clone(),
                verifier: tx.verifier.clone(),
            },
        );
    }

    /// Returns a TxHash only if the blob transaction calls only native verifiers and thus can be
    /// settled directly (or in the special case of the 'hyle' TLD contract)
    fn handle_blob_tx(
        &mut self,
        tx: &BlobTransaction,
        tx_context: Arc<TxContext>,
    ) -> Result<Option<TxHash>, Error> {
        debug!("Handle blob tx: {:?} (hash: {})", tx, tx.hash());

        tx.validate_identity()?;

        if tx.blobs.is_empty() {
            bail!("Blob Transaction must have at least one blob");
        }

        let (blob_tx_hash, blobs_hash) = (tx.hash(), tx.blobs_hash());

        let mut should_try_and_settle = true;

        let blobs: Vec<UnsettledBlobMetadata> = tx
            .blobs
            .iter()
            .enumerate()
            .map(|(index, blob)| {
                if let Some(Ok(verifier)) = self
                    .contracts
                    .get(&blob.contract_name)
                    .map(|b| TryInto::<NativeVerifiers>::try_into(&b.verifier))
                {
                    let hyle_output = verifiers::verify_native(
                        blob_tx_hash.clone(),
                        BlobIndex(index),
                        &tx.blobs,
                        verifier,
                    );
                    return UnsettledBlobMetadata {
                        blob: blob.clone(),
                        possible_proofs: vec![(verifier.into(), hyle_output)],
                    };
                } else if blob.contract_name.0 == "hyle" {
                    // Special case for 'hyle' - we generate a fake proof like for native verifiers
                    // but this proof will need to be checked at settling time as it's stateful.
                    if let Ok(reg) =
                        StructuredBlobData::<RegisterContractAction>::try_from(blob.data.clone())
                    {
                        #[allow(clippy::expect_used, reason = "we don't handle oom yet")]
                        let synthetic_output = HyleOutput {
                            success: true,
                            registered_contracts: vec![RegisterContractEffect {
                                contract_name: reg.parameters.contract_name,
                                verifier: reg.parameters.verifier,
                                program_id: reg.parameters.program_id,
                                state_digest: reg.parameters.state_digest,
                            }],
                            ..HyleOutput::default()
                        };
                        return UnsettledBlobMetadata {
                            blob: blob.clone(),
                            possible_proofs: vec![(ProgramId(vec![]), synthetic_output)],
                        };
                    }
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
        should_try_and_settle = self.unsettled_transactions.add(UnsettledBlobTransaction {
            identity: tx.identity.clone(),
            hash: blob_tx_hash.clone(),
            tx_context,
            blobs_hash,
            blobs,
        }) && should_try_and_settle;

        // Update timeouts
        self.timeouts
            .set(blob_tx_hash.clone(), self.current_height + 100); // TODO: Timeout after 100 blocks, make it configurable !

        if should_try_and_settle {
            Ok(Some(blob_tx_hash))
        } else {
            Ok(None)
        }
    }

    fn handle_blob_proof(
        &mut self,
        proof_tx_hash: TxHash,
        blob_proof_outputs: &mut Vec<HandledBlobProofOutput>,
        blob_proof_data: &BlobProofOutput,
    ) -> Result<Option<TxHash>, Error> {
        // Find the blob being proven and whether we should try to settle the TX.
        let (unsettled_tx, should_settle_tx) = match self
            .unsettled_transactions
            .get_for_settlement(&blob_proof_data.blob_tx_hash)
        {
            Some(a) => a,
            _ => {
                bail!("BlobTx {} not found", blob_proof_data.blob_tx_hash);
            }
        };

        // TODO: add diverse verifications ? (without the inital state checks!).
        // TODO: success to false is valid outcome and can be settled.
        if let Err(e) = Self::verify_hyle_output(unsettled_tx, &blob_proof_data.hyle_output) {
            bail!("Failed to validate blob proof: {:?}", e);
        }

        let Some(blob) = unsettled_tx
            .blobs
            .get_mut(blob_proof_data.hyle_output.index.0)
        else {
            bail!(
                "blob at index {} not found in blob TX {}",
                blob_proof_data.hyle_output.index.0,
                blob_proof_data.blob_tx_hash
            );
        };

        // If we arrived here, HyleOutput provided is OK and can now be saved
        debug!(
            "Saving a hyle_output for BlobTx {} index {}",
            blob_proof_data.hyle_output.tx_hash.0, blob_proof_data.hyle_output.index
        );

        blob.possible_proofs.push((
            blob_proof_data.program_id.clone(),
            blob_proof_data.hyle_output.clone(),
        ));

        let unsettled_tx_hash = unsettled_tx.hash.clone();

        blob_proof_outputs.push(HandledBlobProofOutput {
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

    fn settle_txs_until_done(
        &mut self,
        block_under_construction: &mut Block,
        mut blob_tx_to_try_and_settle: BTreeSet<TxHash>,
    ) {
        loop {
            // TODO: investigate most performant order;
            let Some(bth) = blob_tx_to_try_and_settle.pop_first() else {
                break;
            };

            match self.try_to_settle_blob_tx(&bth) {
                Ok(SettledTxOutput {
                    tx: settled_tx,
                    blob_proof_output_indices,
                    updated_contracts: tx_updated_contracts,
                    success,
                }) => {
                    // Settle the TX and add any new TXs to try and settle next.
                    blob_tx_to_try_and_settle.append(&mut self.on_settled_blob_tx(
                        block_under_construction,
                        bth,
                        settled_tx,
                        blob_proof_output_indices,
                        tx_updated_contracts,
                        success,
                    ));
                }
                Err(e) => debug!("Tx {:?} not ready to settle: {:?}", &bth, e),
            }
        }
    }

    fn try_to_settle_blob_tx(
        &mut self,
        unsettled_tx_hash: &TxHash,
    ) -> Result<SettledTxOutput, Error> {
        trace!("Trying to settle blob tx: {:?}", unsettled_tx_hash);

        let unsettled_tx =
            self.unsettled_transactions
                .get(unsettled_tx_hash)
                .ok_or(anyhow::anyhow!(
                    "Unsettled transaction not found in the state: {:?}",
                    unsettled_tx_hash
                ))?;

        // Sanity check: if some of the blob contracts are not registered, we can't proceed
        if !unsettled_tx.blobs.iter().all(|blob_metadata| {
            self.contracts
                .contains_key(&blob_metadata.blob.contract_name)
        }) {
            bail!("Cannot settle TX: some blob contracts are not registered");
        }

        let updated_contracts = BTreeMap::new();

        let (updated_contracts, blob_proof_output_indices, success) =
            match Self::settle_blobs_recursively(
                &self.contracts,
                updated_contracts,
                unsettled_tx.blobs.iter(),
                vec![],
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
            blob_proof_output_indices,
            updated_contracts,
            success,
        })
    }

    fn settle_blobs_recursively<'a>(
        contracts: &HashMap<ContractName, Contract>,
        current_contracts: BTreeMap<ContractName, Contract>,
        mut blob_iter: impl Iterator<Item = &'a UnsettledBlobMetadata> + Clone,
        mut blob_proof_output_indices: Vec<usize>,
    ) -> Option<(BTreeMap<ContractName, Contract>, Vec<usize>, bool)> {
        // Recursion end-case: we succesfully settled all prior blobs, so success.
        let Some(current_blob) = blob_iter.next() else {
            return Some((current_contracts, blob_proof_output_indices, true));
        };

        let contract_name = &current_blob.blob.contract_name;
        #[allow(
            clippy::unwrap_used,
            reason = "all contract names are validated to exist above"
        )]
        let known_contract_state = current_contracts
            .get(contract_name)
            .unwrap_or(contracts.get(contract_name).unwrap());
        // Super special case - the hyle contract has "synthetic proofs".
        // We need to check the current state of 'current_contracts' to check validity,
        // so we really can't do this before we've settled the earlier blobs.
        if contract_name.0 == "hyle" {
            // Have to push something here or the rest of the logic breaks
            blob_proof_output_indices.push(0);
            return match Self::handle_blob_for_hyle_tld(
                contracts,
                &current_contracts,
                &current_blob.blob,
            ) {
                Ok(contract) => {
                    let mut us = current_contracts.clone();
                    us.insert(contract.name.clone(), contract);
                    Self::settle_blobs_recursively(
                        contracts,
                        us,
                        blob_iter.clone(),
                        blob_proof_output_indices.clone(),
                    )
                }
                Err(err) => {
                    // We have a valid proof of failure, we short-circuit.
                    debug!("Could not settle blob proof output for 'hyle': {:?}", err);
                    Some((current_contracts, blob_proof_output_indices, false))
                }
            };
        }
        // Regular case: go through each proof for this blob. If they settle, carry on recursively.
        for (i, proof_metadata) in current_blob.possible_proofs.iter().enumerate() {
            if !Self::validate_proof_metadata(proof_metadata, known_contract_state) {
                // Not a valid proof, log it and try the next one.
                debug!(
                "Could not settle blob proof output #{} for contract '{}'. Expected initial state: {:?}, got: {:?}, expected program ID: {:?}, got: {:?}",
                i,
                contract_name,
                known_contract_state.state,
                proof_metadata.1.initial_state,
                known_contract_state.program_id,
                proof_metadata.0
            );
                continue;
            }
            if !proof_metadata.1.success {
                // We have a valid proof of failure, we short-circuit.
                debug!("Proven failure for blob {}", i);
                return Some((current_contracts, blob_proof_output_indices, false));
            }
            // TODO: ideally make this CoW
            let mut us = current_contracts.clone();
            us.insert(
                contract_name.clone(),
                Contract {
                    name: contract_name.clone(),
                    program_id: proof_metadata.0.clone(),
                    state: proof_metadata.1.next_state.clone(),
                    verifier: known_contract_state.verifier.clone(),
                },
            );
            blob_proof_output_indices.push(i);
            match Self::settle_blobs_recursively(
                contracts,
                us,
                blob_iter.clone(),
                blob_proof_output_indices.clone(),
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
        blob_proof_output_indices: Vec<usize>,
        tx_updated_contracts: BTreeMap<ContractName, Contract>,
        success: bool,
    ) -> BTreeSet<TxHash> {
        // Transaction was settled, update our state.
        if success {
            info!("✨ Settled tx {}", &bth);
        } else {
            info!("⛈️ Settled tx {} has failed", &bth);
        }

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

        // Handle side-effect of each blobs on the node.
        if !success {
            block_under_construction.failed_txs.push(bth);
        } else {
            // Take note of staking and contract registration
            for (i, mut blob_metadata) in settled_tx.blobs.into_iter().enumerate() {
                #[allow(clippy::indexing_slicing, reason = "all exist by construction")]
                let settled_proof = blob_metadata
                    .possible_proofs
                    .remove(blob_proof_output_indices[i]);

                for rce in settled_proof.1.registered_contracts {
                    self.handle_register_contract_effect(&rce);
                    block_under_construction
                        .registered_contracts
                        .push((bth.clone(), rce));
                }

                let blob = blob_metadata.blob;
                // Keep track of all stakers
                if blob.contract_name.0 == "staking" {
                    if let Some(structured_blob) = parse_structured_blob(&[blob], &BlobIndex(0)) {
                        let staking_action: StakingAction = structured_blob.data.parameters;

                        block_under_construction
                            .staking_actions
                            .push((settled_tx.identity.clone(), staking_action));
                    } else {
                        error!("Failed to parse StakingAction");
                    }
                }
            }

            // Keep track of settled txs
            block_under_construction.successful_txs.push(bth);

            // Update contract states
            // Have to put the clippy here because it's experimental on expressions
            #[allow(
                clippy::unwrap_used,
                reason = "all contract names are validated to exist above"
            )]
            for (contract_name, next_state) in tx_updated_contracts.iter() {
                debug!(
                    "Update {} contract state: {:?}",
                    contract_name, next_state.state
                );
                self.contracts.get_mut(contract_name).unwrap().state = next_state.state.clone(); // unwrap, see above ^

                // TODO: would be nice to have a drain-like API here.
                block_under_construction
                    .updated_states
                    .insert(contract_name.clone(), next_state.state.clone());
            }
        }

        next_txs_to_try_and_settle
    }

    fn handle_blob_for_hyle_tld(
        contracts: &HashMap<ContractName, Contract>,
        current_contracts: &BTreeMap<ContractName, Contract>,
        current_blob: &Blob,
    ) -> Result<Contract> {
        let Ok(reg) =
            StructuredBlobData::<RegisterContractAction>::try_from(current_blob.data.clone())
        else {
            bail!("Blob is  not a RegisterContractAction");
        };

        // Check name, it's either a direct subdomain or a TLD
        validate_contract_registration(&"hyle".into(), &reg.parameters.contract_name)?;

        // Check it's not already registered
        if contracts.contains_key(&reg.parameters.contract_name)
            || current_contracts.contains_key(&reg.parameters.contract_name)
        {
            bail!(
                "Contract {} is already registered",
                reg.parameters.contract_name.0
            );
        }

        Ok(Contract {
            name: reg.parameters.contract_name.clone(),
            program_id: reg.parameters.program_id.clone(),
            state: reg.parameters.state_digest.clone(),
            verifier: reg.parameters.verifier.clone(),
        })
    }

    // Assumes verify_hyle_output was already called
    fn validate_proof_metadata(
        proof_metadata: &(ProgramId, HyleOutput),
        contract: &Contract,
    ) -> bool {
        if proof_metadata.1.registered_contracts.iter().any(|effect| {
            validate_contract_registration(&contract.name, &effect.contract_name).is_err()
        }) {
            return false;
        }

        proof_metadata.1.initial_state == contract.state && proof_metadata.0 == contract.program_id
    }

    fn verify_hyle_output(
        unsettled_tx: &UnsettledBlobTransaction,
        hyle_output: &HyleOutput,
    ) -> Result<(), Error> {
        // Identity verification
        if unsettled_tx.identity != hyle_output.identity {
            bail!(
                "Proof identity '{:?}' does not correspond to BlobTx identity '{:?}'.",
                hyle_output.identity,
                unsettled_tx.identity
            )
        }

        // Verify Tx hash matches
        if hyle_output.tx_hash != unsettled_tx.hash {
            bail!(
                "Proof tx hash '{:?}' does not correspond to BlobTx hash '{:?}'.",
                hyle_output.tx_hash,
                unsettled_tx.hash
            )
        }

        if let Some(tx_ctx) = &hyle_output.tx_ctx {
            if *tx_ctx != *unsettled_tx.tx_context {
                bail!(
                    "Proof tx context '{:?}' does not correspond to BlobTx tx context '{:?}'.",
                    tx_ctx,
                    unsettled_tx.tx_context
                )
            }
        }

        // blob_hash verification
        let extracted_blobs_hash = BlobsHash::from_concatenated(&hyle_output.blobs);
        if extracted_blobs_hash != unsettled_tx.blobs_hash {
            bail!(
                "Proof blobs hash '{:?}' do not correspond to BlobTx blobs hash '{:?}'.",
                extracted_blobs_hash,
                unsettled_tx.blobs_hash
            )
        }

        Ok(())
    }

    fn clear_timeouts(&mut self, block_under_construction: &mut Block) {
        let mut txs_at_timeout = self.timeouts.drop(&block_under_construction.block_height);
        txs_at_timeout.retain(|tx| {
            if let Some(mut tx) = self.unsettled_transactions.remove(tx) {
                debug!("Blob tx timeout: {}", &tx.hash);

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
                self.settle_txs_until_done(block_under_construction, blob_tx_to_try_and_settle);

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
    use hyle_contract_sdk::flatten_blobs;
    use utils::get_current_timestamp_ms;

    async fn new_node_state() -> NodeState {
        NodeState::default()
    }

    fn new_blob(contract: &str) -> Blob {
        Blob {
            contract_name: ContractName::new(contract),
            data: BlobData(vec![0, 1, 2, 3]),
        }
    }

    pub fn make_register_contract_tx(name: ContractName) -> BlobTransaction {
        BlobTransaction {
            identity: "hyle.hyle".into(),
            blobs: vec![RegisterContractAction {
                verifier: "test".into(),
                program_id: ProgramId(vec![]),
                state_digest: StateDigest(vec![0, 1, 2, 3]),
                contract_name: name,
            }
            .as_blob("hyle".into(), None, None)],
        }
    }

    fn make_register_contract_effect(contract_name: ContractName) -> RegisterContractEffect {
        RegisterContractEffect {
            verifier: "test".into(),
            program_id: ProgramId(vec![]),
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name,
        }
    }

    pub fn new_proof_tx(
        contract: &ContractName,
        hyle_output: &HyleOutput,
        blob_tx_hash: &TxHash,
    ) -> VerifiedProofTransaction {
        let proof = ProofTransaction {
            contract_name: contract.clone(),
            proof: ProofData(
                bincode::encode_to_vec(vec![hyle_output.clone()], bincode::config::standard())
                    .unwrap(),
            ),
        };
        VerifiedProofTransaction {
            contract_name: contract.clone(),
            proven_blobs: vec![BlobProofOutput {
                hyle_output: hyle_output.clone(),
                program_id: ProgramId(vec![]),
                blob_tx_hash: blob_tx_hash.clone(),
                original_proof_hash: proof.proof.hash(),
            }],
            proof_hash: proof.proof.hash(),
            proof: Some(proof.proof),
            is_recursive: false,
        }
    }

    pub fn make_hyle_output(blob_tx: BlobTransaction, blob_index: BlobIndex) -> HyleOutput {
        HyleOutput {
            version: 1,
            identity: blob_tx.identity.clone(),
            index: blob_index,
            blobs: flatten_blobs(&blob_tx.blobs),
            initial_state: StateDigest(vec![0, 1, 2, 3]),
            next_state: StateDigest(vec![4, 5, 6]),
            success: true,
            tx_hash: blob_tx.hash(),
            tx_ctx: None,
            registered_contracts: vec![],
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
            blobs: flatten_blobs(&blob_tx.blobs),
            initial_state: StateDigest(initial_state.to_vec()),
            next_state: StateDigest(next_state.to_vec()),
            success: true,
            tx_hash: blob_tx.hash(),
            tx_ctx: None,
            program_outputs: vec![],
            registered_contracts: vec![],
        }
    }

    fn craft_signed_block(height: u64, txs: Vec<Transaction>) -> SignedBlock {
        SignedBlock {
            certificate: AggregateSignature::default(),
            consensus_proposal: ConsensusProposal {
                slot: height,
                ..ConsensusProposal::default()
            },
            data_proposals: vec![(
                ValidatorPublicKey::default(),
                vec![DataProposal {
                    id: 1,
                    parent_data_proposal_hash: None,
                    txs,
                }],
            )],
        }
    }

    // Small wrapper for the general case until we get a larger refactoring?
    fn handle_verify_proof_transaction(
        state: &mut NodeState,
        proof: &VerifiedProofTransaction,
    ) -> Result<(), Error> {
        let mut bhpo = vec![];
        let blob_tx_to_try_and_settle = proof
            .proven_blobs
            .iter()
            .filter_map(|blob_proof_data| {
                state
                    .handle_blob_proof(TxHash::new(""), &mut bhpo, blob_proof_data)
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        if blob_tx_to_try_and_settle.len() != 1 {
            return Err(anyhow::anyhow!(
                "Test can only handle exactly one TX to settle"
            ));
        }
        let SettledTxOutput {
            updated_contracts, ..
        } = state.try_to_settle_blob_tx(blob_tx_to_try_and_settle.first().unwrap())?;
        for (contract_name, contract) in updated_contracts.iter() {
            state
                .contracts
                .insert(contract_name.clone(), contract.clone());
        }
        Ok(())
    }

    fn bogus_tx_context() -> Arc<TxContext> {
        Arc::new(TxContext {
            block_hash: ConsensusProposalHash("0xfedbeef".to_owned()),
            block_height: BlockHeight(133),
            timestamp: get_current_timestamp_ms().into(),
            chain_id: HYLE_TESTNET_CHAIN_ID,
        })
    }

    #[test_log::test(tokio::test)]
    async fn happy_path_with_tx_context() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let register_c1 = make_register_contract_effect(c1.clone());
        state.handle_register_contract_effect(&register_c1);

        let identity = Identity::new("test.c1");
        let blob_tx = BlobTransaction {
            identity: identity.clone(),
            blobs: vec![new_blob("c1")],
        };

        let ctx = bogus_tx_context();
        state.handle_blob_tx(&blob_tx, ctx.clone()).unwrap();

        let mut hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        hyle_output.tx_ctx = Some((*ctx).clone());
        let verified_proof = new_proof_tx(&c1, &hyle_output, &blob_tx.hash());
        // Modify something so it would fail.
        let mut ctx = (*ctx).clone();
        ctx.timestamp = 1234;
        hyle_output.tx_ctx = Some(ctx);
        let verified_proof_bad = new_proof_tx(&c1, &hyle_output, &blob_tx.hash());

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
        let identity = Identity::new("test.c1");

        let blob_tx = BlobTransaction {
            identity: identity.clone(),
            blobs: vec![],
        };

        assert_err!(state.handle_blob_tx(&blob_tx, bogus_tx_context()));
    }

    #[test_log::test(tokio::test)]
    async fn blob_tx_with_incorrect_identity() {
        let mut state = new_node_state().await;
        let identity = Identity::new("incorrect_id");

        let blob_tx = BlobTransaction {
            identity: identity.clone(),
            blobs: vec![new_blob("test")],
        };

        assert_err!(state.handle_blob_tx(&blob_tx, bogus_tx_context()));
    }

    #[test_log::test(tokio::test)]
    async fn two_proof_for_one_blob_tx() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let c2 = ContractName::new("c2");
        let identity = Identity::new("test.c1");

        let register_c1 = make_register_contract_effect(c1.clone());
        let register_c2 = make_register_contract_effect(c2.clone());

        let blob_tx = BlobTransaction {
            identity: identity.clone(),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_effect(&register_c1);
        state.handle_register_contract_effect(&register_c2);
        state.handle_blob_tx(&blob_tx, bogus_tx_context()).unwrap();

        let hyle_output_c1 = make_hyle_output(blob_tx.clone(), BlobIndex(0));

        let verified_proof_c1 = new_proof_tx(&c1, &hyle_output_c1, &blob_tx_hash);

        let hyle_output_c2 = make_hyle_output(blob_tx.clone(), BlobIndex(1));

        let verified_proof_c2 = new_proof_tx(&c2, &hyle_output_c2, &blob_tx_hash);

        let _ = handle_verify_proof_transaction(&mut state, &verified_proof_c1);
        let _ = handle_verify_proof_transaction(&mut state, &verified_proof_c2);

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

        let blob_tx_1 = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let blob_tx_hash_1 = blob_tx_1.hash();

        state.handle_register_contract_effect(&register_c1);
        state.handle_register_contract_effect(&register_c2);
        state
            .handle_blob_tx(&blob_tx_1, bogus_tx_context())
            .unwrap();

        let hyle_output_c1 = make_hyle_output(blob_tx_1.clone(), BlobIndex(1)); // Wrong index

        let verified_proof_c1 = new_proof_tx(&c1, &hyle_output_c1, &blob_tx_hash_1);

        assert_err!(handle_verify_proof_transaction(
            &mut state,
            &verified_proof_c1
        ));

        // Check that we did not settled
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

        let blob_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_effect(&register_c1);
        state.handle_register_contract_effect(&register_c2);
        state.handle_blob_tx(&blob_tx, bogus_tx_context()).unwrap();

        let hyle_output_c1 = make_hyle_output(blob_tx.clone(), BlobIndex(0));

        let verified_proof_c1 = new_proof_tx(&c1, &hyle_output_c1, &blob_tx_hash);

        let _ = handle_verify_proof_transaction(&mut state, &verified_proof_c1);
        let _ = handle_verify_proof_transaction(&mut state, &verified_proof_c1);

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

        let blob_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0), new_blob(&c1.0)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_effect(&register_c1);
        state.handle_blob_tx(&blob_tx, bogus_tx_context()).unwrap();

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
    async fn change_same_contract_state_multiple_times_in_same_tx() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");

        let register_c1 = make_register_contract_effect(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);
        let third_blob = new_blob(&c1.0);

        let blob_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![first_blob, second_blob, third_blob],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_effect(&register_c1);
        state.handle_blob_tx(&blob_tx, bogus_tx_context()).unwrap();

        let first_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));

        let verified_first_proof = new_proof_tx(&c1, &first_hyle_output, &blob_tx_hash);

        let mut second_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(1));
        second_hyle_output.initial_state = first_hyle_output.next_state.clone();
        second_hyle_output.next_state = StateDigest(vec![7, 8, 9]);

        let verified_second_proof = new_proof_tx(&c1, &second_hyle_output, &blob_tx_hash);

        let mut third_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(2));
        third_hyle_output.initial_state = second_hyle_output.next_state.clone();
        third_hyle_output.next_state = StateDigest(vec![10, 11, 12]);

        let verified_third_proof = new_proof_tx(&c1, &third_hyle_output, &blob_tx_hash);

        let _ = handle_verify_proof_transaction(&mut state, &verified_first_proof);
        let _ = handle_verify_proof_transaction(&mut state, &verified_second_proof);
        handle_verify_proof_transaction(&mut state, &verified_third_proof).unwrap();

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
        let blob_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![first_blob, second_blob, third_blob],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_effect(&register_c1);
        state.handle_blob_tx(&blob_tx, bogus_tx_context()).unwrap();

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

        let _ = handle_verify_proof_transaction(&mut state, &first_proof_tx);
        let _ = handle_verify_proof_transaction(&mut state, &second_proof_tx_b);
        let _ = handle_verify_proof_transaction(&mut state, &second_proof_tx_c);
        handle_verify_proof_transaction(&mut state, &third_proof_tx).unwrap();

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

        let blob_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![first_blob, second_blob],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_effect(&register_c1);
        state.handle_blob_tx(&blob_tx, bogus_tx_context()).unwrap();

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
        second_hyle_output.next_state = StateDigest(vec![7, 8, 9]);

        let verified_second_proof = new_proof_tx(&c1, &second_hyle_output, &blob_tx_hash);

        assert_err!(handle_verify_proof_transaction(
            &mut state,
            &verified_first_proof
        ));
        assert_err!(handle_verify_proof_transaction(
            &mut state,
            &another_verified_first_proof
        ));
        assert_err!(handle_verify_proof_transaction(
            &mut state,
            &verified_second_proof
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

        let blob_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![first_blob, second_blob, third_blob],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_effect(&register_c1);
        state.handle_blob_tx(&blob_tx, bogus_tx_context()).unwrap();

        // Create legitimate proof for Blob1
        let first_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        let verified_first_proof = new_proof_tx(&c1, &first_hyle_output, &blob_tx_hash);

        let mut second_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(1));
        second_hyle_output.initial_state = first_hyle_output.next_state.clone();
        second_hyle_output.next_state = StateDigest(vec![7, 8, 9]);

        let verified_second_proof = new_proof_tx(&c1, &second_hyle_output, &blob_tx_hash);

        let mut third_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(2));
        third_hyle_output.initial_state = first_hyle_output.next_state.clone();
        third_hyle_output.next_state = StateDigest(vec![10, 11, 12]);

        let verified_third_proof = new_proof_tx(&c1, &third_hyle_output, &blob_tx_hash);

        assert_err!(handle_verify_proof_transaction(
            &mut state,
            &verified_first_proof
        ));
        assert_err!(handle_verify_proof_transaction(
            &mut state,
            &verified_second_proof
        ));
        assert_err!(handle_verify_proof_transaction(
            &mut state,
            &verified_third_proof
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
        let blocking_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let ready_same_block = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0)],
        };
        let ready_later_block = BlobTransaction {
            identity: Identity::new("test.c2"),
            blobs: vec![new_blob(&c2.0)],
        };
        let ready_last_block = BlobTransaction {
            identity: Identity::new("test2.c1"),
            blobs: vec![new_blob(&c1.0)],
        };
        let blocking_tx_hash = blocking_tx.hash();
        let hyle_output =
            make_hyle_output_with_state(blocking_tx.clone(), BlobIndex(0), &[0, 1, 2, 3], &[12]);
        let blocking_tx_verified_proof_1 = new_proof_tx(&c1, &hyle_output, &blocking_tx_hash);
        let hyle_output =
            make_hyle_output_with_state(blocking_tx.clone(), BlobIndex(1), &[0, 1, 2, 3], &[22]);
        let blocking_tx_verified_proof_2 = new_proof_tx(&c2, &hyle_output, &blocking_tx_hash);

        let ready_same_block_hash = ready_same_block.hash();
        let hyle_output =
            make_hyle_output_with_state(ready_same_block.clone(), BlobIndex(0), &[12], &[13]);
        let ready_same_block_verified_proof =
            new_proof_tx(&c1, &hyle_output, &ready_same_block_hash);

        let ready_later_block_hash = ready_later_block.hash();
        let hyle_output =
            make_hyle_output_with_state(ready_later_block.clone(), BlobIndex(0), &[22], &[23]);
        let ready_later_block_verified_proof =
            new_proof_tx(&c1, &hyle_output, &ready_later_block_hash);

        let ready_last_block_hash = ready_last_block.hash();
        let hyle_output =
            make_hyle_output_with_state(ready_last_block.clone(), BlobIndex(0), &[13], &[14]);
        let ready_last_block_verified_proof =
            new_proof_tx(&c1, &hyle_output, &ready_last_block_hash);

        state.handle_signed_block(&craft_signed_block(
            104,
            vec![
                register_c1.into(),
                register_c2.into(),
                blocking_tx.into(),
                ready_same_block.into(),
                ready_same_block_verified_proof.into(),
                ready_last_block.into(),
                ready_last_block_verified_proof.into(),
            ],
        ));

        state.handle_signed_block(&craft_signed_block(
            108,
            vec![
                ready_later_block.into(),
                ready_later_block_verified_proof.into(),
            ],
        ));

        // Now settle the first, which should auto-settle the pending ones, then the ones waiting for these.
        assert_eq!(
            state
                .handle_signed_block(&craft_signed_block(
                    110,
                    vec![
                        blocking_tx_verified_proof_1.into(),
                        blocking_tx_verified_proof_2.into(),
                    ]
                ))
                .successful_txs,
            vec![
                blocking_tx_hash,
                ready_same_block_hash,
                ready_later_block_hash,
                ready_last_block_hash
            ]
        );
    }

    #[test_log::test(tokio::test)]
    async fn test_tx_timeout_simple() {
        let mut state = new_node_state().await;
        let c1 = ContractName::new("c1");
        let register_c1 = make_register_contract_tx(c1.clone());

        // First basic test - Time out a TX.
        let blob_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0), new_blob(&c1.0)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_signed_block(&craft_signed_block(
            3,
            vec![register_c1.into(), blob_tx.into()],
        ));

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
        let blob_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0)],
        };
        let blob_tx_hash = blob_tx.hash();
        state.handle_signed_block(&craft_signed_block(
            104,
            vec![register_c1.clone().into(), blob_tx.clone().into()],
        ));
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
        let blocking_tx = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let blocking_tx_hash = blocking_tx.hash();
        let ready_same_block = BlobTransaction {
            identity: Identity::new("test.c1"),
            blobs: vec![new_blob(&c1.0)],
        };
        let ready_later_block = BlobTransaction {
            identity: Identity::new("test.c2"),
            blobs: vec![new_blob(&c2.0)],
        };
        let ready_same_block_hash = ready_same_block.hash();
        let hyle_output = make_hyle_output(ready_same_block.clone(), BlobIndex(0));
        let ready_same_block_verified_proof =
            new_proof_tx(&c1, &hyle_output, &ready_same_block_hash);

        let ready_later_block_hash = ready_later_block.hash();
        let hyle_output = make_hyle_output(ready_later_block.clone(), BlobIndex(0));
        let ready_later_block_verified_proof =
            new_proof_tx(&c2, &hyle_output, &ready_later_block_hash);

        state.handle_signed_block(&craft_signed_block(
            104,
            vec![
                register_c1.into(),
                register_c2.into(),
                blocking_tx.into(),
                ready_same_block.into(),
                ready_same_block_verified_proof.into(),
            ],
        ));

        state.handle_signed_block(&craft_signed_block(
            108,
            vec![
                ready_later_block.into(),
                ready_later_block_verified_proof.into(),
            ],
        ));

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

    mod contract_registration {
        use std::collections::HashSet;

        use super::*;

        pub fn make_tx(sender: Identity, tld: ContractName, name: ContractName) -> BlobTransaction {
            BlobTransaction {
                identity: sender,
                blobs: vec![RegisterContractAction {
                    verifier: "test".into(),
                    program_id: ProgramId(vec![]),
                    state_digest: StateDigest(vec![0, 1, 2, 3]),
                    contract_name: name,
                }
                .as_blob(tld, None, None)],
            }
        }

        #[test_log::test(tokio::test)]
        async fn test_register_contract_simple_hyle() {
            let mut state = new_node_state().await;
            let register_c1 = make_tx("hyle.hyle".into(), "hyle".into(), "c1".into());
            let register_c2 = make_tx("hyle.hyle".into(), "hyle".into(), "c2.hyle".into());
            let register_c3 = make_tx("hyle.hyle".into(), "hyle".into(), "c3".into());

            state.handle_signed_block(&craft_signed_block(1, vec![register_c1.clone().into()]));

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
            assert_eq!(block.failed_txs, vec![register_c1.hash()]);
            assert_eq!(state.contracts.len(), 4);
        }

        #[test_log::test(tokio::test)]
        async fn test_register_contract_failure() {
            let mut state = new_node_state().await;
            let register_1 = make_tx("hyle.hyle".into(), "hyle".into(), "c1.hyle.lol".into());
            let register_2 = make_tx("other.hyle".into(), "hyle".into(), "c2.hyle.hyle".into());
            let register_3 = make_tx("hyle.hyle".into(), "hyle".into(), "c3.other".into());
            let register_4 = make_tx("hyle.hyle".into(), "hyle".into(), ".hyle".into());
            let register_5 = BlobTransaction {
                identity: "hyle.hyle".into(),
                blobs: vec![Blob {
                    contract_name: "hyle".into(),
                    data: BlobData(vec![0, 1, 2, 3]),
                }],
            };
            let register_good = make_tx("hyle.hyle".into(), "hyle".into(), "c1.hyle".into());

            let block = state.handle_signed_block(&craft_signed_block(
                1,
                vec![
                    register_1.clone().into(),
                    register_2.clone().into(),
                    register_3.clone().into(),
                    register_4.clone().into(),
                    register_5.clone().into(),
                    register_good.clone().into(),
                ],
            ));

            assert_eq!(state.contracts.len(), 2);
            assert_eq!(block.successful_txs, vec![register_good.hash(),]);
            assert_eq!(
                block.failed_txs,
                vec![
                    register_1.hash(),
                    register_2.hash(),
                    register_3.hash(),
                    register_4.hash(),
                    register_5.hash(),
                ]
            );
        }

        #[test_log::test(tokio::test)]
        async fn test_register_contract_composition() {
            let mut state = new_node_state().await;
            let register = make_tx("hyle.hyle".into(), "hyle".into(), "hydentity".into());
            state.handle_signed_block(&craft_signed_block(1, vec![register.clone().into()]));
            assert_eq!(state.contracts.len(), 2);

            let compositing_register_willfail = BlobTransaction {
                identity: "test.hydentity".into(),
                blobs: vec![
                    RegisterContractAction {
                        verifier: "test".into(),
                        program_id: ProgramId(vec![]),
                        state_digest: StateDigest(vec![0, 1, 2, 3]),
                        contract_name: "c1".into(),
                    }
                    .as_blob("hyle".into(), None, None),
                    Blob {
                        contract_name: "hydentity".into(),
                        data: BlobData(vec![0, 1, 2, 3]),
                    },
                ],
            };
            // Try to register the same contract validly later.
            let mut compositing_register_good = compositing_register_willfail.clone();
            // Change identity to change blob tx hash
            compositing_register_good.identity = "test2.hydentity".into();

            state.handle_signed_block(&craft_signed_block(
                102,
                vec![
                    compositing_register_willfail.clone().into(),
                    compositing_register_good.clone().into(),
                ],
            ));
            assert_eq!(state.contracts.len(), 2);

            let proof_tx = new_proof_tx(
                &"hyle".into(),
                &make_hyle_output(compositing_register_good.clone(), BlobIndex(1)),
                &compositing_register_good.hash(),
            );

            state.handle_signed_block(&craft_signed_block(103, vec![proof_tx.into()]));
            assert_eq!(state.contracts.len(), 2);

            // Send a third one that will fail early on settlement of the second because duplication
            // (and thus test the early-failure settlement path)

            let mut third_tx = compositing_register_willfail.clone();
            third_tx.identity = "test3.hydentity".into();
            let proof_tx = new_proof_tx(
                &"hyle".into(),
                &make_hyle_output(third_tx.clone(), BlobIndex(1)),
                &third_tx.hash(),
            );

            state.handle_signed_block(&craft_signed_block(
                104,
                vec![third_tx.clone().into(), proof_tx.clone().into()],
            ));
            assert_eq!(state.contracts.len(), 2);

            let block = state.handle_signed_block(&craft_signed_block(202, vec![]));

            assert_eq!(
                block.timed_out_txs,
                vec![compositing_register_willfail.hash(),]
            );
            assert_eq!(state.contracts.len(), 3);
        }
    }
}
