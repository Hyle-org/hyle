use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Display,
    hash::Hash,
    vec,
};
use tracing::{debug, error, warn};

use crate::model::{Transaction, ValidatorPublicKey};

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TipData {
    pub pos: usize,
    pub parent: Option<usize>,
    pub poa: Vec<ValidatorPublicKey>,
}

#[derive(Debug, Clone)]
pub enum ProposalVerdict {
    Wait(Option<usize>),
    Vote,
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

pub type Cut = BTreeMap<ValidatorPublicKey, Option<CutCar>>;

#[derive(Debug, Default, Clone, Deserialize, Serialize, Encode, Decode)]
pub struct CutWithTxs {
    pub tips: Cut,
    pub txs: Vec<Transaction>,
}

impl CutWithTxs {
    fn extend_from_lane(
        &mut self,
        validator: &ValidatorPublicKey,
        lane: &mut Lane,
        txs: &mut HashSet<Transaction>,
    ) {
        self.tips.insert(
            validator.clone(),
            lane.cars.last().and_then(|car| {
                if car.used_in_cut {
                    None
                } else {
                    txs.extend(car.txs.clone());
                    Some(CutCar {
                        id: car.id,
                        parent: car.parent,
                    })
                }
            }),
        );
    }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct CutCar {
    pub id: usize,
    pub parent: Option<usize>,
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
            .map(|car| car.poa.clone().drain().collect())
    }

    pub fn try_a_new_cut(&mut self, nb_validators: usize) -> Option<CutWithTxs> {
        if let Some(car) = self.lane.current() {
            if car.poa.len() > nb_validators / 3 {
                let mut txs = HashSet::new();
                let mut cut = CutWithTxs {
                    txs: Vec::new(),
                    tips: BTreeMap::new(),
                };
                cut.extend_from_lane(&self.id, &mut self.lane, &mut txs);
                for (validator, lane) in self.other_lanes.iter_mut() {
                    cut.extend_from_lane(validator, lane, &mut txs);
                }
                cut.txs.extend(txs);
                return Some(cut);
            }
        }
        None
    }

    // Called after receiving a transaction, before broadcasting a dataproposal
    pub fn add_data_to_local_lane(&mut self, txs: Vec<Transaction>) -> usize {
        let tip_id = self.lane.current().map(|car| car.id);

        let current_id = tip_id.unwrap_or(0) + 1;

        self.lane.cars.push(Car {
            id: current_id,
            parent: tip_id,
            txs,
            poa: HashSet::from([self.id.clone()]),
            used_in_cut: false,
        });

        current_id
    }

    // Called by the initial proposal validator to aggregate votes
    pub fn new_data_vote(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> Option<()> {
        let car = self
            .lane
            .cars
            .iter_mut()
            .find(|c| c.id == data_proposal.pos && c.txs == data_proposal.txs);

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
                    warn!("{} already voted for data proposal", validator);
                    None
                } else {
                    c.poa.insert(validator.clone());
                    Some(())
                }
            }
        }
    }

    pub fn new_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> Result<ProposalVerdict> {
        if !self.has_parent_data_proposal(validator, data_proposal) {
            self.push_data_proposal_into_waiting_room(validator, data_proposal.clone());
            return Ok(ProposalVerdict::Wait(data_proposal.parent));
        }
        if let (Some(parent), Some(parent_poa)) = (data_proposal.parent, &data_proposal.parent_poa)
        {
            self.update_parent_poa(validator, parent, parent_poa)
        }
        if self.has_data_proposal(validator, data_proposal) {
            bail!("we already have voted for {}'s data_proposal", validator);
        }
        self.add_data_proposal(validator, data_proposal);
        Ok(ProposalVerdict::Vote)
    }

    fn add_data_proposal(&mut self, validator: &ValidatorPublicKey, data_proposal: &DataProposal) {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        let tip_id = lane.current().map(|car| car.id);
        lane.cars.push(Car {
            id: tip_id.unwrap_or(0) + 1,
            parent: tip_id,
            txs: data_proposal.txs.clone(),
            poa: HashSet::from([self.id.clone(), validator.clone()]),
            used_in_cut: false,
        });
    }

    fn has_parent_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> bool {
        let lane = self.other_lanes.entry(validator.clone()).or_default();
        let tip = lane.current_mut();
        if let Some(parent) = data_proposal.parent {
            tip.map(|car| car.id == parent).unwrap_or_default()
        } else {
            true
        }
    }

    fn update_parent_poa(
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

    fn has_data_proposal(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> bool {
        self.other_lanes
            .entry(validator.clone())
            .or_default()
            .cars
            .iter()
            .any(|c| c.id == data_proposal.pos && c.txs == data_proposal.txs)
    }

    #[cfg(test)]
    fn get_last_data_index(&self, validator: &ValidatorPublicKey) -> Option<usize> {
        self.other_lanes
            .get(validator)
            .and_then(|lane| lane.current())
            .map(|c| c.id)
    }

    pub fn get_missing_cars(
        &self,
        last_index: Option<usize>,
        data_proposal: &DataProposal,
    ) -> Option<Vec<Car>> {
        let car = self
            .lane
            .cars
            .iter()
            .find(|c| c.id == data_proposal.pos && c.txs == data_proposal.txs);

        match car {
            None => {
                error!(
                    "data proposal does exist locally as a car! lane: {}",
                    self.lane
                );
                None
            }
            Some(c) => {
                //Normally last_index must be < current_car since we are on the lane reference (that must have more data than others)
                match last_index {
                    // Nothing on the lane, we send everything, up to the data proposal id/pos
                    None => Some(
                        self.lane
                            .cars
                            .clone()
                            .into_iter()
                            .take_while(|car| car.id != c.id)
                            .collect(),
                    ),
                    // If there is an index, two cases
                    // - it matches the current tip, in this case we don't send any more data
                    // - it does not match, we send the diff
                    Some(last_index_usize) => {
                        if last_index_usize == c.id {
                            None
                        } else {
                            debug!(
                                "Trying to compute diff between {} and last_index {}",
                                c, last_index_usize
                            );
                            let mut missing_cars: Vec<Car> = vec![];
                            let mut current_car = c;
                            loop {
                                current_car =
                                    self.lane.cars.get(current_car.parent.unwrap() - 1).unwrap();
                                if current_car.id == last_index_usize
                                    || current_car.parent.is_none()
                                {
                                    break;
                                }
                                missing_cars.push(current_car.clone());
                            }

                            missing_cars.sort_by_key(|car| car.id);

                            Some(missing_cars)
                        }
                    }
                }
            }
        }
    }

    pub fn get_waiting_proposals(&mut self, validator: &ValidatorPublicKey) -> Vec<DataProposal> {
        match self.other_lanes.get_mut(validator) {
            Some(sl) => sl.waiting.drain().collect(),
            None => vec![],
        }
    }

    // Updates local view other lane matching the validator with sent cars
    pub fn add_missing_cars(&mut self, validator: &ValidatorPublicKey, cars: Vec<Car>) {
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
    pub fn push_data_proposal_into_waiting_room(
        &mut self,
        validator: &ValidatorPublicKey,
        data_proposal: DataProposal,
    ) {
        self.other_lanes
            .entry(validator.clone())
            .or_default()
            .waiting
            .insert(data_proposal);
    }

    /// Received a new transaction when the previous data proposal had no PoA yet
    pub fn accumulate_tx(&mut self, tx: Transaction) {
        self.pending_txs.push(tx);
    }

    pub fn tip_data(&self) -> Option<(TipData, Vec<Transaction>)> {
        self.lane.current().map(|car| {
            (
                TipData {
                    pos: car.id,
                    parent: car.parent,
                    poa: car.poa.clone().into_iter().collect(),
                },
                car.txs.clone(),
            )
        })
    }

    pub fn tip_already_used(&self) -> bool {
        self.lane
            .current()
            .map(|car| car.used_in_cut)
            .unwrap_or_default()
    }

    pub fn flush_pending_txs(&mut self) -> Vec<Transaction> {
        self.pending_txs.drain(0..).collect()
    }

    fn collect_old_used_cars(cars: &mut Vec<Car>, some_tip: &Option<CutCar>) {
        if let Some(tip) = some_tip {
            cars.retain_mut(|car| match car.id.cmp(&tip.id) {
                Ordering::Less => false,
                Ordering::Equal => {
                    car.used_in_cut = true;
                    true
                }
                Ordering::Greater => true,
            });
        }
    }

    pub fn update_lanes_after_commit(&mut self, lanes: Cut) {
        if let Some(tip) = lanes.get(&self.id) {
            Self::collect_old_used_cars(&mut self.lane.cars, tip);
        }
        for (validator, lane) in self.other_lanes.iter_mut() {
            if let Some(tip) = lanes.get(validator) {
                Self::collect_old_used_cars(&mut lane.cars, tip);
            }
        }
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Car {
    id: usize,
    parent: Option<usize>,
    txs: Vec<Transaction>,
    pub poa: HashSet<ValidatorPublicKey>,
    used_in_cut: bool,
}

impl Display for Car {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}/{:?}/{}v] (used: {})",
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
    waiting: HashSet<DataProposal>,
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

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct DataProposal {
    pub pos: usize,
    pub parent: Option<usize>,
    pub parent_poa: Option<Vec<ValidatorPublicKey>>,
    pub txs: Vec<Transaction>,
}

impl Display for DataProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ {:?} <- [{}/{:?}] }}",
            self.parent,
            self.pos,
            self.txs.first()
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::{
        mempool::storage::{Car, DataProposal, InMemoryStorage},
        model::{
            Blob, BlobData, BlobTransaction, ContractName, Identity, Transaction, TransactionData,
            ValidatorPublicKey,
        },
    };

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

        store.add_data_proposal(
            &pubkey2,
            &DataProposal {
                pos: 1,
                parent: None,
                txs: vec![make_tx("test1")],
                parent_poa: None,
            },
        );

        store.add_data_proposal(
            &pubkey2,
            &DataProposal {
                pos: 2,
                parent: Some(1),
                txs: vec![make_tx("test2")],
                parent_poa: None,
            },
        );

        store.add_data_proposal(
            &pubkey2,
            &DataProposal {
                pos: 3,
                parent: Some(2),
                txs: vec![make_tx("test3")],
                parent_poa: None,
            },
        );

        store.add_data_proposal(
            &pubkey2,
            &DataProposal {
                pos: 4,
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
                poa: HashSet::from([pubkey3.clone(), pubkey2.clone()]),
                used_in_cut: false,
            }
        );

        let missing = store.get_missing_cars(
            Some(1),
            &DataProposal {
                pos: 4,
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

        store.accumulate_tx(make_tx("test1"));
        store.accumulate_tx(make_tx("test2"));
        store.accumulate_tx(make_tx("test3"));
        store.accumulate_tx(make_tx("test4"));

        let txs = store.flush_pending_txs();
        store.add_data_to_local_lane(txs.clone());

        let data_proposal = DataProposal {
            txs,
            pos: 1,
            parent: None,
            parent_poa: None,
        };

        store.add_data_proposal(&pubkey2, &data_proposal);
        assert!(store.has_data_proposal(&pubkey2, &data_proposal));
        assert_eq!(store.get_last_data_index(&pubkey2), Some(1));
        store.add_data_proposal(&pubkey1, &data_proposal);
        assert!(store.has_data_proposal(&pubkey1, &data_proposal));
        assert_eq!(store.get_last_data_index(&pubkey1), Some(1));

        let some_tip = store.tip_data();
        assert!(some_tip.is_some());
        let (tip, txs) = some_tip.unwrap();
        assert_eq!(tip.poa.len(), 1);
        assert_eq!(txs.len(), 4);

        store.new_data_vote(&pubkey2, &data_proposal);
        store.new_data_vote(&pubkey1, &data_proposal);

        let some_tip = store.tip_data();
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

        let data_proposal1 = DataProposal {
            txs: vec![make_tx("test1"), make_tx("test2"), make_tx("test3")],
            pos: 1,
            parent: None,
            parent_poa: None,
        };

        let data_proposal2 = DataProposal {
            txs: vec![make_tx("test4"), make_tx("test5"), make_tx("test6")],
            pos: 1,
            parent: None,
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        let data_proposal3 = DataProposal {
            txs: vec![make_tx("test7"), make_tx("test8"), make_tx("test9")],
            pos: 2,
            parent: Some(1),
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        store.add_data_to_local_lane(data_proposal1.txs.clone());
        store.new_data_vote(&pubkey2, &data_proposal1);
        store
            .new_data_proposal(&pubkey2, &data_proposal2)
            .expect("add proposal 2");
        store
            .new_data_proposal(&pubkey2, &data_proposal3)
            .expect("add proposal 3");

        assert_eq!(store.lane.size(), 1);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(2));

        let cut = store.try_a_new_cut(2);
        assert!(cut.is_some());
        assert_eq!(cut.as_ref().map(|cut| cut.txs.len()), Some(6));

        store.update_lanes_after_commit(cut.unwrap().tips);

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
    }

    #[test]
    fn test_add_missing_cars() {
        let pubkey1 = ValidatorPublicKey(vec![1]);
        let pubkey2 = ValidatorPublicKey(vec![2]);
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());

        store.add_missing_cars(
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
                    poa: HashSet::from([pubkey1.clone(), pubkey2.clone()]),
                    used_in_cut: false,
                },
                Car {
                    id: 2,
                    parent: Some(1),
                    txs: vec![make_tx("test5"), make_tx("test6"), make_tx("test7")],
                    poa: HashSet::from([pubkey1.clone(), pubkey2.clone()]),
                    used_in_cut: false,
                },
            ],
        );

        assert_eq!(store.get_last_data_index(&pubkey2), Some(2));
    }

    #[test]
    fn test_missing_cars() {
        let pubkey3 = ValidatorPublicKey(vec![3]);
        let mut store = InMemoryStorage::new(pubkey3.clone());

        store.add_data_to_local_lane(vec![make_tx("test_local")]);
        store.add_data_to_local_lane(vec![make_tx("test_local2")]);
        store.add_data_to_local_lane(vec![make_tx("test_local3")]);
        store.add_data_to_local_lane(vec![make_tx("test_local4")]);

        let missing = store.get_missing_cars(
            Some(1),
            &DataProposal {
                pos: 4,
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
                    poa: HashSet::from_iter(vec![pubkey3.clone()]),
                    used_in_cut: false,
                },
                Car {
                    id: 3,
                    parent: Some(2),
                    txs: vec![make_tx("test_local3")],
                    poa: HashSet::from_iter(vec![pubkey3.clone()]),
                    used_in_cut: false,
                }
            ])
        );

        let missing = store.get_missing_cars(
            None,
            &DataProposal {
                pos: 4,
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
                    poa: HashSet::from_iter(vec![pubkey3.clone()]),
                    used_in_cut: false,
                },
                Car {
                    id: 2,
                    parent: Some(1),
                    txs: vec![make_tx("test_local2")],
                    poa: HashSet::from_iter(vec![pubkey3.clone()]),
                    used_in_cut: false,
                },
                Car {
                    id: 3,
                    parent: Some(2),
                    txs: vec![make_tx("test_local3")],
                    poa: HashSet::from_iter(vec![pubkey3.clone()]),
                    used_in_cut: false,
                }
            ])
        );
    }
}
