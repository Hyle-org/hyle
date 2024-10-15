use anyhow::{bail, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    hash::Hash,
    vec,
};
use tracing::{debug, error, warn};

use crate::{
    mempool::BatchInfo,
    model::{Block, Transaction, ValidatorPublicKey},
};

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TipData {
    pub pos: usize,
    pub parent: Option<usize>,
    pub votes: Vec<ValidatorPublicKey>,
    pub used: bool,
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

    fn collect_lane(
        lane: &mut Lane,
        validator: &ValidatorPublicKey,
        batch_info: &BatchInfo,
        txs: &Vec<Transaction>,
    ) {
        if validator == &batch_info.validator {
            if let Some(i) = lane.cars.iter().position(|c| c.id == batch_info.tip.pos) {
                // anything prior to last_pos can be collected
                lane.cars.drain(0..i);
            }
        } else if let Some(i) = lane
            .cars
            .iter()
            .position(|c| c.id == batch_info.tip.pos && &c.txs == txs)
        {
            lane.cars.drain(0..i);
        }
    }

    // Called after receiving a transaction, before broadcasting a dataproposal
    pub fn add_data_to_local_lane(&mut self, txs: Vec<Transaction>) -> usize {
        let tip_id = self.lane.current().map(|car| car.id);

        let current_id = tip_id.unwrap_or(0) + 1;

        self.lane.cars.push(Car {
            id: current_id,
            parent: tip_id,
            txs,
            votes: HashSet::from([self.id.clone()]),
            used: false,
        });

        current_id
    }

    // Called by the initial proposal sender to aggregate votes
    pub fn new_data_vote(
        &mut self,
        sender: &ValidatorPublicKey,
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
                    "Vote for Car that does not exist! ({sender}) data_proposal: {} / lane: {}",
                    data_proposal, self.lane
                );
                None
            }
            Some(v) => {
                if v.votes.contains(sender) {
                    warn!(
                        "{} already voted for data proposal {}",
                        sender, data_proposal
                    );
                    None
                } else {
                    v.votes.insert(sender.clone());
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
            bail!(
                "we have already voted for {}'s data_proposal {} ",
                validator,
                data_proposal
            );
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
            votes: HashSet::from([self.id.clone(), validator.clone()]),
            used: false,
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
                tip.votes.extend(parent_poa.iter().cloned());
            }
        }
    }

    fn has_data_proposal(
        &mut self,
        sender: &ValidatorPublicKey,
        data_proposal: &DataProposal,
    ) -> bool {
        self.other_lanes
            .entry(sender.clone())
            .or_default()
            .cars
            .iter()
            .any(|c| c.id == data_proposal.pos && c.txs == data_proposal.txs)
    }

    #[cfg(test)]
    pub fn get_last_data_index(&self, sender: &ValidatorPublicKey) -> Option<usize> {
        self.other_lanes
            .get(sender)
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
                    "data proposal does exist locally as a car! data_proposal: {} / lane: {}",
                    data_proposal, self.lane
                );
                None
            }
            Some(v) => {
                //Normally last_index must be < current_car since we are on the lane reference (that must have more data than others)
                match last_index {
                    // Nothing on the lane, we send everything, up to the data proposal id/pos
                    None => Some(
                        self.lane
                            .cars
                            .clone()
                            .into_iter()
                            .take_while(|c| c.id != v.id)
                            .collect(),
                    ),
                    // If there is an index, two cases
                    // - it matches the current tip, in this case we don't send any more data
                    // - it does not match, we send the diff
                    Some(last_index_usize) => {
                        if last_index_usize == v.id {
                            None
                        } else {
                            debug!(
                                "Trying to compute diff between {} and last_index {}",
                                v, last_index_usize
                            );
                            let mut missing_cars: Vec<Car> = vec![];
                            let mut current_car = v;
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

    pub fn get_waiting_proposals(&mut self, sender: &ValidatorPublicKey) -> Vec<DataProposal> {
        match self.other_lanes.get_mut(sender) {
            Some(sl) => sl.waiting.drain().collect(),
            None => vec![],
        }
    }

    // Updates local view other lane matching the sender with sent cars
    pub fn add_missing_cars(&mut self, sender: &ValidatorPublicKey, cars: Vec<Car>) {
        let lane = self.other_lanes.entry(sender.clone()).or_default();

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
        sender: &ValidatorPublicKey,
        data_proposal: DataProposal,
    ) {
        self.other_lanes
            .entry(sender.clone())
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
                    votes: car.votes.clone().into_iter().collect(),
                    used: car.used,
                },
                car.txs.clone(),
            )
        })
    }

    pub fn tip_already_used(&self) -> bool {
        self.lane.current().map(|car| car.used).unwrap_or_default()
    }

    pub fn tip_used(&mut self) {
        if let Some(tip) = self.lane.current_mut() {
            tip.used = true;
        }
    }

    pub fn flush_pending_txs(&mut self) -> Vec<Transaction> {
        self.pending_txs.drain(0..).collect()
    }

    pub fn update_lanes_after_commit(&mut self, batch_info: BatchInfo, block: Block) {
        Self::collect_lane(&mut self.lane, &self.id, &batch_info, &block.txs);
        for (v, lane) in self.other_lanes.iter_mut() {
            Self::collect_lane(lane, v, &batch_info, &block.txs);
        }
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Car {
    id: usize,
    parent: Option<usize>,
    txs: Vec<Transaction>,
    pub votes: HashSet<ValidatorPublicKey>,
    used: bool,
}

impl Display for Car {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}/{:?}/{}v] (used: {})",
            self.id,
            self.txs.first(),
            self.votes.len(),
            self.used,
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
    pub fn size(&self) -> usize {
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
        mempool::{
            storage::{Car, DataProposal, InMemoryStorage},
            BatchInfo,
        },
        model::{
            Blob, BlobData, BlobTransaction, Block, BlockHash, BlockHeight, ContractName, Identity,
            Transaction, TransactionData, ValidatorPublicKey,
        },
    };

    use super::TipData;

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
                votes: HashSet::from([pubkey3.clone(), pubkey2.clone()]),
                used: false,
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
        assert_eq!(tip.votes.len(), 1);
        assert_eq!(txs.len(), 4);

        store.new_data_vote(&pubkey2, &data_proposal);
        store.new_data_vote(&pubkey1, &data_proposal);

        let some_tip = store.tip_data();
        assert!(some_tip.is_some());
        let tip = some_tip.unwrap().0;
        assert_eq!(tip.votes.len(), 3);
        assert!(&tip.votes.contains(&pubkey3));
        assert!(&tip.votes.contains(&pubkey1));
        assert!(&tip.votes.contains(&pubkey2));
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
            pos: 2,
            parent: Some(1),
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        let data_proposal3 = DataProposal {
            txs: vec![make_tx("test7"), make_tx("test8"), make_tx("test9")],
            pos: 3,
            parent: Some(2),
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        let data_proposal4 = DataProposal {
            txs: vec![make_tx("testA"), make_tx("testB"), make_tx("testC")],
            pos: 4,
            parent: Some(3),
            parent_poa: Some(vec![pubkey3.clone(), pubkey2.clone()]),
        };

        store.add_data_to_local_lane(data_proposal1.txs.clone());
        store.add_data_to_local_lane(data_proposal2.txs.clone());
        store.add_data_to_local_lane(data_proposal3.txs.clone());
        store.add_data_to_local_lane(data_proposal4.txs.clone());

        store.add_data_proposal(&pubkey2, &data_proposal1);
        store.add_data_proposal(&pubkey2, &data_proposal2);
        store.add_data_proposal(&pubkey2, &data_proposal3);
        store.add_data_proposal(&pubkey2, &data_proposal4);

        let batch_info = BatchInfo {
            tip: TipData {
                pos: 4,
                parent: Some(3),
                votes: vec![pubkey3.clone(), pubkey2.clone()],
                used: false,
            },
            validator: pubkey3.clone(),
        };
        let block = Block {
            parent_hash: BlockHash {
                inner: vec![4, 5, 6],
            },
            height: BlockHeight(42),
            timestamp: 1234,
            txs: vec![make_tx("testA"), make_tx("testB"), make_tx("testC")],
            new_bonded_validators: vec![pubkey3.clone(), pubkey2.clone()],
        };

        assert_eq!(store.lane.size(), 4);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(4));

        store.update_lanes_after_commit(batch_info, block);

        assert_eq!(store.lane.size(), 1);
        assert_eq!(store.other_lanes.get(&pubkey2).map(|l| l.size()), Some(1));
        assert_eq!(store.lane.current().map(|c| c.id), Some(4));
        assert_eq!(
            store
                .other_lanes
                .get(&pubkey2)
                .and_then(|l| l.current().map(|c| c.id)),
            Some(4)
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
                    votes: HashSet::from([pubkey1.clone(), pubkey2.clone()]),
                    used: false,
                },
                Car {
                    id: 2,
                    parent: Some(1),
                    txs: vec![make_tx("test5"), make_tx("test6"), make_tx("test7")],
                    votes: HashSet::from([pubkey1.clone(), pubkey2.clone()]),
                    used: false,
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
                    votes: HashSet::from_iter(vec![pubkey3.clone()]),
                    used: false,
                },
                Car {
                    id: 3,
                    parent: Some(2),
                    txs: vec![make_tx("test_local3")],
                    votes: HashSet::from_iter(vec![pubkey3.clone()]),
                    used: false,
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
                    votes: HashSet::from_iter(vec![pubkey3.clone()]),
                    used: false,
                },
                Car {
                    id: 2,
                    parent: Some(1),
                    txs: vec![make_tx("test_local2")],
                    votes: HashSet::from_iter(vec![pubkey3.clone()]),
                    used: false,
                },
                Car {
                    id: 3,
                    parent: Some(2),
                    txs: vec![make_tx("test_local3")],
                    votes: HashSet::from_iter(vec![pubkey3.clone()]),
                    used: false,
                }
            ])
        );
    }
}
