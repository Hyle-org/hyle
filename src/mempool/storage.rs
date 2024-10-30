use bincode::{BorrowDecode, Decode, Encode};
use derive_more::derive::Display;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt::Display,
    hash::Hash,
    vec,
};
use tracing::{debug, error, warn};

use crate::{
    model::{Transaction, TransactionData, ValidatorPublicKey},
    node_state::NodeState,
};

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TipInfo {
    pub pos: CarId,
    pub parent: Option<CarId>,
    pub poa: Vec<ValidatorPublicKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalVerdict {
    Empty,
    Wait(Option<CarId>),
    Vote,
    DidVote,
    Refuse,
}

pub type Cut = Vec<(ValidatorPublicKey, CarId)>;

fn prepare_cut(cut: &mut Cut, validator: &ValidatorPublicKey, lane: &mut Lane) {
    if let Some(tip) = lane.cars.last() {
        cut.push((validator.clone(), tip.id));
    }
}

#[derive(Debug, Clone)]
pub struct InMemoryStorage {
    pub id: ValidatorPublicKey,
    pub pending_txs: Vec<Transaction>,
    pub lane: Lane,
    pub other_lanes: HashMap<ValidatorPublicKey, Lane>,
}

impl Display for InMemoryStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Replica {}", self.id)?;
        write!(f, "\nLane {}", self.lane)?;
        for (i, l) in self.other_lanes.iter() {
            write!(f, "\n - OL {}: {}", i, l)?;
        }

        Ok(())
    }
}

impl InMemoryStorage {
    pub fn new(id: ValidatorPublicKey) -> InMemoryStorage {
        InMemoryStorage {
            id,
            pending_txs: vec![],
            lane: Lane {
                cars: vec![],
                waiting: HashSet::new(),
            },
            other_lanes: HashMap::new(),
        }
    }

    pub fn tip_poa(&self) -> Option<Vec<ValidatorPublicKey>> {
        self.lane
            .current()
            .map(|car| car.poa.iter().cloned().collect())
    }

    pub fn try_new_cut(&mut self, validators: &[ValidatorPublicKey]) -> Option<Cut> {
        if let Some(car) = self.lane.current() {
            if car.poa.len() > validators.len() / 3 {
                let mut cut = Vec::new();
                for validator in validators.iter() {
                    if validator == &self.id {
                        prepare_cut(&mut cut, validator, &mut self.lane);
                    } else if let Some(lane) = self.other_lanes.get_mut(validator) {
                        prepare_cut(&mut cut, validator, lane);
                    } else {
                        // can happen if validator does not have any car proposal yet
                        debug!(
                            "Validator {} not found in lane of {} (cutting)",
                            validator, self.id
                        );
                    }
                }
                return Some(cut);
            }
        }
        None
    }

    // Called after receiving a transaction, before broadcasting a car proposal
    pub fn add_new_car_to_lane(&mut self, txs: Vec<Transaction>) -> CarId {
        let tip_id = self.lane.current().map(|car| car.id);

        let current_id = tip_id.unwrap_or(CarId(0)) + 1;

        self.lane.cars.push(Car {
            id: current_id,
            parent: tip_id,
            txs,
            poa: Poa(BTreeSet::from([self.id.clone()])),
        });

        current_id
    }

    // Called by the initial proposal validator to aggregate votes
    pub fn new_vote_for_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: &CarProposal,
    ) -> Option<()> {
        let car = self
            .lane
            .cars
            .iter_mut()
            .find(|c| c.id == car_proposal.id && c.txs == car_proposal.txs);

        match car {
            None => {
                warn!(
                    "Vote for Car that does not exist! ({validator}) / lane: {}",
                    self.lane
                );
                None
            }
            Some(c) => {
                if c.poa.contains(validator) {
                    warn!("{} already voted for Car proposal", validator);
                    None
                } else {
                    c.poa.insert(validator.clone());
                    Some(())
                }
            }
        }
    }

    pub fn new_car_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: &CarProposal,
        node_state: &NodeState,
    ) -> ProposalVerdict {
        if car_proposal.txs.is_empty() {
            return ProposalVerdict::Empty;
        }
        if self.other_lane_has_proposal(validator, car_proposal) {
            return ProposalVerdict::DidVote;
        }
        if !self.other_lane_has_parent_proposal(validator, car_proposal) {
            self.proposal_will_wait(validator, car_proposal.clone());
            return ProposalVerdict::Wait(car_proposal.parent);
        }
        for tx in car_proposal.txs.iter() {
            match &tx.transaction_data {
                TransactionData::Proof(_) => {
                    warn!("Refusing Car Proposal: unverified proof transaction");
                    return ProposalVerdict::Refuse;
                }
                TransactionData::VerifiedProof(proof_tx) => {
                    // Verifying the proof before voting
                    match proof_tx.proof_transaction.clone().verify(node_state) {
                        Ok(verified_proof_tx) => {
                            if verified_proof_tx.hyle_outputs != proof_tx.hyle_outputs {
                                warn!("Refusing Car proposal: incorrect HyleOutput in proof transaction");
                                return ProposalVerdict::Refuse;
                            }
                        }
                        Err(e) => {
                            warn!("Refusing Car Proposal: invalid proof transaction: {}", e);
                            return ProposalVerdict::Refuse;
                        }
                    }
                }
                _ => {}
            }
        }
        if let (Some(parent), Some(parent_poa)) = (car_proposal.parent, &car_proposal.parent_poa) {
            self.update_other_lane_parent_poa(validator, parent, parent_poa)
        }
        self.other_lane_add_proposal(validator, car_proposal);
        ProposalVerdict::Vote
    }

    fn other_lane_add_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: &CarProposal,
    ) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        let tip_id = lane.current().map(|car| car.id);
        // Removing proofs from transactions
        let mut txs_without_proofs = car_proposal.txs.clone();
        txs_without_proofs.iter_mut().for_each(|tx| {
            match &mut tx.transaction_data {
                TransactionData::VerifiedProof(proof_tx) => {
                    proof_tx.proof_transaction.proof = Default::default();
                }
                TransactionData::Proof(_) => {
                    // This can never happen
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
            txs: txs_without_proofs,
            poa: Poa(BTreeSet::from([self.id.clone(), validator.clone()])),
        });
    }

    fn other_lane_has_parent_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: &CarProposal,
    ) -> bool {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        car_proposal.parent == lane.current().map(|car| car.id)
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

    fn other_lane_has_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        car_proposal: &CarProposal,
    ) -> bool {
        self.other_lanes
            .entry(validator.clone())
            .or_default()
            .cars
            .iter()
            .any(|c| c.id == car_proposal.id && c.txs == car_proposal.txs)
    }

    #[cfg(test)]
    fn other_lane_tip(&self, validator: &ValidatorPublicKey) -> Option<CarId> {
        self.other_lanes
            .get(validator)
            .and_then(|lane| lane.current())
            .map(|c| c.id)
    }

    pub fn get_missing_cars(
        &self,
        last_car_id: Option<CarId>,
        car_proposal: &CarProposal,
    ) -> Option<Vec<Car>> {
        let car = self
            .lane
            .cars
            .iter()
            .find(|c| c.id == car_proposal.id && c.txs == car_proposal.txs);

        match car {
            None => {
                error!("Car proposal does exist locally");
                None
            }
            Some(c) => {
                //Normally last_car_id must be < current_car since we are on the lane reference (that must have more Cars than others)
                match last_car_id {
                    // Nothing on the lane, we send everything, up to the Car proposal id/pos
                    None => Some(
                        self.lane
                            .cars
                            .iter()
                            .take_while(|car| car.id < c.id)
                            .cloned()
                            .collect(),
                    ),
                    // If there is an index, two cases
                    // - it matches the current tip, in this case we don't send any more Cars
                    // - it does not match, we send the diff
                    Some(last_car_id) => {
                        if last_car_id == c.id {
                            None
                        } else {
                            Some(
                                self.lane
                                    .cars
                                    .iter()
                                    .skip_while(|car| car.id <= last_car_id)
                                    .take_while(|car| car.id < c.id)
                                    .cloned()
                                    .collect(),
                            )
                        }
                    }
                }
            }
        }
    }

    pub fn get_waiting_proposals(&mut self, validator: &ValidatorPublicKey) -> Vec<CarProposal> {
        match self.other_lanes.get_mut(validator) {
            Some(sl) => {
                let mut wp = sl.waiting.drain().collect::<Vec<_>>();
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

        for c in ordered_cars.into_iter() {
            if c.parent == lane.current().map(|l| l.id) {
                lane.cars.push(c);
            }
        }
    }

    // Called when validate return
    fn proposal_will_wait(&mut self, validator: &ValidatorPublicKey, car_proposal: CarProposal) {
        self.other_lanes
            .entry(validator.clone())
            .or_default()
            .waiting
            .insert(car_proposal);
    }

    /// Received a new transaction when the previous Car proposal had no PoA yet
    pub fn add_new_tx(&mut self, tx: Transaction) {
        self.pending_txs.push(tx);
    }

    pub fn tip_info(&self) -> Option<(TipInfo, Vec<Transaction>)> {
        self.lane.current().map(|car| {
            (
                TipInfo {
                    pos: car.id,
                    parent: car.parent,
                    poa: car.poa.0.clone().into_iter().collect(),
                },
                car.txs.clone(),
            )
        })
    }

    fn flush_pending_txs(&mut self) -> Vec<Transaction> {
        std::mem::take(&mut self.pending_txs)
    }

    pub fn genesis(&self) -> bool {
        self.lane.cars.is_empty()
    }

    pub fn try_car_proposal(
        &mut self,
        parent_poa: Option<Vec<ValidatorPublicKey>>,
    ) -> Option<CarProposal> {
        let pending_txs = self.flush_pending_txs();
        if pending_txs.is_empty() {
            return None;
        }
        let tip_id = self.add_new_car_to_lane(pending_txs.clone());
        Some(CarProposal {
            txs: pending_txs,
            id: tip_id,
            parent: if tip_id == CarId(1) {
                None
            } else {
                Some(tip_id - 1)
            },
            parent_poa,
        })
    }

    fn dedup_push_txs(txs: &mut Vec<Transaction>, car_txs: Vec<Transaction>) {
        for tx in car_txs.into_iter() {
            if !txs.contains(&tx) {
                txs.push(tx);
            }
        }
    }

    fn collect_old_used_cars(cars: &mut Vec<Car>, tip: CarId, txs: &mut Vec<Transaction>) {
        if let Some(pos) = cars.iter().position(|car| car.id == tip) {
            let latest_txs = std::mem::take(&mut cars[pos].txs);
            // collect all cars but the last. we need it for future cuts.
            cars.drain(..pos).for_each(|car| {
                Self::dedup_push_txs(txs, car.txs);
            });
            Self::dedup_push_txs(txs, latest_txs);
        } else {
            error!("Car {:?} not found !", tip);
        }
    }

    pub fn update_lanes_after_commit(&mut self, lanes: Cut) -> Vec<Transaction> {
        let mut txs = Vec::new();
        for (validator, tip) in lanes.iter() {
            if validator == &self.id {
                Self::collect_old_used_cars(&mut self.lane.cars, *tip, &mut txs);
            } else if let Some(lane) = self.other_lanes.get_mut(validator) {
                Self::collect_old_used_cars(&mut lane.cars, *tip, &mut txs);
            } else {
                // can happen if validator does not have any car proposal yet
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
pub struct CarProposal {
    pub id: CarId,
    pub parent: Option<CarId>,
    pub parent_poa: Option<Vec<ValidatorPublicKey>>,
    pub txs: Vec<Transaction>,
}

impl Display for CarProposal {
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
    id: CarId,
    parent: Option<CarId>,
    txs: Vec<Transaction>,
    pub poa: Poa,
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
    waiting: HashSet<CarProposal>,
}

impl Display for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for c in self.cars.iter() {
            match c.parent {
                None => {
                    let _ = write!(f, "{}", c);
                }
                Some(_p) => {
                    let _ = write!(f, " <- {}", c);
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

    #[cfg(test)]
    fn size(&self) -> usize {
        self.cars.len()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::{
        mempool::storage::{Car, CarId, CarProposal, InMemoryStorage, Poa, ProposalVerdict},
        model::{
            Blob, BlobData, BlobTransaction, ContractName, ProofData, ProofTransaction,
            Transaction, TransactionData, ValidatorPublicKey,
        },
        node_state::NodeState,
    };
    use hyle_contract_sdk::Identity;

    fn make_tx(inner_tx: &'static str) -> Transaction {
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

    #[test]
    fn test_workflow() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());

        store.other_lane_add_proposal(
            &pubkey2,
            &CarProposal {
                id: CarId(1),
                parent: None,
                txs: vec![make_tx("test1")],
                parent_poa: None,
            },
        );

        store.other_lane_add_proposal(
            &pubkey2,
            &CarProposal {
                id: CarId(2),
                parent: Some(CarId(1)),
                txs: vec![make_tx("test2")],
                parent_poa: None,
            },
        );

        store.other_lane_add_proposal(
            &pubkey2,
            &CarProposal {
                id: CarId(3),
                parent: Some(CarId(2)),
                txs: vec![make_tx("test3")],
                parent_poa: None,
            },
        );

        store.other_lane_add_proposal(
            &pubkey2,
            &CarProposal {
                id: CarId(4),
                parent: Some(CarId(3)),
                txs: vec![make_tx("test4")],
                parent_poa: None,
            },
        );

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
                txs: vec![make_tx("test1")],
                poa: Poa(BTreeSet::from([pubkey3.clone(), pubkey2.clone()])),
            }
        );

        let missing = store.get_missing_cars(
            Some(CarId(1)),
            &CarProposal {
                id: CarId(4),
                parent: Some(CarId(3)),
                txs: vec![make_tx("test4")],
                parent_poa: None,
            },
        );

        assert_eq!(missing, None);
    }

    #[test]
    fn test_vote() {
        let pubkey1 = ValidatorPublicKey(vec![1]);
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());

        store.add_new_tx(make_tx("test1"));
        store.add_new_tx(make_tx("test2"));
        store.add_new_tx(make_tx("test3"));
        store.add_new_tx(make_tx("test4"));

        let txs = store.flush_pending_txs();
        store.add_new_car_to_lane(txs.clone());

        let car_proposal = CarProposal {
            txs,
            id: CarId(1),
            parent: None,
            parent_poa: None,
        };

        store.other_lane_add_proposal(&pubkey2, &car_proposal);
        assert!(store.other_lane_has_proposal(&pubkey2, &car_proposal));
        assert_eq!(store.other_lane_tip(&pubkey2), Some(CarId(1)));
        store.other_lane_add_proposal(&pubkey1, &car_proposal);
        assert!(store.other_lane_has_proposal(&pubkey1, &car_proposal));
        assert_eq!(store.other_lane_tip(&pubkey1), Some(CarId(1)));

        let some_tip = store.tip_info();
        assert!(some_tip.is_some());
        let (tip, txs) = some_tip.unwrap();
        assert_eq!(tip.poa.len(), 1);
        assert_eq!(txs.len(), 4);

        store.new_vote_for_proposal(&pubkey2, &car_proposal);
        store.new_vote_for_proposal(&pubkey1, &car_proposal);

        let some_tip = store.tip_info();
        assert!(some_tip.is_some());
        let tip = some_tip.unwrap().0;
        assert_eq!(tip.poa.len(), 3);
        assert!(&tip.poa.contains(&pubkey3));
        assert!(&tip.poa.contains(&pubkey1));
        assert!(&tip.poa.contains(&pubkey2));
    }

    #[test]
    fn test_update_lane_with_unverified_proof_transaction() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());
        let node_state = NodeState::default();

        let proof_tx = Transaction {
            version: 1,
            transaction_data: TransactionData::Proof(ProofTransaction {
                blobs_references: vec![],
                proof: ProofData::default(),
            }),
        };

        let car_proposal = CarProposal {
            txs: vec![proof_tx.clone()],
            id: 1,
            parent: None,
            parent_poa: None,
        };

        let verdict = store.new_car_proposal(&pubkey2, &car_proposal, &node_state);
        assert_eq!(verdict, ProposalVerdict::Refuse);

        // Ensure the lane was not updated with the unverified proof transaction
        assert!(!store.other_lane_has_proposal(&pubkey2, &car_proposal));
    }

    #[test]
    fn test_update_lanes_after_commit() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());
        let node_state = NodeState::default();

        let car_proposal1 = CarProposal {
            txs: vec![make_tx("test1"), make_tx("test2"), make_tx("test3")],
            id: CarId(1),
            parent: None,
            parent_poa: None,
        };

        let car_proposal2 = CarProposal {
            txs: vec![make_tx("test4"), make_tx("test5"), make_tx("test6")],
            id: CarId(1),
            parent: None,
            parent_poa: None,
        };

        let car_proposal3 = CarProposal {
            txs: vec![make_tx("test7"), make_tx("test8"), make_tx("test9")],
            id: CarId(2),
            parent: Some(CarId(1)),
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        store.add_new_car_to_lane(car_proposal1.txs.clone());
        store.new_vote_for_proposal(&pubkey2, &car_proposal1);

        assert_eq!(
            store.new_car_proposal(&pubkey2, &car_proposal2, &node_state),
            ProposalVerdict::Vote
        );

        assert_eq!(
            store.new_car_proposal(&pubkey2, &car_proposal3, &node_state),
            ProposalVerdict::Vote
        );

        assert_eq!(store.lane.size(), 1);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(2));

        let cut = store.try_new_cut(&[pubkey3.clone(), pubkey2.clone()]);
        assert!(cut.is_some());

        store.update_lanes_after_commit(cut.unwrap());

        // should contain only the tip on all the lanes
        assert_eq!(store.lane.size(), 1);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(1));
        assert_eq!(store.lane.current().map(|c| c.id), Some(CarId(1)));
        assert_eq!(
            store
                .other_lanes
                .get(&pubkey2)
                .and_then(|l| l.current().map(|c| c.id)),
            Some(CarId(2))
        );
    }

    #[test]
    fn test_add_missing_cars() {
        let pubkey1 = ValidatorPublicKey(vec![1]);
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());

        store.other_lane_add_missing_cars(
            &pubkey2,
            vec![
                Car {
                    id: CarId(1),
                    parent: None,
                    txs: vec![
                        make_tx("test1"),
                        make_tx("test2"),
                        make_tx("test3"),
                        make_tx("test4"),
                    ],
                    poa: Poa(BTreeSet::from([pubkey1.clone(), pubkey2.clone()])),
                },
                Car {
                    id: CarId(2),
                    parent: Some(CarId(1)),
                    txs: vec![make_tx("test5"), make_tx("test6"), make_tx("test7")],
                    poa: Poa(BTreeSet::from([pubkey1.clone(), pubkey2.clone()])),
                },
            ],
        );

        assert_eq!(store.other_lane_tip(&pubkey2), Some(CarId(2)));
    }

    #[test]
    fn test_missing_cars() {
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());

        store.add_new_car_to_lane(vec![make_tx("test_local")]);
        store.add_new_car_to_lane(vec![make_tx("test_local2")]);
        store.add_new_car_to_lane(vec![make_tx("test_local3")]);
        store.add_new_car_to_lane(vec![make_tx("test_local4")]);
        assert_eq!(store.lane.cars.len(), 4);

        let missing = store.get_missing_cars(
            Some(CarId(1)),
            &CarProposal {
                id: CarId(4),
                parent: Some(CarId(3)),
                txs: vec![make_tx("test_local4")],
                parent_poa: None,
            },
        );

        assert_eq!(
            missing,
            Some(vec![
                Car {
                    id: CarId(2),
                    parent: Some(CarId(1)),
                    txs: vec![make_tx("test_local2")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                },
                Car {
                    id: CarId(3),
                    parent: Some(CarId(2)),
                    txs: vec![make_tx("test_local3")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                }
            ])
        );

        let missing = store.get_missing_cars(
            None,
            &CarProposal {
                id: CarId(4),
                parent: Some(CarId(3)),
                txs: vec![make_tx("test_local4")],
                parent_poa: None,
            },
        );

        assert_eq!(
            missing,
            Some(vec![
                Car {
                    id: CarId(1),
                    parent: None,
                    txs: vec![make_tx("test_local")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                },
                Car {
                    id: CarId(2),
                    parent: Some(CarId(1)),
                    txs: vec![make_tx("test_local2")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                },
                Car {
                    id: CarId(3),
                    parent: Some(CarId(2)),
                    txs: vec![make_tx("test_local3")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                }
            ])
        );
    }
}
