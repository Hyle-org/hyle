use anyhow::Result;
use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle, Slice};
use hyle_model::{DataProposalHash, ValidatorPublicKey};
use std::{fmt::Debug, path::Path, sync::Arc};
use tracing::{error, info, trace};

use crate::{
    mempool::storage::LaneEntry,
    model::{BlockHeight, ConsensusProposalHash, Hashable, SignedBlock},
};

type Car = LaneEntry;
struct FjallBlockHashKey(ConsensusProposalHash);
struct FjallBlockHeightKey([u8; 8]);
struct FjallBlockValue(Vec<u8>);

impl AsRef<[u8]> for FjallBlockHashKey {
    fn as_ref(&self) -> &[u8] {
        self.0 .0.as_bytes()
    }
}

impl FjallBlockHeightKey {
    fn new(height: BlockHeight) -> Self {
        Self(height.0.to_be_bytes())
    }
}

impl AsRef<[u8]> for FjallBlockHeightKey {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl FjallBlockValue {
    fn new(block: &SignedBlock) -> Result<Self> {
        Ok(Self(bincode::encode_to_vec(
            block,
            bincode::config::standard(),
        )?))
    }
}

impl AsRef<[u8]> for FjallBlockValue {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

struct FjallCarValue(Vec<u8>);

impl FjallCarValue {
    fn new(car: &Car) -> Result<Self> {
        Ok(Self(bincode::encode_to_vec(
            car,
            bincode::config::standard(),
        )?))
    }
}

impl AsRef<[u8]> for FjallCarValue {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

pub struct DAStorage {
    db: Keyspace,
    block_by_hash: PartitionHandle,
    block_by_height: PartitionHandle,
    car_by_dp_hash: PartitionHandle,
    car_by_dp_id: PartitionHandle,
}

impl DAStorage {
    fn decode_block_item(item: Slice) -> Result<SignedBlock> {
        bincode::decode_from_slice(&item, bincode::config::standard())
            .map(|(b, _)| b)
            .map_err(Into::into)
    }

    fn decode_car_item(item: Slice) -> Result<Car> {
        bincode::decode_from_slice(&item, bincode::config::standard())
            .map(|(b, _)| b)
            .map_err(Into::into)
    }

    pub fn new(path: &Path) -> Result<Self> {
        let db = Config::new(path)
            .blob_cache(Arc::new(fjall::BlobCache::with_capacity_bytes(
                128 * 1024 * 1024,
            )))
            .block_cache(Arc::new(fjall::BlockCache::with_capacity_bytes(
                128 * 1024 * 1024,
            )))
            .open()?;
        let block_by_hash = db.open_partition(
            "blocks_by_hash",
            PartitionCreateOptions::default()
                .block_size(56 * 1024)
                .manual_journal_persist(true)
                .max_memtable_size(128 * 1024 * 1024),
        )?;
        let block_by_height =
            db.open_partition("block_hashes_by_height", PartitionCreateOptions::default())?;

        let car_by_dp_hash = db.open_partition(
            "car_by_dp_hash",
            PartitionCreateOptions::default()
                .block_size(56 * 1024)
                .manual_journal_persist(true)
                .max_memtable_size(128 * 1024 * 1024),
        )?;

        let car_by_dp_id = db.open_partition(
            "car_by_dp_id",
            PartitionCreateOptions::default()
                .block_size(56 * 1024)
                .manual_journal_persist(true)
                .max_memtable_size(128 * 1024 * 1024),
        )?;

        info!("{} block(s) available", block_by_hash.len()?);
        info!("{} car(s) available", car_by_dp_hash.len()?);

        Ok(DAStorage {
            db,
            block_by_hash,
            block_by_height,
            car_by_dp_hash,
            car_by_dp_id,
        })
    }

    pub fn block_is_empty(&self) -> bool {
        self.block_by_hash.is_empty().unwrap_or(true)
    }

    pub fn car_is_empty(&self) -> bool {
        self.car_by_dp_hash.is_empty().unwrap_or(true)
    }

    pub fn persist(&self) -> Result<()> {
        self.db
            .persist(fjall::PersistMode::Buffer)
            .map_err(Into::into)
    }

    pub fn put_block(&mut self, block: SignedBlock) -> Result<()> {
        let block_hash = block.hash();
        if self.contains_block(&block_hash) {
            return Ok(());
        }
        trace!("📦 storing block in fjall {}", block.height());
        self.block_by_hash.insert(
            FjallBlockHashKey(block_hash).as_ref(),
            FjallBlockValue::new(&block)?.as_ref(),
        )?;
        self.block_by_height.insert(
            FjallBlockHeightKey::new(block.height()).as_ref(),
            FjallBlockValue::new(&block)?.as_ref(),
        )?;
        Ok(())
    }

    pub fn put_car(&mut self, validator_key: ValidatorPublicKey, car: Car) -> Result<()> {
        let dp_hash = car.data_proposal.hash();
        if self.contains_car(&validator_key, &dp_hash) {
            return Ok(());
        }
        trace!("📦 storing car in fjall {}", car.data_proposal.id);
        self.car_by_dp_hash.insert(
            format!("{}:{}", validator_key, dp_hash),
            FjallCarValue::new(&car)?.as_ref(),
        )?;
        self.car_by_dp_id.insert(
            format!("{}:{}", validator_key, car.data_proposal.id),
            FjallCarValue::new(&car)?.as_ref(),
        )?;
        Ok(())
    }

    pub fn get_block(&mut self, block_hash: &ConsensusProposalHash) -> Result<Option<SignedBlock>> {
        let item = self
            .block_by_hash
            .get(FjallBlockHashKey(block_hash.clone()))?;
        item.map(Self::decode_block_item).transpose()
    }

    pub fn get_car(
        &mut self,
        validator_key: &ValidatorPublicKey,
        dp_hash: &DataProposalHash,
    ) -> Result<Option<Car>> {
        let item = self
            .car_by_dp_hash
            .get(format!("{}:{}", validator_key, dp_hash))?;
        item.map(Self::decode_car_item).transpose()
    }

    pub fn contains_block(&mut self, block: &ConsensusProposalHash) -> bool {
        self.block_by_hash
            .contains_key(FjallBlockHashKey(block.clone()))
            .unwrap_or(false)
    }

    pub fn contains_car(
        &mut self,
        validator_key: &ValidatorPublicKey,
        dp_hash: &DataProposalHash,
    ) -> bool {
        self.car_by_dp_hash
            .contains_key(format!("{}:{}", validator_key, dp_hash))
            .unwrap_or(false)
    }

    pub fn last_block(&self) -> Option<SignedBlock> {
        match self.block_by_height.last_key_value() {
            Ok(Some((_, v))) => Self::decode_block_item(v).ok(),
            Ok(None) => None,
            Err(e) => {
                error!("Error getting last block: {:?}", e);
                None
            }
        }
    }

    pub fn last_block_hash(&self) -> Option<ConsensusProposalHash> {
        self.last_block().map(|b| b.hash())
    }

    pub fn range_block(
        &mut self,
        min: BlockHeight,
        max: BlockHeight,
    ) -> impl Iterator<Item = Result<SignedBlock>> {
        self.block_by_height
            .range(FjallBlockHeightKey::new(min)..FjallBlockHeightKey::new(max))
            .map_while(|maybe_item| match maybe_item {
                Ok((_, v)) => Some(Self::decode_block_item(v).map_err(Into::into)),
                Err(_) => None,
            })
    }
}

impl Debug for DAStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blocks")
            .field("len", &self.block_by_height.len())
            .finish()
    }
}
