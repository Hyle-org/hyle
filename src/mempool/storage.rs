use bincode::{BorrowDecode, Decode, Encode};
use derive_more::derive::Display;
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

pub type Cut = Vec<(ValidatorPublicKey, CarId)>;

fn prepare_cut(cut: &mut Cut, validator: &ValidatorPublicKey, lane: &Lane) {
    if let Some(tip) = lane.cars.last() {
        cut.push((validator.clone(), tip.id));
    }
}

#[derive(Debug, Clone)]
pub struct Storage {
    pub id: ValidatorPublicKey,
    pub pending_txs: Vec<Transaction>,
    pub lane: Lane,
    pub other_lanes: HashMap<ValidatorPublicKey, Lane>,
}

impl Display for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Replica {}", self.id)?;
        write!(f, "\nLane {}", self.lane)?;
        for (i, l) in self.other_lanes.iter() {
            write!(f, "\n - OL {}: {}", i, l)?;
        }

        Ok(())
    }
}

impl Storage {
    pub fn new(id: ValidatorPublicKey) -> Storage {
        Storage {
            id,
            pending_txs: vec![],
            lane: Lane {
                cars: vec![],
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
                prepare_cut(&mut cut, validator, &self.lane);
            } else if let Some(lane) = self.other_lanes.get(validator) {
                prepare_cut(&mut cut, validator, lane);
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

    // Called by the initial proposal validator to aggregate votes
    pub fn on_data_vote(
        &mut self,
        validator: &ValidatorPublicKey,
        car_hash: &CarHash,
    ) -> Option<()> {
        let car = self
            .lane
            .cars
            .iter_mut()
            .find(|car| &car.hash() == car_hash);

        match car {
            None => {
                warn!(
                    "Vote for Car that does not exist! ({validator}) / lane: {}",
                    self.lane
                );
                None
            }
            Some(car) => {
                if car.poa.contains(validator) {
                    warn!("{} already voted for DataProposal", validator);
                    None
                } else {
                    // FIXME: we should generate a real PoA when we gather enough signatures.
                    car.poa.insert(validator.clone());
                    Some(())
                }
            }
        }
    }

    pub fn on_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
        node_state: &NodeState,
    ) -> DataProposalVerdict {
        if data_proposal.txs.is_empty() {
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
            return DataProposalVerdict::Wait(data_proposal.parent_hash.clone());
        }
        // optimistic_node_state is here to handle the case where a contract is registered in a car that is not yet committed.
        // For performance reasons, we only clone node_state in it for unregistered contracts that are potentially in those uncommitted cars.
        let mut optimistic_node_state: Option<NodeState> = None;
        for tx in data_proposal.txs.iter() {
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
        if let (Some(parent), Some(parent_poa)) = (data_proposal.parent, &data_proposal.parent_poa)
        {
            self.update_other_lane_parent_poa(validator, parent, parent_poa)
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
        let tip_id = lane.current().map(|car| car.id);
        let parent_hash = lane.current_hash();
        // Removing proofs from transactions
        let mut txs_without_proofs = data_proposal.txs.clone();
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
            id: tip_id.unwrap_or(CarId(0)) + 1,
            parent: tip_id,
            parent_hash,
            txs: txs_without_proofs,
            poa: Poa(BTreeSet::from([self.id.clone(), validator.clone()])),
        });
    }

    fn other_lane_has_parent_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> bool {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        data_proposal.parent == lane.current().map(|car| car.id)
    }

    fn update_other_lane_parent_poa(
        &mut self,
        validator: &ValidatorPublicKey,
        parent: CarId,
        parent_poa: &[ValidatorPublicKey],
    ) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        if let Some(tip) = lane.current_mut() {
            if tip.id == parent {
                tip.poa.extend(parent_poa.iter().cloned());
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
            .any(|car| car.hash() == data_proposal.hash())
    }

    #[cfg(test)]
    fn other_lane_tip(&self, validator: &ValidatorPublicKey) -> Option<CarId> {
        self.other_lanes
            .get(validator)
            .and_then(|lane| lane.current())
            .map(|car| car.id)
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
            Some(sl) => {
                let mut wp = sl.waiting.drain(0..).collect::<Vec<_>>();
                wp.sort_by_key(|wp| wp.id);
                wp
            }
            None => vec![],
        }
    }

    // Updates local view other lane matching the validator with sent cars
    pub fn other_lane_add_missing_cars(&mut self, validator: &ValidatorPublicKey, cars: Vec<Car>) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();

        let mut ordered_cars = cars;
        ordered_cars.sort_by_key(|car| car.id);

        debug!(
            "Trying to add missing cars on lane \n {}\n {:?}",
            lane, ordered_cars
        );

        for car in ordered_cars.into_iter() {
            if car.parent == lane.current().map(|l| l.id) {
                lane.cars.push(car);
            }
        }
    }

    /// Received a new transaction when the previous DataProposal had no PoA yet
    pub fn on_new_tx(&mut self, tx: Transaction) {
        self.pending_txs.push(tx);
    }

    pub fn genesis(&self) -> bool {
        self.lane.cars.is_empty()
    }

    // Called after receiving a transaction, before broadcasting a DataProposal
    pub fn add_new_car_to_lane(&mut self, txs: Vec<Transaction>) -> CarId {
        let tip_id = self.lane.current().map(|car| car.id);
        let next_id = tip_id.unwrap_or(CarId(0)) + 1;
        let parent_hash = self.lane.current_hash();

        // FIXME: we should wait for DataProposals to have received enough votes to make them Cars
        self.lane.cars.push(Car {
            id: next_id,
            parent: tip_id,
            parent_hash,
            txs,
            poa: Poa(BTreeSet::from([self.id.clone()])),
        });

        next_id
    }

    pub fn new_data_proposal(&mut self) -> Option<DataProposal> {
        let pending_txs = std::mem::take(&mut self.pending_txs);
        if pending_txs.is_empty() {
            return None;
        }
        let parent_poa = self
            .lane
            .current()
            .map(|car| car.poa.0.clone().into_iter().collect());
        let parent_hash = self.lane.current_hash();
        let tip_id = self.add_new_car_to_lane(pending_txs.clone());
        Some(DataProposal {
            txs: pending_txs,
            id: tip_id,
            parent: if tip_id == CarId(1) {
                None
            } else {
                Some(tip_id - 1)
            },
            parent_hash,
            parent_poa,
        })
    }

    pub fn collect_lanes(&mut self, lanes: Cut) -> Vec<Transaction> {
        let mut txs = Vec::new();
        for (validator, tip) in lanes.iter() {
            if validator == &self.id {
                self.lane.collect_cars(*tip, &mut txs);
            } else if let Some(lane) = self.other_lanes.get_mut(validator) {
                lane.collect_cars(*tip, &mut txs);
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

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct DataProposal {
    pub id: CarId,
    pub parent: Option<CarId>,
    pub parent_hash: Option<CarHash>,
    pub parent_poa: Option<Vec<ValidatorPublicKey>>,
    pub txs: Vec<Transaction>,
}

impl Hashable<CarHash> for DataProposal {
    fn hash(&self) -> CarHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.id.0);
        if let Some(parent) = &self.parent {
            _ = write!(hasher, "{}", parent);
        }
        for tx in self.txs.iter() {
            hasher.update(tx.hash().0);
        }
        CarHash(hex::encode(hasher.finalize()))
    }
}

impl Display for DataProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ {:?} <- [{:?}/{:?}] }}",
            self.parent,
            self.id,
            self.txs.first()
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
pub struct CarHash(String);

impl Display for CarHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(
    Clone,
    Copy,
    Default,
    Display,
    Debug,
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Serialize,
    Deserialize,
    Encode,
    Decode,
)]
pub struct CarId(pub usize);

impl std::ops::Add for CarId {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        CarId(self.0 + other.0)
    }
}

impl std::ops::Add<usize> for CarId {
    type Output = Self;

    fn add(self, other: usize) -> Self {
        CarId(self.0 + other)
    }
}

impl std::ops::Sub<usize> for CarId {
    type Output = Self;

    fn sub(self, other: usize) -> Self {
        CarId(self.0 - other)
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Car {
    pub id: CarId,
    pub parent: Option<CarId>,
    pub parent_hash: Option<CarHash>,
    pub txs: Vec<Transaction>,
    pub poa: Poa,
}

impl Hashable<CarHash> for Car {
    fn hash(&self) -> CarHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.id.0);
        if let Some(parent) = self.parent {
            _ = write!(hasher, "{}", parent);
        }
        for tx in self.txs.iter() {
            hasher.update(tx.hash().0);
        }
        CarHash(hex::encode(hasher.finalize()))
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

impl Encode for Poa {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> std::result::Result<(), bincode::error::EncodeError> {
        self.len().encode(encoder)?;
        for pubkey in self.iter() {
            pubkey.encode(encoder)?;
        }
        Ok(())
    }
}

impl<'de> BorrowDecode<'de> for Poa {
    fn borrow_decode<D: bincode::de::BorrowDecoder<'de>>(
        decoder: &mut D,
    ) -> std::result::Result<Self, bincode::error::DecodeError> {
        let len = usize::borrow_decode(decoder)?;
        let mut bts = BTreeSet::new();
        for _ in 0..len {
            bts.insert(ValidatorPublicKey::borrow_decode(decoder)?);
        }
        Ok(Poa(bts))
    }
}

impl Decode for Poa {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> std::result::Result<Self, bincode::error::DecodeError> {
        let len = usize::decode(decoder)?;
        let mut bts = BTreeSet::new();
        for _ in 0..len {
            bts.insert(ValidatorPublicKey::decode(decoder)?);
        }
        Ok(Poa(bts))
    }
}

impl Display for Car {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}/{:?}/{}v]",
            self.id.0,
            self.txs.first(),
            self.poa.len(),
        )
    }
}

#[derive(Default, Debug, Clone)]
pub struct Lane {
    cars: Vec<Car>,
    waiting: Vec<DataProposal>,
}

impl Display for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for car in self.cars.iter() {
            match car.parent {
                None => {
                    let _ = write!(f, "{}", car);
                }
                Some(_p) => {
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

    fn collect_cars(&mut self, tip: CarId, txs: &mut Vec<Transaction>) {
        if let Some(pos) = self.cars.iter().position(|car| car.id == tip) {
            let latest_txs = std::mem::take(&mut self.cars[pos].txs);
            // collect all self.cars but the last. we need it for future cuts.
            self.cars.drain(..pos).for_each(|car| {
                Self::dedup_push_txs(txs, car.txs);
            });
            Self::dedup_push_txs(txs, latest_txs);
        } else {
            error!("Car {:?} not found !", tip);
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

    #[cfg(test)]
    fn size(&self) -> usize {
        self.cars.len()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::{
        mempool::storage::{Car, CarId, DataProposal, DataProposalVerdict, Poa, Storage},
        model::{
            Blob, BlobData, BlobReference, BlobTransaction, ContractName, Hashable, ProofData,
            ProofTransaction, RegisterContractTransaction, Transaction, TransactionData,
            ValidatorPublicKey, VerifiedProofTransaction,
        },
        node_state::NodeState,
    };
    use hyle_contract_sdk::{BlobIndex, HyleOutput, Identity, StateDigest, TxHash};

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
        let mut store = Storage::new(pubkey3.clone());

        let data_proposal1 = DataProposal {
            id: CarId(1),
            parent: None,
            parent_hash: None,
            txs: vec![make_blob_tx("test1")],
            parent_poa: None,
        };
        store.other_lane_add_data_proposal(&pubkey2, &data_proposal1);

        let data_proposal2 = DataProposal {
            id: CarId(2),
            parent: Some(CarId(1)),
            parent_hash: Some(data_proposal1.hash()),
            txs: vec![make_blob_tx("test2")],
            parent_poa: None,
        };
        store.other_lane_add_data_proposal(&pubkey2, &data_proposal2);

        let data_proposal3 = DataProposal {
            id: CarId(3),
            parent: Some(CarId(2)),
            parent_hash: Some(data_proposal2.hash()),
            txs: vec![make_blob_tx("test3")],
            parent_poa: None,
        };
        store.other_lane_add_data_proposal(&pubkey2, &data_proposal2);

        let data_proposal4 = DataProposal {
            id: CarId(4),
            parent: Some(CarId(3)),
            parent_hash: Some(data_proposal3.hash()),
            txs: vec![make_blob_tx("test4")],
            parent_poa: None,
        };
        store.other_lane_add_data_proposal(&pubkey2, &data_proposal4);

        assert_eq!(store.other_lanes.len(), 1);
        assert!(store.other_lanes.contains_key(&pubkey2));

        assert_eq!(
            *store
                .other_lanes
                .get(&pubkey2)
                .unwrap()
                .cars
                .first()
                .unwrap(),
            Car {
                id: CarId(1),
                parent: None,
                parent_hash: None,
                txs: vec![make_blob_tx("test1")],
                poa: Poa(BTreeSet::from([pubkey3.clone(), pubkey2.clone()])),
            }
        );

        let missing = store.get_missing_cars(Some(data_proposal1.hash()), &data_proposal3.hash());

        assert_eq!(missing, None);
    }

    #[test_log::test]
    fn test_vote() {
        let pubkey1 = ValidatorPublicKey(vec![1]);
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());

        let txs = vec![
            make_blob_tx("test1"),
            make_blob_tx("test2"),
            make_blob_tx("test3"),
            make_blob_tx("test4"),
        ];

        store.add_new_car_to_lane(txs.clone());

        let data_proposal = DataProposal {
            txs,
            id: CarId(1),
            parent: None,
            parent_hash: None,
            parent_poa: None,
        };

        store.other_lane_add_data_proposal(&pubkey2, &data_proposal);
        assert!(store.other_lane_has_data_proposal(&pubkey2, &data_proposal));
        assert_eq!(store.other_lane_tip(&pubkey2), Some(CarId(1)));
        store.other_lane_add_data_proposal(&pubkey1, &data_proposal);
        assert!(store.other_lane_has_data_proposal(&pubkey1, &data_proposal));
        assert_eq!(store.other_lane_tip(&pubkey1), Some(CarId(1)));

        let car = store.lane.current().expect("a car");
        assert_eq!(car.poa.len(), 1);
        assert_eq!(car.txs.len(), 4);

        store.on_data_vote(&pubkey2, &data_proposal.hash());
        store.on_data_vote(&pubkey1, &data_proposal.hash());

        let car = store.lane.current().expect("a car");
        assert_eq!(car.poa.len(), 3);
        assert!(&car.poa.contains(&pubkey3));
        assert!(&car.poa.contains(&pubkey1));
        assert!(&car.poa.contains(&pubkey2));
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
            txs: vec![register_tx, proof_tx],
            id: CarId(1),
            parent: None,
            parent_hash: None,
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
            txs: vec![proof_tx.clone()],
            id: CarId(1),
            parent: None,
            parent_hash: None,
            parent_poa: None,
        };

        let verdict = store.on_data_proposal(&pubkey2, &data_proposal, &node_state);
        assert_eq!(verdict, DataProposalVerdict::Refuse); // refused because contract not found

        let data_proposal = DataProposal {
            txs: vec![register_tx, proof_tx],
            id: CarId(1),
            parent: None,
            parent_hash: None,
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
            id: CarId(1),
            parent: None,
            parent_hash: None,
            txs: vec![register_tx],
            parent_poa: None,
        };
        store.other_lane_add_data_proposal(&pubkey2, &data_proposal1);

        let data_proposal = DataProposal {
            txs: vec![proof_tx.clone()],
            id: CarId(2),
            parent: Some(CarId(1)),
            parent_hash: Some(data_proposal1.hash()),
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
            txs: vec![register_tx, proof_tx],
            id: CarId(1),
            parent: None,
            parent_hash: None,
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
            txs: vec![proof_tx, register_tx],
            id: CarId(1),
            parent: None,
            parent_hash: None,
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
        let mut store = Storage::new(pubkey3.clone());
        let node_state = NodeState::default();

        let data_proposal1 = DataProposal {
            txs: vec![
                make_blob_tx("test1"),
                make_blob_tx("test2"),
                make_blob_tx("test3"),
            ],
            id: CarId(1),
            parent: None,
            parent_hash: None,
            parent_poa: None,
        };

        let data_proposal2 = DataProposal {
            txs: vec![
                make_blob_tx("test4"),
                make_blob_tx("test5"),
                make_blob_tx("test6"),
            ],
            id: CarId(1),
            parent: None,
            parent_hash: None,
            parent_poa: None,
        };

        let data_proposal3 = DataProposal {
            txs: vec![
                make_blob_tx("test7"),
                make_blob_tx("test8"),
                make_blob_tx("test9"),
            ],
            id: CarId(2),
            parent: Some(CarId(1)),
            parent_hash: Some(data_proposal1.hash()),
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        store.add_new_car_to_lane(data_proposal1.txs.clone());
        store.on_data_vote(&pubkey2, &data_proposal1.hash());

        assert_eq!(
            store.on_data_proposal(&pubkey2, &data_proposal2, &node_state),
            DataProposalVerdict::Vote
        );

        assert_eq!(
            store.on_data_proposal(&pubkey2, &data_proposal3, &node_state),
            DataProposalVerdict::Vote
        );

        assert_eq!(store.lane.size(), 1);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(2));

        let cut = store.new_cut(&[pubkey3.clone(), pubkey2.clone()]);
        assert!(!cut.is_empty());

        store.collect_lanes(cut);

        // should contain only the tip on all the lanes
        assert_eq!(store.lane.size(), 1);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(1));
        assert_eq!(store.lane.current().map(|car| car.id), Some(CarId(1)));
        assert_eq!(
            store
                .other_lanes
                .get(&pubkey2)
                .and_then(|l| l.current().map(|car| car.id)),
            Some(CarId(2))
        );
    }

    #[test_log::test]
    fn test_add_missing_cars() {
        let pubkey1 = ValidatorPublicKey(vec![1]);
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());

        let car1 = Car {
            id: CarId(1),
            parent: None,
            parent_hash: None,
            txs: vec![
                make_blob_tx("test1"),
                make_blob_tx("test2"),
                make_blob_tx("test3"),
                make_blob_tx("test4"),
            ],
            poa: Poa(BTreeSet::from([pubkey1.clone(), pubkey2.clone()])),
        };
        store.other_lane_add_missing_cars(
            &pubkey2,
            vec![
                car1.clone(),
                Car {
                    id: CarId(2),
                    parent: Some(CarId(1)),
                    parent_hash: Some(car1.hash()),
                    txs: vec![
                        make_blob_tx("test5"),
                        make_blob_tx("test6"),
                        make_blob_tx("test7"),
                    ],
                    poa: Poa(BTreeSet::from([pubkey1.clone(), pubkey2.clone()])),
                },
            ],
        );

        assert_eq!(store.other_lane_tip(&pubkey2), Some(CarId(2)));
    }

    #[test_log::test]
    fn test_missing_cars() {
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = Storage::new(pubkey3.clone());

        store.add_new_car_to_lane(vec![make_blob_tx("test_local1")]);
        let last_know_car_hash = store.lane.current_hash();
        store.add_new_car_to_lane(vec![make_blob_tx("test_local2")]);
        store.add_new_car_to_lane(vec![make_blob_tx("test_local3")]);
        store.add_new_car_to_lane(vec![make_blob_tx("test_local4")]);
        let data_proposal_tip_hash = store.lane.current_hash().unwrap();
        assert_eq!(store.lane.cars.len(), 4);

        let missing = store
            .get_missing_cars(last_know_car_hash.clone(), &data_proposal_tip_hash)
            .expect("missing cars");

        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0].txs[0], make_blob_tx("test_local2"));
        assert_eq!(missing[1].txs[0], make_blob_tx("test_local3"));

        let missing = store
            .get_missing_cars(None, &data_proposal_tip_hash)
            .expect("missing cars");

        assert_eq!(missing.len(), 3);
        assert_eq!(missing[0].txs[0], make_blob_tx("test_local1"));
        assert_eq!(missing[1].txs[0], make_blob_tx("test_local2"));
        assert_eq!(missing[2].txs[0], make_blob_tx("test_local3"));
    }
}
