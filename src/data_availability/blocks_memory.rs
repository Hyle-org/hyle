#![allow(unused)]
use std::{collections::HashMap, path::Path};

use crate::{
    mempool::storage::LaneEntry,
    model::{BlockHeight, ConsensusProposalHash, Hashable, SignedBlock},
};
use anyhow::Result;
use hyle_model::{DataProposal, DataProposalHash, ValidatorPublicKey};
use indexmap::IndexMap;
use tracing::{info, trace};

type Car = LaneEntry;
#[derive(Debug)]
pub struct DAStorage {
    blocks: IndexMap<ConsensusProposalHash, SignedBlock>,
    cars: HashMap<(ValidatorPublicKey, DataProposalHash), Car>,
}

impl DAStorage {
    pub fn new(_: &Path) -> Result<Self> {
        Ok(Self {
            blocks: IndexMap::new(),
            cars: HashMap::new(),
        })
    }

    pub fn block_is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn car_is_empty(&self) -> bool {
        self.cars.is_empty()
    }

    pub fn persist(&self) -> Result<()> {
        Ok(())
    }

    pub fn put_block(&mut self, data: SignedBlock) -> Result<()> {
        let block_hash = data.hash();
        if self.contains_block(&block_hash) {
            return Ok(());
        }
        trace!("📦 storing block {}", data.height());
        self.blocks.insert(block_hash, data);
        Ok(())
    }

    pub fn put_car(&mut self, validator_key: ValidatorPublicKey, car: Car) -> Result<()> {
        let dp_hash = car.data_proposal.hash();
        if self.contains_car(&validator_key, &dp_hash) {
            return Ok(());
        }
        trace!("📦 storing car in fjall {}", car.data_proposal.id);
        self.cars.insert((validator_key, dp_hash), car);
        Ok(())
    }

    pub fn get_block(&mut self, block_hash: &ConsensusProposalHash) -> Result<Option<SignedBlock>> {
        Ok(self.blocks.get(block_hash).cloned())
    }

    pub fn get_car(
        &mut self,
        validator_key: &ValidatorPublicKey,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<Car>> {
        Ok(self
            .cars
            .get(&(validator_key.clone(), dp_hash.clone()))
            .cloned())
    }

    pub fn contains_block(&mut self, block_hash: &ConsensusProposalHash) -> bool {
        self.blocks.contains_key(block_hash)
    }

    pub fn contains_car(
        &mut self,
        validator_key: &ValidatorPublicKey,
        dp_hash: &DataProposalHash,
    ) -> bool {
        self.cars
            .contains_key(&(validator_key.clone(), dp_hash.clone()))
    }

    pub fn last_block(&self) -> Option<SignedBlock> {
        self.blocks.last().map(|(_, block)| block.clone())
    }

    pub fn last_block_hash(&self) -> Option<ConsensusProposalHash> {
        self.last_block().map(|b| b.hash())
    }

    pub fn range_block(
        &self,
        min: BlockHeight,
        max: BlockHeight,
    ) -> Box<dyn Iterator<Item = Result<SignedBlock>> + '_> {
        // Items are in order but we don't know wher they are. Binary search.
        let Ok(min) = self
            .blocks
            .binary_search_by(|_, block| block.height().0.cmp(&min.0))
        else {
            return Box::new(::std::iter::empty());
        };
        let Ok(max) = self
            .blocks
            .binary_search_by(|_, block| block.height().0.cmp(&(max.0 - 1)))
        else {
            return Box::new(::std::iter::empty());
        };
        let Some(iter) = self.blocks.get_range(min..max + 1) else {
            return Box::new(::std::iter::empty());
        };
        Box::new(iter.values().map(|block| Ok(block.clone())))
    }
}
