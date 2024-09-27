//! State required for participation in consensus by the node.

use crate::{
    bus::{bus_client, command_response::Query, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{
        BlobTransaction, BlobsHash, Block, BlockHeight, ContractName, Hashable, ProofTransaction,
        RegisterContractTransaction, SharedRunContext, StateDigest, Transaction, TxHash,
    },
    utils::{conf::SharedConf, logger::LogMe, modules::Module},
};
use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use model::{Contract, HyleOutput, Timeouts, UnsettledBlobMetadata, UnsettledTransaction};
use ordered_tx_map::OrderedTxMap;
use std::{
    collections::{HashMap, HashSet},
    ops::{Deref, DerefMut},
    path::PathBuf,
};
use tracing::{debug, error, info, warn};

pub mod model;
mod ordered_tx_map;
mod verifiers;

bus_client! {
struct NodeStateBusClient {
    receiver(Query<ContractName, Contract>),
    receiver(ConsensusEvent),
}
}

#[derive(Default, Encode, Decode)]
pub struct NodeStateStore {
    timeouts: Timeouts,
    current_height: BlockHeight,
    contracts: HashMap<ContractName, Contract>,
    unsettled_transactions: OrderedTxMap,
}

pub struct NodeState {
    bus: NodeStateBusClient,
    file: PathBuf,
    store: NodeStateStore,
}

impl Module for NodeState {
    fn name() -> &'static str {
        "NodeState"
    }

    type Context = SharedRunContext;

    async fn build(ctx: &Self::Context) -> Result<Self> {
        let file = ctx.config.data_directory.clone().join("node_state.bin");
        let store = Self::load_from_disk_or_default(file.as_path());
        Ok(NodeState::new(ctx.bus.new_handle(), file, store).await)
    }

    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        self.start(ctx.config.clone())
    }
}

impl NodeState {
    pub async fn new(bus: SharedMessageBus, file: PathBuf, store: NodeStateStore) -> NodeState {
        NodeState {
            bus: NodeStateBusClient::new_from_bus(bus).await,
            file,
            store,
        }
    }

    pub async fn start(&mut self, _config: SharedConf) -> Result<(), Error> {
        info!(
            "Starting NodeState with {} contracts and {} unsettled transactions at height {}",
            self.contracts.len(),
            self.unsettled_transactions.len(),
            self.current_height
        );

        handle_messages! {
            on_bus self.bus,
            command_response<ContractName, Contract> cmd => {
                self.contracts.get(cmd).cloned().context("Contract not found")
            }
            listen<ConsensusEvent> event => {
                _ = self.handle_event(event)
                    .log_error("NodeState: Error while handling consensus event");
            }
        }
    }

    fn handle_event(&mut self, event: ConsensusEvent) -> anyhow::Result<()> {
        match event {
            ConsensusEvent::CommitBlock { block } => {
                info!("New block to handle: {:}", block.hash());
                self.handle_new_block(block).context("handle new block")?;
            }
        };
        Ok(())
    }

    fn handle_new_block(&mut self, block: Block) -> Result<(), Error> {
        self.clear_timeouts(&block.height);
        self.current_height = block.height;
        let txs_count = block.txs.len();

        for tx in block.txs {
            let tx_hash = tx.hash();
            match self.handle_transaction(tx) {
                Ok(_) => debug!("Handled tx {tx_hash}"),
                Err(e) => error!("Failed handling tx {tx_hash} with error: {e}"),
            }
        }

        info!("Handled {txs_count} transactions");
        _ = Self::save_on_disk(self.file.as_path(), &self.store);
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
        self.store
            .timeouts
            .set(blob_tx_hash, self.store.current_height + 10); // TODO: Timeout after 10 blocks, make it configurable !

        Ok(())
    }

    fn handle_proof(&mut self, tx: ProofTransaction) -> Result<(), Error> {
        // TODO extract correct verifier
        let verifier: String = "test".to_owned();
        // Verify proof
        let blobs_metadata: Vec<HyleOutput> = verifiers::verify_proof(&tx, &verifier)?;

        // TODO: add diverse verifications ? (without the inital state checks!).
        self.process_verifications(&tx, &blobs_metadata)?;

        // If we arrived here, proof provided is OK and its outputs can now be saved
        self.save_blob_metadata(&tx, blobs_metadata)?;
        let unsettled_transactions = self.unsettled_transactions.clone();

        // Only catch unique unsettled_txs that are next to be settled
        let unique_next_unsettled_txs: HashSet<&UnsettledTransaction> = tx
            .blobs_references
            .iter()
            .filter_map(|blob_ref| {
                unsettled_transactions
                    .is_next_unsettled_tx(&blob_ref.blob_tx_hash)
                    .then(|| unsettled_transactions.get(&blob_ref.blob_tx_hash))
                    .flatten()
            })
            .collect::<HashSet<_>>();

        debug!("Next unsettled txs: {:?}", unique_next_unsettled_txs);
        for unsettled_tx in unique_next_unsettled_txs {
            if self.is_settlement_ready(unsettled_tx) {
                self.settle_tx(unsettled_tx)?;
                // FIXME: Shall we catch failed settlement for settling tx as failed ?
            }
        }
        debug!("Done! Contract states: {:?}", self.contracts);

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
        blobs_metadata: Vec<HyleOutput>,
    ) -> Result<(), Error> {
        for (proof_blob_key, blob_ref) in tx.blobs_references.iter().enumerate() {
            let unsettled_tx = self
                .unsettled_transactions
                .get_mut(&blob_ref.blob_tx_hash)
                .context("Tx is either settled or does not exists.")?;

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
                    "Tx: {}: No initial state match current contract state for contract '{}'",
                    unsettled_tx.hash, unsettled_blob.contract_name
                );
                return false; // No valid metadata found for this blob
            }
        }
        true // All blobs have at lease one valid metadata
    }

    fn settle_tx(&mut self, unsettled_tx: &UnsettledTransaction) -> Result<(), Error> {
        info!("Settle tx {:?}", unsettled_tx.hash);

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
            self.update_state_contract(&blob.contract_name, next_state)?;
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
        let contract = self.contracts.get_mut(contract_name).context(format!(
            "Contract {} not found when settling",
            contract_name,
        ))?;
        debug!("Update {} contract state: {:?}", contract_name, next_state);
        contract.state = next_state;
        Ok(())
    }
}

// TODO: move it somewhere else ?
fn hash_transaction(tx: &BlobTransaction) -> (TxHash, BlobsHash) {
    (tx.hash(), tx.blobs_hash())
}

impl Deref for NodeState {
    type Target = NodeStateStore;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl DerefMut for NodeState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.store
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::{bus::SharedMessageBus, model::*};

    use super::*;

    async fn new_node_state() -> NodeState {
        NodeState::new(
            SharedMessageBus::default(),
            PathBuf::default(),
            NodeStateStore::default(),
        )
        .await
    }

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

    #[tokio::test]
    #[test_log::test]
    async fn two_proof_for_one_blob_tx() {
        let mut state = new_node_state().await;
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
    #[tokio::test]
    #[test_log::test]
    async fn one_proof_for_two_blobs() {
        let mut state = new_node_state().await;
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

    #[tokio::test]
    #[test_log::test]
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

    #[tokio::test]
    #[test_log::test]
    async fn two_proof_for_same_blob() {
        let mut state = new_node_state().await;
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
    }
}
