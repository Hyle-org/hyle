//! State required for participation in consensus by the node.

use crate::bus::{command_response::Query, BusClientSender, BusMessage};
use crate::data_availability::{DataEvent, QueryBlockHeight};
use crate::model::data_availability::{
    Contract, HandledBlobProofOutput, UnsettledBlobMetadata, UnsettledBlobTransaction,
};
use crate::model::{
    BlobProofOutput, BlobTransaction, BlobsHash, Block, BlockHeight, CommonRunContext,
    ContractName, Hashable, RegisterContractTransaction, SignedBlock, TransactionData,
};
use crate::module_handle_messages;
use crate::utils::conf::SharedConf;
use crate::utils::logger::LogMe;
use crate::utils::modules::{module_bus_client, Module};
use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use hyle_contract_sdk::{utils::parse_structured_blob, BlobIndex, HyleOutput, TxHash};
use ordered_tx_map::OrderedTxMap;
use serde::{Deserialize, Serialize};
use staking::StakingAction;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use timeouts::Timeouts;
use tracing::{debug, error, info};

mod api;
mod ordered_tx_map;
mod timeouts;
pub mod verifiers;

pub struct SettledTxOutput {
    // Original blob transaction, now settled.
    pub tx: UnsettledBlobTransaction,
    /// This is the index of the blob proof output used in the blob settlement, for each blob.
    pub blob_proof_output_indices: Vec<usize>,
    /// New data for contracts modified by the settled TX.
    pub updated_contracts: BTreeMap<ContractName, Contract>,
}

#[derive(Default, Encode, Decode, Debug, Clone)]
pub struct NodeStateStorage {
    timeouts: Timeouts,
    current_height: BlockHeight,
    // This field is public for testing purposes
    pub contracts: HashMap<ContractName, Contract>,
    unsettled_transactions: OrderedTxMap,
}

pub struct NodeState {
    config: SharedConf,
    bus: NodeStateBusClient,
    storage: NodeStateStorage,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum NodeStateEvent {
    NewBlock(Box<Block>),
}
impl BusMessage for NodeStateEvent {}

module_bus_client! {
#[derive(Debug)]
pub struct NodeStateBusClient {
    sender(NodeStateEvent),
    receiver(DataEvent),
    receiver(Query<ContractName, Contract>),
    receiver(Query<QueryBlockHeight , BlockHeight>),
}
}

impl Module for NodeState {
    type Context = Arc<CommonRunContext>;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = NodeStateBusClient::new_from_bus(ctx.bus.new_handle()).await;

        let api = api::api(&ctx).await;
        if let Ok(mut guard) = ctx.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/", api));
            }
        }

        let storage = Self::load_from_disk_or_default::<NodeStateStorage>(
            ctx.config.data_directory.join("node_state.bin").as_path(),
        );

        Ok(Self {
            config: ctx.config.clone(),
            bus,
            storage,
        })
    }

    async fn run(&mut self) -> Result<()> {
        module_handle_messages! {
            on_bus self.bus,
            command_response<QueryBlockHeight, BlockHeight> _ => {
                Ok(self.storage.current_height)
            }
            command_response<ContractName, Contract> cmd => {
                self.storage.contracts.get(cmd).cloned().context("Contract not found")
            }
            listen<DataEvent> block => {
                match block {
                    DataEvent::OrderedSignedBlock(block) => {
                        let node_state_block = self.storage.handle_signed_block(&block);
                        _ = self
                            .bus
                            .send(NodeStateEvent::NewBlock(Box::new(node_state_block)))
                            .log_error("Sending DataEvent while processing SignedBlock");
                    }
                }
            }
        }

        let _ = Self::save_on_disk::<NodeStateStorage>(
            self.config.data_directory.join("node_state.bin").as_path(),
            &self.storage,
        )
        .log_error("Saving node state");

        Ok(())
    }
}

impl NodeStateStorage {
    pub fn handle_signed_block(&mut self, signed_block: &SignedBlock) -> Block {
        self.current_height = signed_block.height();

        let mut block_under_construction = Block {
            parent_hash: signed_block.parent_hash().clone(),
            hash: signed_block.hash(),
            block_height: signed_block.height(),
            block_timestamp: signed_block.consensus_proposal.timestamp,
            txs: vec![], // To avoid a double borrow, we'll add the transactions later
            failed_txs: HashSet::new(),
            blob_proof_outputs: vec![],
            settled_blob_tx_hashes: vec![],
            verified_blobs: vec![],
            staking_actions: vec![],
            new_bounded_validators: signed_block
                .consensus_proposal
                .new_validators_to_bond
                .iter()
                .map(|v| v.pubkey.clone())
                .collect(),
            timed_out_tx_hashes: vec![], // Added below as it needs the block
            updated_states: BTreeMap::new(),
        };

        self.clear_timeouts(&mut block_under_construction);

        let txs = signed_block.txs();
        // Handle all transactions
        for tx in txs.iter() {
            match &tx.transaction_data {
                TransactionData::Blob(blob_transaction) => {
                    match self.handle_blob_tx(blob_transaction) {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Failed to handle blob transaction: {:?}", e);
                            block_under_construction.failed_txs.insert(tx.hash());
                        }
                    }
                }
                TransactionData::Proof(_) => {
                    error!("Unverified recursive proof transaction should not be in a block");
                }
                TransactionData::VerifiedProof(proof_tx) => {
                    // First, store the proofs and check if we can settle the transaction
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
                                    error!(
                                        "Failed to handle verified proof transaction {:?}: {err}",
                                        proof_tx.hash()
                                    );
                                    block_under_construction.failed_txs.insert(tx.hash());
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
                TransactionData::RegisterContract(register_contract_transaction) => {
                    match self.handle_register_contract_tx(register_contract_transaction) {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Failed to handle register contract transaction: {:?}", e);
                            block_under_construction.failed_txs.insert(tx.hash());
                        }
                    }
                }
            }
        }
        block_under_construction.txs = txs;
        block_under_construction
    }

    pub fn handle_register_contract_tx(
        &mut self,
        tx: &RegisterContractTransaction,
    ) -> Result<(), Error> {
        if self.contracts.contains_key(&tx.contract_name) {
            bail!("Contract already exists")
        }
        info!("📝 Registering new contract {}", tx.contract_name);
        self.contracts.insert(
            tx.contract_name.clone(),
            Contract {
                name: tx.contract_name.clone(),
                program_id: tx.program_id.clone(),
                state: tx.state_digest.clone(),
                verifier: tx.verifier.clone(),
            },
        );

        Ok(())
    }

    fn handle_blob_tx(&mut self, tx: &BlobTransaction) -> Result<(), Error> {
        debug!("Handle blob tx: {:?}", tx);
        let identity_parts: Vec<&str> = tx.identity.0.split('.').collect();
        if identity_parts.len() != 2 {
            bail!("Transaction identity is not correctly formed. It should be in the form <id>.<contract_id_name>");
        }
        if identity_parts[1].is_empty() {
            bail!("Transaction identity must include a contract name");
        }

        if tx.blobs.is_empty() {
            bail!("Blob Transaction must have at least one blob");
        }

        let (blob_tx_hash, blobs_hash) = (tx.hash(), tx.blobs_hash());

        let blobs: Vec<UnsettledBlobMetadata> = tx
            .blobs
            .iter()
            .map(|blob| UnsettledBlobMetadata {
                blob: blob.clone(),
                possible_proofs: vec![],
            })
            .collect();

        debug!("Add blob transaction {} to state {:?}", tx.hash(), tx);
        self.unsettled_transactions.add(UnsettledBlobTransaction {
            identity: tx.identity.clone(),
            hash: blob_tx_hash.clone(),
            blobs_hash,
            blobs,
        });

        // Update timeouts
        self.timeouts.set(blob_tx_hash, self.current_height + 100); // TODO: Timeout after 100 blocks, make it configurable !

        Ok(())
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
                bail!("BlobTx not found");
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
            "Saving metadata for BlobTx {} for {}",
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
            blob_index: blob_proof_data.hyle_output.index.clone(),
            blob_proof_output_index: blob.possible_proofs.len() - 1,
            // Guaranteed to exist by the above
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
                }) => {
                    // Settle the TX and add any new TXs to try and settle next.
                    blob_tx_to_try_and_settle.append(&mut self.on_settled_blob_tx(
                        block_under_construction,
                        bth,
                        settled_tx,
                        blob_proof_output_indices,
                        tx_updated_contracts,
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
        debug!("Trying to settle blob tx: {:?}", unsettled_tx_hash);

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

        let (updated_contracts, blob_proof_output_indices, did_settle) =
            Self::settle_blobs_recursively(
                &self.contracts,
                updated_contracts,
                unsettled_tx.blobs.iter(),
                vec![],
            );

        if !did_settle {
            bail!("Tx: {} is not ready to settle.", unsettled_tx.hash);
        }

        // Safe to unwrap - we must exist.
        let unsettled_tx = self
            .unsettled_transactions
            .remove(unsettled_tx_hash)
            .unwrap();

        Ok(SettledTxOutput {
            tx: unsettled_tx,
            blob_proof_output_indices,
            updated_contracts,
        })
    }

    fn settle_blobs_recursively<'a>(
        contracts: &HashMap<ContractName, Contract>,
        current_contracts: BTreeMap<ContractName, Contract>,
        mut blob_iter: impl Iterator<Item = &'a UnsettledBlobMetadata> + Clone,
        mut blob_proof_output_indices: Vec<usize>,
    ) -> (BTreeMap<ContractName, Contract>, Vec<usize>, bool) {
        let Some(current_blob) = blob_iter.next() else {
            return (current_contracts, blob_proof_output_indices, true);
        };
        let contract_name = &current_blob.blob.contract_name;
        let known_contract_state = current_contracts
            .get(contract_name)
            .unwrap_or(contracts.get(contract_name).unwrap()); // Safe to unwrap - all contract names are validated to exist above.
        for (i, proof_metadata) in current_blob.possible_proofs.iter().enumerate() {
            if proof_metadata.1.initial_state == known_contract_state.state
                && proof_metadata.0 == known_contract_state.program_id
            {
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
                if let (updated_states, blob_proof_output_indices, true) =
                    Self::settle_blobs_recursively(
                        contracts,
                        us,
                        blob_iter.clone(),
                        blob_proof_output_indices.clone(),
                    )
                {
                    return (updated_states, blob_proof_output_indices, true);
                }
            } else {
                debug!(
                    "Could not settle blob proof output #{} for contract '{}'. Expected initial state: {:?}, got: {:?}, expected program ID: {:?}, got: {:?}",
                    i,
                    contract_name,
                    known_contract_state.state,
                    proof_metadata.1.initial_state,
                    known_contract_state.program_id,
                    proof_metadata.0
                );
            }
        }
        (current_contracts, blob_proof_output_indices, false)
    }

    /// Handle a settled blob transaction.
    /// This returns the list of new TXs to try and settle next,
    /// i.e. the "next" TXs for each contract.
    fn on_settled_blob_tx(
        &mut self,
        block_under_construction: &mut Block,
        bth: TxHash,
        mut settled_tx: UnsettledBlobTransaction,
        blob_proof_output_indices: Vec<usize>,
        tx_updated_contracts: BTreeMap<ContractName, Contract>,
    ) -> BTreeSet<TxHash> {
        // Transaction was settled, update our state.
        info!("Settled tx {:?}", &bth);

        // When a proof tx is handled, three things happen:
        // 1. Blobs get verified
        // 2. Maybe: BlobTransactions get settled
        // 3. Maybe: Contract state digests are updated

        // Keep track of verified blobs
        std::mem::take(&mut settled_tx.blobs)
            .iter_mut()
            .enumerate()
            .for_each(|(i, blob_metadata)| {
                // Keep track of all stakers
                if blob_metadata.blob.contract_name.0 == "staking" {
                    let blob = std::mem::take(&mut blob_metadata.blob);
                    let staking_action: StakingAction =
                        parse_structured_blob(&[blob], &BlobIndex(0))
                            .data
                            .parameters;
                    block_under_construction
                        .staking_actions
                        .push((settled_tx.identity.clone(), staking_action));
                }

                // Everything must exist by construction
                block_under_construction.verified_blobs.push((
                    bth.clone(),
                    hyle_contract_sdk::BlobIndex(i),
                    blob_proof_output_indices[i],
                ));
            });

        // Keep track of settled txs
        block_under_construction.settled_blob_tx_hashes.push(bth);

        // Update states
        let mut next_txs_to_try_and_settle = BTreeSet::new();
        for (contract_name, next_state) in tx_updated_contracts.iter() {
            debug!("Update {} contract state: {:?}", contract_name, next_state);
            // Safe to unwrap - all contract names are validated to exist above.
            self.contracts.get_mut(contract_name).unwrap().state = next_state.state.clone();
            if let Some(tx) = self
                .unsettled_transactions
                .get_next_unsettled_tx(contract_name)
            {
                next_txs_to_try_and_settle.insert(tx.clone());
            }
            // TODO: would be nice to have a drain-like API here.
            block_under_construction
                .updated_states
                .insert(contract_name.clone(), next_state.state.clone());
        }

        next_txs_to_try_and_settle
    }

    fn verify_hyle_output(
        unsettled_tx: &UnsettledBlobTransaction,
        hyle_output: &HyleOutput,
    ) -> Result<(), Error> {
        // TODO: this is perfectly fine and can be settled, and should be removed.
        if !hyle_output.success {
            bail!("Contract execution is not a success");
        }

        // Identity verification
        if unsettled_tx.identity != hyle_output.identity {
            bail!(
                "Proof identity '{:?}' does not correspond to BlobTx identity '{:?}'.",
                hyle_output.identity,
                unsettled_tx.identity
            )
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

        block_under_construction.timed_out_tx_hashes = txs_at_timeout;
    }

    pub fn verify_proof_single_output(
        &self,
        proof: &[u8],
        contract_name: &ContractName,
    ) -> Result<HyleOutput, Error> {
        // Verify proof
        let contract = match self.contracts.get(contract_name) {
            Some(contract) => contract,
            None => {
                bail!(
                    "No contract '{}' found when checking for proof verification",
                    contract_name
                );
            }
        };
        let program_id = &contract.program_id;
        let verifier = &contract.verifier;
        let hyle_output = verifiers::verify_proof(proof, verifier, program_id)?
            .pop()
            .unwrap();
        Ok(hyle_output)
    }
}

#[cfg(test)]
pub mod test {
    use core::panic;

    use super::*;
    use crate::model::*;
    use assertables::assert_err;
    use consensus::ConsensusProposal;
    use crypto::AggregateSignature;
    use hyle_contract_sdk::{flatten_blobs, BlobIndex, Identity, ProgramId, StateDigest};
    use mempool::DataProposal;

    async fn new_node_state() -> NodeStateStorage {
        NodeStateStorage::default()
    }

    fn new_blob(contract: &str) -> Blob {
        Blob {
            contract_name: ContractName(contract.to_owned()),
            data: BlobData(vec![0, 1, 2, 3]),
        }
    }

    fn new_register_contract(name: ContractName) -> RegisterContractTransaction {
        RegisterContractTransaction {
            owner: "test".to_string(),
            verifier: "test".into(),
            program_id: ProgramId(vec![]),
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name: name,
        }
    }

    fn new_proof_tx(
        contract: &ContractName,
        hyle_output: &HyleOutput,
        blob_tx_hash: &TxHash,
    ) -> VerifiedProofTransaction {
        let proof = ProofTransaction {
            contract_name: contract.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&vec![hyle_output]).unwrap()),
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

    fn make_hyle_output(blob_tx: BlobTransaction, blob_index: BlobIndex) -> HyleOutput {
        HyleOutput {
            version: 1,
            tx_hash: blob_tx.hash(),
            index: blob_index,
            identity: blob_tx.identity.clone(),
            blobs: flatten_blobs(&blob_tx.blobs),
            initial_state: StateDigest(vec![0, 1, 2, 3]),
            next_state: StateDigest(vec![4, 5, 6]),
            program_outputs: vec![],
            success: true,
        }
    }

    fn make_hyle_output_with_state(
        blob_tx: BlobTransaction,
        blob_index: BlobIndex,
        initial_state: &[u8],
        next_state: &[u8],
    ) -> HyleOutput {
        HyleOutput {
            version: 1,
            tx_hash: blob_tx.hash(),
            index: blob_index,
            identity: blob_tx.identity.clone(),
            blobs: flatten_blobs(&blob_tx.blobs),
            initial_state: StateDigest(initial_state.to_vec()),
            next_state: StateDigest(next_state.to_vec()),
            program_outputs: vec![],
            success: true,
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
        state: &mut NodeStateStorage,
        proof: &VerifiedProofTransaction,
    ) -> Result<(), Error> {
        let mut bhpo = vec![];
        let blob_tx_to_try_and_settle = proof
            .proven_blobs
            .iter()
            .filter_map(|blob_proof_data| {
                state
                    .handle_blob_proof(TxHash("".to_owned()), &mut bhpo, blob_proof_data)
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

    #[test_log::test(tokio::test)]
    async fn blob_tx_without_blobs() {
        let mut state = new_node_state().await;
        let identity = Identity("test.c1".to_string());

        let blob_tx = BlobTransaction {
            identity: identity.clone(),
            blobs: vec![],
        };

        assert_err!(state.handle_blob_tx(&blob_tx));
    }

    #[test_log::test(tokio::test)]
    async fn blob_tx_with_incorrect_identity() {
        let mut state = new_node_state().await;
        let identity = Identity("incorrect_id".to_string());

        let blob_tx = BlobTransaction {
            identity: identity.clone(),
            blobs: vec![new_blob("test")],
        };

        assert_err!(state.handle_blob_tx(&blob_tx));
    }

    #[test_log::test(tokio::test)]
    async fn two_proof_for_one_blob_tx() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());
        let identity = Identity("test.c1".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_tx = BlobTransaction {
            identity: identity.clone(),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_register_contract_tx(&register_c2).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

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
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_tx_1 = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let blob_tx_hash_1 = blob_tx_1.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_register_contract_tx(&register_c2).unwrap();
        state.handle_blob_tx(&blob_tx_1).unwrap();

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
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_register_contract_tx(&register_c2).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

        let hyle_output_c1 = make_hyle_output(blob_tx.clone(), BlobIndex(0));

        let verified_proof_c1 = new_proof_tx(&c1, &hyle_output_c1, &blob_tx_hash);

        let _ = handle_verify_proof_transaction(&mut state, &verified_proof_c1);
        let _ = handle_verify_proof_transaction(&mut state, &verified_proof_c1);

        assert_eq!(
            state
                .unsettled_transactions
                .get(&blob_tx_hash)
                .unwrap()
                .blobs[0]
                .possible_proofs
                .len(),
            2
        );
        // Check that we did not settled
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![0, 1, 2, 3]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![0, 1, 2, 3]);
    }

    #[test_log::test(tokio::test)]
    async fn change_same_contract_state_multiple_times_in_same_tx() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());

        let register_c1 = new_register_contract(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);
        let third_blob = new_blob(&c1.0);

        let blob_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![first_blob, second_blob, third_blob],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

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

        let c1 = ContractName("c1".to_string());
        let register_c1 = new_register_contract(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);
        let third_blob = new_blob(&c1.0);
        let blob_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![first_blob, second_blob, third_blob],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

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
        let c1 = ContractName("c1".to_string());

        let register_c1 = new_register_contract(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);

        let blob_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![first_blob, second_blob],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

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
        let c1 = ContractName("c1".to_string());

        let register_c1 = new_register_contract(c1.clone());

        let first_blob = new_blob(&c1.0);
        let second_blob = new_blob(&c1.0);
        let third_blob = new_blob(&c1.0);

        let blob_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![first_blob, second_blob, third_blob],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

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

        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());
        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        // Add four transactions - A blocks B/C, B blocks D.
        // Send proofs for B, C, D before A.
        let blocking_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let ready_same_block = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![new_blob(&c1.0)],
        };
        let ready_later_block = BlobTransaction {
            identity: Identity("test.c2".to_string()),
            blobs: vec![new_blob(&c2.0)],
        };
        let ready_last_block = BlobTransaction {
            identity: Identity("test2.c1".to_string()),
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
                .settled_blob_tx_hashes,
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
        let c1 = ContractName("c1".to_string());
        let register_c1 = new_register_contract(c1.clone());

        // First basic test - Time out a TX.
        let blob_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
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
            .timed_out_tx_hashes;

        // Check that the transaction has timed out
        assert!(timed_out_tx_hashes.contains(&blob_tx_hash));
        assert!(state.unsettled_transactions.get(&blob_tx_hash).is_none());
    }

    #[test_log::test(tokio::test)]
    async fn test_tx_no_timeout_once_settled() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let register_c1 = new_register_contract(c1.clone());

        // Add a new transaction and settle it.
        let blob_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![new_blob(&c1.0)],
        };
        let blob_tx_hash = blob_tx.hash();
        state.handle_signed_block(&craft_signed_block(
            104,
            vec![register_c1.into(), blob_tx.clone().into()],
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
                .settled_blob_tx_hashes,
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
            .timed_out_tx_hashes;

        // Check that the transaction remains settled and cleared from the timeout map
        assert!(!timed_out_tx_hashes.contains(&blob_tx_hash));
        assert!(state.unsettled_transactions.get(&blob_tx_hash).is_none());
        assert_eq!(timeouts::tests::get(&state.timeouts, &blob_tx_hash), None);
    }

    #[test_log::test(tokio::test)]
    async fn test_tx_on_timeout_settle_next_txs() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());
        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        // Add Three transactions - the first blocks the next two, but the next two are ready to settle.
        let blocking_tx = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![new_blob(&c1.0), new_blob(&c2.0)],
        };
        let blocking_tx_hash = blocking_tx.hash();
        let ready_same_block = BlobTransaction {
            identity: Identity("test.c1".to_string()),
            blobs: vec![new_blob(&c1.0)],
        };
        let ready_later_block = BlobTransaction {
            identity: Identity("test.c2".to_string()),
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
        assert_eq!(block.timed_out_tx_hashes, vec![blocking_tx_hash]);

        // The others have been settled
        [ready_same_block_hash, ready_later_block_hash]
            .iter()
            .for_each(|tx_hash| {
                assert!(!block.timed_out_tx_hashes.contains(tx_hash));
                assert!(state.unsettled_transactions.get(tx_hash).is_none());
                assert!(block.settled_blob_tx_hashes.contains(tx_hash));
            });
    }
}
