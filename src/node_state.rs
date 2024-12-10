//! State required for participation in consensus by the node.

use crate::{
    consensus::staking::Staker,
    model::{
        BlobTransaction, BlobsHash, Block, BlockHeight, ContractName, HandledBlockOutput, Hashable,
        ProofTransaction, RegisterContractTransaction, TransactionData, VerifiedProofTransaction,
    },
};
use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use hyle_contract_sdk::{HyleOutput, StateDigest, TxHash};
use model::{Contract, Timeouts, UnsettledBlobMetadata, UnsettledBlobTransaction};
use ordered_tx_map::OrderedTxMap;
use std::collections::HashMap;
use tracing::{debug, error, info};

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

#[derive(Debug)]
pub struct HandledProofTxOutput {
    pub settled_blob_tx_hashes: Vec<TxHash>,
    pub updated_states: HashMap<ContractName, StateDigest>,
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
                            // 2. Maybe: BlobTransactions get settled
                            // 3. Maybe: Contract state digests are updated

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

        let (unsettled_tx, is_next_to_settle) = self
            .unsettled_transactions
            .get_for_settlement(&tx.proof_transaction.blob_tx_hash)
            .context("BlobTx that is been proved is either settled or does not exists")?;

        // Sanity check: if some of the blob contracts are not registered, we can't proceed
        if !unsettled_tx
            .blobs
            .iter()
            .all(|blob| self.contracts.contains_key(&blob.contract_name))
        {
            bail!("Cannot settle TX: some blob contracts are not registered");
        }

        // TODO: add diverse verifications ? (without the inital state checks!).
        // TODO: success to false is valid outcome and can be settled.
        Self::verify_hyle_output(
            unsettled_tx,
            &tx.proof_transaction.contract_name,
            &tx.hyle_output,
            &tx.proof_transaction.blob_tx_hash,
        )?;

        // If we arrived here, HyleOutput provided is OK and can now be saved
        debug!(
            "Saving metadata for BlobTx {} for {}",
            tx.hyle_output.tx_hash.0, tx.hyle_output.index
        );
        unsettled_tx.blobs[tx.hyle_output.index.0 as usize]
            .metadata
            .push(tx.hyle_output.clone());

        let mut settled_blob_tx_hashes = vec![];
        let updated_states = HashMap::new();

        if !is_next_to_settle {
            debug!(
                "Tx: {} is not the next transaction to settle.",
                unsettled_tx.hash
            );
            return Ok(HandledProofTxOutput {
                settled_blob_tx_hashes,
                updated_states,
            });
        }

        Self::verify_identity(unsettled_tx)?;

        let (updated_states, did_settle) = Self::settle_blobs_recursively(
            &self.contracts,
            updated_states,
            unsettled_tx.blobs.iter(),
        );

        if !did_settle {
            debug!("Tx: {} is not ready to settle.", unsettled_tx.hash);
            return Ok(HandledProofTxOutput {
                settled_blob_tx_hashes,
                updated_states,
            });
        }

        info!("Settle tx {:?}", unsettled_tx.hash);

        for (contract_name, next_state) in updated_states.iter() {
            debug!("Update {} contract state: {:?}", contract_name, next_state);
            // Safe to unwrap - all contract names are validated to exist above.
            self.contracts.get_mut(contract_name).unwrap().state = next_state.clone();
        }

        let settled_blob_tx_hash = std::mem::take(&mut unsettled_tx.hash);
        // Clean the unsettled tx from the state
        self.unsettled_transactions.remove(&settled_blob_tx_hash);

        settled_blob_tx_hashes.push(settled_blob_tx_hash);

        Ok(HandledProofTxOutput {
            settled_blob_tx_hashes,
            updated_states,
        })
    }

    fn settle_blobs_recursively<'a>(
        contracts: &HashMap<ContractName, Contract>,
        current_states: HashMap<ContractName, StateDigest>,
        mut blob_iter: impl Iterator<Item = &'a UnsettledBlobMetadata> + Clone,
    ) -> (HashMap<ContractName, StateDigest>, bool) {
        let Some(current_blob) = blob_iter.next() else {
            return (current_states, true);
        };
        let contract_name = &current_blob.contract_name;
        let known_initial_state = current_states
            .get(contract_name)
            .unwrap_or(&contracts.get(contract_name).unwrap().state); // Safe to unwrap - all contract names are validated to exist above.
        for proof_metadata in current_blob.metadata.iter() {
            if proof_metadata.initial_state == *known_initial_state {
                // TODO: ideally make this CoW
                let mut us = current_states.clone();
                us.insert(contract_name.clone(), proof_metadata.next_state.clone());
                if let (updated_states, true) =
                    Self::settle_blobs_recursively(contracts, us, blob_iter.clone())
                {
                    return (updated_states, true);
                }
            }
        }
        (current_states, false)
    }

    // TODO: this should probably be done much earlier, proofs aren't involved
    fn verify_identity(unsettled_tx: &UnsettledBlobTransaction) -> Result<(), Error> {
        // Checks that there is a blob that proves the identity
        let identity_contract_name = unsettled_tx
            .identity
            .0
            .split('.')
            .last()
            .context("Transaction identity is not correctly formed. It should be in the form <id>.<contract_id_name>")?;

        // Check that there is at least one blob that has identity_contract_name as contract name
        if !unsettled_tx
            .blobs
            .iter()
            .any(|blob| blob.contract_name.0 == identity_contract_name)
        {
            bail!(
                "Can't find blob that proves the identity on contract '{}'",
                identity_contract_name
            );
        }
        Ok(())
    }

    fn verify_hyle_output(
        unsettled_tx: &mut UnsettledBlobTransaction,
        contract_name: &ContractName,
        hyle_output: &HyleOutput,
        unsettled_tx_hash: &TxHash,
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

        // Verify the contract name
        let expected_contract = &unsettled_tx.blobs[hyle_output.index.0 as usize].contract_name;
        if expected_contract != contract_name {
            bail!("Blob reference from proof for {unsettled_tx_hash} does not match the BlobTx contract name {expected_contract}");
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

        let proof_c1 = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&hyle_output_c1).unwrap()),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            proof_hash: proof_c1.proof.hash(),
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
            proof_hash: proof_c2.proof.hash(),
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
            proof_hash: proof_c1.proof.hash(),
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
            proof_hash: proof_c1.proof.hash(),
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

        let first_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&first_hyle_output).unwrap()),
        };

        let verified_first_proof = VerifiedProofTransaction {
            proof_hash: first_proof.proof.hash(),
            hyle_output: state.verify_proof(&first_proof).unwrap(),
            proof_transaction: first_proof,
        };

        let mut second_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(1));
        second_hyle_output.initial_state = first_hyle_output.next_state.clone();
        second_hyle_output.next_state = StateDigest(vec![7, 8, 9]);

        let second_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&second_hyle_output).unwrap()),
        };

        let verified_second_proof = VerifiedProofTransaction {
            proof_hash: second_proof.proof.hash(),
            hyle_output: state.verify_proof(&second_proof).unwrap(),
            proof_transaction: second_proof,
        };

        let mut third_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(2));
        third_hyle_output.initial_state = second_hyle_output.next_state.clone();
        third_hyle_output.next_state = StateDigest(vec![10, 11, 12]);

        let third_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&third_hyle_output).unwrap()),
        };

        let verified_third_proof = VerifiedProofTransaction {
            proof_hash: third_proof.proof.hash(),
            hyle_output: state.verify_proof(&third_proof).unwrap(),
            proof_transaction: third_proof,
        };

        state
            .handle_verified_proof_tx(&verified_first_proof)
            .unwrap();
        state
            .handle_verified_proof_tx(&verified_second_proof)
            .unwrap();
        state
            .handle_verified_proof_tx(&verified_third_proof)
            .unwrap();

        // Check that we did settled with the last state
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![10, 11, 12]);
    }

    fn new_verified_proof_tx(
        state: &NodeState,
        contract_name: &ContractName,
        blob_tx_hash: &TxHash,
        blob_tx: &BlobTransaction,
        blob_index: BlobIndex,
        initial_state: &[u8],
        next_state: &[u8],
    ) -> VerifiedProofTransaction {
        let mut hyle_output = make_hyle_output(blob_tx.clone(), blob_index);
        hyle_output.initial_state = StateDigest(initial_state.to_vec());
        hyle_output.next_state = StateDigest(next_state.to_vec());

        let proof = ProofTransaction {
            contract_name: contract_name.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&hyle_output).unwrap()),
        };

        VerifiedProofTransaction {
            proof_hash: proof.proof.hash(),
            hyle_output: state.verify_proof(&proof).unwrap(),
            proof_transaction: proof,
        }
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

        let first_proof_tx = new_verified_proof_tx(
            &state,
            &c1,
            &blob_tx_hash,
            &blob_tx,
            BlobIndex(0),
            &[0, 1, 2, 3],
            &[2],
        );

        let second_proof_tx_b = new_verified_proof_tx(
            &state,
            &c1,
            &blob_tx_hash,
            &blob_tx,
            BlobIndex(1),
            &[2],
            &[3],
        );

        let second_proof_tx_c = new_verified_proof_tx(
            &state,
            &c1,
            &blob_tx_hash,
            &blob_tx,
            BlobIndex(1),
            &[2],
            &[4],
        );

        let third_proof_tx = new_verified_proof_tx(
            &state,
            &c1,
            &blob_tx_hash,
            &blob_tx,
            BlobIndex(2),
            &[4],
            &[5],
        );

        state.handle_verified_proof_tx(&first_proof_tx).unwrap();
        state.handle_verified_proof_tx(&second_proof_tx_b).unwrap();
        state.handle_verified_proof_tx(&second_proof_tx_c).unwrap();
        state.handle_verified_proof_tx(&third_proof_tx).unwrap();

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
        let first_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&first_hyle_output).unwrap()),
        };

        let verified_first_proof = VerifiedProofTransaction {
            proof_hash: first_proof.proof.hash(),
            hyle_output: state.verify_proof(&first_proof).unwrap(),
            proof_transaction: first_proof,
        };

        // Create hacky proof for Blob1
        let mut another_first_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(0));
        another_first_hyle_output.initial_state = first_hyle_output.next_state.clone();
        another_first_hyle_output.next_state = first_hyle_output.initial_state.clone();

        let another_first_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&another_first_hyle_output).unwrap()),
        };

        let another_verified_first_proof = VerifiedProofTransaction {
            proof_hash: another_first_proof.proof.hash(),
            hyle_output: state.verify_proof(&another_first_proof).unwrap(),
            proof_transaction: another_first_proof,
        };

        let mut second_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(1));
        second_hyle_output.initial_state = another_first_hyle_output.next_state.clone();
        second_hyle_output.next_state = StateDigest(vec![7, 8, 9]);

        let second_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&second_hyle_output).unwrap()),
        };

        let verified_second_proof = VerifiedProofTransaction {
            proof_hash: second_proof.proof.hash(),
            hyle_output: state.verify_proof(&second_proof).unwrap(),
            proof_transaction: second_proof,
        };

        state
            .handle_verified_proof_tx(&verified_first_proof)
            .unwrap();
        state
            .handle_verified_proof_tx(&another_verified_first_proof)
            .unwrap();
        state
            .handle_verified_proof_tx(&verified_second_proof)
            .unwrap();

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
        let first_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&first_hyle_output).unwrap()),
        };

        let verified_first_proof = VerifiedProofTransaction {
            proof_hash: first_proof.proof.hash(),
            hyle_output: state.verify_proof(&first_proof).unwrap(),
            proof_transaction: first_proof,
        };

        let mut second_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(1));
        second_hyle_output.initial_state = first_hyle_output.next_state.clone();
        second_hyle_output.next_state = StateDigest(vec![7, 8, 9]);

        let second_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&second_hyle_output).unwrap()),
        };

        let verified_second_proof = VerifiedProofTransaction {
            proof_hash: second_proof.proof.hash(),
            hyle_output: state.verify_proof(&second_proof).unwrap(),
            proof_transaction: second_proof,
        };

        let mut third_hyle_output = make_hyle_output(blob_tx.clone(), BlobIndex(2));
        third_hyle_output.initial_state = first_hyle_output.next_state.clone();
        third_hyle_output.next_state = StateDigest(vec![10, 11, 12]);

        let third_proof = ProofTransaction {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            proof: ProofData::Bytes(serde_json::to_vec(&third_hyle_output).unwrap()),
        };

        let verified_third_proof = VerifiedProofTransaction {
            proof_hash: third_proof.proof.hash(),
            hyle_output: state.verify_proof(&third_proof).unwrap(),
            proof_transaction: third_proof,
        };

        state
            .handle_verified_proof_tx(&verified_first_proof)
            .unwrap();
        state
            .handle_verified_proof_tx(&verified_second_proof)
            .unwrap();
        state
            .handle_verified_proof_tx(&verified_third_proof)
            .unwrap();

        // Check that we did not settled
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![0, 1, 2, 3]);
    }
}
