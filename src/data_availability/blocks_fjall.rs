use anyhow::Result;
use fjall::{Config, Keyspace, PartitionCreateOptions, PartitionHandle, Slice};
use std::{fmt::Debug, path::Path};
use tracing::{error, info};

use crate::{
    consensus::ConsensusProposalHash,
    model::{BlockHeight, Hashable, SignedBlock},
};

struct FjallHashKey(ConsensusProposalHash);
struct FjallHeightKey([u8; 8]);
struct FjallValue(Vec<u8>);

impl AsRef<[u8]> for FjallHashKey {
    fn as_ref(&self) -> &[u8] {
        self.0 .0.as_bytes()
    }
}

impl FjallHeightKey {
    fn new(height: BlockHeight) -> Self {
        Self(height.0.to_be_bytes())
    }
}

impl AsRef<[u8]> for FjallHeightKey {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl FjallValue {
    fn new(block: &SignedBlock) -> Result<Self> {
        Ok(Self(bincode::encode_to_vec(
            block,
            bincode::config::standard(),
        )?))
    }
}

impl AsRef<[u8]> for FjallValue {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

pub struct Blocks {
    db: Keyspace,
    by_hash: PartitionHandle,
    by_height: PartitionHandle,
}

impl Blocks {
    fn decode_item(item: Slice) -> Result<SignedBlock> {
        bincode::decode_from_slice(&item, bincode::config::standard())
            .map(|(b, _)| b)
            .map_err(Into::into)
    }

    pub fn new(path: &Path) -> Result<Self> {
        let db = Config::new(path).open()?;
        let by_hash = db.open_partition(
            "blocks_by_hash",
            PartitionCreateOptions::default()
                .block_size(56 * 1024)
                .manual_journal_persist(true),
        )?;
        let by_height =
            db.open_partition("block_hashes_by_height", PartitionCreateOptions::default())?;

        info!("{} block(s) available", by_hash.len()?);

        Ok(Blocks {
            db,
            by_hash,
            by_height,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty().unwrap_or(true)
    }

    pub fn persist(&self) -> Result<()> {
        self.db
            .persist(fjall::PersistMode::Buffer)
            .map_err(Into::into)
    }

    pub fn put(&mut self, block: SignedBlock) -> Result<()> {
        let block_hash = block.hash();
        if self.contains(&block_hash) {
            return Ok(());
        }
        info!("📦 storing block in fjall {}", block.height());
        self.by_hash
            .insert(FjallHashKey(block_hash), FjallValue::new(&block)?)?;
        self.by_height.insert(
            FjallHeightKey::new(block.height()),
            FjallValue::new(&block)?,
        )?;
        Ok(())
    }

    pub fn get(&mut self, block_hash: &ConsensusProposalHash) -> Result<Option<SignedBlock>> {
        let item = self.by_hash.get(FjallHashKey(block_hash.clone()))?;
        item.map(Self::decode_item).transpose()
    }

    pub fn contains(&mut self, block: &ConsensusProposalHash) -> bool {
        self.by_hash
            .contains_key(FjallHashKey(block.clone()))
            .unwrap_or(false)
    }

    pub fn last(&self) -> Option<SignedBlock> {
        match self.by_height.last_key_value() {
            Ok(Some((_, v))) => Self::decode_item(v).ok(),
            Ok(None) => None,
            Err(e) => {
                error!("Error getting last block: {:?}", e);
                None
            }
        }
    }

    pub fn last_block_hash(&self) -> Option<ConsensusProposalHash> {
        self.last().map(|b| b.hash())
    }

    pub fn range(
        &mut self,
        min: BlockHeight,
        max: BlockHeight,
    ) -> impl Iterator<Item = Result<SignedBlock>> {
        self.by_height
            .range(FjallHeightKey::new(min)..FjallHeightKey::new(max))
            .map_while(|maybe_item| match maybe_item {
                Ok((_, v)) => Some(Self::decode_item(v).map_err(Into::into)),
                Err(_) => None,
            })
    }
}

impl Debug for Blocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blocks")
            .field("len", &self.by_height.len())
            .finish()
    }
}
