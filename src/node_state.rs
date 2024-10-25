//! State required for participation in consensus by the node.

use crate::{
    bus::{bus_client, command_response::Query, SharedMessageBus},
    consensus::{staking::Staker, ConsensusCommand},
    data_availability::DataEvent,
    handle_messages,
    model::{
        BlobDataHash, BlobReference, Blobs, BlobsHash, Block, BlockHeight, ContractName, Hashable,
        RegisterContractTransaction, SharedRunContext, Transaction,
    },
    utils::{conf::SharedConf, logger::LogMe, modules::Module},
};
use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use hyle_contract_sdk::{BlobIndex, HyleOutput, StateDigest, TxHash};
use model::{Contract, Timeouts, UnsettledBlobMetadata, UnsettledTransaction};
use ordered_tx_map::OrderedTxMap;
use std::{
    collections::{HashMap, HashSet},
    ops::{Deref, DerefMut},
    path::PathBuf,
};
use tracing::{debug, error, info, warn};
use verifiers::{CairoVerifier, Risc0Verifier, Verifier};

pub mod model;
mod ordered_tx_map;
mod verifiers;

bus_client! {
struct NodeStateBusClient {
    sender(ConsensusCommand),
    receiver(Query<ContractName, Contract>),
    receiver(DataEvent),
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
    verifiers: HashMap<String, Box<dyn Verifier>>,
    config: SharedConf,
}

impl Module for NodeState {
    fn name() -> &'static str {
        "NodeState"
    }

    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let file = ctx
            .common
            .config
            .data_directory
            .clone()
            .join("node_state.bin");
        let store = Self::load_from_disk_or_default(file.as_path());
        let bus = NodeStateBusClient::new_from_bus(ctx.common.bus.new_handle()).await;
        let config = ctx.common.config.clone();
        let mut verifiers: HashMap<String, Box<dyn Verifier>> = HashMap::new();
        verifiers.insert("cairo".into(), Box::new(CairoVerifier));
        verifiers.insert("risc0".into(), Box::new(Risc0Verifier));
        Ok(NodeState {
            bus,
            file,
            store,
            verifiers,
            config,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start(self.config.clone())
    }
}

#[derive(Debug)]
struct UnsettledBlobReference {
    contract_name: ContractName,
    blobs_hash: BlobsHash,
    blob_index: BlobIndex,
}

impl NodeState {
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
            listen<DataEvent> event => {
                _ = self.handle_data_event(event)
                    .log_error("NodeState: Error while handling data event");
            }
        }
    }

    fn handle_data_event(&mut self, event: DataEvent) -> anyhow::Result<()> {
        match event {
            DataEvent::NewBlock(block) => {
                debug!("New block to handle: {:}", block.hash());
                self.handle_new_block(block).context("handle new block")?;
            }
        };
        Ok(())
    }

    fn handle_new_block(&mut self, block: Block) -> Result<(), Error> {
        self.clear_timeouts(&block.height);
        self.current_height = block.height;
        let txs_count = block.txs.len();
        let block_hash = block.hash();

        for tx in block.txs {
            let tx_hash = tx.hash();
            match self.handle_transaction(tx) {
                Ok(_) => debug!("Handled tx {tx_hash}"),
                Err(e) => error!("Failed handling tx {tx_hash} with error: {e}"),
            }
        }

        for validator in block.new_bonded_validators {
            self.bus
                .send(ConsensusCommand::NewBonded(validator))
                .context("Send new staker consensus command")?;
        }

        info!(
           block_hash = %block_hash,
            block_height = %block.height,
            "Handled {txs_count} transactions");

        _ = Self::save_on_disk(self.file.as_path(), &self.store);
        Ok(())
    }

    fn handle_transaction(&mut self, transaction: Transaction) -> Result<(), Error> {
        debug!("Got transaction to handle: {:?}", transaction);

        match transaction.transaction_data {
            crate::model::TransactionData::Stake(staker) => self.handle_stake_tx(staker),
            crate::model::TransactionData::Blob(tx) => self
                .handle_blobs(tx.hash(), tx.fees.clone(), true)
                .and_then(|_| self.handle_blobs(tx.hash(), tx.blobs.clone(), false))
                .and_then(|_| self.set_timeout(tx.hash())),
            crate::model::TransactionData::Proof(tx) => {
                self.handle_proof_tx(&tx.proof, &tx.blobs_references, false)
            }
            crate::model::TransactionData::RegisterContract(tx) => {
                self.handle_register_contract(tx)
            }
            crate::model::TransactionData::FeeProof(tx) => {
                debug!("Got a new fee proof transaction: {:?}", tx);
                self.handle_proof_tx(&tx.proof, &tx.blobs_references, true)
            }
        }
    }

    fn handle_register_contract(&mut self, tx: RegisterContractTransaction) -> Result<(), Error> {
        if self.contracts.contains_key(&tx.contract_name) {
            bail!("Contract {} already exists", tx.contract_name);
        }
        info!("Register contract {}", tx.contract_name);
        self.contracts.insert(
            tx.contract_name.clone(),
            Contract {
                name: tx.contract_name,
                program_id: tx.program_id,
                state: tx.state_digest,
                verifier: tx.verifier,
            },
        );

        Ok(())
    }

    // FIXME: to remove when we have a real staking smart contract
    fn handle_stake_tx(&mut self, staker: Staker) -> Result<(), Error> {
        self.bus
            .send(ConsensusCommand::NewStaker(staker))
            .context("Send new staker consensus command")?;
        Ok(())
    }

    fn handle_blobs(&mut self, tx_hash: TxHash, blobs: Blobs, fees: bool) -> Result<(), Error> {
        let blobs_hash = blobs.hash();
        let blobs_data_hash = BlobDataHash::from_vec(&blobs.blobs);
        debug!("Handle tx {}, blob {}, fees {}", tx_hash, blobs_hash, fees);
        debug!("Blobs: {:?}", blobs);

        let blobs_metadata: Vec<UnsettledBlobMetadata> = blobs
            .blobs
            .iter()
            .map(|blob| UnsettledBlobMetadata {
                contract_name: blob.contract_name.clone(),
                metadata: vec![],
            })
            .collect();

        debug!("Add transaction to state");
        self.unsettled_transactions.add(
            UnsettledTransaction {
                identity: blobs.identity,
                tx_hash: tx_hash.clone(),
                blobs_hash,
                blobs_data_hash,
                blobs_unsettled_metadata: blobs_metadata,
            },
            fees,
        );

        Ok(())
    }

    fn set_timeout(&mut self, tx: TxHash) -> Result<()> {
        self.store.timeouts.set(tx, self.store.current_height + 10);
        Ok(())
    }

    fn handle_proof_tx(
        &mut self,
        proof: &[u8],
        blobs_references: &[BlobReference],
        fees: bool,
    ) -> Result<(), Error> {
        debug!("🦄🦄 {:?}", self.unsettled_transactions);
        let references = blobs_references
            .iter()
            .map(|blob_ref| {
                self.unsettled_transactions
                    .get_for(&blob_ref.blob_tx_hash, fees)
                    .map(|tx| UnsettledBlobReference {
                        blobs_hash: tx.blobs_hash.clone(),
                        contract_name: blob_ref.contract_name.clone(),
                        blob_index: blob_ref.blob_index.clone(),
                    })
                    .context("Tx is either settled or does not exists.")
            })
            .collect::<Result<Vec<_>>>()?;

        self.handle_proof(proof, references)
    }

    fn handle_proof(
        &mut self,
        proof: &[u8],
        references: Vec<UnsettledBlobReference>,
    ) -> Result<(), Error> {
        debug!("Handle proof with references: {:?}", references);

        // Verify proof
        let blobs_metadata = match references.len() {
            0 => bail!("ProofTx needs to specify a BlobTx"),
            1 => {
                let contract_name = references.first().unwrap().contract_name.clone();
                let contract = match self.contracts.get(&contract_name) {
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
                vec![self
                    .verifiers
                    .get(verifier)
                    .context(format!("verifier {verifier} not found"))?
                    .verify_proof(proof, program_id)?]
            }
            _ => self
                .verifiers
                .get("test")
                .context("verifier test not found")?
                .verify_recursion_proof(proof)?,
        };

        debug!("Proof verified: {:?}", blobs_metadata);

        // TODO: add diverse verifications ? (without the inital state checks!).
        self.process_verifications(&references, &blobs_metadata)?;

        // If we arrived here, proof provided is OK and its outputs can now be saved
        self.save_blob_metadata(&references, blobs_metadata)?;
        let unsettled_transactions = self.unsettled_transactions.clone();
        debug!("🦄 Unsettled txs: {:?}", unsettled_transactions);

        // Only catch unique unsettled_txs that are next to be settled
        let unique_next_unsettled_txs: HashSet<&UnsettledTransaction> = references
            .iter()
            .filter_map(|blob_ref| {
                unsettled_transactions
                    .is_next_unsettled_tx(&blob_ref.contract_name, &blob_ref.blobs_hash)
                    .then(|| unsettled_transactions.get(&blob_ref.blobs_hash))
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
            info!("Transaction timed out: {}", tx);
            self.unsettled_transactions.remove_for_blob(&tx);
        }
    }

    fn process_verifications(
        &self,
        references: &[UnsettledBlobReference],
        blobs_metadata: &[HyleOutput],
    ) -> Result<(), Error> {
        debug!("Process verifications");
        // Extract unsettled tx of each blob_ref
        let unsettled_txs: Vec<&UnsettledTransaction> = references
            .iter()
            .map(|blob_ref| self.unsettled_transactions.get(&blob_ref.blobs_hash))
            .collect::<Option<Vec<&UnsettledTransaction>>>()
            .context("At lease 1 tx is either settled or does not exists")?;

        // blob_hash verification
        let extracted_blobs_hash: Vec<BlobDataHash> = blobs_metadata
            .iter()
            .map(|hyle_output| BlobDataHash::from_concatenated(&hyle_output.blobs))
            .collect();
        let initial_blobs_hash = unsettled_txs
            .iter()
            .map(|tx| tx.blobs_data_hash.clone())
            .collect::<Vec<BlobDataHash>>();

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
        references: &[UnsettledBlobReference],
        blobs_metadata: Vec<HyleOutput>,
    ) -> Result<(), Error> {
        debug!("Save blob metadata");
        for (proof_blob_key, blob_ref) in references.iter().enumerate() {
            let unsettled_tx = self
                .unsettled_transactions
                .get_mut(&blob_ref.blobs_hash)
                .context("Tx is either settled or does not exists.")?;

            unsettled_tx.blobs_unsettled_metadata[blob_ref.blob_index.0 as usize]
                .metadata
                .push(blobs_metadata[proof_blob_key].clone());
        }

        Ok(())
    }

    fn is_settlement_ready(&mut self, unsettled_tx: &UnsettledTransaction) -> bool {
        // Check for each blob if initial state is correct.
        // As tx is next to be settled, remove all metadata with incorrect initial state.
        for unsettled_blob in unsettled_tx.blobs_unsettled_metadata.iter() {
            if unsettled_blob.metadata.is_empty() {
                return false;
            }
            let contract = match self.contracts.get(&unsettled_blob.contract_name) {
                Some(contract) => contract,
                None => {
                    warn!(
                        "Tx: {}: No contract '{}' found when checking for settlement",
                        unsettled_tx.tx_hash, unsettled_blob.contract_name
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
                    unsettled_tx.tx_hash, unsettled_blob.contract_name
                );
                return false; // No valid metadata found for this blob
            }
        }
        true // All blobs have at lease one valid metadata
    }

    fn settle_tx(&mut self, unsettled_tx: &UnsettledTransaction) -> Result<(), Error> {
        info!("Settle tx {:?}", unsettled_tx.tx_hash);

        for blob in &unsettled_tx.blobs_unsettled_metadata {
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
        self.unsettled_transactions.remove(&unsettled_tx.blobs_hash);
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

    use hyle_contract_sdk::{BlobData, BlobIndex, Identity};

    use crate::{bus::SharedMessageBus, model::*, utils::conf::Conf};

    use super::*;

    pub struct TestVerifier {}
    impl Verifier for TestVerifier {
        fn verify_proof(&self, proof: &[u8], _program_id: &[u8]) -> Result<HyleOutput, Error> {
            let (blob_refs, _) = bincode::decode_from_slice::<Vec<BlobReference>, _>(
                proof,
                bincode::config::standard(),
            )?;
            info!("➡️  Verify proof blob refs: {:?}", blob_refs);
            Ok(HyleOutput {
                version: 1,
                initial_state: StateDigest(vec![0, 1, 2, 3]),
                next_state: StateDigest(vec![4, 5, 6]),
                identity: Identity("test".to_string()),
                tx_hash: blob_refs.first().unwrap().blob_tx_hash.clone(),
                index: blob_refs.first().unwrap().blob_index.clone(),
                blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
                success: true,
                program_outputs: vec![],
            })
        }
        fn verify_recursion_proof(&self, proof: &[u8]) -> Result<Vec<HyleOutput>, Error> {
            let (blob_refs, _) = bincode::decode_from_slice::<Vec<BlobReference>, _>(
                proof,
                bincode::config::standard(),
            )?;
            info!("➡️  Recursion proof blob refs: {:?}", blob_refs);
            Ok(blob_refs
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
                    program_outputs: vec![],
                })
                .collect())
        }
    }

    async fn new_node_state() -> NodeState {
        let mut verifiers: HashMap<String, Box<dyn Verifier>> = HashMap::new();
        verifiers.insert("test".into(), Box::new(TestVerifier {}));
        let mut ns = NodeState {
            bus: NodeStateBusClient::new_from_bus(SharedMessageBus::default()).await,
            file: PathBuf::default(),
            store: NodeStateStore::default(),
            verifiers,
            config: Conf::default().into(),
        };
        ns.handle_transaction(new_register_contract("hyfi".into()))
            .expect("register contract");
        ns.handle_transaction(new_register_contract("hydentity".into()))
            .expect("register contract");
        ns
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

    fn new_proof(blob_references: Vec<BlobReference>) -> ProofTransaction {
        ProofTransaction {
            blobs_references: blob_references.clone(),
            proof: bincode::encode_to_vec(blob_references, bincode::config::standard())
                .expect("blob ref"),
        }
    }

    fn fees() -> Blobs {
        Blobs {
            identity: Identity("payer1".to_string()),
            blobs: vec![new_blob(&"hyfi".into()), new_blob(&"hydentity".into())],
        }
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
            fees: fees(),
            blobs: Blobs {
                identity: Identity("test".to_string()),
                blobs: vec![new_blob(&c1), new_blob(&c2)],
            },
        };
        let blob_tx_hash = blob.hash();
        let blob_tx = new_tx(TransactionData::Blob(blob));

        let proof_c1 = new_tx(TransactionData::Proof(new_proof(vec![BlobReference {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            blob_index: BlobIndex(0),
        }])));

        let proof_c2 = new_tx(TransactionData::Proof(new_proof(vec![BlobReference {
            contract_name: c2.clone(),
            blob_tx_hash,
            blob_index: BlobIndex(1),
        }])));

        state.handle_transaction(register_c1).unwrap();
        state.handle_transaction(register_c2).unwrap();
        state.handle_transaction(blob_tx).unwrap();
        state.handle_transaction(proof_c1).unwrap();
        state.handle_transaction(proof_c2).unwrap();

        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![4, 5, 6]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![4, 5, 6]);
    }

    #[tokio::test]
    #[test_log::test]
    async fn one_proof_for_two_blobs() {
        let mut state = new_node_state().await;
        let c1 = ContractName("c1".to_string());
        let c2 = ContractName("c2".to_string());

        let register_c1 = new_register_contract(c1.clone());
        let register_c2 = new_register_contract(c2.clone());

        let blob_1 = BlobTransaction {
            fees: fees(),
            blobs: Blobs {
                identity: Identity("test1".to_string()),
                blobs: vec![new_blob(&c1), new_blob(&c2)],
            },
        };
        let blob_2 = BlobTransaction {
            fees: fees(),
            blobs: Blobs {
                identity: Identity("test".to_string()),
                blobs: vec![new_blob(&c1), new_blob(&c2)],
            },
        };
        let blob_tx_hash_1 = blob_1.hash();
        let blob_tx_hash_2 = blob_2.hash();
        let blob_tx_1 = new_tx(TransactionData::Blob(blob_1));
        let blob_tx_2 = new_tx(TransactionData::Blob(blob_2));

        let proof_c1 = new_tx(TransactionData::Proof(new_proof(vec![
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
        ])));

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
            fees: fees(),
            blobs: Blobs {
                identity: Identity("test1".to_string()),
                blobs: vec![new_blob(&c1), new_blob(&c2)],
            },
        };
        let blob_2 = BlobTransaction {
            fees: fees(),
            blobs: Blobs {
                identity: Identity("test2".to_string()),
                blobs: vec![new_blob(&c3), new_blob(&c4)],
            },
        };
        let blob_tx_hash_1 = blob_1.hash();
        debug!("🌲 Blob hash 1: {:?}", blob_tx_hash_1);
        let blob_tx_hash_2 = blob_2.hash();
        debug!("🌲 Blob hash 2: {:?}", blob_tx_hash_2);
        let blob_tx_1 = new_tx(TransactionData::Blob(blob_1));
        let blob_tx_2 = new_tx(TransactionData::Blob(blob_2));

        let proof_c1 = new_tx(TransactionData::Proof(new_proof(vec![
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
        ])));

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
            fees: fees(),
            blobs: Blobs {
                identity: Identity("test".to_string()),
                blobs: vec![new_blob(&c1), new_blob(&c2)],
            },
        };
        let blob_tx_hash = blob.hash();
        let blob_tx = new_tx(TransactionData::Blob(blob));

        let proof_c1 = new_tx(TransactionData::Proof(new_proof(vec![BlobReference {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            blob_index: BlobIndex(0),
        }])));

        let proof_c1_bis = new_tx(TransactionData::Proof(new_proof(vec![BlobReference {
            contract_name: c1.clone(),
            blob_tx_hash: blob_tx_hash.clone(),
            blob_index: BlobIndex(0),
        }])));

        state.handle_transaction(register_c1).unwrap();
        state.handle_transaction(register_c2).unwrap();
        state.handle_transaction(blob_tx).unwrap();
        state.handle_transaction(proof_c1).unwrap();
        state.handle_transaction(proof_c1_bis).unwrap();
        assert_eq!(
            state
                .unsettled_transactions
                .get_for_blobs(&blob_tx_hash)
                .unwrap()
                .blobs_unsettled_metadata[0]
                .metadata
                .len(),
            2
        );
        // Check that we did not settled
        assert_eq!(state.contracts.get(&c1).unwrap().state.0, vec![0, 1, 2, 3]);
        assert_eq!(state.contracts.get(&c2).unwrap().state.0, vec![0, 1, 2, 3]);
    }
}
