use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    hash::Hash,
    vec,
};
use tracing::{error, info, warn};

use crate::{
    mempool::BatchInfo,
    model::{Block, Transaction},
};

#[derive(Debug, Clone, Encode, Decode, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TipData {
    pub pos: usize,
    pub parent: Option<usize>,
    pub votes: Vec<String>,
}

pub trait Storage: Display + Send + Sync {
    fn snapshot(&self) -> StateSnapshot;

    // Received a new transaction when the previous data proposal had no PoA yet
    fn accumulate_tx(&mut self, tx: Transaction);

    fn has_pending_txs(&self) -> bool;
    fn flush_pending_txs(&mut self) -> Vec<Transaction>;

    //
    fn tip_data(&self) -> Option<(TipData, Vec<Transaction>)>;

    // Called after receiving a transaction, before broadcasting a dataproposal
    fn add_data_to_local_lane(&mut self, txs: Vec<Transaction>) -> usize;
    // Called when a data proposal is received try to bind it to the previous tip (true) or fails (false)
    fn append_data_proposal(&mut self, sender: &str, data_proposal: &DataProposal) -> bool;
    fn has_data_proposal(&mut self, sender: &str, data_proposal: &DataProposal) -> bool;
    // Called when validate return
    fn push_data_proposal_into_waiting_room(&mut self, sender: &str, data_proposal: DataProposal);
    fn get_last_data_index(&self, sender: &str) -> Option<usize>;
    fn get_missing_cars(
        &self,
        last_index: Option<usize>,
        data_proposal: &DataProposal,
    ) -> Option<Vec<Car>>;

    // Updates local view other lane matching the sender with sent cars
    fn add_missing_cars(&mut self, sender: &str, cars: Vec<Car>);

    fn get_waiting_proposals(&mut self, sender: &str) -> Vec<DataProposal>;

    // Called by the initial proposal sender to aggregate votes
    fn add_data_vote(
        &mut self,
        sender: &str,
        data_proposal: &DataProposal,
    ) -> Option<HashSet<String>>;

    fn update_other_votes(
        &mut self,
        sender: &str,
        data_proposal: &DataProposal,
        voters: HashSet<String>,
    ) -> Option<Vec<String>>;

    fn update_lanes_after_commit(&mut self, info: BatchInfo, block: Block);
}

#[derive(Debug, Clone)]
pub struct InMemoryStorage {
    pub id: String,
    pub pending_txs: Vec<Transaction>,
    pub lane: Lane,
    pub other_lanes: HashMap<String, Lane>,
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
    pub fn new(id: String) -> InMemoryStorage {
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
}

impl Storage for InMemoryStorage {
    fn add_data_to_local_lane(&mut self, txs: Vec<Transaction>) -> usize {
        let tip_id = self.lane.current().map(|car| car.id);

        let current_id = tip_id.unwrap_or(0) + 1;

        self.lane.cars.push(Car {
            id: current_id,
            parent: tip_id,
            txs,
            votes: HashSet::from([self.id.clone()]),
        });

        current_id
    }

    fn add_data_vote(
        &mut self,
        sender: &str,
        data_proposal: &DataProposal,
    ) -> Option<HashSet<String>> {
        let car = self
            .lane
            .cars
            .iter_mut()
            .find(|c| c.id == data_proposal.pos && c.txs == data_proposal.inner);

        match car {
            None => {
                warn!(
                    "Vote for Car that does not exist! ({sender}) data_proposal: {} / lane: {}",
                    data_proposal, self.lane
                );
                None
            }
            Some(v) => {
                v.votes.insert(sender.to_string());
                Some(v.votes.clone())
            }
        }
    }

    fn update_other_votes(
        &mut self,
        sender: &str,
        data_proposal: &DataProposal,
        voters: HashSet<String>,
    ) -> Option<Vec<String>> {
        if let Some(lane) = self.other_lanes.get_mut(sender) {
            let car = lane
                .cars
                .iter_mut()
                .find(|c| c.id == data_proposal.pos && c.txs == data_proposal.inner);

            match car {
                None => {
                    warn!(
                        "Update PoA for Car that does not exist! ({sender}) data_proposal: {} / lane: {}",
                        data_proposal, self.lane
                    );
                    return None;
                }
                Some(v) => {
                    v.votes.extend(voters);
                    return Some(v.votes.clone().into_iter().collect());
                }
            }
        }
        None
    }

    fn append_data_proposal(&mut self, sender: &str, data_proposal: &DataProposal) -> bool {
        let lane = self.other_lanes.entry(sender.to_string()).or_default();
        let tip = lane.current_mut();

        let mut tip_id: Option<usize> = None;

        // merge parent votes
        if let Some(p) = tip {
            tip_id = Some(p.id);
            if let Some(v) = data_proposal.parent_poa.as_ref() {
                p.votes.reserve(v.len());
                for v in v.iter() {
                    p.votes.insert(v.clone());
                }
            }
        }

        if data_proposal.parent == tip_id {
            lane.cars.push(Car {
                id: tip_id.unwrap_or(0) + 1,
                parent: tip_id,
                txs: data_proposal.inner.clone(),
                votes: HashSet::from([self.id.clone(), sender.to_string()]),
            });
            true
        } else {
            false
        }
    }

    fn has_data_proposal(&mut self, sender: &str, data_proposal: &DataProposal) -> bool {
        let found = self
            .other_lanes
            .entry(sender.to_string())
            .or_default()
            .cars
            .iter()
            .any(|c| c.id == data_proposal.pos && c.txs == data_proposal.inner);

        if !found {
            warn!(
                "Data proposal {} not found in lane {}",
                data_proposal, self.lane
            );
        }

        found
    }

    fn get_last_data_index(&self, sender: &str) -> Option<usize> {
        self.other_lanes
            .get(sender)
            .and_then(|lane| lane.current())
            .map(|c| c.id)
    }

    fn get_missing_cars(
        &self,
        last_index: Option<usize>,
        data_proposal: &DataProposal,
    ) -> Option<Vec<Car>> {
        let car = self
            .lane
            .cars
            .iter()
            .find(|c| c.id == data_proposal.pos && c.txs == data_proposal.inner);

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
                            info!(
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

    fn get_waiting_proposals(&mut self, sender: &str) -> Vec<DataProposal> {
        match self.other_lanes.get_mut(sender) {
            Some(sl) => sl.waiting.drain().collect(),
            None => vec![],
        }
    }

    fn add_missing_cars(&mut self, sender: &str, cars: Vec<Car>) {
        let lane = self.other_lanes.entry(sender.to_string()).or_default();

        let mut ordered_cars = cars;
        ordered_cars.sort_by_key(|car| car.id);

        info!(
            "Trying to add missing cars on lane \n {}\n {:?}",
            lane, ordered_cars
        );

        for c in ordered_cars.into_iter() {
            // XXX: that means we can only have 1 missing car ?
            if c.parent == lane.current().map(|l| l.id) {
                lane.cars.push(c);
            }
        }
    }

    fn push_data_proposal_into_waiting_room(&mut self, sender: &str, data_proposal: DataProposal) {
        self.other_lanes
            .entry(sender.to_string())
            .or_default()
            .waiting
            .insert(data_proposal);
    }

    fn snapshot(&self) -> StateSnapshot {
        StateSnapshot(
            self.id.clone(),
            self.lane.clone(),
            self.other_lanes.clone(),
            self.pending_txs.clone(),
        )
    }

    fn accumulate_tx(&mut self, tx: Transaction) {
        self.pending_txs.push(tx);
    }

    fn tip_data(&self) -> Option<(TipData, Vec<Transaction>)> {
        self.lane.current().map(|car| {
            (
                TipData {
                    pos: car.id,
                    parent: car.parent,
                    votes: car.votes.clone().into_iter().collect(),
                },
                car.txs.clone(),
            )
        })
    }

    fn has_pending_txs(&self) -> bool {
        !self.pending_txs.is_empty()
    }

    fn flush_pending_txs(&mut self) -> Vec<Transaction> {
        self.pending_txs.drain(0..).collect()
    }

    fn update_lanes_after_commit(&mut self, batch_info: BatchInfo, block: Block) {
        Self::collect_lane(&mut self.lane, &self.id, &batch_info, &block.txs);
        for (v, lane) in self.other_lanes.iter_mut() {
            Self::collect_lane(lane, v, &batch_info, &block.txs);
        }
    }
}

impl InMemoryStorage {
    fn collect_lane(
        lane: &mut Lane,
        validator: &str,
        batch_info: &BatchInfo,
        txs: &Vec<Transaction>,
    ) {
        if validator == batch_info.validator.0 {
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
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Car {
    id: usize,
    parent: Option<usize>,
    txs: Vec<Transaction>,
    pub votes: HashSet<String>,
}

impl Display for Car {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}/{:?}/{}v]",
            self.id,
            self.txs.first().unwrap(),
            self.votes.len()
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

    pub fn size(&self) -> usize {
        self.cars.len()
    }

    pub fn size_txs(&self) -> usize {
        self.cars.iter().fold(0, |nb, car| nb + car.txs.len())
    }

    pub fn count_poa(&self, nb_replicas: usize) -> usize {
        self.cars
            .iter()
            .filter(|c| c.votes.len() > nb_replicas / 3)
            .count()
    }

    pub fn count_poa_txs(&self, nb_replicas: usize) -> usize {
        self.cars
            .iter()
            .filter(|c| c.votes.len() > nb_replicas / 3)
            .fold(0, |nb, car| nb + car.txs.len())
    }

    pub fn count_waiting(&self) -> usize {
        self.waiting.len()
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct DataProposal {
    pub pos: usize,
    pub parent: Option<usize>,
    pub parent_poa: Option<Vec<String>>,
    pub inner: Vec<Transaction>,
}

impl Display for DataProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ {:?} <- [{}/{:?}] }}",
            self.parent,
            self.pos,
            self.inner.first().unwrap()
        )
    }
}

#[derive(Debug, Clone)]
pub struct StateSnapshot(
    pub String,
    pub Lane,
    pub HashMap<String, Lane>,
    pub Vec<Transaction>,
);

impl Display for StateSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Replica {}", self.0)?;
        write!(f, "\nLane {}", self.1)?;
        for (i, l) in self.2.iter() {
            write!(f, "\n - OL {}: {}", i, l)?;
        }

        Ok(())
    }
}
