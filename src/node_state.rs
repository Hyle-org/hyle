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
use model::{Contract, Timeouts, UnsettledBlobMetadata, UnsettledTransaction};
use ordered_tx_map::OrderedTxMap;
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn};

pub mod model;
mod ordered_tx_map;
mod verifiers;

#[derive(Default, Encode, Decode, Debug)]
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
                            for hyle_output in &verified_proof_transaction.hyle_outputs {
                                verified_blobs
                                    .push((hyle_output.tx_hash.clone(), hyle_output.index.clone()));
                            }
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

        debug!("Add transaction to state");
        self.unsettled_transactions.add(UnsettledTransaction {
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
        // TODO: add diverse verifications ? (without the inital state checks!).
        self.verify_blobs_metadata(&tx.proof_transaction, &tx.hyle_outputs)?;

        // If we arrived here, proof provided is OK and its outputs can now be saved
        self.save_blob_metadata(&tx.proof_transaction, &tx.hyle_outputs)?;
        let unsettled_transactions = self.unsettled_transactions.clone();

        // Only catch unique unsettled_txs that are next to be settled
        let unique_next_unsettled_txs: HashSet<&UnsettledTransaction> = tx
            .proof_transaction
            .blobs_references
            .iter()
            .filter_map(|blob_ref| {
                let blob_tx_hash = &blob_ref.blob_tx_hash;
                if unsettled_transactions.is_next_unsettled_tx(blob_tx_hash) {
                    unsettled_transactions.get(blob_tx_hash)
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();

        debug!("Next unsettled txs: {:?}", unique_next_unsettled_txs);
        let mut settled_blob_tx_hashes = vec![];
        let mut updated_states = HashMap::new();

        for unsettled_tx in unique_next_unsettled_txs {
            if self.is_settlement_ready(unsettled_tx) {
                let state_update = self.settle_tx(unsettled_tx)?;
                // This unsettled blob transaction is now settled.
                // We want to keep track of that hash
                settled_blob_tx_hashes.push(unsettled_tx.hash.clone());
                // We want to keep track of the updated states
                updated_states.extend(state_update);
            }
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

    pub fn verify_proof(&self, tx: &ProofTransaction) -> Result<Vec<HyleOutput>, Error> {
        // TODO extract correct verifier
        let verifier: String = "test".to_owned();

        // Verify proof
        let blobs_metadata = match tx.blobs_references.len() {
            0 => bail!("ProofTx needs to specify a BlobTx"),
            1 => {
                let contract_name = &tx.blobs_references.first().unwrap().contract_name;
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
                vec![verifiers::verify_proof(tx, verifier, program_id)?]
            }
            _ => verifiers::verify_recursion_proof(tx, &verifier)?,
        };
        Ok(blobs_metadata)
    }

    fn verify_blobs_metadata(
        &self,
        tx: &ProofTransaction,
        blobs_metadata: &[HyleOutput],
    ) -> Result<(), Error> {
        // Extract unsettled tx of each blob_ref
        let unsettled_txs: Vec<&UnsettledTransaction> = tx
            .blobs_references
            .iter()
            .map(|blob_ref| self.unsettled_transactions.get(&blob_ref.blob_tx_hash))
            .collect::<Option<Vec<&UnsettledTransaction>>>()
            .context("At lease 1 tx is either settled or does not exists")?;

        // blob_hash verification
        let extracted_blobs_hash: Vec<BlobsHash> = blobs_metadata
            .iter()
            .map(|hyle_output| BlobsHash::from_concatenated(&hyle_output.blobs))
            .collect();
        let initial_blobs_hash = unsettled_txs
            .iter()
            .map(|tx| tx.blobs_hash.clone())
            .collect::<Vec<BlobsHash>>();

        if extracted_blobs_hash != initial_blobs_hash {
            bail!(
                "Proof blobs hash '{:?}' do not correspond to transaction blobs hash '{:?}'.",
                extracted_blobs_hash,
                initial_blobs_hash
            )
        }

        blobs_metadata
            .iter()
            .enumerate()
            .try_for_each(|(_index, hyle_output)| {
                // Success verification
                if !hyle_output.success {
                    bail!("Contract execution is not a success");
                }
                // Identity verification
                // TODO: uncomment when verifier are implemented
                // if hyle_output.identity != unsettled_txs[index].identity {
                //     bail!("Identity is incorrect");
                // }

                Ok(())
            })
    }

    fn save_blob_metadata(
        &mut self,
        tx: &ProofTransaction,
        blobs_metadata: &[HyleOutput],
    ) -> Result<(), Error> {
        for (proof_blob_key, blob_ref) in tx.blobs_references.iter().enumerate() {
            let unsettled_tx = self
                .unsettled_transactions
                .get_mut(&blob_ref.blob_tx_hash)
                .context("Tx is either settled or does not exists.")?;

            if blob_ref.contract_name
                != unsettled_tx.blobs[blob_ref.blob_index.0 as usize].contract_name
            {
                bail!("BlobIndex used is not linked with correct contract");
            }

            unsettled_tx.blobs[blob_ref.blob_index.0 as usize]
                .metadata
                .push(blobs_metadata[proof_blob_key].clone());
        }

        Ok(())
    }

    fn is_settlement_ready(&mut self, unsettled_tx: &UnsettledTransaction) -> bool {
        // Check for each blob if initial state is correct.
        // As tx is next to be settled, remove all metadata with incorrect initial state.
        for unsettled_blob in unsettled_tx.blobs.iter() {
            if unsettled_blob.metadata.is_empty() {
                return false;
            }
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

            if !has_one_valid_initial_state {
                info!(
                    "Tx: {}. It seems that no blob matches the initial state of contract '{}'",
                    unsettled_tx.hash, unsettled_blob.contract_name
                );
                return false; // No valid metadata found for this blob
            }
        }
        true // All blobs have at lease one valid metadata
    }

    fn settle_tx(
        &mut self,
        unsettled_tx: &UnsettledTransaction,
    ) -> Result<HashMap<ContractName, StateDigest>, Error> {
        info!("Settle tx {:?}", unsettled_tx.hash);
        let mut updated_states = HashMap::new();

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
        }
        // Clean the unsettled tx from the state
        self.unsettled_transactions.remove(&unsettled_tx.hash);

        Ok(updated_states)
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
    use hyle_contract_sdk::{BlobIndex, Identity};

    async fn new_node_state() -> NodeState {
        NodeState::default()
    }

    fn new_blob(contract: &ContractName) -> Blob {
        Blob {
            contract_name: contract.clone(),
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

    #[test_log::test(tokio::test)]
    async fn two_proof_for_one_blob_tx() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_tx = BlobTransaction {
            identity: Identity("test".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_register_contract_tx(&register_c2).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

        let proof_c1 = ProofTransaction {
            blobs_references: vec![BlobReference {
                contract_name: c1.clone(),
                blob_tx_hash: blob_tx_hash.clone(),

                blob_index: BlobIndex(0),
            }],
            proof: ProofData::Bytes(vec![]),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            hyle_outputs: state.verify_proof(&proof_c1).unwrap(),
            proof_transaction: proof_c1,
        };

        let proof_c2 = ProofTransaction {
            blobs_references: vec![BlobReference {
                contract_name: c2.clone(),
                blob_tx_hash: blob_tx_hash.clone(),

                blob_index: BlobIndex(1),
            }],
            proof: ProofData::Bytes(vec![]),
        };

        let verified_proof_c2 = VerifiedProofTransaction {
            hyle_outputs: state.verify_proof(&proof_c2).unwrap(),
            proof_transaction: proof_c2,
        };

        state.handle_verified_proof_tx(&verified_proof_c1).unwrap();
        state.handle_verified_proof_tx(&verified_proof_c2).unwrap();

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
    }

    #[ignore] // As long as proof verification is mocked, we provide different blobs on same contract
    #[test_log::test(tokio::test)]
    async fn one_proof_for_two_blobs() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_tx_1 = BlobTransaction {
            identity: Identity("test".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_tx_2 = BlobTransaction {
            identity: Identity("test".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_tx_hash_1 = blob_tx_1.hash();
        let blob_tx_hash_2 = blob_tx_2.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_register_contract_tx(&register_c2).unwrap();
        state.handle_blob_tx(&blob_tx_1).unwrap();
        state.handle_blob_tx(&blob_tx_2).unwrap();

        let proof_c1 = ProofTransaction {
            blobs_references: vec![
                BlobReference {
                    contract_name: c1.clone(),
                    blob_tx_hash: blob_tx_hash_1.clone(),
                    blob_index: BlobIndex(0),
                },
                BlobReference {
                    contract_name: c2.clone(),
                    blob_tx_hash: blob_tx_hash_1.clone(),
                    blob_index: BlobIndex(1),
                },
                BlobReference {
                    contract_name: c1.clone(),
                    blob_tx_hash: blob_tx_hash_2.clone(),
                    blob_index: BlobIndex(0),
                },
                BlobReference {
                    contract_name: c2.clone(),
                    blob_tx_hash: blob_tx_hash_2.clone(),
                    blob_index: BlobIndex(1),
                },
            ],
            proof: ProofData::Bytes(vec![]),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            hyle_outputs: state.verify_proof(&proof_c1).unwrap(),
            proof_transaction: proof_c1,
        };

        state.handle_verified_proof_tx(&verified_proof_c1).unwrap();

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
    }

    #[test_log::test(tokio::test)]
    async fn one_proof_for_two_blobs_txs_on_different_contract() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());
        let c3 = ContractName("c3".to_string());
        let c4 = ContractName("c4".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());
        let register_c3 = new_register_contract(c3.clone());
        let register_c4 = new_register_contract(c4.clone());

        let blob_tx_1 = BlobTransaction {
            identity: Identity("test1".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_tx_2 = BlobTransaction {
            identity: Identity("test2".to_string()),
            blobs: vec![new_blob(&c3), new_blob(&c4)],
        };
        let blob_tx_hash_1 = blob_tx_1.hash();
        let blob_tx_hash_2 = blob_tx_2.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_register_contract_tx(&register_c2).unwrap();
        state.handle_register_contract_tx(&register_c3).unwrap();
        state.handle_register_contract_tx(&register_c4).unwrap();
        state.handle_blob_tx(&blob_tx_1).unwrap();
        state.handle_blob_tx(&blob_tx_2).unwrap();

        let proof_c1 = ProofTransaction {
            blobs_references: vec![
                BlobReference {
                    contract_name: c1.clone(),
                    blob_tx_hash: blob_tx_hash_1.clone(),
                    blob_index: BlobIndex(0),
                },
                BlobReference {
                    contract_name: c2.clone(),
                    blob_tx_hash: blob_tx_hash_1.clone(),
                    blob_index: BlobIndex(1),
                },
                BlobReference {
                    contract_name: c3.clone(),
                    blob_tx_hash: blob_tx_hash_2.clone(),
                    blob_index: BlobIndex(0),
                },
                BlobReference {
                    contract_name: c4.clone(),
                    blob_tx_hash: blob_tx_hash_2.clone(),
                    blob_index: BlobIndex(1),
                },
            ],
            proof: ProofData::Bytes(vec![]),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            hyle_outputs: state.verify_proof(&proof_c1).unwrap(),
            proof_transaction: proof_c1,
        };

        state.handle_verified_proof_tx(&verified_proof_c1).unwrap();

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c3).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c4).unwrap().state.0, vec![4, 5, 6]);
    }

    #[test_log::test(tokio::test)]
    async fn two_proof_for_same_blob() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_tx = BlobTransaction {
            identity: Identity("test".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_register_contract_tx(&register_c2).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

        let proof_c1 = ProofTransaction {
            blobs_references: vec![BlobReference {
                contract_name: c1.clone(),
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: BlobIndex(0),
            }],
            proof: ProofData::Bytes(vec![1]),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            hyle_outputs: state.verify_proof(&proof_c1).unwrap(),
            proof_transaction: proof_c1,
        };

        let proof_c1_bis = ProofTransaction {
            blobs_references: vec![BlobReference {
                contract_name: c1.clone(),
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: BlobIndex(0),
            }],
            proof: ProofData::Bytes(vec![1]),
        };

        let verified_proof_c1_bis = VerifiedProofTransaction {
            hyle_outputs: state.verify_proof(&proof_c1_bis).unwrap(),
            proof_transaction: proof_c1_bis.clone(),
        };

        state.handle_verified_proof_tx(&verified_proof_c1).unwrap();
        state
            .handle_verified_proof_tx(&verified_proof_c1_bis)
            .unwrap();

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
    async fn proof_with_reference_for_incorrect_contract() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_tx = BlobTransaction {
            identity: Identity("test".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_tx_hash = blob_tx.hash();

        state.handle_register_contract_tx(&register_c1).unwrap();
        state.handle_register_contract_tx(&register_c2).unwrap();
        state.handle_blob_tx(&blob_tx).unwrap();

        let proof_c1 = ProofTransaction {
            blobs_references: vec![BlobReference {
                contract_name: c2.clone(), // This is the introduced error. It should be c1
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: BlobIndex(0),
            }],
            proof: ProofData::Bytes(vec![1]),
        };

        let verified_proof_c1 = VerifiedProofTransaction {
            hyle_outputs: state.verify_proof(&proof_c1).unwrap(),
            proof_transaction: proof_c1,
        };

        let result = state.handle_verified_proof_tx(&verified_proof_c1);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "BlobIndex used is not linked with correct contract"
        );
    }
}
