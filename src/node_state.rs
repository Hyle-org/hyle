use crate::{
    bus::SharedMessageBus,
    consensus::ConsensusEvent,
    model::{
        BlobIndex, BlobTransaction, BlobsHash, Block, BlockHeight, ContractName, Hashable,
        Identity, ProofTransaction, RegisterContractTransaction, StateDigest, Transaction, TxHash,
    },
    utils::{
        conf::SharedConf,
        vec_utils::{SequenceOption, SequenceResult},
    },
};
use anyhow::{bail, Context, Error, Result};
use model::{Contract, HyleOutput, Timeouts, UnsettledBlobMetadata, UnsettledTransaction};
use ordered_tx_map::OrderedTxMap;
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info};

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
    pub async fn start(&mut self, bus: SharedMessageBus, _config: SharedConf) -> Result<(), Error> {
        info!(
            "Starting NodeState with {} contracts and {} unsettled transactions at height {}",
            self.contracts.len(),
            self.unsettled_transactions.len(),
            self.current_height
        );
        let mut events = bus.receiver::<ConsensusEvent>().await;
        loop {
            if let Ok(msg) = events.recv().await {
                let res = match msg {
                    ConsensusEvent::CommitBlock { batch_id: _, block } => {
                        info!("New block to handle: {:}", block.hash());
                        self.handle_new_block(block).context("handle new block")
                    }
                };

                match res {
                    Ok(_) => (),
                    Err(e) => error!("Error while handling consensus event: {e}"),
                }
            }
        }
    }

    pub fn handle_new_block(&mut self, block: Block) -> Result<(), Error> {
        self.clear_timeouts(&block.height);
        self.current_height = block.height;
        let txs_count = block.txs.len();

        for tx in block.txs {
            let tx_hash = tx.hash();
            match self.handle_transaction(tx) {
                Ok(_) => info!("Handled tx {tx_hash}"),
                Err(e) => error!("Failed handling tx {tx_hash} with error: {e}"),
            }
        }

        info!("Handled {txs_count} transactions");
        Ok(())
    }

    fn handle_transaction(&mut self, transaction: Transaction) -> Result<(), Error> {
        debug!("Got transaction to handle: {:?}", transaction);

        match transaction.transaction_data {
            crate::model::TransactionData::Blob(tx) => self.handle_blob_tx(tx),
            crate::model::TransactionData::Proof(tx) => self.handle_proof(tx),
            crate::model::TransactionData::RegisterContract(tx) => {
                self.handle_register_contract(tx)
            }
        }
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
        // Verify proof
        let blobs_metadata: Vec<HyleOutput> = Self::verify_proof(&tx)?;

        // TODO: add diverse verifications ? (without the inital state checks!).
        self.process_verifications(&tx, &blobs_metadata)?;

        // If we arrived here, proof provided is OK and its outputs can now be saved
        self.save_blob_metadata(&tx, blobs_metadata)?;
        let cloned_self = self.clone();

        // Only catch unique unsettled_txs
        let mut seen_unsettled_tx: HashSet<TxHash> = HashSet::new();
        let mut involved_unsettled_tx: Vec<&UnsettledTransaction> = Vec::new();

        for blob_ref in tx.blobs_references {
            if seen_unsettled_tx.insert(blob_ref.blob_tx_hash.clone()) {
                match cloned_self
                    .unsettled_transactions
                    .get(&blob_ref.blob_tx_hash)
                {
                    Some(tx) => involved_unsettled_tx.push(tx),
                    None => bail!("Unsettled transaction not found. Should never happen here"),
                }
            }
        }

        for unsettled_tx in involved_unsettled_tx {
            let all_blobs_proved_at_least_once = unsettled_tx
                .blobs
                .iter()
                .all(|blob_metadata| blob_metadata.metadata.len() > 0);
            if all_blobs_proved_at_least_once {
                // FIXME: Shall we catch failed settlement for settling tx as failed ?
                let _ = self.settle_tx(unsettled_tx);
            }
        }
        debug!("Done {:?}", self);

        Ok(())
    }

    fn clear_timeouts(&mut self, height: &BlockHeight) {
        let dropped = self.timeouts.drop(height);
        for tx in dropped {
            self.unsettled_transactions.remove(&tx);
        }
    }

    fn process_verifications(
        &self,
        tx: &ProofTransaction,
        blobs_metadata: &Vec<HyleOutput>,
    ) -> Result<(), Error> {
        // Extract unsettled tx of each blob_ref
        let unsettled_txs: Vec<&UnsettledTransaction> = tx
            .blobs_references
            .iter()
            .map(|blob_ref| self.unsettled_transactions.get(&blob_ref.blob_tx_hash))
            .collect::<Vec<Option<&UnsettledTransaction>>>()
            .sequence()
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
            .try_for_each(|(index, hyle_output)| {
                // Success verification
                if hyle_output.success == false {
                    bail!("Contract execution is not a success");
                }
                // Identity verification
                // TODO: uncomment when verifier are implemented
                // else if hyle_output.identity != unsettled_txs[index].identity {
                //     return Err(anyhow!("Identity is incorrect"));
                // }
                else {
                    Ok(())
                }
            })
    }

    fn save_blob_metadata(
        &mut self,
        tx: &ProofTransaction,
        blobs_metadata: Vec<HyleOutput>,
    ) -> Result<(), Error> {
        tx.blobs_references
            .iter()
            .enumerate()
            .map(|(proof_blob_key, blob_ref)| {
                self.unsettled_transactions
                    .get_mut(&blob_ref.blob_tx_hash)
                    .map(|unsettled_tx| {
                        unsettled_tx.blobs[blob_ref.blob_index.0 as usize]
                            .metadata
                            .push(blobs_metadata[proof_blob_key].clone());
                    })
                    .context("Tx is either settled or does not exists.")
            })
            .collect::<Vec<Result<(), Error>>>()
            .sequence()
            .map(|_| ()) // transform ok result from Vec<()> to ()
    }

    fn verify_proof(tx: &ProofTransaction) -> Result<Vec<HyleOutput>, Error> {
        // TODO real verification implementation
        match tx.blobs_references.len() {
            0 => bail!("Proof not linked to any blob transaction"),
            1 => Ok(vec![HyleOutput {
                version: 1,
                initial_state: StateDigest(vec![0, 1, 2, 3]),
                next_state: StateDigest(vec![4, 5, 6]),
                identity: Identity("test".to_string()),
                tx_hash: TxHash(vec![0]),
                index: BlobIndex(0),
                blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
                success: true,
            }]),
            _ => Ok(tx
                .blobs_references
                .iter()
                .map(|blob_ref| HyleOutput {
                    version: 1,
                    initial_state: StateDigest(vec![0, 1, 2, 3]),
                    next_state: StateDigest(vec![4, 5, 6]),
                    identity: Identity("test".to_string()),
                    tx_hash: blob_ref.blob_tx_hash.clone(),
                    index: blob_ref.blob_index.clone(),
                    blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
                    success: true,
                })
                .collect()),
        }
    }

    fn settle_tx(&mut self, unsettled_tx: &UnsettledTransaction) -> Result<(), Error> {
        info!("Settle tx {:?}", unsettled_tx.hash);

        for blob in &unsettled_tx.blobs {
            let contract_name = blob.contract_name.clone();

            let contract = self.contracts.get_mut(&contract_name).with_context(|| {
                format!(
                    "Contract {} not found when settling transaction",
                    contract_name
                )
            })?;

            let is_next_to_settle = self
                .unsettled_transactions
                .is_next_unsettled_tx(&unsettled_tx.hash, &contract_name);

            if is_next_to_settle {
                let next_state = blob
                    .metadata
                    .iter()
                    .find(|hyle_output| hyle_output.initial_state == contract.state)
                    .with_context(|| "No provided proofs are based on the correct initial state")?
                    .next_state
                    .clone();

                self.update_state_contract(&contract_name, next_state)?;
            }
        }
        // Clean the unsettled tx from the state
        self.unsettled_transactions.remove(&unsettled_tx.hash);
        Ok(())
    }

    fn update_state_contract(
        &mut self,
        contract_name: &ContractName,
        next_state: StateDigest,
    ) -> Result<(), Error> {
        let contract = self
            .contracts
            .get_mut(contract_name)
            .with_context(|| format!("Contract {} not found when settling", contract_name,))?;
        debug!("Update {} contract state: {:?}", contract_name, next_state);
        contract.state = next_state;
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
                state_digest: StateDigest(vec![0, 1, 2, 3]),
                contract_name: name,
            },
        ))
    }

    #[test_log::test]
    fn two_proof_for_one_blob_tx() {
        let mut state = NodeState::default();
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
            blobs_references: vec![BlobReference {
                contract_name: c1.clone(),
                blob_tx_hash: blob_tx_hash.clone(),

                blob_index: BlobIndex(0),
            }],
            proof: vec![],
        }));

        let proof_c2 = new_tx(TransactionData::Proof(ProofTransaction {
            blobs_references: vec![BlobReference {
                contract_name: c2.clone(),
                blob_tx_hash,
                blob_index: BlobIndex(1),
            }],
            proof: vec![],
        }));

        state.handle_transaction(register_c1).unwrap();
        state.handle_transaction(register_c2).unwrap();
        state.handle_transaction(blob_tx).unwrap();
        state.handle_transaction(proof_c1).unwrap();
        state.handle_transaction(proof_c2).unwrap();

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
    }

    #[ignore] // As long as proof verification is mocked, we provide different blobs on same contract
    #[test_log::test]
    fn one_proof_for_two_blobs() {
        let mut state = NodeState::default();
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
            proof: vec![],
        }));

        state.handle_transaction(register_c1).unwrap();
        state.handle_transaction(register_c2).unwrap();
        state.handle_transaction(blob_tx_1).unwrap();
        state.handle_transaction(blob_tx_2).unwrap();
        state.handle_transaction(proof_c1).unwrap();

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
    }

    #[test_log::test]
    fn one_proof_for_two_blobs_txs_on_different_contract() {
        let mut state = NodeState::default();
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());
        let c3 = ContractName("c3".to_string());
        let c4 = ContractName("c4".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());
        let register_c3 = new_register_contract(c3.clone());
        let register_c4 = new_register_contract(c4.clone());

        let blob_1 = BlobTransaction {
            identity: Identity("test1".to_string()),
            blobs: vec![new_blob(&c1), new_blob(&c2)],
        };
        let blob_2 = BlobTransaction {
            identity: Identity("test2".to_string()),
            blobs: vec![new_blob(&c3), new_blob(&c4)],
        };
        let blob_tx_hash_1 = blob_1.hash();
        let blob_tx_hash_2 = blob_2.hash();
        let blob_tx_1 = new_tx(TransactionData::Blob(blob_1));
        let blob_tx_2 = new_tx(TransactionData::Blob(blob_2));

        let proof_c1 = new_tx(TransactionData::Proof(ProofTransaction {
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
            proof: vec![],
        }));

        state.handle_transaction(register_c1).unwrap();
        state.handle_transaction(register_c2).unwrap();
        state.handle_transaction(register_c3).unwrap();
        state.handle_transaction(register_c4).unwrap();
        state.handle_transaction(blob_tx_1).unwrap();
        state.handle_transaction(blob_tx_2).unwrap();
        state.handle_transaction(proof_c1).unwrap();

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c3).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c4).unwrap().state.0, vec![4, 5, 6]);
    }

    #[test_log::test]
    fn two_proof_for_same_blob() {
        let mut state = NodeState::default();
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
            blobs_references: vec![BlobReference {
                contract_name: c1.clone(),
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: BlobIndex(0),
            }],
            proof: vec![1],
        }));

        let proof_c1_bis = new_tx(TransactionData::Proof(ProofTransaction {
            blobs_references: vec![BlobReference {
                contract_name: c1.clone(),
                blob_tx_hash: blob_tx_hash.clone(),
                blob_index: BlobIndex(0),
            }],
            proof: vec![2],
        }));

        state.handle_transaction(register_c1).unwrap();
        state.handle_transaction(register_c2).unwrap();
        state.handle_transaction(blob_tx).unwrap();
        state.handle_transaction(proof_c1).unwrap();
        state.handle_transaction(proof_c1_bis).unwrap();
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
        // TODO: other checks?
    }
}
