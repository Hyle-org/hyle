use std::collections::HashMap;

use anyhow::{bail, Context, Error, Result};
use ordered_tx_map::OrderedTxMap;
use tracing::{debug, info};

use crate::{
    model::{
        BlobReference, BlobTransaction, BlobsHash, Block, BlockHeight, ContractName, Hashable,
        Identity, ProofTransaction, RegisterContractTransaction, StateDigest, Transaction, TxHash,
    },
    utils::vec_utils::{SequenceOption, SequenceResult},
};
use model::{Contract, Timeouts, UnsettledBlobDetail, UnsettledTransaction, VerificationStatus};

mod model;
mod ordered_tx_map;

#[derive(Default, Debug, Clone)]
pub struct NodeState {
    timeouts: Timeouts,
    current_height: BlockHeight,
    contracts: HashMap<ContractName, Contract>,
    unsettled_transactions: OrderedTxMap,
}

impl NodeState {
    pub fn handle_new_block(&self, block: Block) -> NodeState {
        let mut new_state = self.clone();

        new_state.clear_timeouts(&block.height);
        new_state.current_height = block.height;
        new_state
    }

    pub fn handle_transaction(&self, transaction: Transaction) -> Result<NodeState, Error> {
        debug!("Got transaction to handle: {:?}", transaction);
        let mut new_state = self.clone();

        let res = match transaction.transaction_data {
            crate::model::TransactionData::Blob(tx) => new_state.handle_blob_tx(tx),
            crate::model::TransactionData::Proof(tx) => new_state.handle_proof(tx),
            crate::model::TransactionData::RegisterContract(tx) => {
                new_state.handle_register_contract(tx)
            }
        };

        res.map(|_| new_state)
    }

    fn handle_register_contract(&mut self, tx: RegisterContractTransaction) -> Result<(), Error> {
        self.contracts.insert(
            tx.contract_name.clone(),
            Contract {
                name: tx.contract_name,
                program_id: tx.program_id,
                state: tx.state_digest,
            },
        );

        Ok(())
    }

    fn handle_blob_tx(&mut self, tx: BlobTransaction) -> Result<(), Error> {
        let (blob_tx_hash, blobs_hash) = hash_transaction(&tx);

        let blobs: Vec<UnsettledBlobDetail> = tx
            .blobs
            .iter()
            .map(|blob| UnsettledBlobDetail {
                contract_name: blob.contract_name.clone(),
                verification_status: VerificationStatus::WaitingProof,
            })
            .collect();

        debug!("Add transaction to state");
        self.unsettled_transactions.add(UnsettledTransaction {
            identity: tx.identity,
            hash: blob_tx_hash.clone(),
            blobs_hash,
            blobs,
        });

        // Update timeouts
        self.timeouts.set(blob_tx_hash, self.current_height + 10); // TODO: Timeout after 10 blocks, make it configurable !

        Ok(())
    }

    fn handle_proof(&mut self, tx: ProofTransaction) -> Result<(), Error> {
        // Diverse verifications
        let unsettled_tx: Vec<&UnsettledTransaction> = tx
            .blobs_references
            .iter()
            .map(|blob_ref| self.unsettled_transactions.get(&blob_ref.blob_tx_hash))
            .collect::<Vec<Option<&UnsettledTransaction>>>()
            .sequence()
            .context("At lease 1 tx is either settled or does not exists")?;

        // Verify proof
        let blobs_detail = Self::verify_proof(&tx)?;

        // hash payloads
        let extracted_blobs_hash = Self::extract_blobs_hash(&blobs_detail)?;

        let initial_blobs_hash = unsettled_tx
            .iter()
            .map(|tx| tx.blobs_hash.clone())
            .collect::<Vec<BlobsHash>>();
        // some verifications
        if extracted_blobs_hash != initial_blobs_hash {
            bail!(
                "Proof blobs hash '{:?}' do not correspond to transaction blobs hash '{:?}'.",
                extracted_blobs_hash,
                initial_blobs_hash
            )
        }

        self.save_blob_details(&tx, blobs_detail)?;

        tx.blobs_references
            .iter()
            .map(|blob_ref| {
                let is_next_to_settle = self
                    .unsettled_transactions
                    .is_next_unsettled_tx(&blob_ref.blob_tx_hash, &tx.contract_name);

                // check if tx can be settled
                if is_next_to_settle && self.is_ready_for_settlement(&blob_ref.blob_tx_hash) {
                    // settle tx
                    return self.settle_tx(blob_ref);
                }
                Ok(())
            })
            .collect::<Vec<Result<(), Error>>>()
            .sequence()
            .map(|_| ())?;

        debug!("Done {:?}", self);

        Ok(())
    }

    fn clear_timeouts(&mut self, height: &BlockHeight) {
        let dropped = self.timeouts.drop(height);
        for tx in dropped {
            self.unsettled_transactions.remove(&tx);
        }
    }

    fn save_blob_details(
        &mut self,
        tx: &ProofTransaction,
        blobs_detail: Vec<UnsettledBlobDetail>,
    ) -> Result<(), Error> {
        tx.blobs_references
            .iter()
            .enumerate()
            .map(|(proof_blob_key, blob_ref)| {
                self.unsettled_transactions
                    .get_mut(&blob_ref.blob_tx_hash)
                    .map(|unsettled_tx| {
                        unsettled_tx.blobs[blob_ref.blob_index.0 as usize] =
                            blobs_detail[proof_blob_key].clone();
                    })
                    .context("Tx is either settled or does not exists.")
            })
            .collect::<Vec<Result<(), Error>>>()
            .sequence()
            .map(|_| ()) // transform ok result from Vec<()> to ()
    }

    fn is_ready_for_settlement(&self, tx_hash: &TxHash) -> bool {
        let tx = match self.unsettled_transactions.get(tx_hash) {
            Some(tx) => tx,
            None => {
                return false;
            }
        };

        tx.blobs
            .iter()
            .all(|blob| blob.verification_status.is_success())
    }

    fn verify_proof(tx: &ProofTransaction) -> Result<Vec<UnsettledBlobDetail>, Error> {
        // TODO real implementation
        Ok(tx
            .blobs_references
            .iter()
            .map(|blob_ref| UnsettledBlobDetail {
                contract_name: tx.contract_name.clone(),
                verification_status: VerificationStatus::Success(model::HyleOutput {
                    version: 1,
                    initial_state: StateDigest(vec![0, 1, 2, 3]),
                    next_state: StateDigest(vec![4, 5, 6]),
                    identity: Identity("test".to_string()),
                    tx_hash: blob_ref.blob_tx_hash.clone(),
                    index: blob_ref.blob_index.clone(),
                    blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
                    success: true,
                }),
            })
            .collect())
    }

    fn extract_blobs_hash(blobs: &[UnsettledBlobDetail]) -> Result<Vec<BlobsHash>, Error> {
        blobs
            .iter()
            .map(|blob| match &blob.verification_status {
                VerificationStatus::Success(hyle_output) => {
                    Ok(BlobsHash::from_concatenated(&hyle_output.blobs))
                }
                _ => {
                    bail!("Blob details if not success, cannot extract blobs hash!");
                }
            })
            .collect()
    }

    // TODO rewrite this function and update_state_contract to avoid re-query of unsettled_tx
    fn settle_tx(&mut self, blob_ref: &BlobReference) -> Result<(), Error> {
        info!("Settle tx {:?}", blob_ref);
        let unsettled_tx = match self.unsettled_transactions.get_mut(&blob_ref.blob_tx_hash) {
            Some(tx) => tx,
            None => bail!("Tx to settle not found!"),
        };
        let contracts = unsettled_tx
            .blobs
            .iter()
            .map(|b| b.contract_name.clone())
            .collect::<Vec<ContractName>>();

        for contract_name in &contracts {
            self.update_state_contract(contract_name, &blob_ref.blob_tx_hash)?;
        }

        Ok(())
    }

    fn update_state_contract(
        &mut self,
        contract_name: &ContractName,
        tx: &TxHash,
    ) -> Result<(), Error> {
        let unsettled_tx = match self.unsettled_transactions.get_mut(tx) {
            Some(tx) => tx,
            None => bail!("Tx to settle not found!"),
        };

        let contract = self.contracts.get_mut(contract_name).with_context(|| {
            format!(
                "Contract {} not found when settling transaction {}",
                contract_name, tx,
            )
        })?;

        let blob_detail = unsettled_tx
            .blobs
            .iter()
            .find(|b| b.contract_name == *contract_name)
            .with_context(|| {
                format!(
                    "Blob not found for contract {} on transaction to settle: {}",
                    contract_name, tx
                )
            })?;

        match &blob_detail.verification_status {
            VerificationStatus::Success(hyle_output) => {
                debug!("Update contract state: {:?}", hyle_output.next_state);
                contract.state = hyle_output.next_state.clone();
            }
            _ => {
                bail!("Blob detail is not success, tx is not settled!")
            }
        }
        Ok(())
    }
}

// TODO: move it somewhere else ?
fn hash_transaction(tx: &BlobTransaction) -> (TxHash, BlobsHash) {
    (tx.hash(), tx.blobs_hash())
}

#[cfg(test)]
mod test {
    use crate::model::*;

    use super::NodeState;

    fn new_blob(contract: &ContractName) -> Blob {
        Blob {
            contract_name: contract.clone(),
            data: BlobData(vec![0, 1, 2, 3]),
        }
    }

    fn new_tx(transaction_data: TransactionData) -> Transaction {
        Transaction {
            version: 1,
            transaction_data,
            inner: "useless".to_string(),
        }
    }

    fn new_register_contract(name: ContractName) -> Transaction {
        new_tx(TransactionData::RegisterContract(
            RegisterContractTransaction {
                owner: "test".to_string(),
                verifier: "test".to_string(),
                program_id: vec![],
                state_digest: StateDigest(vec![]),
                contract_name: name,
            },
        ))
    }

    #[test_log::test]
    fn two_proof_for_one_blob_tx() {
        let state = NodeState::default();
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob = BlobTransaction {
            identity: Identity("test".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_tx_hash = blob.hash();
        let blob_tx = new_tx(TransactionData::Blob(blob));

        let proof_c1 = new_tx(TransactionData::Proof(ProofTransaction {
            contract_name: c1.clone(),
            blobs_references: vec![BlobReference {
                blob_tx_hash: blob_tx_hash.clone(),

                blob_index: BlobIndex(0),
            }],
            proof: vec![],
        }));

        let proof_c2 = new_tx(TransactionData::Proof(ProofTransaction {
            contract_name: c2.clone(),
            blobs_references: vec![BlobReference {
                blob_tx_hash,
                blob_index: BlobIndex(1),
            }],
            proof: vec![],
        }));

        let new_state = state
            .handle_transaction(register_c1)
            .unwrap()
            .handle_transaction(register_c2)
            .unwrap()
            .handle_transaction(blob_tx)
            .unwrap()
            .handle_transaction(proof_c1)
            .unwrap()
            .handle_transaction(proof_c2)
            .unwrap();

        assert_eq!(new_state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(new_state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
    }

    #[test_log::test]
    fn one_proof_for_two_blobs() {
        let state = NodeState::default();
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_1 = BlobTransaction {
            identity: Identity("test".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_2 = BlobTransaction {
            identity: Identity("test".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_tx_hash_1 = blob_1.hash();
        let blob_tx_hash_2 = blob_2.hash();
        let blob_tx_1 = new_tx(TransactionData::Blob(blob_1));
        let blob_tx_2 = new_tx(TransactionData::Blob(blob_2));

        let proof_c1 = new_tx(TransactionData::Proof(ProofTransaction {
            contract_name: c1.clone(),
            blobs_references: vec![
                BlobReference {
                    blob_tx_hash: blob_tx_hash_1.clone(),
                    blob_index: BlobIndex(0),
                },
                BlobReference {
                    blob_tx_hash: blob_tx_hash_1.clone(),
                    blob_index: BlobIndex(1),
                },
                BlobReference {
                    blob_tx_hash: blob_tx_hash_2.clone(),
                    blob_index: BlobIndex(0),
                },
                BlobReference {
                    blob_tx_hash: blob_tx_hash_2.clone(),
                    blob_index: BlobIndex(1),
                },
            ],
            proof: vec![],
        }));

        let new_state = state
            .handle_transaction(register_c1)
            .unwrap()
            .handle_transaction(register_c2)
            .unwrap()
            .handle_transaction(blob_tx_1)
            .unwrap()
            .handle_transaction(blob_tx_2)
            .unwrap()
            .handle_transaction(proof_c1)
            .unwrap();

        assert_eq!(new_state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert!(new_state.contracts.get(&c2).unwrap().state.0.is_empty());
    }
}
