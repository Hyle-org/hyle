use bincode::{BorrowDecode, Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Display,
    hash::Hash,
    vec,
};
use tracing::{debug, error, warn};

use crate::model::{Transaction, ValidatorPublicKey};

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TipInfo {
    pub pos: usize,
    pub parent: Option<usize>,
    pub poa: Vec<ValidatorPublicKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalVerdict {
    Empty,
    Wait(Option<usize>),
    Vote,
    DidVote,
}

pub type Cut = BTreeMap<ValidatorPublicKey, usize>;

fn prepare_cut(cut: &mut Cut, validator: &ValidatorPublicKey, lane: &mut Lane) {
    if let Some(tip) = lane.cars.last() {
        if !tip.used_in_cut {
            cut.insert(validator.clone(), tip.id);
        }
    }
    lane.cars
        .iter_mut()
        .filter(|car| !car.used_in_cut)
        .for_each(|car| {
            car.used_in_cut = true;
        });
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

    pub fn try_new_cut(&mut self, nb_validators: usize) -> Option<Cut> {
        if let Some(car) = self.lane.current() {
            if car.poa.len() > nb_validators / 3 {
                let mut cut = BTreeMap::new();
                prepare_cut(&mut cut, &self.id, &mut self.lane);
                for (validator, lane) in self.other_lanes.iter_mut() {
                    prepare_cut(&mut cut, validator, lane);
                }
                return Some(cut);
            }
        }
        None
    }

    // Called after receiving a transaction, before broadcasting a car proposal
    pub fn add_new_car_to_lane(&mut self, txs: Vec<Transaction>) -> usize {
        let tip_id = self.lane.current().map(|car| car.id);

        let current_id = tip_id.unwrap_or(0) + 1;

        self.lane.cars.push(Car {
            id: current_id,
            parent: tip_id,
            txs,
            poa: Poa(BTreeSet::from([self.id.clone()])),
            used_in_cut: false,
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
        lane.cars.push(Car {
            id: tip_id.unwrap_or(0) + 1,
            parent: tip_id,
            txs: car_proposal.txs.clone(),
            poa: Poa(BTreeSet::from([self.id.clone(), validator.clone()])),
            used_in_cut: false,
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
        parent: usize,
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
    fn other_lane_tip(&self, validator: &ValidatorPublicKey) -> Option<usize> {
        self.other_lanes
            .get(validator)
            .and_then(|lane| lane.current())
            .map(|c| c.id)
    }

    pub fn get_missing_cars(
        &self,
        last_index: Option<usize>,
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
                //Normally last_index must be < current_car since we are on the lane reference (that must have more Cars than others)
                match last_index {
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
                    Some(last_index) => {
                        if last_index == c.id {
                            None
                        } else {
                            Some(
                                self.lane
                                    .cars
                                    .iter()
                                    .skip_while(|car| car.id <= last_index)
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
        self.pending_txs.drain(0..).collect()
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
            parent: if tip_id == 1 { None } else { Some(tip_id - 1) },
            parent_poa,
        })
    }

    fn collect_old_used_cars(cars: &mut Vec<Car>, tip: usize, txs: &mut HashSet<Transaction>) {
        cars.retain_mut(|car| {
            txs.extend(std::mem::take(&mut car.txs));
            car.id >= tip
        });
    }

    pub fn update_lanes_after_commit(&mut self, lanes: Cut) -> Vec<Transaction> {
        let mut txs = HashSet::new();
        if let Some(tip) = lanes.get(&self.id) {
            Self::collect_old_used_cars(&mut self.lane.cars, *tip, &mut txs);
        }
        for (validator, lane) in self.other_lanes.iter_mut() {
            if let Some(tip) = lanes.get(validator) {
                Self::collect_old_used_cars(&mut lane.cars, *tip, &mut txs);
            }
        }
        txs.drain().collect()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct CarProposal {
    pub id: usize,
    pub parent: Option<usize>,
    pub parent_poa: Option<Vec<ValidatorPublicKey>>,
    pub txs: Vec<Transaction>,
}

impl Display for CarProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ {:?} <- [{}/{:?}] }}",
            self.parent,
            self.id,
            self.txs.first()
        )
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Car {
    id: usize,
    parent: Option<usize>,
    txs: Vec<Transaction>,
    pub poa: Poa,
    used_in_cut: bool,
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
            "[{}/{:?}/{}v] (used:{})",
            self.id,
            self.txs.first(),
            self.poa.len(),
            self.used_in_cut,
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
        mempool::storage::{Car, CarProposal, InMemoryStorage, Poa, ProposalVerdict},
        model::{
            Blob, BlobData, BlobTransaction, ContractName, Transaction, TransactionData,
            ValidatorPublicKey,
        },
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
                id: 1,
                parent: None,
                txs: vec![make_tx("test1")],
                parent_poa: None,
            },
        );

        store.other_lane_add_proposal(
            &pubkey2,
            &CarProposal {
                id: 2,
                parent: Some(1),
                txs: vec![make_tx("test2")],
                parent_poa: None,
            },
        );

        store.other_lane_add_proposal(
            &pubkey2,
            &CarProposal {
                id: 3,
                parent: Some(2),
                txs: vec![make_tx("test3")],
                parent_poa: None,
            },
        );

        store.other_lane_add_proposal(
            &pubkey2,
            &CarProposal {
                id: 4,
                parent: Some(3),
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
                id: 1,
                parent: None,
                txs: vec![make_tx("test1")],
                poa: Poa(BTreeSet::from([pubkey3.clone(), pubkey2.clone()])),
                used_in_cut: false,
            }
        );

        let missing = store.get_missing_cars(
            Some(1),
            &CarProposal {
                id: 4,
                parent: Some(3),
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
            id: 1,
            parent: None,
            parent_poa: None,
        };

        store.other_lane_add_proposal(&pubkey2, &car_proposal);
        assert!(store.other_lane_has_proposal(&pubkey2, &car_proposal));
        assert_eq!(store.other_lane_tip(&pubkey2), Some(1));
        store.other_lane_add_proposal(&pubkey1, &car_proposal);
        assert!(store.other_lane_has_proposal(&pubkey1, &car_proposal));
        assert_eq!(store.other_lane_tip(&pubkey1), Some(1));

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
    fn test_update_lanes_after_commit() {
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());

        let car_proposal1 = CarProposal {
            txs: vec![make_tx("test1"), make_tx("test2"), make_tx("test3")],
            id: 1,
            parent: None,
            parent_poa: None,
        };

        let car_proposal2 = CarProposal {
            txs: vec![make_tx("test4"), make_tx("test5"), make_tx("test6")],
            id: 1,
            parent: None,
            parent_poa: None,
        };

        let car_proposal3 = CarProposal {
            txs: vec![make_tx("test7"), make_tx("test8"), make_tx("test9")],
            id: 2,
            parent: Some(1),
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        store.add_new_car_to_lane(car_proposal1.txs.clone());
        store.new_vote_for_proposal(&pubkey2, &car_proposal1);

        assert_eq!(
            store.new_car_proposal(&pubkey2, &car_proposal2),
            ProposalVerdict::Vote
        );

        assert_eq!(
            store.new_car_proposal(&pubkey2, &car_proposal3),
            ProposalVerdict::Vote
        );

        assert_eq!(store.lane.size(), 1);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(2));

        let cut = store.try_new_cut(2);
        assert!(cut.is_some());

        store.update_lanes_after_commit(cut.unwrap());

        // should contain only the tip on all the lanes
        assert_eq!(store.lane.size(), 1);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(1));
        assert_eq!(store.lane.current().map(|c| c.id), Some(1));
        assert_eq!(
            store
                .other_lanes
                .get(&pubkey2)
                .and_then(|l| l.current().map(|c| c.id)),
            Some(2)
        );
        // the tips should be marked as used in a cut
        assert_eq!(store.lane.current().map(|car| car.used_in_cut), Some(true));
        assert_eq!(
            store
                .other_lanes
                .get(&pubkey2)
                .and_then(|lane| lane.cars.first())
                .map(|car| car.used_in_cut),
            Some(true)
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
                    id: 1,
                    parent: None,
                    txs: vec![
                        make_tx("test1"),
                        make_tx("test2"),
                        make_tx("test3"),
                        make_tx("test4"),
                    ],
                    poa: Poa(BTreeSet::from([pubkey1.clone(), pubkey2.clone()])),
                    used_in_cut: false,
                },
                Car {
                    id: 2,
                    parent: Some(1),
                    txs: vec![make_tx("test5"), make_tx("test6"), make_tx("test7")],
                    poa: Poa(BTreeSet::from([pubkey1.clone(), pubkey2.clone()])),
                    used_in_cut: false,
                },
            ],
        );

        assert_eq!(store.other_lane_tip(&pubkey2), Some(2));
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
            Some(1),
            &CarProposal {
                id: 4,
                parent: Some(3),
                txs: vec![make_tx("test_local4")],
                parent_poa: None,
            },
        );

        assert_eq!(
            missing,
            Some(vec![
                Car {
                    id: 2,
                    parent: Some(1),
                    txs: vec![make_tx("test_local2")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                    used_in_cut: false,
                },
                Car {
                    id: 3,
                    parent: Some(2),
                    txs: vec![make_tx("test_local3")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                    used_in_cut: false,
                }
            ])
        );

        let missing = store.get_missing_cars(
            None,
            &CarProposal {
                id: 4,
                parent: Some(3),
                txs: vec![make_tx("test_local4")],
                parent_poa: None,
            },
        );

        assert_eq!(
            missing,
            Some(vec![
                Car {
                    id: 1,
                    parent: None,
                    txs: vec![make_tx("test_local")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                    used_in_cut: false,
                },
                Car {
                    id: 2,
                    parent: Some(1),
                    txs: vec![make_tx("test_local2")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                    used_in_cut: false,
                },
                Car {
                    id: 3,
                    parent: Some(2),
                    txs: vec![make_tx("test_local3")],
                    poa: Poa(BTreeSet::from_iter(vec![pubkey3.clone()])),
                    used_in_cut: false,
                }
            ])
        );
    }
}
