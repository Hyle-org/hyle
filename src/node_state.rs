//! State required for participation in consensus by the node.

use crate::{
    consensus::staking::Staker,
    model::{
        BlobTransaction, BlobsHash, Block, BlockHeight, ContractName, HandledBlockOutput,
        HandledProofTxOutput, Hashable, ProofTransaction, RegisterContractTransaction,
        TransactionData, VerifiedProofTransaction,
    },
};
use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use hyle_contract_sdk::{HyleOutput, StateDigest, TxHash};
use model::{Contract, Timeouts, UnsettledBlobMetadata, UnsettledBlobTransaction};
use ordered_tx_map::OrderedTxMap;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

pub mod model;
mod ordered_tx_map;
mod verifiers;

#[derive(Default, Encode, Decode, Debug, Clone)]
pub struct NodeState {
    timeouts: Timeouts,
    current_height: BlockHeight,
    // This field is public for testing purposes
    pub contracts: HashMap<ContractName, Contract>,
    unsettled_transactions: OrderedTxMap,
}

impl NodeState {
    pub fn handle_new_block(&mut self, block: Block) -> HandledBlockOutput {
        let timed_out_tx_hashes = self.clear_timeouts(&block.height);
        self.current_height = block.height;

        let mut new_contract_txs = vec![];
        let mut new_blob_txs = vec![];
        let mut new_verified_proof_txs = vec![];
        let mut verified_blobs = vec![];
        let mut failed_txs = vec![];
        let mut stakers: Vec<Staker> = vec![];
        let mut settled_blob_tx_hashes = vec![];
        let mut updated_states = HashMap::new();
        // Handle all transactions
        for tx in block.txs.iter() {
            match &tx.transaction_data {
                // FIXME: to remove when we have a real staking smart contract
                TransactionData::Stake(staker) => {
                    stakers.push(staker.clone());
                }
                TransactionData::Blob(blob_transaction) => {
                    match self.handle_blob_tx(blob_transaction) {
                        Ok(_) => {
                            // Keep track of all blob txs
                            new_blob_txs.push(tx.clone());
                        }
                        Err(e) => {
                            error!("Failed to handle blob transaction: {:?}", e);
                            failed_txs.push(tx.clone());
                        }
                    }
                }
                TransactionData::Proof(_) => {
                    error!("Unverified proof transaction should not be in a block");
                }
                TransactionData::VerifiedProof(verified_proof_transaction) => {
                    match self.handle_verified_proof_tx(verified_proof_transaction) {
                        Ok(proof_tx_output) => {
                            // When a proof tx is handled, three things happen:
                            // 1. Blobs get verified
                            // 2. Optionnal: BlobTransactions get settled
                            // 3. Optionnal: Contracts' state digests are updated

                            // Keep track of verified blobs
                            verified_blobs.push((
                                verified_proof_transaction.hyle_output.tx_hash.clone(),
                                verified_proof_transaction.hyle_output.index.clone(),
                            ));
                            // Keep track of settled txs
                            settled_blob_tx_hashes.extend(proof_tx_output.settled_blob_tx_hashes);
                            // Update the contract's updated states
                            updated_states.extend(proof_tx_output.updated_states);
                            // Keep track of all verified proof txs
                            new_verified_proof_txs.push(tx.clone());
                        }
                        Err(e) => {
                            error!("Failed to handle proof transaction: {:?}", e);
                            failed_txs.push(tx.clone());
                        }
                    }
                }
                TransactionData::RegisterContract(register_contract_transaction) => {
                    match self.handle_register_contract_tx(register_contract_transaction) {
                        Ok(_) => {
                            new_contract_txs.push(tx.clone());
                        }
                        Err(e) => {
                            error!("Failed to handle register contract transaction: {:?}", e);
                            failed_txs.push(tx.clone());
                        }
                    }
                }
            }
        }
        HandledBlockOutput {
            new_contract_txs,
            new_blob_txs,
            new_verified_proof_txs,
            verified_blobs,
            failed_txs,
            stakers,
            timed_out_tx_hashes,
            settled_blob_tx_hashes,
            updated_states,
        }
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
        let (blob_tx_hash, blobs_hash) = (tx.hash(), tx.blobs_hash());

        let blobs: Vec<UnsettledBlobMetadata> = tx
            .blobs
            .iter()
            .map(|blob| UnsettledBlobMetadata {
                contract_name: blob.contract_name.clone(),
                metadata: vec![],
            })
            .collect();

        debug!("Add blob transaction to state {:?}", tx);
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

    fn handle_verified_proof_tx(
        &mut self,
        tx: &VerifiedProofTransaction,
    ) -> Result<HandledProofTxOutput, Error> {
        debug!("Handle verified proof tx: {:?}", tx);
        // TODO: add diverse verifications ? (without the inital state checks!).
        self.verify_hyle_output(
            &tx.proof_transaction.contract_name,
            &tx.hyle_output,
            &tx.proof_transaction.blob_tx_hash,
        )?;

        // If we arrived here, HyleOutput provided is OK and can now be saved
        debug!("Save metadata for tx: {:?}", tx);
        self.unsettled_transactions
            .add_metadata(&tx.proof_transaction.blob_tx_hash, &tx.hyle_output)?;

        let mut settled_blob_tx_hashes = vec![];
        let mut updated_states = HashMap::new();

        let unsettled_tx = self
            .unsettled_transactions
            .get(&tx.proof_transaction.blob_tx_hash)
            .context("BlobTx that is been proved is either settled or does not exists")?
            .clone();

        if self.is_settlement_ready(&unsettled_tx) {
            self.verify_identities(&unsettled_tx)?;
            // This unsettled blob transaction can now be settled.
            // We want to keep track of tx hashes that have been settled and state updates associated
            (updated_states, settled_blob_tx_hashes) = self.settle_tx(&unsettled_tx)?;
        } else {
            debug!("Tx: {} is not ready for settlement", unsettled_tx.hash);
        }
        debug!("Done! Contract states: {:?}", self.contracts);
        Ok(HandledProofTxOutput {
            settled_blob_tx_hashes,
            updated_states,
        })
    }

    fn clear_timeouts(&mut self, height: &BlockHeight) -> Vec<TxHash> {
        let dropped = self.timeouts.drop(height);
        for tx in dropped.iter() {
            self.unsettled_transactions.remove(tx);
        }
        dropped
    }

    pub fn verify_proof(&self, tx: &ProofTransaction) -> Result<HyleOutput, Error> {
        // Verify proof
        let contract_name = &tx.contract_name;
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
        let hyle_output = verifiers::verify_proof(tx, verifier, program_id)?;
        Ok(hyle_output)
    }

    fn verify_identities(&self, unsettled_tx: &UnsettledBlobTransaction) -> Result<(), Error> {
        let mut outputs: Vec<(&HyleOutput, &ContractName)> = vec![];
        for blob in &unsettled_tx.blobs {
            let contract = self.contracts.get(&blob.contract_name).context(format!(
                "Contract {} not found when settling transaction",
                &blob.contract_name
            ))?;

            let hyle_output = blob
                .metadata
                .iter()
                .find(|hyle_output| hyle_output.initial_state == contract.state)
                .context("No provided proofs are based on the correct initial state")?;

            outputs.append(&mut vec![(hyle_output, &blob.contract_name)])
        }

        let (_, identity_contract_name) = outputs
            .iter()
            .find(|(out, contract_name)| out.identity.0.ends_with(&format!(".{}", contract_name.0)))
            .context("No identity match the contract name")?;

        let tx_identity_match_contract =
            unsettled_tx.identity.0.ends_with(&identity_contract_name.0);

        if !tx_identity_match_contract {
            bail!(
                "Tx identity '{}' does not match contract name: {}",
                unsettled_tx.identity.0,
                identity_contract_name
            );
        }

        let all_identities_match = outputs
            .iter()
            .all(|(out, _)| out.identity.0 == unsettled_tx.identity.0);

        if !all_identities_match {
            let identities = outputs
                .iter()
                .map(|(out, _)| &out.identity.0)
                .collect::<Vec<_>>();
            bail!(
                "Identities do not match {}: {:?}",
                unsettled_tx.identity,
                identities
            );
        }

        Ok(())
    }

    fn verify_hyle_output(
        &self,
        contract_name: &ContractName,
        hyle_output: &HyleOutput,
        unsettled_tx_hash: &TxHash,
    ) -> Result<(), Error> {
        if !hyle_output.success {
            bail!("Contract execution is not a success");
        }

        let unsettled_tx = self
            .unsettled_transactions
            .get(unsettled_tx_hash)
            .context("BlobTx that is been proved is either settled or does not exists")?;

        // blob_hash verification
        let extracted_blobs_hash = BlobsHash::from_concatenated(&hyle_output.blobs);
        if extracted_blobs_hash != unsettled_tx.blobs_hash {
            bail!(
                "Proof blobs hash '{:?}' do not correspond to transaction blobs hash '{:?}'.",
                extracted_blobs_hash,
                unsettled_tx.blobs_hash
            )
        }
        // Verify the contract name
        let expected_contract = &unsettled_tx.blobs[hyle_output.index.0 as usize].contract_name;
        if expected_contract != contract_name {
            bail!("Blob reference from proof for {unsettled_tx_hash} does not match the blob transaction contract name {expected_contract}");
        }

        Ok(())
    }

    fn is_settlement_ready(&self, unsettled_tx: &UnsettledBlobTransaction) -> bool {
        if !self
            .unsettled_transactions
            .is_next_unsettled_tx(&unsettled_tx.hash)
        {
            return false;
        }

        // Check for each blob if initial state is correct.
        // As tx is next to be settled, remove all metadata with incorrect initial state.
        for unsettled_blob in unsettled_tx.blobs.iter() {
            let contract = match self.contracts.get(&unsettled_blob.contract_name) {
                Some(contract) => contract,
                None => {
                    warn!(
                        "Tx: {}: No contract '{}' found when checking for settlement",
                        unsettled_tx.hash, unsettled_blob.contract_name
                    );
                    return false;
                } // No contract found for this blob
            };
            let has_one_valid_initial_state = unsettled_blob
                .metadata
                .iter()
                .any(|hyle_output| hyle_output.initial_state == contract.state);

            debug!(
                "Tx: {}: Blob '{}' has valid initial state: {}",
                unsettled_tx.hash, unsettled_blob.contract_name, has_one_valid_initial_state
            );

            if !has_one_valid_initial_state {
                info!(
                    "Tx: {}: No initial state match current contract state for contract '{}'",
                    unsettled_tx.hash, unsettled_blob.contract_name
                );
                return false; // No valid metadata found for this blob
            }
        }
        debug!(
            "Tx: {}: All blobs have valid initial state",
            unsettled_tx.hash
        );
        true // All blobs have at least one valid metadata
    }

    fn settle_tx(
        &mut self,
        unsettled_tx: &UnsettledBlobTransaction,
    ) -> Result<(HashMap<ContractName, StateDigest>, Vec<TxHash>), Error> {
        info!("Settle tx {:?}", unsettled_tx.hash);
        let mut updated_states = HashMap::new();
        let mut settled_blob_tx_hashes = vec![];

        for blob in &unsettled_tx.blobs {
            let contract = self
                .contracts
                .get_mut(&blob.contract_name)
                .context(format!(
                    "Contract {} not found when settling transaction",
                    &blob.contract_name
                ))?;

            let next_state = blob
                .metadata
                .iter()
                .find(|hyle_output| hyle_output.initial_state == contract.state)
                .context("No provided proofs are based on the correct initial state")?
                .next_state
                .clone();

            // TODO: chain settlements for all transactions on that contract
            self.update_state_contract(&blob.contract_name, &next_state)?;
            updated_states.insert(blob.contract_name.clone(), next_state);
            settled_blob_tx_hashes.push(unsettled_tx.hash.clone());
        }
        // Clean the unsettled tx from the state
        self.unsettled_transactions.remove(&unsettled_tx.hash);
        Ok((updated_states, settled_blob_tx_hashes))
    }

    fn update_state_contract(
        &mut self,
        contract_name: &ContractName,
        next_state: &StateDigest,
    ) -> Result<(), Error> {
        let contract = self.contracts.get_mut(contract_name).context(format!(
            "Contract {} not found when settling",
            contract_name,
        ))?;
        debug!("Update {} contract state: {:?}", contract_name, next_state);
        contract.state = next_state.clone();
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::model::*;
    use assertables::assert_err;
    use hyle_contract_sdk::{flatten_blobs, BlobIndex, Identity};

    async fn new_node_state() -> NodeState {
        NodeState::default()
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
            verifier: "test".to_string(),
            program_id: vec![],
            state_digest: StateDigest(vec![0, 1, 2, 3]),
            contract_name: name,
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

        let proof_c1 = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&hyle_output_c1).unwrap()),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            hyle_output: state.verify_proof(&proof_c1).unwrap(),
            proof_transaction: proof_c1,
        };

        let hyle_output_c2 = make_hyle_output(blob_tx.clone(), BlobIndex(1));

        let proof_c2 = ProofTransaction {
            contract_name: c2.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&hyle_output_c2).unwrap()),
        };

        let verified_proof_c2 = VerifiedProofTransaction {
            hyle_output: state.verify_proof(&proof_c2).unwrap(),
            proof_transaction: proof_c2,
        };

        state.handle_verified_proof_tx(&verified_proof_c1).unwrap();
        state.handle_verified_proof_tx(&verified_proof_c2).unwrap();

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

        let proof_c1 = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash_1.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&hyle_output_c1).unwrap()),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            hyle_output: state.verify_proof(&proof_c1).unwrap(),
            proof_transaction: proof_c1,
        };

        assert_err!(state.handle_verified_proof_tx(&verified_proof_c1));

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

        let proof_c1 = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&hyle_output_c1).unwrap()),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            hyle_output: state.verify_proof(&proof_c1).unwrap(),
            proof_transaction: proof_c1,
        };

        state.handle_verified_proof_tx(&verified_proof_c1).unwrap();
        state.handle_verified_proof_tx(&verified_proof_c1).unwrap();

        assert_eq!(
            state
                .unsettled_transactions
                .get(&blob_tx_hash)
                .unwrap()
                .blobs[0]
                .metadata
                .len(),
            2
        );
        // Check that we did not settled
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![0, 1, 2, 3]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![0, 1, 2, 3]);
    }
}
