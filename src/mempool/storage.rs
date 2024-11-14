use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    collections::{BTreeSet, HashMap},
    fmt::Display,
    io::Write,
    vec,
};
use tracing::{debug, error, warn};

use crate::{
    model::{Hashable, Transaction, TransactionData, ValidatorPublicKey},
    node_state::NodeState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataProposalVerdict {
    Empty,
    Wait(Option<CarHash>),
    Vote,
    DidVote,
    Refuse,
}

pub type Cut = Vec<(ValidatorPublicKey, CarHash)>;

#[derive(Debug, Clone)]
pub struct Storage {
    pub id: ValidatorPublicKey,
    pub pending_txs: Vec<Transaction>,
    pub data_proposal: Option<DataProposal>,
    pub lane: Lane,
    pub other_lanes: HashMap<ValidatorPublicKey, Lane>,
}

impl Display for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Replica {}", self.id)?;
        write!(f, "\nLane {}", self.lane)?;
        for (i, lane) in self.other_lanes.iter() {
            write!(f, "\n - OL {}: {}", i, lane)?;
        }

        Ok(())
    }
}

impl Storage {
    pub fn new(id: ValidatorPublicKey) -> Storage {
        let lane_id = id.clone();
        Storage {
            id,
            pending_txs: vec![],
            data_proposal: None,
            lane: Lane {
                cars: vec![],
                poa: Poa(BTreeSet::from([lane_id])),
                waiting: vec![],
            },
            other_lanes: HashMap::new(),
        }
    }

    pub fn new_cut(&mut self, validators: &[ValidatorPublicKey]) -> Cut {
        // For each validator, we get the last validated car and put it in the cut
        let mut cut: Cut = vec![];
        for validator in validators.iter() {
            if validator == &self.id {
                self.lane.prepare_cut(&mut cut, validator);
            } else if let Some(lane) = self.other_lanes.get(validator) {
                lane.prepare_cut(&mut cut, validator);
            } else {
                // can happen if validator does not have any DataProposal yet
                debug!(
                    "Validator {} not found in lane of {} (cutting)",
                    validator, self.id
                );
            }
        }

        cut
    }

    pub fn commit_data_proposal(&mut self) {
        if let Some(data_proposal) = self.data_proposal.take() {
            self.lane.cars.push(data_proposal.car);
        }
    }

    // Called by the initial proposal validator to aggregate votes
    pub fn on_data_vote(
        &mut self,
        validator: &ValidatorPublicKey,
        car_hash: &CarHash,
    ) -> Result<()> {
        if let Some(current_data_proposal) = &self.data_proposal {
            if &current_data_proposal.car.hash() == car_hash {
                if self.lane.poa.contains(validator) {
                    bail!("{validator} already voted for DataProposal");
                } else {
                    self.lane.poa.insert(validator.clone());
                }
            } else {
                bail!("DataVote for wrong DataProposal ({validator})");
            }
        }
        debug!("Unexpected DataVote received from {validator} (no current DataProposal)");
        Ok(())
    }

    pub fn on_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
        node_state: &NodeState,
    ) -> DataProposalVerdict {
        if data_proposal.car.txs.is_empty() {
            return DataProposalVerdict::Empty;
        }
        if self.other_lane_has_data_proposal(validator, data_proposal) {
            return DataProposalVerdict::DidVote;
        }
        if !self.other_lane_has_parent_data_proposal(validator, data_proposal) {
            self.other_lanes
                .entry(validator.clone())
                .or_default()
                .waiting
                .push(data_proposal.clone());
            return DataProposalVerdict::Wait(data_proposal.car.parent_hash.clone());
        }
        // optimistic_node_state is here to handle the case where a contract is registered in a car that is not yet committed.
        // For performance reasons, we only clone node_state in it for unregistered contracts that are potentially in those uncommitted cars.
        let mut optimistic_node_state: Option<NodeState> = None;
        for tx in data_proposal.car.txs.iter() {
            match &tx.transaction_data {
                TransactionData::Proof(_) => {
                    warn!("Refusing DataProposal: unverified proof transaction");
                    return DataProposalVerdict::Refuse;
                }
                TransactionData::VerifiedProof(proof_tx) => {
                    // Ensure all referenced contracts are registered
                    for blob_ref in &proof_tx.proof_transaction.blobs_references {
                        if !node_state.contracts.contains_key(&blob_ref.contract_name)
                            && !optimistic_node_state.as_ref().map_or(false, |state| {
                                state.contracts.contains_key(&blob_ref.contract_name)
                            })
                        {
                            // Process previous cars to register the missing contract
                            if let Some(lane) = self.other_lanes.get_mut(validator) {
                                for car in &lane.cars {
                                    for tx in &car.txs {
                                        if let TransactionData::RegisterContract(reg_tx) =
                                            &tx.transaction_data
                                        {
                                            if reg_tx.contract_name == blob_ref.contract_name {
                                                if optimistic_node_state.is_none() {
                                                    optimistic_node_state =
                                                        Some(node_state.clone());
                                                }
                                                if optimistic_node_state
                                                    .as_mut()
                                                    .unwrap()
                                                    .handle_register_contract_tx(reg_tx)
                                                    .is_err()
                                                {
                                                    // Register transactions in a validated car should never fail
                                                    // as car is accepted only if all transactions are valid
                                                    unreachable!();
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Verifying the proof before voting
                    match optimistic_node_state
                        .as_ref()
                        .unwrap_or(node_state)
                        .verify_proof(&proof_tx.proof_transaction)
                    {
                        Ok(hyle_outputs) => {
                            if hyle_outputs != proof_tx.hyle_outputs {
                                warn!("Refusing DataProposal: incorrect HyleOutput in proof transaction");
                                return DataProposalVerdict::Refuse;
                            }
                        }
                        Err(e) => {
                            warn!("Refusing DataProposal: invalid proof transaction: {}", e);
                            return DataProposalVerdict::Refuse;
                        }
                    }
                }
                TransactionData::RegisterContract(register_contract_tx) => {
                    if optimistic_node_state.is_none() {
                        optimistic_node_state = Some(node_state.clone());
                    }
                    match optimistic_node_state
                        .as_mut()
                        .unwrap()
                        .handle_register_contract_tx(register_contract_tx)
                    {
                        Ok(_) => (),
                        Err(e) => {
                            warn!("Refusing DataProposal: {}", e);
                            return DataProposalVerdict::Refuse;
                        }
                    }
                }
                _ => {}
            }
        }
        if let (Some(parent_hash), Some(parent_poa)) =
            (&data_proposal.car.parent_hash, &data_proposal.parent_poa)
        {
            self.other_lane_update_parent_poa(validator, parent_hash, parent_poa)
        }
        self.other_lane_add_data_proposal(validator, data_proposal);
        DataProposalVerdict::Vote
    }

    fn other_lane_add_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        let parent_hash = lane.current_hash();
        // Removing proofs from transactions
        let mut txs_without_proofs = data_proposal.car.txs.clone();
        txs_without_proofs.iter_mut().for_each(|tx| {
            match &mut tx.transaction_data {
                TransactionData::VerifiedProof(proof_tx) => {
                    proof_tx.proof_transaction.proof = Default::default();
                }
                TransactionData::Proof(_) => {
                    // This can never happen.
                    // A DataProposal that has been processed has turned all TransactionData::Proof into TransactionData::VerifiedProof
                    unreachable!();
                }
                TransactionData::Blob(_)
                | TransactionData::Stake(_)
                | TransactionData::RegisterContract(_) => {}
            }
        });
        lane.cars.push(Car {
            parent_hash,
            txs: txs_without_proofs,
        });
        lane.poa.extend([self.id.clone(), validator.clone()]);
    }

    fn other_lane_has_parent_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> bool {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        data_proposal.car.parent_hash == lane.current_hash()
    }

    fn other_lane_update_parent_poa(
        &mut self,
        validator: &ValidatorPublicKey,
        parent_hash: &CarHash,
        parent_poa: &[ValidatorPublicKey],
    ) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        if let Some(car) = lane.current_mut() {
            if &car.hash() == parent_hash {
                lane.poa.extend(parent_poa.iter().cloned());
            }
        }
    }

    fn other_lane_has_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> bool {
        self.other_lanes
            .entry(validator.clone())
            .or_default()
            .cars
            .iter()
            .any(|car| car.hash() == data_proposal.car.hash())
    }

    pub fn get_missing_cars(
        &self,
        last_car_hash: Option<CarHash>,
        data_proposal_tip_hash: &CarHash,
    ) -> Option<Vec<Car>> {
        self.lane
            .get_missing_cars(last_car_hash, data_proposal_tip_hash)
    }

    pub fn get_waiting_data_proposals(
        &mut self,
        validator: &ValidatorPublicKey,
    ) -> Vec<DataProposal> {
        match self.other_lanes.get_mut(validator) {
            Some(lane) => lane.waiting.drain(0..).collect::<Vec<_>>(),
            None => vec![],
        }
    }

    // Updates local view other lane matching the validator with sent cars
    pub fn other_lane_add_missing_cars(&mut self, validator: &ValidatorPublicKey, cars: Vec<Car>) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();

        debug!(
            "Trying to add missing cars on lane \n {}\n {:?}",
            lane, cars
        );

        for car in cars.into_iter() {
            if car.parent_hash == lane.current_hash() {
                lane.cars.push(car);
            }
        }
    }

    /// Received a new transaction when the previous DataProposal had no PoA yet
    pub fn on_new_tx(&mut self, tx: Transaction) {
        self.pending_txs.push(tx);
    }

    pub fn new_data_proposal(&mut self) -> Option<DataProposal> {
        if self.pending_txs.is_empty() || self.data_proposal.is_some() {
            return None;
        }
        self.data_proposal.replace(DataProposal {
            car: Car {
                parent_hash: self.lane.current_hash(),
                txs: std::mem::take(&mut self.pending_txs),
            },
            parent_poa: if self.lane.poa.len() == 1 {
                None
            } else {
                Some(std::mem::take(&mut self.lane.poa).0.into_iter().collect())
            },
        });
        self.lane.poa.insert(self.id.clone());
        self.data_proposal.clone()
    }

    pub fn collect_lanes(&mut self, lanes: Cut) -> Vec<Transaction> {
        let mut txs = Vec::new();
        for (validator, car_hash) in lanes.iter() {
            if validator == &self.id {
                self.lane.collect_cars(car_hash, &mut txs);
            } else if let Some(lane) = self.other_lanes.get_mut(validator) {
                lane.collect_cars(car_hash, &mut txs);
            } else {
                // can happen if validator does not have any DataProposal yet
                debug!(
                    "Validator {} not found in lane of {} (updating)",
                    validator, self.id
                );
            }
        }
        txs
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct DataProposal {
    pub car: Car,
    pub parent_poa: Option<Vec<ValidatorPublicKey>>,
}

impl Display for DataProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ {:?} <- [{:?}] }}",
            self.car.parent_hash,
            self.car.txs.first()
        )
    }
}

#[derive(
    Clone,
    Debug,
    Default,
    PartialOrd,
    Ord,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Encode,
    Decode,
    Hash,
)]
pub struct CarHash(pub String);

impl Display for CarHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Car {
    pub parent_hash: Option<CarHash>,
    pub txs: Vec<Transaction>,
}

impl Hashable<CarHash> for Car {
    fn hash(&self) -> CarHash {
        let mut hasher = Sha3_256::new();
        if let Some(parent_hash) = &self.parent_hash {
            _ = write!(hasher, "{}", parent_hash);
        }
        for tx in self.txs.iter() {
            hasher.update(tx.hash().0);
        }
        CarHash(hex::encode(hasher.finalize()))
    }
}

impl Display for Car {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hash(),)
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Poa(pub BTreeSet<ValidatorPublicKey>);

impl std::ops::Deref for Poa {
    type Target = BTreeSet<ValidatorPublicKey>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Poa {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Default, Debug, Clone)]
pub struct Lane {
    cars: Vec<Car>,
    pub poa: Poa,
    waiting: Vec<DataProposal>,
}

impl Display for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for car in self.cars.iter() {
            match &car.parent_hash {
                None => {
                    let _ = write!(f, "{}", car);
                }
                Some(_car_hash) => {
                    let _ = write!(f, " <- {}", car);
                }
            }
        }

        Ok(())
    }
}

impl Lane {
    pub fn current_mut(&mut self) -> Option<&mut Car> {
        self.cars.last_mut()
    }

    pub fn current(&self) -> Option<&Car> {
        self.cars.last()
    }

    pub fn current_hash(&self) -> Option<CarHash> {
        self.current().map(|car| car.hash())
    }

    fn dedup_push_txs(txs: &mut Vec<Transaction>, car_txs: Vec<Transaction>) {
        for tx in car_txs.into_iter() {
            if !txs.contains(&tx) {
                txs.push(tx);
            }
        }
    }

    fn prepare_cut(&self, cut: &mut Cut, validator: &ValidatorPublicKey) {
        if let Some(car) = self.current() {
            cut.push((validator.clone(), car.hash()));
        }
    }

    fn collect_cars(&mut self, car_hash: &CarHash, txs: &mut Vec<Transaction>) {
        if let Some(pos) = self.cars.iter().position(|car| &car.hash() == car_hash) {
            let latest_txs = std::mem::take(&mut self.cars[pos].txs);
            // collect all self.cars but the last. we need it for future cuts.
            self.cars.drain(..pos).for_each(|car| {
                Self::dedup_push_txs(txs, car.txs);
            });
            Self::dedup_push_txs(txs, latest_txs);
        } else {
            error!("Car {:?} not found !", car_hash);
        }
    }

    fn get_missing_cars(
        &self,
        last_car_hash: Option<CarHash>,
        data_proposal_tip_hash: &CarHash,
    ) -> Option<Vec<Car>> {
        let data_proposal_car = self
            .cars
            .iter()
            .find(|car| &car.hash() == data_proposal_tip_hash);

        match data_proposal_car {
            None => {
                error!("DataProposal does not exist locally");
                None
            }
            Some(data_proposal_car) => {
                //Normally last_car_id must be < current_car since we are on the lane reference (that must have more Cars than others)
                match last_car_hash {
                    // Nothing on the lane, we send everything, up to the DataProposal id/pos
                    None => Some(
                        self.cars
                            .iter()
                            .take_while(|car| &car.hash() != data_proposal_tip_hash)
                            .cloned()
                            .collect(),
                    ),
                    // If there is an index, two cases
                    // - it matches the current tip, in this case we don't send any more Cars
                    // - it does not match, we send the diff
                    Some(last_car_hash) => {
                        if last_car_hash == data_proposal_car.hash() {
                            None
                        } else {
                            Some(
                                self.cars
                                    .iter()
                                    .skip_while(|car| car.hash() != last_car_hash)
                                    .skip(1)
                                    .take_while(|car| car.hash() != data_proposal_car.hash())
                                    .cloned()
                                    .collect(),
                            )
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        mempool::storage::{Car, DataProposal, DataProposalVerdict, Storage},
        model::{
            Blob, BlobData, BlobReference, BlobTransaction, ContractName, Hashable, ProofData,
            ProofTransaction, RegisterContractTransaction, Transaction, TransactionData,
            ValidatorPublicKey, VerifiedProofTransaction,
        },
        node_state::NodeState,
    };
    use hyle_contract_sdk::{BlobIndex, HyleOutput, Identity, StateDigest, TxHash};

    use super::CarHash;

    fn other_lane_current_hash(store: &Storage, validator: &ValidatorPublicKey) -> Option<CarHash> {
        store
            .other_lanes
            .get(validator)
            .and_then(|lane| lane.current_hash())
    }

    fn make_unverified_proof_tx() -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::Proof(ProofTransaction {
                blobs_references: vec![],
                proof: ProofData::default(),
            }),
        }
    }

    fn make_verified_proof_tx(contract_name: ContractName) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::VerifiedProof(VerifiedProofTransaction {
                proof_transaction: ProofTransaction {
                    blobs_references: vec![BlobReference {
                        contract_name,
                        blob_tx_hash: TxHash("".to_owned()),
                        blob_index: BlobIndex(0),
                    }],
                    proof: ProofData::default(),
                },
                hyle_outputs: vec![HyleOutput {
                    version: 1,
                    initial_state: StateDigest(vec![0, 1, 2, 3]),
                    next_state: StateDigest(vec![4, 5, 6]),
                    identity: Identity("test".to_string()),
                    tx_hash: TxHash("".to_owned()),
                    index: BlobIndex(0),
                    blobs: vec![0, 1, 2, 3, 0, 1, 2, 3],
                    success: true,
                    program_outputs: vec![],
                }],
            }),
        }
    }

    fn make_blob_tx(inner_tx: &'static str) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::Blob(BlobTransaction {
                identity: Identity("id".to_string()),
                blobs: vec![Blob {
                    contract_name: ContractName("c1".to_string()),
                    data: BlobData(inner_tx.as_bytes().to_vec()),
                }],
            }),
        }
    }

    fn make_register_contract_tx(name: ContractName) -> Transaction {
        Transaction {
            version: 1,
            transaction_data: TransactionData::RegisterContract(RegisterContractTransaction {
                owner: "test".to_string(),
                verifier: "test".to_string(),
                program_id: vec![],
                state_digest: StateDigest(vec![0, 1, 2, 3]),
                contract_name: name,
            }),
        }
    }

    #[test_log::test]
    fn test_workflow() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store3 = Storage::new(pubkey3.clone());
        let mut store2 = Storage::new(pubkey2.clone());
        let node_state3 = NodeState::default();

        store2.on_new_tx(make_blob_tx("test1"));
        let data_proposal1 = store2.new_data_proposal().expect("a DataProposal");
        assert_eq!(
            store3.on_data_proposal(&pubkey2, &data_proposal1, &node_state3),
            DataProposalVerdict::Vote
        );

        store2
            .on_data_vote(&pubkey3, &data_proposal1.car.hash())
            .expect("vote success");
        store2.commit_data_proposal();
        assert!(store2.data_proposal.is_none());

        store2.on_new_tx(make_blob_tx("test2"));
        let data_proposal2 = store2.new_data_proposal().expect("a DataProposal");
        assert_eq!(
            store3.on_data_proposal(&pubkey2, &data_proposal2, &node_state3),
            DataProposalVerdict::Vote
        );

        store2
            .on_data_vote(&pubkey3, &data_proposal2.car.hash())
            .expect("vote success");
        store2.commit_data_proposal();
        assert!(store2.data_proposal.is_none());

        store2.on_new_tx(make_blob_tx("test3"));
        let data_proposal3 = store2.new_data_proposal().expect("a DataProposal");
        assert_eq!(
            store3.on_data_proposal(&pubkey2, &data_proposal3, &node_state3),
            DataProposalVerdict::Vote
        );

        store2
            .on_data_vote(&pubkey3, &data_proposal3.car.hash())
            .expect("vote success");
        store2.commit_data_proposal();
        assert!(store2.data_proposal.is_none());

        store2.on_new_tx(make_blob_tx("test4"));
        let data_proposal4 = store2.new_data_proposal().expect("a DataProposal");
        assert_eq!(
            store3.on_data_proposal(&pubkey2, &data_proposal4, &node_state3),
            DataProposalVerdict::Vote
        );

        store2
            .on_data_vote(&pubkey3, &data_proposal4.car.hash())
            .expect("vote success");
        store2.commit_data_proposal();
        assert!(store2.data_proposal.is_none());

        assert_eq!(store3.other_lanes.len(), 1);
        assert!(store3.other_lanes.contains_key(&pubkey2));

        let lane2 = store3.other_lanes.get(&pubkey2).expect("lane");
        let first_car = lane2.cars.first().expect("first car");
        assert_eq!(first_car.parent_hash, None);
        assert_eq!(first_car.txs, vec![make_blob_tx("test1")]);
        assert!(lane2.poa.contains(&pubkey2));
        assert!(lane2.poa.contains(&pubkey3));

        let missing = store3.get_missing_cars(
            Some(first_car.hash()),
            &store3
                .other_lanes
                .get(&pubkey2)
                .and_then(|lane| lane.current_hash())
                .unwrap_or_default(),
        );

        assert_eq!(missing, None);
    }

    #[test_log::test]
    fn test_vote() {
        let pubkey1 = ValidatorPublicKey(vec![1]);
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store3 = Storage::new(pubkey3.clone());
        let mut store2 = Storage::new(pubkey2.clone());
        let node_state2 = NodeState::default();

        store3.on_new_tx(make_blob_tx("test1"));
        store3.on_new_tx(make_blob_tx("test2"));
        store3.on_new_tx(make_blob_tx("test3"));
        store3.on_new_tx(make_blob_tx("test4"));

        let data_proposal = store3.new_data_proposal().expect("data proposal");
        assert_eq!(store3.lane.poa.len(), 1);

        assert_eq!(
            store2.on_data_proposal(&pubkey3, &data_proposal, &node_state2),
            DataProposalVerdict::Vote
        );
        assert_eq!(
            other_lane_current_hash(&store2, &pubkey3),
            Some(CarHash(
                "84ffbeea5a3d9fe1e1f64b6af44447a5d42b2ba938e8b2f00e84161990d7a2a0".to_string()
            ))
        );
        assert_eq!(
            store2.on_data_proposal(&pubkey3, &data_proposal, &node_state2),
            DataProposalVerdict::DidVote
        );

        store3
            .on_data_vote(&pubkey2, &data_proposal.car.hash())
            .expect("success");
        assert!(store3.data_proposal.is_some());
        assert_eq!(store3.lane.poa.len(), 2);

        store3
            .on_data_vote(&pubkey1, &data_proposal.car.hash())
            .expect("success");
        store3.commit_data_proposal();
        assert!(store3.data_proposal.is_none());
        assert_eq!(store3.lane.poa.len(), 3);

        assert!(store3.lane.poa.contains(&pubkey3));
        assert!(store3.lane.poa.contains(&pubkey1));
        assert!(store3.lane.poa.contains(&pubkey2));
    }

    #[test_log::test]
    fn test_update_lane_with_unverified_proof_transaction() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());
        let node_state = NodeState::default();

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_unverified_proof_tx();

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![register_tx, proof_tx],
            },
            parent_poa: None,
        };

        let verdict = store.on_data_proposal(&pubkey2, &data_proposal, &node_state);
        assert_eq!(verdict, DataProposalVerdict::Refuse);

        // Ensure the lane was not updated with the unverified proof transaction
        assert!(!store.other_lane_has_data_proposal(&pubkey2, &data_proposal));
    }

    #[test_log::test]
    fn test_update_lane_with_verified_proof_transaction() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());
        let node_state = NodeState::default();

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![proof_tx.clone()],
            },
            parent_poa: None,
        };

        let verdict = store.on_data_proposal(&pubkey2, &data_proposal, &node_state);
        assert_eq!(verdict, DataProposalVerdict::Refuse); // refused because contract not found

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![register_tx, proof_tx],
            },
            parent_poa: None,
        };
        let verdict = store.on_data_proposal(&pubkey2, &data_proposal, &node_state);
        assert_eq!(verdict, DataProposalVerdict::Vote);
    }

    #[test_log::test]
    fn test_new_data_proposal_with_register_tx_in_previous_uncommitted_car() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());
        let node_state = NodeState::default();

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());

        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal1 = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![register_tx],
            },
            parent_poa: None,
        };
        store.other_lane_add_data_proposal(&pubkey2, &data_proposal1);

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: Some(data_proposal1.car.hash()),
                txs: vec![proof_tx.clone()],
            },
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        let verdict = store.on_data_proposal(&pubkey2, &data_proposal, &node_state);
        assert_eq!(verdict, DataProposalVerdict::Vote);

        // Ensure the lane was updated with the DataProposal
        assert!(store.other_lane_has_data_proposal(&pubkey2, &data_proposal));
    }

    #[test_log::test]
    fn test_register_contract_and_proof_tx_in_same_car() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());
        let node_state = NodeState::default();

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());
        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![register_tx, proof_tx],
            },
            parent_poa: None,
        };

        let verdict = store.on_data_proposal(&pubkey2, &data_proposal, &node_state);
        assert_eq!(verdict, DataProposalVerdict::Vote);

        // Ensure the lane was updated with the DataProposal
        assert!(store.other_lane_has_data_proposal(&pubkey2, &data_proposal));
    }

    #[test_log::test]
    fn test_register_contract_and_proof_tx_in_same_car_wrong_order() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());
        let node_state = NodeState::default();

        let contract_name = ContractName("test".to_string());
        let register_tx = make_register_contract_tx(contract_name.clone());
        let proof_tx = make_verified_proof_tx(contract_name);

        let data_proposal = DataProposal {
            car: Car {
                parent_hash: None,
                txs: vec![proof_tx, register_tx],
            },
            parent_poa: None,
        };

        let verdict = store.on_data_proposal(&pubkey2, &data_proposal, &node_state);
        assert_eq!(verdict, DataProposalVerdict::Refuse);

        // Ensure the lane was not updated with the DataProposal
        assert!(!store.other_lane_has_data_proposal(&pubkey2, &data_proposal));
    }

    #[test_log::test]
    fn test_update_lanes_after_commit() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store3 = Storage::new(pubkey3.clone());
        let node_state3 = NodeState::default();
        let mut store2 = Storage::new(pubkey2.clone());
        let node_state2 = NodeState::default();

        store3.on_new_tx(make_blob_tx("test1"));
        let data_proposal1 = store3.new_data_proposal().expect("a DataProposal");
        assert_eq!(
            store2.on_data_proposal(&pubkey3, &data_proposal1, &node_state2),
            DataProposalVerdict::Vote
        );
        store3
            .on_data_vote(&pubkey2, &data_proposal1.car.hash())
            .expect("success");
        store3.commit_data_proposal();

        store3.on_new_tx(make_blob_tx("test2"));
        let data_proposal1 = store3.new_data_proposal().expect("a DataProposal");
        assert_eq!(
            store2.on_data_proposal(&pubkey3, &data_proposal1, &node_state2),
            DataProposalVerdict::Vote
        );
        store3
            .on_data_vote(&pubkey2, &data_proposal1.car.hash())
            .expect("success");
        store3.commit_data_proposal();

        store2.on_new_tx(make_blob_tx("test3"));
        let data_proposal3 = store2.new_data_proposal().expect("a DataProposal");
        assert_eq!(
            store3.on_data_proposal(&pubkey2, &data_proposal3, &node_state3),
            DataProposalVerdict::Vote
        );
        store2
            .on_data_vote(&pubkey3, &data_proposal3.car.hash())
            .expect("success");
        store2.commit_data_proposal();

        assert_eq!(store3.lane.cars.len(), 2);
        assert_eq!(
            store3.other_lanes.get(&pubkey2).map(|lane| lane.cars.len()),
            Some(1)
        );

        let cut = store3.new_cut(&[pubkey3.clone(), pubkey2.clone()]);

        assert_eq!(store3.collect_lanes(cut.clone()).len(), 3);
        assert_eq!(store2.collect_lanes(cut).len(), 3);

        // should contain only the tip on all the lanes
        assert_eq!(store3.lane.cars.len(), 1);
        assert_eq!(store2.lane.cars.len(), 1);
        assert_eq!(
            store3.other_lanes.get(&pubkey2).map(|lane| lane.cars.len()),
            Some(1)
        );
        assert_eq!(
            store2.other_lanes.get(&pubkey3).map(|lane| lane.cars.len()),
            Some(1)
        );
    }

    #[test_log::test]
    fn test_add_missing_cars() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());

        let car1 = Car {
            parent_hash: None,
            txs: vec![make_blob_tx("test1")],
        };
        store.other_lane_add_missing_cars(
            &pubkey2,
            vec![
                car1.clone(),
                Car {
                    parent_hash: Some(car1.hash()),
                    txs: vec![make_blob_tx("test2")],
                },
            ],
        );

        assert_eq!(
            store.other_lanes.get(&pubkey2).map(|lane| lane.cars.len()),
            Some(2)
        );
    }

    #[test_log::test]
    fn test_missing_cars() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());

        store.on_new_tx(make_blob_tx("test_local1"));
        let data_proposal = store.new_data_proposal().expect("a DataProposal");
        store
            .on_data_vote(&pubkey2, &data_proposal.car.hash())
            .expect("vote success");
        store.commit_data_proposal();
        let last_car_hash = store.lane.current_hash();

        store.on_new_tx(make_blob_tx("test_local2"));
        let data_proposal = store.new_data_proposal().expect("a DataProposal");
        store
            .on_data_vote(&pubkey2, &data_proposal.car.hash())
            .expect("vote success");
        store.commit_data_proposal();

        store.on_new_tx(make_blob_tx("test_local3"));
        let data_proposal = store.new_data_proposal().expect("a DataProposal");
        store
            .on_data_vote(&pubkey2, &data_proposal.car.hash())
            .expect("vote success");
        store.commit_data_proposal();

        store.on_new_tx(make_blob_tx("test_local4"));
        let data_proposal = store.new_data_proposal().expect("a DataProposal");
        store
            .on_data_vote(&pubkey2, &data_proposal.car.hash())
            .expect("vote success");
        store.commit_data_proposal();

        assert_eq!(store.lane.cars.len(), 4);

        let missing = store
            .get_missing_cars(last_car_hash, &data_proposal.car.hash())
            .expect("missing cars");

        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0].txs[0], make_blob_tx("test_local2"));
        assert_eq!(missing[1].txs[0], make_blob_tx("test_local3"));

        let missing = store
            .get_missing_cars(None, &data_proposal.car.hash())
            .expect("missing cars");

        assert_eq!(missing.len(), 3);
        assert_eq!(missing[0].txs[0], make_blob_tx("test_local1"));
        assert_eq!(missing[1].txs[0], make_blob_tx("test_local2"));
        assert_eq!(missing[2].txs[0], make_blob_tx("test_local3"));
    }
}
